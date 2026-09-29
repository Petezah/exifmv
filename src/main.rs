#![recursion_limit = "1024"]
//! Moves images into a folder hierarchy based on EXIF tags.
//!
//! XMP sidecar files are also moved, if present.
//!
//! The folder hierarchy is configurable via a template string
//! (`-f`/`--format`). The default template is:
//!
//! `{year}/{month}/{day}/{filename}.{extension}`
//!
//! Available template variables: `year`, `month`, `day`, `hour`, `minute`,
//! `second`, `filename`, `extension`, `camera_make`, `camera_model`, `lens`,
//! `iso`, `focal_length`, `album`.
//!
//! `album` is taken from the image's folder name, for libraries that encode
//! events in the path, like old iPhoto exports: an image in
//! `2011-07-14--Iceland/Originals` gets the album `Iceland`. Container folders
//! (`Originals`, `Modified`, `Masters`, …) and purely numeric/date folders
//! (`2011`) are skipped, and a leading `YYYY-MM-DD` date is stripped.
//!
//! Run `exifmv --help` for full variable descriptions and examples.
//!
//! # Files Without EXIF
//!
//! Movies, and images whose EXIF has been stripped, have no `DateTimeOriginal`
//! to sort by. `--date-from` names sources to try instead, in priority order:
//! `filename` (a date, and optionally a time, encoded in the file name, e.g.
//! `IMG_20190310_123456.jpg`) and `folders` (a date encoded in the names of
//! the containing folders, as for `album` above). `exif` is implied first if
//! not listed, so `--date-from folders` tries EXIF, then folder dates; listing
//! `exif` explicitly lets a path date override it, e.g.
//! `--date-from folders,exif`.
//!
//! A `folders` date need not be complete: with no full date, a year (and
//! month, if named) in the album folder is used, as in `E3 2006` or
//! `July 2012`. The unknown month and day expand to `00`, e.g.
//! `2006/00/00/`, so such files are easy to find later.
//!
//! A `filename` or `folders` date is only trusted for a file whose contents
//! are recognized as an image or movie, checked independently of its
//! extension. If the destination template uses `{album}` and none can be
//! found, the file is skipped rather than filed under `unknown`. See
//! `--unclassified` below for filing these files instead of skipping them.
//!
//! # Unclassifiable Files
//!
//! By default a file `exifmv` can't sort — no date from any `--date-from`
//! source, a filename/folders date on something not recognized as media, or
//! a missing `{album}` — is left in place, reported, and (with
//! `--halt-on-errors`) treated as a failure.
//!
//! `--unclassified DIR` files such files into `DIR` instead, keeping each
//! one's path relative to SOURCE (e.g. `SOURCE/2011 trip/misc/clip.avi` ends
//! up at `DIR/2011 trip/misc/clip.avi`). This also covers any other,
//! non-hidden file under SOURCE that isn't recognized as an image or movie
//! by extension — so a run with `--unclassified` set can be pointed at a
//! folder of mixed content and sort out everything it recognizes. An XMP
//! sidecar always follows whichever path its image ends up at, sorted or
//! not.
//!
//! # Example
//!
//! If you have an image shot on _Aug. 15 2020_ named
//! `Foo1234.ARW` it will e.g. end up in a folder hierarchy like so:
//!
//! ```text
//! 2020
//! ├── 08
//! │   ├── 15
//! │   │   ├── foo1234.arw
//! │   │   ├── …
//! ```
//!
//! # Safety
//!
//! With default settings `exifmv` uses move/rename only for organizing files.
//! The only thing you risk is having files end up somewhere you didn’t intend.
//!
//! But – if you specify the `--remove-source` it will _remove the original_.
//!
//! > **In this case the original is permanently deleted!**
//!
//! Alternatively you can use the `--trash-source` which will move source files
//! to the user’s trash folder from where they can be restored to their original
//! location on most operating systems.
//!
//! Before doing any deletion or moving-to-trash `exifmv` checks that the file
//! size matches. Use `--checksum` to verify file contents instead, eliminating
//! false positives from same-size different-content files.
//!
//! # Name Collisions
//!
//! When two different photos would land on the same destination name, the
//! second one is moved aside as `IMG_1234_1.jpg`, `IMG_1234_2.jpg` and so on;
//! an existing file is never overwritten. A source that matches a file already
//! at any of those names is treated as a duplicate instead, so re-running
//! `exifmv` over the same photos does not pile up extra copies. XMP sidecars
//! follow the name their image ended up with.
//!
//! Note that "different" is judged by file size unless `--checksum` is given,
//! so same-size different-content photos are still taken for duplicates by
//! default.
//!
//! `--dry-run` predicts these names: it keeps track of the names it hands
//! out, so colliding files are reported under the names a real run would give
//! them, and a photo matching one already accounted for is reported as a
//! duplicate. Files are processed in parallel, so which photo gets which
//! number can differ between runs; the set of names does not.
//!
//! # Configuration File
//!
//! `exifmv` supports a TOML configuration file. The default location is
//! platform-specific (e.g., `~/.config/exifmv/config.toml` on Linux).
//!
//! ```toml
//! format = "{year}/{month}/{day}/{filename}.{extension}"
//! make-lowercase = true
//! recursive = true
//! day-wrap = "04:00"
//! verbose = false
//! halt-on-errors = false
//! dereference = false
//! checksum = false
//! date-from = ["folders", "filename"]
//! unclassified = "unsorted"
//! ```
//!
//! CLI arguments override config file settings.
//!
//! # Features
//!
//! - **color** (default): Enables colored CLI help output. Disable with
//!   `--no-default-features`.
//!
//! # History
//!
//! This is based on a Python script that did more or less the same thing and
//! which served me well for 15 years. When I started to learn Rust in 2018 I
//! decided to port the Python code to Rust as CLI app learning experience.
//!
//! As such this app may not be the prettiest code you’ve come across lately.
//! It may also contain non-idiomatic (aka: non-Rust) ways of doing stuff. If
//! you feel like fixing any of those or add some nice features, I look forward
//! to merge your PRs. Beers!
use anyhow::{Context, Result, anyhow};
use chrono::{Days, NaiveDate, NaiveTime, Timelike};
#[cfg(feature = "color")]
use clap::builder::styling::{AnsiColor, Styles};
use clap::{Arg, ArgAction, ArgMatches, arg, command};
use exif::{DateTime, Tag, Value};
use indicatif::MultiProgress;
use indicatif_log_bridge::LogWrapper;
use log::{info, warn};
use rayon::prelude::*;
use simplelog::*;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use walkdir::{DirEntry, WalkDir};

mod config;
mod template;
#[cfg(test)]
mod tests;
mod util;

use config::{Config as AppConfig, DateSource};
use template::{Template, TemplateContext};
use util::*;

#[cfg(feature = "color")]
const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().bold())
    .usage(AnsiColor::Green.on_default().bold())
    .literal(AnsiColor::Cyan.on_default().bold())
    .placeholder(AnsiColor::Cyan.on_default())
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Red.on_default())
    .error(AnsiColor::Red.on_default().bold());

fn main() -> Result<()> {
    // Get default config path for help text.
    let default_config_path =
        confy::get_configuration_file_path("exifmv", "config")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~/.config/exifmv/config.toml".into());
    let config_help =
        format!("Config file path [default: {default_config_path}]");

    #[cfg(feature = "color")]
    let cmd = command!().styles(STYLES);
    #[cfg(not(feature = "color"))]
    let cmd = command!();

    let args = cmd
        .author("Moritz Moeller <virtualritz@protonmail.com>")
        .about("Moves images into a folder hierarchy based on EXIF DateTime tags")
        .long_about("Moves images into a folder hierarchy based on EXIF DateTime tags.\nUse -f/--format to customize the destination path template. See -f for details.")
        .arg(
            arg!(-v --verbose "Babble a lot").action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("recursive")
                .short('r')
                .long("recursive")
                .help("Recurse subdirectories")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("trash-source")
                .long("trash-source")
                .conflicts_with("remove-source")
                .help("Move any SOURCE file existing at DESTINATION and matching in size to the system's trash")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("remove-source")
                .long("remove-source")
                .conflicts_with("trash-source")
                .help("Delete source files that already exist at the destination")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .help("Do not move any files (forces --verbose)")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("make-lowercase")
                .short('l')
                .long("make-lowercase")
                .help("Change filename & extension to lowercase")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("dereference-symlinks")
                .short('L')
                .long("dereference")
                .help("Dereference symbolic links")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("halt")
                .short('H')
                .long("halt-on-errors")
                .help("Exit if any errors are encountered")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("checksum")
                .long("checksum")
                .help("Verify file contents for duplicate detection")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("date-from")
                .long("date-from")
                .value_name("SOURCE[,SOURCE...]")
                .value_delimiter(',')
                .value_parser(["exif", "filename", "folders"])
                .help("Where to look for a file's date, in priority order")
                .long_help("\
Sources to try a file's date from, in priority order, for files whose EXIF\n\
DateTime is missing or unreadable (e.g. movies, or stripped images).\n\
\n\
  exif      DateTimeOriginal (implied first if not listed)\n\
  filename  a date, and optionally a time, encoded in the file name\n\
  folders   a date encoded in the names of the folders the file is in\n\
\n\
A `filename` or `folders` date is trusted only for a file whose contents are\n\
recognised as an image or movie, checked independently of the file's\n\
extension. A `folders` date without a time expands {hour}/{minute}/{second}\n\
as 00. With no full date, a `folders` date may be just a year, or a year and\n\
month, from names like `E3 2006` or `July 2012`; the unknown month/day\n\
expand as 00.\n\
\n\
Examples:\n\
  --date-from folders            ➞  try EXIF, then folder dates\n\
  --date-from filename,folders   ➞  try EXIF, then filename, then folders\n\
  --date-from folders,exif       ➞  trust a folder date over EXIF"),
        )
        /*.arg(
            Arg::new("cleanup")
                .short("c")
                .long("cleanup")
                .help("Remove empty directories (including hidden files)"),
        )*/
       .arg(
            Arg::new("day-wrap")
                .long("day-wrap")
                .value_name("H[H][:M[M]]")
                .help("The time at which the date wraps to the next day"),
        )
        .arg(
            Arg::new("format")
                .short('f')
                .long("format")
                .value_name("TEMPLATE")
                .help("Path format template – see --help for syntax")
                .long_help("\
Path format template for the destination file hierarchy.\n\
Variables are enclosed in braces. Literal braces: \\{ \\}\n\
\n\
Available variables:\n\
  Date/time (from EXIF DateTimeOriginal):\n\
    {year}          ➞  2024\n\
    {month}         ➞  08       (zero-padded)\n\
    {day}           ➞  15       (zero-padded)\n\
    {hour}          ➞  14       (zero-padded, 24h)\n\
    {minute}        ➞  30       (zero-padded)\n\
    {second}        ➞  00       (zero-padded)\n\
  File:\n\
    {filename}      ➞  IMG_1234 (stem, without extension)\n\
    {extension}     ➞  arw\n\
  Camera (from EXIF, 'unknown' if absent):\n\
    {camera_make}   ➞  Sony\n\
    {camera_model}  ➞  ILCE-7M3\n\
    {lens}          ➞  FE-35mm-F1.4-GM\n\
    {iso}           ➞  400\n\
    {focal_length}  ➞  35\n\
  Path (from the source folder, 'unknown' if none found):\n\
    {album}         ➞  Iceland  (nearest folder name, skipping\n\
                                 Originals/Modified/… and date-only\n\
                                 folders; leading YYYY-MM-DD stripped:\n\
                                 2011-07-14--Iceland/Originals ➞ Iceland)\n\
\n\
Examples:\n\
  Default:\n\
    {year}/{month}/{day}/{filename}.{extension}\n\
    ➞  2024/08/15/IMG_1234.arw\n\
  By camera and date:\n\
    {camera_make}/{camera_model}/{year}-{month}-{day}/{filename}.{extension}\n\
    ➞  Sony/ILCE-7M3/2024-08-15/IMG_1234.arw\n\
  Flat with timestamp:\n\
    {year}{month}{day}_{hour}{minute}{second}_{filename}.{extension}\n\
    ➞  20240815_143000_IMG_1234.arw\n\
  By date and album:\n\
    {year}/{year}-{month}-{day} {album}/{filename}.{extension}\n\
    ➞  2011/2011-07-14 Iceland/IMG_1234.jpg"),
        )
        .arg(
            Arg::new("unclassified")
                .long("unclassified")
                .value_name("DIR")
                .help("Move files that can't be sorted here instead of leaving them in place")
                .long_help("\
Move files exifmv can't sort into DIR instead of leaving them at SOURCE,\n\
each keeping its path relative to SOURCE (e.g. a file at\n\
'SOURCE/2011 trip/misc/clip.avi' ends up at 'DIR/2011 trip/misc/clip.avi').\n\
\n\
This covers:\n\
  - images/movies with no date from any --date-from source\n\
  - a filename/folders date on a file not recognized as media\n\
  - a template using {album} with none found in the path\n\
  - any other, non-hidden file under SOURCE that isn't an image or movie\n\
\n\
An XMP sidecar still follows whichever path its image ends up at.\n\
Files already under DIR (e.g. from a previous run) are left alone."),
        )
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("PATH")
                .help(config_help),
        )
        .arg(
            Arg::new("SOURCE")
                .required(true)
                .help("Where to search for images"),
        )
        .arg(
            Arg::new("DESTINATION")
                .required(false)
                .default_value(".")
                .help("Where to move the images"),
        )
        .get_matches();

    // Load config file.
    let config_path = args.get_one::<String>("config").map(PathBuf::from);
    let app_config = AppConfig::load(config_path.as_ref())?;

    // Merge CLI args with config (CLI wins).
    let verbose = args.get_flag("verbose")
        || args.get_flag("dry-run")
        || app_config.verbose.unwrap_or(false);
    let recursive =
        args.get_flag("recursive") || app_config.recursive.unwrap_or(false);
    let make_lowercase = args.get_flag("make-lowercase")
        || app_config.make_lowercase.unwrap_or(false);
    let halt =
        args.get_flag("halt") || app_config.halt_on_errors.unwrap_or(false);
    let dereference = args.get_flag("dereference-symlinks")
        || app_config.dereference.unwrap_or(false);
    let checksum =
        args.get_flag("checksum") || app_config.checksum.unwrap_or(false);

    let multi = MultiProgress::new();
    let logger = TermLogger::new(
        if verbose {
            LevelFilter::Info
        } else {
            LevelFilter::Warn
        },
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    );
    LogWrapper::new(multi.clone(), logger).try_init().unwrap();

    // Parse day-wrap time.
    let day_wrap_str = args
        .get_one::<String>("day-wrap")
        .map(String::as_str)
        .or(app_config.day_wrap.as_deref())
        .unwrap_or("00:00");
    let time_offset = NaiveTime::parse_from_str(day_wrap_str, "%H:%M")
        .with_context(|| {
            format!(
                "Option --day-wrap {} is formatted incorrectly.",
                day_wrap_str
            )
        })?;

    // Parse and validate template.
    let format_str = args
        .get_one::<String>("format")
        .map(String::as_str)
        .unwrap_or_else(|| app_config.format());
    let template = Template::parse(format_str)?;
    template.validate()?;

    // A CLI --date-from replaces the config value entirely.
    let date_sources = config::date_sources(
        args.get_many::<String>("date-from")
            .map(|values| {
                values
                    .map(|v| v.parse().unwrap())
                    .collect::<Vec<DateSource>>()
            })
            .unwrap_or_else(|| app_config.date_from.unwrap_or_default()),
    );

    let source: &String = args.get_one("SOURCE").unwrap();
    let source_root = PathBuf::from(source);
    let dest_dir =
        PathBuf::from(args.get_one::<String>("DESTINATION").unwrap());
    let unclassified_dir = args
        .get_one::<String>("unclassified")
        .map(PathBuf::from)
        .or(app_config.unclassified);

    let (media_files, other_files) = collect_files(
        &source_root,
        recursive,
        dereference,
        unclassified_dir.as_deref(),
    );

    let args = Arc::new(args);
    let template = Arc::new(template);
    let multi = Arc::new(multi);

    let errors: Vec<_> = media_files
        .par_iter()
        .filter_map(|file| {
            let result = move_image(
                file.path(),
                &source_root,
                &dest_dir,
                unclassified_dir.as_deref(),
                &time_offset,
                &template,
                make_lowercase,
                checksum,
                &date_sources,
                args.clone(),
                multi.clone(),
            );
            match result {
                Ok(()) => None,
                Err(e) => {
                    warn!("{:#}", e);
                    Some(e)
                }
            }
        })
        .chain(other_files.par_iter().filter_map(|file| {
            // `collect_files` only returns "other" files when
            // `unclassified_dir` is set.
            let dir = unclassified_dir.as_deref().unwrap();
            let result = move_unclassified(
                file.path(),
                &source_root,
                dir,
                checksum,
                args.clone(),
                multi.clone(),
            );
            match result {
                Ok(()) => None,
                Err(e) => {
                    warn!("{:#}", e);
                    Some(e)
                }
            }
        }))
        .collect();

    if halt && !errors.is_empty() {
        Err(anyhow!("{} error(s) encountered.", errors.len()))
    } else {
        Ok(())
    }
}

/// Walk `source`, splitting entries into sortable media files and, when
/// `unclassified` is set, every other non-hidden file (excluding XMP
/// sidecars of a media file, which follow their image).
///
/// Note: `contents_first(true)` must not be combined with `filter_entry`;
/// walkdir ends the iteration early when a directory is filtered out,
/// silently skipping everything after it.
fn collect_files(
    source: &Path,
    recursive: bool,
    dereference: bool,
    unclassified: Option<&Path>,
) -> (Vec<DirEntry>, Vec<DirEntry>) {
    // Skip the unclassified folder itself if it happens to live inside
    // SOURCE, so a re-run doesn't file its own output back into itself.
    let unclassified_canon = unclassified.and_then(|d| d.canonicalize().ok());

    let mut media_files = Vec::new();
    let mut other_files = Vec::new();

    WalkDir::new(source)
        .max_depth(if recursive { usize::MAX } else { 1 })
        .follow_links(dereference)
        .sort_by(|a, b| a.file_name().cmp(b.file_name()))
        .into_iter()
        .filter_entry(|e| {
            is_not_hidden(e)
                && match &unclassified_canon {
                    Some(dir) => e
                        .path()
                        .canonicalize()
                        .map(|p| p != *dir)
                        .unwrap_or(true),
                    None => true,
                }
        })
        .filter_map(|e| match e {
            Ok(e) => Some(e),
            // Report unreadable entries but keep walking the rest of the tree.
            Err(e) => {
                warn!("{:#}", e);
                None
            }
        })
        .filter(|e| e.file_type().is_file())
        .for_each(|e| {
            if has_image_extension(&e) {
                media_files.push(e);
            } else if unclassified.is_some() && !is_sidecar_of_media(e.path()) {
                other_files.push(e);
            }
        });

    (media_files, other_files)
}

fn is_not_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| entry.depth() == 0 || !s.starts_with('.'))
        .unwrap_or(false)
}

/// A file's date, found from one of the configured [`DateSource`]s.
struct FoundDate {
    date: PathDate,
    /// `None` when the source (currently only `folders`) can't supply a time.
    time: Option<(u32, u32, u32)>,
    source: DateSource,
}

/// Try `source_file`'s EXIF `DateTimeOriginal`.
fn date_from_exif(meta_data: Option<&exif::Exif>) -> Option<FoundDate> {
    let time_stamp = meta_data
        .and_then(|meta| {
            meta.get_field(Tag::DateTimeOriginal, exif::In::PRIMARY)
        })
        .and_then(|f| match f.value {
            Value::Ascii(ref vec) if !vec.is_empty() => {
                DateTime::from_ascii(&vec[0]).ok()
            }
            _ => None,
        })?;

    let date = NaiveDate::from_ymd_opt(
        time_stamp.year as i32,
        time_stamp.month as u32,
        time_stamp.day as u32,
    )?;

    Some(FoundDate {
        date: date.into(),
        time: Some((
            time_stamp.hour as u32,
            time_stamp.minute as u32,
            time_stamp.second as u32,
        )),
        source: DateSource::Exif,
    })
}

/// Marks an error as meaning "this file can't be sorted", as opposed to an
/// I/O failure. When `--unclassified` is set, an error of this kind routes
/// the file there instead of being reported as a failure.
#[derive(Debug)]
struct Unclassifiable(String);

impl std::fmt::Display for Unclassifiable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Unclassifiable {}

/// The relative destination path an image, EXIF metadata (for sidecars, if
/// wanted later), and the file's found date decided together, without
/// touching the filesystem beyond reading `source_file` itself.
#[allow(clippy::too_many_arguments)]
fn classify(
    source_file: &Path,
    time_offset: &NaiveTime,
    template: &Template,
    make_lowercase: bool,
    date_sources: &[DateSource],
) -> Result<PathBuf> {
    let source_file_handle =
        std::fs::File::open(source_file).with_context(|| {
            format!("Unable to open '{}'.", source_file.display())
        })?;

    let exif_reader = exif::Reader::new();
    // A file this crate can't parse as EXIF at all (e.g. a movie) is not an
    // error here; it just means EXIF doesn't supply a date.
    let meta_data = exif_reader
        .read_from_container(&mut std::io::BufReader::new(&source_file_handle))
        .ok();

    // Extract filename and extension.
    let file_stem = source_file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let extension = source_file
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let found = date_sources
        .iter()
        .find_map(|source| match source {
            DateSource::Exif => date_from_exif(meta_data.as_ref()),
            DateSource::Filename => {
                datetime_from_filename(file_stem).map(|(date, time)| {
                    FoundDate {
                        date: date.into(),
                        time: time.map(|t| (t.hour(), t.minute(), t.second())),
                        source: DateSource::Filename,
                    }
                })
            }
            DateSource::Folders => source_file
                .parent()
                .and_then(date_from_folders)
                .map(|date| FoundDate {
                    date,
                    time: None,
                    source: DateSource::Folders,
                }),
        })
        .ok_or_else(|| {
            Unclassifiable(format!(
                "No timestamp in {} of '{}'.",
                date_sources
                    .iter()
                    .map(|s| match s {
                        DateSource::Exif => "EXIF",
                        DateSource::Filename => "the filename",
                        DateSource::Folders => "the containing folders",
                    })
                    .collect::<Vec<_>>()
                    .join(" or "),
                source_file.display()
            ))
        })?;

    // A date taken from the path is only trusted for a file that is really
    // an image or movie, checked independently of its extension.
    if found.source != DateSource::Exif
        && !is_media_file(source_file).with_context(|| {
            format!(
                "Unable to check '{}' for a media signature.",
                source_file.display()
            )
        })?
    {
        return Err(anyhow!(Unclassifiable(format!(
            "'{}' is not recognized as an image or movie; refusing to trust \
             its {} date.",
            source_file.display(),
            if found.source == DateSource::Filename {
                "filename"
            } else {
                "folder"
            }
        ))));
    }

    if found.source != DateSource::Exif {
        info!(
            "Using {} date {} for '{}'.",
            if found.source == DateSource::Filename {
                "filename"
            } else {
                "folder"
            },
            found.date,
            source_file.display()
        );
    }

    // Only a known time of day can wrap into the next day.
    let date = if let Some((hour, minute, _)) = found.time
        && day_wrap(hour, minute, time_offset) == 1
    {
        found
            .date
            .full()
            .and_then(|date| date.checked_add_days(Days::new(1)))
            .with_context(|| {
                format!("Date overflow for '{}'.", source_file.display())
            })?
            .into()
    } else {
        found.date
    };

    let (hour, minute, second) = found.time.unwrap_or((0, 0, 0));

    let album = source_file.parent().and_then(album_from_path).map(|album| {
        if make_lowercase {
            album.to_lowercase()
        } else {
            album
        }
    });

    require_album_if_needed(
        found.source,
        template,
        album.as_deref(),
        source_file,
    )?;

    // Build template context.
    let ctx = TemplateContext {
        year: format!("{}", date.year),
        month: format!("{:02}", date.month.unwrap_or(0)),
        day: format!("{:02}", date.day.unwrap_or(0)),
        hour: format!("{:02}", hour),
        minute: format!("{:02}", minute),
        second: format!("{:02}", second),
        filename: if make_lowercase {
            file_stem.to_lowercase()
        } else {
            file_stem.to_string()
        },
        extension: if make_lowercase {
            extension.to_lowercase()
        } else {
            extension.to_string()
        },
        camera_make: exif_string(meta_data.as_ref(), Tag::Make),
        camera_model: exif_string(meta_data.as_ref(), Tag::Model),
        lens: exif_string(meta_data.as_ref(), Tag::LensModel),
        iso: exif_string(meta_data.as_ref(), Tag::PhotographicSensitivity),
        focal_length: exif_string(meta_data.as_ref(), Tag::FocalLength)
            .map(|s| s.trim_end_matches("-mm").to_string()),
        album,
    };

    // Expand template to get relative path.
    Ok(PathBuf::from(template.expand(&ctx)))
}

/// Move `source_file` to `dest_dir.join(relative_path)`, creating parent
/// directories as needed, then move along any XMP sidecar.
fn move_with_sidecars(
    source_file: &Path,
    dest_dir: &Path,
    relative_path: &Path,
    make_lowercase: bool,
    checksum: bool,
    args: Arc<ArgMatches>,
    multi: Arc<MultiProgress>,
) -> Result<()> {
    let dest_file = dest_dir.join(relative_path);

    // Create parent directories.
    if let Some(parent) = dest_file.parent()
        && !args.get_flag("dry-run")
        && !parent.exists()
    {
        info!("Creating folder {}", parent.display());

        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Unable to create destination folder '{}'.",
                parent.display()
            )
        })?;
    }

    // The image may have been given a unique name to avoid clobbering a
    // different file; sidecars follow whatever name it ended up with.
    let mut dest_file =
        move_file(source_file, &dest_file, checksum, args.clone(), &multi)?;

    // Move possible sidecar files.
    let source_xmp_file = source_file.to_path_buf();

    let mut source_xmp_file_lower = source_xmp_file.clone();
    source_xmp_file_lower.as_mut_os_string().push(".xmp");

    let mut source_xmp_file_upper = source_xmp_file.clone();
    source_xmp_file_upper.as_mut_os_string().push(".XMP");

    if source_xmp_file_lower.exists() {
        dest_file.as_mut_os_string().push(".xmp");

        move_file(&source_xmp_file_lower, &dest_file, checksum, args, &multi)?;
    } else if source_xmp_file_upper.exists() {
        if make_lowercase {
            dest_file.as_mut_os_string().push(".xmp");
        } else {
            dest_file.as_mut_os_string().push(".XMP");
        };

        move_file(&source_xmp_file_upper, &dest_file, checksum, args, &multi)?;
    }

    Ok(())
}

/// `source_file`'s path relative to `source_root`, for filing it elsewhere
/// while preserving its place in the tree. Falls back to just the file name
/// if `source_root` is the file itself (a non-recursive run given a file as
/// SOURCE) or otherwise not a prefix of it.
pub(crate) fn relative_to_source(
    source_file: &Path,
    source_root: &Path,
) -> PathBuf {
    match source_file.strip_prefix(source_root) {
        Ok(relative) if !relative.as_os_str().is_empty() => {
            relative.to_path_buf()
        }
        _ => PathBuf::from(
            source_file.file_name().unwrap_or(source_file.as_os_str()),
        ),
    }
}

/// Sort `source_file` into `dest_dir` via `--format`, or, if it can't be
/// classified and `unclassified` is set, file it there instead, keeping its
/// path relative to `source_root`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn move_image(
    source_file: &Path,
    source_root: &Path,
    dest_dir: &Path,
    unclassified: Option<&Path>,
    time_offset: &NaiveTime,
    template: &Template,
    make_lowercase: bool,
    checksum: bool,
    date_sources: &[DateSource],
    args: Arc<ArgMatches>,
    multi: Arc<MultiProgress>,
) -> Result<()> {
    match classify(
        source_file,
        time_offset,
        template,
        make_lowercase,
        date_sources,
    ) {
        Ok(relative_path) => move_with_sidecars(
            source_file,
            dest_dir,
            &relative_path,
            make_lowercase,
            checksum,
            args,
            multi,
        ),
        Err(e)
            if unclassified.is_some()
                && e.downcast_ref::<Unclassifiable>().is_some() =>
        {
            let dir = unclassified.unwrap();
            info!(
                "{:#} Moving '{}' to unclassified instead.",
                e,
                source_file.display()
            );
            move_with_sidecars(
                source_file,
                dir,
                &relative_to_source(source_file, source_root),
                false,
                checksum,
                args,
                multi,
            )
        }
        Err(e) => Err(e),
    }
}

/// Move a non-media file (or one exifmv doesn't otherwise touch) straight
/// into the unclassified folder, keeping its path relative to `source_root`.
fn move_unclassified(
    source_file: &Path,
    source_root: &Path,
    dir: &Path,
    checksum: bool,
    args: Arc<ArgMatches>,
    multi: Arc<MultiProgress>,
) -> Result<()> {
    move_with_sidecars(
        source_file,
        dir,
        &relative_to_source(source_file, source_root),
        false,
        checksum,
        args,
        multi,
    )
}

/// Extract a string value from EXIF metadata.
/// Spaces are replaced with hyphens for filesystem-friendly paths.
/// A date taken from the path (rather than EXIF) is only good enough for a
/// template that spells out `{album}` if an album was actually found;
/// otherwise the file would silently land under `unknown`.
pub(crate) fn require_album_if_needed(
    date_source: DateSource,
    template: &Template,
    album: Option<&str>,
    source_file: &Path,
) -> Result<()> {
    if date_source != DateSource::Exif
        && template.uses("album")
        && album.is_none()
    {
        return Err(anyhow!(Unclassifiable(format!(
            "No album found in the path of '{}'.",
            source_file.display()
        ))));
    }
    Ok(())
}

fn exif_string(meta_data: Option<&exif::Exif>, tag: Tag) -> Option<String> {
    meta_data?
        .get_field(tag, exif::In::PRIMARY)
        .map(|f| f.display_value().to_string().trim().replace(' ', "-"))
        .filter(|s| !s.is_empty())
}

pub(crate) fn day_wrap(hour: u32, minute: u32, time_offset: &NaiveTime) -> u8 {
    // Hour wrap.
    if hour + time_offset.hour() + {
        // Minute wrap.
        if minute + time_offset.minute() > 59 {
            1
        } else {
            0
        }
    } > 23
    {
        1
    } else {
        0
    }
}

#[test]
fn test_day_wrap() {
    assert_eq!(
        1,
        day_wrap(23, 59, &NaiveTime::from_hms_opt(0, 1, 0).unwrap())
    );
    assert_eq!(
        0,
        day_wrap(23, 59, &NaiveTime::from_hms_opt(0, 0, 0).unwrap())
    );
}
