use crate::*;
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
        if ALBUM_SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
            return None;
        }
        let name = strip_date_prefix(name).trim();
        let is_date_like = name
            .chars()
            .all(|c| c.is_ascii_digit() || "-_. ".contains(c));
        (!is_date_like).then(|| name.to_string())
    })
}

/// Strip a leading `YYYY-MM-DD`, `YYYY_MM_DD`, `YYYY.MM.DD` or `YYYYMMDD`
/// date and any separators following it.
fn strip_date_prefix(name: &str) -> &str {
    let bytes = name.as_bytes();
    let mut i = 0;
    for (n, width) in [4, 2, 2].into_iter().enumerate() {
        if n > 0 && matches!(bytes.get(i), Some(b'-' | b'_' | b'.')) {
            i += 1;
        }
        if bytes.len() < i + width
            || !bytes[i..i + width].iter().all(u8::is_ascii_digit)
        {
            return name;
        }
        i += width;
    }
    name[i..].trim_start_matches(['-', '_', '.', ' '])
}

pub(crate) fn has_image_extension(entry: &walkdir::DirEntry) -> bool {
    if let Some(extension) = PathBuf::from(entry.file_name()).extension()
        && let Some(extension) = extension.to_str()
    {
        EXTENSIONS.contains(&extension.to_lowercase().as_str())
    } else {
        false
    }
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
}
