use crate::*;
use chrono::{Datelike, NaiveDate, NaiveTime};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::info;
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::{self, BufReader, ErrorKind, Read},
    sync::{Condvar, LazyLock, Mutex},
};
use xxhash_rust::xxh3::{Xxh3, xxh3_64};

const EXTENSIONS: &[&str] = &[
    // RAW file extensions.
    "3fr", "ari", "arw", "bay", "cap", "cr2", "cr3", "crw", "data", "dcr",
    "dcs", "dng", "drf", "eip", "erf", "fff", "gpr", "iiq", "k25", "kdc",
    "mdc", "mef", "mos", "mrw", "nef", "nrw", "obm", "orf", "pef", "ptx",
    "pxn", "r3d", "raf", "raw", "rw2", "rwl", "rwz", "sr2", "srf", "srw",
    "x3f", // Other image files.
    "avif", "bmp", "fpx", "gif", "heic", "heif", "j2k", "jfif", "jif", "jp2",
    "jpeg", "jpg", "jpx", "pcd", "png", "psd", "tif", "tiff",
    "webp", // Movie file formats.
    "264", "3g2", "3gp", "amv", "asf", "avi", "cine", "drc", "f4a", "f4b",
    "f4p", "f4v", "flv", "gifv", "m2ts", "m2v", "m4p", "m4v", "mkv", "mng",
    "mp4", "mpeg", "mpg", "mts", "mxf", "nsv", "ogg", "qt", "roq", "svi",
    "vob", "wmv", "yuv",
];

/// Folder names that hold an album's images rather than naming the album
/// (e.g. iPhoto's `Originals`/`Modified`). Compared case-insensitively.
const ALBUM_SKIP_DIRS: &[&str] = &[
    "originals",
    "original",
    "modified",
    "masters",
    "previews",
    "thumbnails",
    "edited",
];

/// Derive an album name from the folder an image lives in.
///
/// Walks from the nearest folder upwards, skipping container folders like
/// `Originals` and purely numeric/date folders like `2011`, and strips a
/// leading `YYYY-MM-DD` style date prefix: `2011-07-14--Iceland/Originals`
/// yields `Iceland`.
pub(crate) fn album_from_path(dir: &Path) -> Option<String> {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    dir.components().rev().find_map(|component| {
        let name = component.as_os_str().to_str()?;
        if is_album_skip_dir(name) {
            return None;
        }
        let name = strip_date_prefix(name).trim();
        (!is_date_like(name)).then(|| name.to_string())
    })
}

/// A date that may be only partly known: a year, perhaps a month, perhaps a
/// day. Missing parts expand to `00` in destination paths, so such files are
/// easy to find and fix up later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PathDate {
    pub year: i32,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

impl PathDate {
    fn partial(year: i32, month: Option<u32>) -> Self {
        Self {
            year,
            month,
            day: None,
        }
    }

    /// The complete date, if year, month and day are all known.
    pub(crate) fn full(&self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month?, self.day?)
    }
}

impl From<NaiveDate> for PathDate {
    fn from(date: NaiveDate) -> Self {
        Self {
            year: date.year(),
            month: Some(date.month()),
            day: Some(date.day()),
        }
    }
}

impl std::fmt::Display for PathDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02}",
            self.year,
            self.month.unwrap_or(0),
            self.day.unwrap_or(0)
        )
    }
}

/// Derive a date from the folders an image lives in.
///
/// Only the album folder (see [`album_from_path`]) and the date-only folders
/// between it and the image are considered, so an unrelated date further up,
/// like a backup's, is never picked: `2011-07-14--Iceland/Originals` and
/// `Trips/2011/07/14` both yield 2011-07-14, but
/// `Backups/20190311/Trips/2011` yields nothing. The album folder's own date
/// wins over date-only folders below it.
///
/// Failing a full date, a year (and month, if any) is used with the rest
/// left unknown: `E3 2006` yields 2006, `July 2012` and `2012-07 Beach` yield
/// 2012-07, and `Trips/2019` yields 2019.
pub(crate) fn date_from_folders(dir: &Path) -> Option<PathDate> {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    // Date-only folders below the album folder, nearest first.
    let mut dated = Vec::new();
    let mut album_folder = None;
    for component in dir.components().rev() {
        let Some(name) = component.as_os_str().to_str() else {
            break;
        };
        if is_album_skip_dir(name) {
            continue;
        }
        if is_date_like(name) {
            dated.push(name);
            continue;
        }
        // The album folder, which ends the search.
        if let Some(date) = split_date_prefix(name).map(|(date, _)| date) {
            return Some(date.into());
        }
        album_folder = Some(name);
        break;
    }

    let full = dated
        .iter()
        .find_map(|name| split_date_prefix(name).map(|(date, _)| date))
        .or_else(|| {
            // Nested `YYYY/MM/DD` folders.
            let [day, month, year] = dated.get(..3)? else {
                return None;
            };
            if year.len() != 4 || month.len() > 2 || day.len() > 2 {
                return None;
            }
            valid_date([
                year.parse().ok()?,
                month.parse().ok()?,
                day.parse().ok()?,
            ])
        });
    if let Some(date) = full {
        return Some(date.into());
    }

    album_folder
        .and_then(partial_date_in_name)
        .or_else(|| partial_date_in_folders(&dated))
}

/// A year, and a month if one is named, found in a folder name like
/// `E3 2006`, `July 2012` or `2012-07 Beach`.
fn partial_date_in_name(name: &str) -> Option<PathDate> {
    let bytes = name.as_bytes();
    let (year, end) = (0..bytes.len()).find_map(|start| {
        let run = &bytes[start..];
        // A year is a run of exactly four digits.
        if !run[0].is_ascii_digit()
            || (start > 0 && bytes[start - 1].is_ascii_digit())
            || run.len() < 4
            || !run[..4].iter().all(u8::is_ascii_digit)
            || run.get(4).is_some_and(u8::is_ascii_digit)
        {
            return None;
        }
        let year: u32 = name[start..start + 4].parse().ok()?;
        (1900..=2100)
            .contains(&year)
            .then_some((year as i32, start + 4))
    })?;

    let month = month_from_words(name).or_else(|| {
        // `YYYY-MM`, `YYYY_MM` or `YYYY.MM` right after the year.
        let rest = name[end..].strip_prefix(['-', '_', '.'])?.as_bytes();
        if rest.len() < 2
            || !rest[..2].iter().all(u8::is_ascii_digit)
            || rest.get(2).is_some_and(u8::is_ascii_digit)
        {
            return None;
        }
        month_number(std::str::from_utf8(&rest[..2]).ok()?)
    });
    Some(PathDate::partial(year, month))
}

/// A year, and a month if the folder just below it is one, from date-only
/// folders (nearest first) like `2019/07` or `2019`.
fn partial_date_in_folders(dated: &[&str]) -> Option<PathDate> {
    dated.iter().enumerate().find_map(|(i, name)| {
        let mut date = partial_date_in_name(name)?;
        if date.month.is_none() && name.len() == 4 {
            date.month = i.checked_sub(1).and_then(|j| month_number(dated[j]));
        }
        Some(date)
    })
}

/// A month from a one or two digit number.
fn month_number(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 2 {
        return None;
    }
    s.parse().ok().filter(|m| (1..=12).contains(m))
}

/// The first month named, in full or abbreviated, as a word of `name`.
fn month_from_words(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    name.split(|c: char| !c.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .find_map(|word| {
            let word = word.to_ascii_lowercase();
            MONTHS
                .iter()
                .position(|month| {
                    *month == word
                        || (month.starts_with(&word)
                            && (3..=4).contains(&word.len()))
                })
                .map(|i| i as u32 + 1)
        })
}

/// Derive a date, and a time if there is one, from a file name like
/// `IMG_20190310_123456`, `PXL_20210101_123456789`, `2019-03-10 12.34.56` or
/// `IMG-20190310-WA0001`.
pub(crate) fn datetime_from_filename(
    stem: &str,
) -> Option<(NaiveDate, Option<NaiveTime>)> {
    let bytes = stem.as_bytes();
    (0..bytes.len()).find_map(|start| {
        // A date starts with a digit that is not part of a longer number.
        if !bytes[start].is_ascii_digit()
            || (start > 0 && bytes[start - 1].is_ascii_digit())
        {
            return None;
        }
        let (parts, rest) = date_prefix_parts(&stem[start..])?;
        let date = valid_date(parts)?;
        let time = time_prefix(rest);
        // Digits running on past the date without forming a time mean this
        // was some other number.
        if time.is_none() && rest.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        Some((date, time))
    })
}

/// Parse a leading `HHMMSS` or `HH.MM.SS` style time, optionally preceded by
/// one of `_`, `-`, ` ` or `T`. Digits after the seconds (e.g. milliseconds)
/// are ignored.
fn time_prefix(s: &str) -> Option<NaiveTime> {
    let s = s.strip_prefix(['_', '-', ' ', 'T']).unwrap_or(s);
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut parts = [0u32; 3];
    for (n, part) in parts.iter_mut().enumerate() {
        if n > 0 && matches!(bytes.get(i), Some(b'.' | b':' | b'-')) {
            i += 1;
        }
        if bytes.len() < i + 2
            || !bytes[i..i + 2].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        *part = s[i..i + 2].parse().ok()?;
        i += 2;
    }
    NaiveTime::from_hms_opt(parts[0], parts[1], parts[2])
}

fn is_album_skip_dir(name: &str) -> bool {
    ALBUM_SKIP_DIRS.contains(&name.to_lowercase().as_str())
}

fn is_date_like(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_digit() || "-_. ".contains(c))
}

/// Split a leading `YYYY-MM-DD`, `YYYY_MM_DD`, `YYYY.MM.DD` or `YYYYMMDD` into
/// its numbers and the rest of the name.
fn date_prefix_parts(name: &str) -> Option<([u32; 3], &str)> {
    let bytes = name.as_bytes();
    let mut i = 0;
    let mut parts = [0u32; 3];
    for (n, (part, width)) in parts.iter_mut().zip([4, 2, 2]).enumerate() {
        if n > 0 && matches!(bytes.get(i), Some(b'-' | b'_' | b'.')) {
            i += 1;
        }
        if bytes.len() < i + width
            || !bytes[i..i + width].iter().all(u8::is_ascii_digit)
        {
            return None;
        }
        *part = name[i..i + width].parse().ok()?;
        i += width;
    }
    Some((parts, &name[i..]))
}

/// Split a leading date, as [`date_prefix_parts`], into a valid date and the
/// rest of the name.
fn split_date_prefix(name: &str) -> Option<(NaiveDate, &str)> {
    let (parts, rest) = date_prefix_parts(name)?;
    Some((valid_date(parts)?, rest))
}

/// A date from `[year, month, day]`, if it is real and plausibly a photo's.
fn valid_date([year, month, day]: [u32; 3]) -> Option<NaiveDate> {
    if !(1900..=2100).contains(&year) {
        return None;
    }
    NaiveDate::from_ymd_opt(year as i32, month, day)
}

/// Strip a leading `YYYY-MM-DD`, `YYYY_MM_DD`, `YYYY.MM.DD` or `YYYYMMDD`
/// date and any separators following it.
fn strip_date_prefix(name: &str) -> &str {
    date_prefix_parts(name).map_or(name, |(_, rest)| {
        rest.trim_start_matches(['-', '_', '.', ' '])
    })
}

pub(crate) fn has_image_extension(entry: &walkdir::DirEntry) -> bool {
    has_image_extension_str(entry.file_name().to_str().unwrap_or(""))
}

fn has_image_extension_str(file_name: &str) -> bool {
    if let Some(extension) = Path::new(file_name).extension()
        && let Some(extension) = extension.to_str()
    {
        EXTENSIONS.contains(&extension.to_lowercase().as_str())
    } else {
        false
    }
}

/// Whether `path` is an XMP sidecar of a file this app would otherwise sort:
/// its extension is `xmp` (any case), and stripping it leaves the path to a
/// file with a recognized image/movie extension.
pub(crate) fn is_sidecar_of_media(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    if !extension.eq_ignore_ascii_case("xmp") {
        return false;
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(has_image_extension_str)
}

/// Signatures at the start of image and movie files.
const MEDIA_PREFIXES: &[&[u8]] = &[
    // JPEG, PNG, GIF, PSD.
    b"\xFF\xD8\xFF",
    b"\x89PNG\r\n\x1A\n",
    b"GIF87a",
    b"GIF89a",
    b"8BPS",
    // TIFF and the TIFF-based RAW formats (DNG, NEF, CR2, ARW, PEF, …).
    b"II*\0",
    b"MM\0*",
    // Other RAW formats: ORF, RW2, RAF, X3F, MRW.
    b"IIRO",
    b"IIRS",
    b"MMOR",
    b"IIU\0",
    b"FUJIFILMCCD-RAW",
    b"FOVb",
    b"\0MRM",
    // JPEG 2000 file and codestream.
    b"\0\0\0\x0CjP  ",
    b"\xFF\x4F\xFF\x51",
    // Matroska/WebM, MPEG program stream and video, ASF/WMV, FLV, Ogg.
    b"\x1A\x45\xDF\xA3",
    b"\0\0\x01\xBA",
    b"\0\0\x01\xB3",
    b"\x30\x26\xB2\x75\x8E\x66\xCF\x11",
    b"FLV\x01",
    b"OggS",
];

/// How much of a file [`is_media_file`] looks at; enough for the second
/// packet of an M2TS stream.
const MEDIA_HEADER_SIZE: u64 = 200;

/// Check a file's contents, rather than its extension, for an image or movie
/// signature.
pub(crate) fn is_media_file(path: &Path) -> Result<bool> {
    let mut header = Vec::with_capacity(MEDIA_HEADER_SIZE as usize);
    fs::File::open(path)
        .and_then(|file| file.take(MEDIA_HEADER_SIZE).read_to_end(&mut header))
        .with_context(|| format!("Unable to read '{}'.", path.display()))?;
    Ok(has_media_signature(&header))
}

fn has_media_signature(header: &[u8]) -> bool {
    let at = |offset: usize, magic: &[u8]| {
        header.get(offset..offset + magic.len()) == Some(magic)
    };
    MEDIA_PREFIXES.iter().any(|magic| at(0, magic))
        // BMP, with its reserved header bytes zero.
        || (at(0, b"BM") && at(6, b"\0\0\0\0"))
        // CRW.
        || at(6, b"HEAPCCDR")
        // ISO base media (HEIC, AVIF, MP4, MOV, 3GP, CR3, …).
        || at(4, b"ftyp")
        // Old QuickTime files starting with another atom; a small atom size
        // keeps text that happens to contain these words out.
        || (header.first() == Some(&0)
            && [b"moov", b"mdat", b"wide", b"free", b"skip"]
                .iter()
                .any(|atom| at(4, *atom)))
        // WebP, AVI, AMV.
        || (at(0, b"RIFF")
            && [b"WEBP", b"AVI ", b"AMV "].iter().any(|kind| at(8, *kind)))
        // MPEG transport stream and M2TS: sync bytes 188 bytes apart.
        || (at(0, b"G") && at(188, b"G"))
        || (at(4, b"G") && at(196, b"G"))
}

/// Files larger than 64MB use streaming hash to avoid memory pressure.
const STREAMING_THRESHOLD: u64 = 64 * 1024 * 1024;
/// Buffer size for streaming hash (64KB).
const HASH_BUFFER_SIZE: usize = 64 * 1024;

/// Compute XXH3-64 hash of a file.
/// Uses streaming for files larger than `STREAMING_THRESHOLD` to reduce memory
/// usage.
fn file_hash(path: &Path, size: u64) -> Result<u64> {
    let mut file = fs::File::open(path).with_context(|| {
        format!("Unable to open '{}' for hashing.", path.display())
    })?;

    if size <= STREAMING_THRESHOLD {
        // Small files: read entire file into memory (fast path).
        let mut buffer = Vec::with_capacity(size as usize);
        file.read_to_end(&mut buffer).with_context(|| {
            format!("Unable to read '{}' for hashing.", path.display())
        })?;
        Ok(xxh3_64(&buffer))
    } else {
        // Large files: stream with fixed buffer.
        let mut hasher = Xxh3::new();
        let mut buffer = [0u8; HASH_BUFFER_SIZE];
        loop {
            let bytes_read = file.read(&mut buffer).with_context(|| {
                format!("Unable to read '{}' for hashing.", path.display())
            })?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }
        Ok(hasher.digest())
    }
}

/// Check if two files are duplicates.
/// If `use_checksum` is true, compares file contents via XXH3 hash.
/// Otherwise, only compares file sizes.
fn files_match(
    source: &Path,
    dest: &Path,
    source_size: u64,
    dest_size: u64,
    use_checksum: bool,
) -> Result<bool> {
    if source_size != dest_size {
        return Ok(false);
    }
    if use_checksum {
        let source_hash = file_hash(source, source_size)?;
        let dest_hash = file_hash(dest, dest_size)?;
        Ok(source_hash == dest_hash)
    } else {
        Ok(true)
    }
}

/// Move a file, falling back to copy+delete with a progress bar for
/// cross-device moves.
fn move_or_copy(
    source: &Path,
    dest: &Path,
    multi: &MultiProgress,
) -> io::Result<()> {
    match fs::rename(source, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
            let size = source.metadata()?.len();
            let pb = multi.add(ProgressBar::new(size));
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{msg} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec}")
                    .unwrap()
                    .progress_chars("=> "),
            );
            pb.set_message(
                source
                    .file_name()
                    .unwrap_or(source.as_os_str())
                    .to_string_lossy()
                    .to_string(),
            );

            let file = fs::File::open(source)?;
            let mut reader = pb.wrap_read(BufReader::new(file));
            let mut writer = fs::File::create(dest)?;
            io::copy(&mut reader, &mut writer)?;
            pb.finish_and_clear();

            fs::remove_file(source)?;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Build a numbered variant of a destination path: `IMG_1234.jpg` with `n` of
/// 1 becomes `IMG_1234_1.jpg`.
fn numbered_path(dest: &Path, n: u32) -> PathBuf {
    let mut name: OsString =
        dest.file_stem().unwrap_or(dest.as_os_str()).to_os_string();
    name.push(format!("_{}", n));
    if let Some(extension) = dest.extension() {
        name.push(".");
        name.push(extension);
    }
    dest.with_file_name(name)
}

/// A destination name spoken for by a file this run is still dealing with.
enum Claim {
    /// A move is underway. The name holds an empty placeholder until it
    /// finishes, so anything else aiming here has to wait for the real
    /// contents before it can tell a duplicate from a collision.
    InFlight,
    /// A dry run would put this source here. Nothing is moved, so the claim
    /// stands for the rest of the run and the source stays the file to
    /// compare against.
    DryRun(PathBuf),
}

/// Destination names spoken for, so that files racing for the same name
/// neither overwrite each other nor mistake an in-progress move for a
/// different file. Entries are dropped as moves complete; dry-run entries
/// stay, since a dry run never makes the file that would replace them.
static CLAIMS: LazyLock<(Mutex<HashMap<PathBuf, Claim>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(HashMap::new()), Condvar::new()));

/// Releases an in-flight claim however the move turns out.
struct ClaimGuard(PathBuf);

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        CLAIMS.0.lock().unwrap().remove(&self.0);
        CLAIMS.1.notify_all();
    }
}

/// Read a file's size.
fn file_size(path: &Path) -> Result<u64> {
    Ok(path
        .metadata()
        .with_context(|| {
            format!("Unable to read size of '{}'.", path.display())
        })?
        .len())
}

/// Drop the source file once its contents are known to be at the destination.
fn discard_source(source_file: &Path, args: &ArgMatches) -> Result<()> {
    if args.get_flag("dry-run") {
        return Ok(());
    }
    if args.get_flag("remove-source") {
        fs::remove_file(source_file).with_context(|| {
            format!("Failed to remove {}.", source_file.display())
        })?;
        info!("Removed {}.", source_file.display());
    } else if args.get_flag("trash-source") {
        trash::delete(source_file).with_context(|| {
            format!("Failed to trash {}.", source_file.display())
        })?;
        info!("Trashed {}.", source_file.display());
    }
    Ok(())
}

/// Move `source_file` to `dest_file`, or, when a *different* file already sits
/// at that name, to the first free `name_1`, `name_2`, ... variant of it.
/// An existing duplicate is never copied again; the source is only skipped,
/// removed or trashed according to the flags.
///
/// Returns the path the source ended up at (or the path it matched), so
/// sidecar files can be named after it.
pub(crate) fn move_file(
    source_file: &Path,
    dest_file: &Path,
    checksum: bool,
    args: Arc<ArgMatches>,
    multi: &MultiProgress,
) -> Result<PathBuf> {
    let dry_run = args.get_flag("dry-run");
    let verbose = args.get_flag("verbose") || dry_run;

    if source_file == dest_file {
        if verbose {
            info!("{} is already in place, skipping.", source_file.display());
        }
        return Ok(dest_file.to_path_buf());
    }

    let source_size = file_size(source_file)?;

    // Find a free name, treating an identical file already at any of the
    // candidate names as a duplicate of the source.
    let mut target = dest_file.to_path_buf();
    let mut attempt = 0u32;
    loop {
        if target == source_file {
            // Numbering walked onto the source itself; it is already in place.
            if verbose {
                info!(
                    "{} is already in place, skipping.",
                    source_file.display()
                );
            }
            return Ok(target);
        }

        // Which file, if any, holds this name? Besides what is on disk that
        // covers names other threads are moving onto right now, and, on a
        // dry run, names earlier files in this same run would have taken.
        let occupant = {
            let mut claims = CLAIMS.0.lock().unwrap();
            loop {
                match claims.get(&target) {
                    // Wait for the move to land so the comparison below sees
                    // the real contents rather than the placeholder.
                    Some(Claim::InFlight) => {
                        claims = CLAIMS.1.wait(claims).unwrap();
                    }
                    Some(Claim::DryRun(claimant)) => {
                        break Some(claimant.clone());
                    }
                    None if target.exists() => break Some(target.clone()),
                    None if dry_run => {
                        claims.insert(
                            target.clone(),
                            Claim::DryRun(source_file.to_path_buf()),
                        );
                        break None;
                    }
                    None => {
                        // Claim the name on disk too, so a second exifmv
                        // cannot pick it; the move overwrites the
                        // placeholder.
                        match fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&target)
                        {
                            Ok(_) => {
                                claims.insert(target.clone(), Claim::InFlight);
                                break None;
                            }
                            // Another process got there first; treat the name
                            // as occupied by whatever it put there.
                            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                                break Some(target.clone());
                            }
                            Err(e) => {
                                return Err(e).with_context(|| {
                                    format!(
                                        "Unable to create '{}'.",
                                        target.display()
                                    )
                                });
                            }
                        }
                    }
                }
            }
        };

        let Some(occupant) = occupant else {
            break;
        };

        if files_match(
            source_file,
            &occupant,
            source_size,
            file_size(&occupant)?,
            checksum,
        )? {
            if verbose
                && !args.get_flag("remove-source")
                && !args.get_flag("trash-source")
            {
                let method = if checksum { "checksum" } else { "size" };
                info!(
                    "{} exists with matching {}; skipping {}.",
                    occupant.display(),
                    method,
                    source_file.display()
                );
            }
            discard_source(source_file, &args)?;
            return Ok(target);
        }

        attempt += 1;
        target = numbered_path(dest_file, attempt);
    }

    if verbose {
        if attempt > 0 {
            let method = if checksum { "content" } else { "size" };
            info!(
                "{} is taken by a file with different {}; moving {} ➔ {}",
                dest_file.display(),
                method,
                source_file.display(),
                target.display()
            );
        } else {
            info!("{} ➔ {}", source_file.display(), target.display());
        }
    }

    if !dry_run {
        let _claim = ClaimGuard(target.clone());
        move_or_copy(source_file, &target, multi)
            .map_err(|e| {
                // Do not leave the claimed placeholder behind on failure.
                let _ = fs::remove_file(&target);
                anyhow::Error::new(e)
            })
            .with_context(|| {
                format!(
                    "Unable to move {} to {}.",
                    source_file.display(),
                    target.display()
                )
            })?;
    }

    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(path: &str) -> Option<String> {
        album_from_path(Path::new(path))
    }

    #[test]
    fn album_iphoto_exports() {
        let base = "Backups_Old/201903110930/Staging/2019-03-10/\
                    iPhoto Exports/Album-0";
        for (dir, expected) in [
            ("2011-07-14--Iceland/Originals", "Iceland"),
            ("2012-03-09--Beach Weekend/Originals", "Beach Weekend"),
            ("2013-10-21--Garden", "Garden"),
            ("2014-06-02--Lake Cabin pics/Originals", "Lake Cabin pics"),
        ] {
            assert_eq!(
                album(&format!("{base}/{dir}")).as_deref(),
                Some(expected)
            );
        }
    }

    #[test]
    fn album_iphoto_library() {
        assert_eq!(
            album("Backups_Old/iPhoto Library/Modified/2011/Iceland")
                .as_deref(),
            Some("Iceland")
        );
    }

    #[test]
    fn album_skips_date_and_container_folders() {
        assert_eq!(album("Trips/2019/ORIGINALS").as_deref(), Some("Trips"));
        assert_eq!(album("Trips/2019-03-10").as_deref(), Some("Trips"));
        assert_eq!(album("Trips/201903110930").as_deref(), Some("Trips"));
        assert_eq!(album("2019/Originals"), None);
    }

    #[test]
    fn album_date_prefix_variants() {
        assert_eq!(album("20110714 Iceland").as_deref(), Some("Iceland"));
        assert_eq!(album("2011_07_14_Iceland").as_deref(), Some("Iceland"));
        assert_eq!(album("2011.07.14 Iceland").as_deref(), Some("Iceland"));
        assert_eq!(album("Album-0").as_deref(), Some("Album-0"));
    }

    fn folder_date(path: &str) -> Option<PathDate> {
        date_from_folders(Path::new(path))
    }

    fn ymd(year: i32, month: u32, day: u32) -> Option<PathDate> {
        NaiveDate::from_ymd_opt(year, month, day).map(PathDate::from)
    }

    fn partial(year: i32, month: Option<u32>) -> Option<PathDate> {
        Some(PathDate::partial(year, month))
    }

    #[test]
    fn folder_date_from_album_prefix() {
        let base = "Backups_Old/201903110930/Staging/2019-03-10/\
                    iPhoto Exports/Album-0";
        for (dir, expected) in [
            ("2011-07-14--Iceland/Originals", (2011, 7, 14)),
            ("2012-03-09--Beach Weekend/Originals", (2012, 3, 9)),
            ("2013-10-21--Garden", (2013, 10, 21)),
        ] {
            let (y, m, d) = expected;
            assert_eq!(folder_date(&format!("{base}/{dir}")), ymd(y, m, d));
        }
    }

    #[test]
    fn folder_date_from_nested_ymd() {
        assert_eq!(folder_date("Trips/2019/07/14"), ymd(2019, 7, 14));
    }

    #[test]
    fn folder_date_from_full_date_folder() {
        assert_eq!(folder_date("Trips/2019-03-10/Originals"), ymd(2019, 3, 10));
    }

    #[test]
    fn folder_date_none_without_year() {
        // Album folder reached with no date prefix, and nothing dated below.
        assert_eq!(
            folder_date("Backups_Old/201903110930/Staging/random"),
            None
        );
        // A backup's date above the album folder is never used.
        assert_eq!(folder_date("Backups/20190311/Trips/Iceland"), None);
    }

    #[test]
    fn folder_date_year_only() {
        let base = "Backups_Old/201502041445/StagedBackup/2015-01-24/\
                    iPhoto/Current";
        assert_eq!(
            folder_date(&format!("{base}/E3 2006")),
            partial(2006, None)
        );
        assert_eq!(folder_date("Trips/2019"), partial(2019, None));
    }

    #[test]
    fn folder_date_year_and_month() {
        let base = "Backups_Old/201502041445/StagedBackup/2015-01-24/\
                    iPhoto/Current";
        assert_eq!(
            folder_date(&format!("{base}/July 2012")),
            partial(2012, Some(7))
        );
        assert_eq!(folder_date("Sept 2014 Trip"), partial(2014, Some(9)));
        assert_eq!(
            folder_date("Trip to Paris, dec 2010"),
            partial(2010, Some(12))
        );
        assert_eq!(folder_date("2012-07 Beach"), partial(2012, Some(7)));
        assert_eq!(folder_date("2012_7 Beach"), partial(2012, None));
        assert_eq!(folder_date("Trips/2019/07"), partial(2019, Some(7)));
    }

    #[test]
    fn folder_date_partial_rejects_non_years() {
        assert_eq!(folder_date("Album201202"), None);
        assert_eq!(folder_date("Trip 3000"), None);
        assert_eq!(folder_date("Trip 20123"), None);
        assert_eq!(folder_date("Trips/123456789012"), None);
    }

    #[test]
    fn folder_date_full_beats_partial() {
        assert_eq!(folder_date("2011-07-14--Iceland 2012"), ymd(2011, 7, 14));
        assert_eq!(folder_date("July 2012/2013-02-03"), ymd(2013, 2, 3));
    }

    #[test]
    fn path_date_display_pads_unknown_parts() {
        assert_eq!(PathDate::partial(2006, None).to_string(), "2006-00-00");
        assert_eq!(PathDate::partial(2012, Some(7)).to_string(), "2012-07-00");
        assert_eq!(PathDate::partial(2012, Some(7)).full(), None);
    }

    #[test]
    fn datetime_from_filename_date_and_time() {
        assert_eq!(
            datetime_from_filename("IMG_20190310_123456"),
            Some((
                NaiveDate::from_ymd_opt(2019, 3, 10).unwrap(),
                NaiveTime::from_hms_opt(12, 34, 56)
            ))
        );
        // Trailing milliseconds after the seconds are ignored.
        assert_eq!(
            datetime_from_filename("PXL_20210101_123456789"),
            Some((
                NaiveDate::from_ymd_opt(2021, 1, 1).unwrap(),
                NaiveTime::from_hms_opt(12, 34, 56)
            ))
        );
        assert_eq!(
            datetime_from_filename("2019-03-10 12.34.56"),
            Some((
                NaiveDate::from_ymd_opt(2019, 3, 10).unwrap(),
                NaiveTime::from_hms_opt(12, 34, 56)
            ))
        );
    }

    #[test]
    fn datetime_from_filename_date_only() {
        assert_eq!(
            datetime_from_filename("Screenshot 2019-03-10 at 3.34.56 pm"),
            Some((NaiveDate::from_ymd_opt(2019, 3, 10).unwrap(), None))
        );
        assert_eq!(
            datetime_from_filename("IMG-20190310-WA0001"),
            Some((NaiveDate::from_ymd_opt(2019, 3, 10).unwrap(), None))
        );
    }

    #[test]
    fn datetime_from_filename_rejects_non_dates() {
        assert_eq!(datetime_from_filename("IMG_1234"), None);
        // Invalid month.
        assert_eq!(datetime_from_filename("20191340"), None);
        // No left boundary: part of a longer run of digits.
        assert_eq!(datetime_from_filename("123201903101"), None);
    }

    #[test]
    fn media_signatures() {
        assert!(has_media_signature(b"\xFF\xD8\xFF\xE0\0\x10JFIF"));
        assert!(has_media_signature(b"\x89PNG\r\n\x1A\n\0\0\0\rIHDR"));
        assert!(has_media_signature(b"II*\0\x08\0\0\0"));
        assert!(has_media_signature(b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom"));
        assert!(has_media_signature(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!has_media_signature(b"just some plain text data"));
        assert!(!has_media_signature(b""));
    }
}
