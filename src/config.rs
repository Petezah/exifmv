//! Configuration file loading and management.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default format string for destination paths.
pub const DEFAULT_FORMAT: &str = "{year}/{month}/{day}/{filename}.{extension}";

/// Application name for confy.
const APP_NAME: &str = "exifmv";

/// A source `exifmv` can take a file's date from, in addition to EXIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DateSource {
    /// The EXIF `DateTimeOriginal` tag.
    Exif,
    /// A date (and, if present, a time) encoded in the file name.
    Filename,
    /// A date encoded in the names of the folders the file lives in.
    Folders,
}

impl std::str::FromStr for DateSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "exif" => Ok(Self::Exif),
            "filename" => Ok(Self::Filename),
            "folders" => Ok(Self::Folders),
            _ => Err(format!(
                "'{s}' is not a valid date source. Valid sources: exif, filename, folders."
            )),
        }
    }
}

/// Order the sources to try a file's date from: put `exif` first if the
/// caller didn't mention it, and drop any source repeated later in the list.
pub fn date_sources(mut sources: Vec<DateSource>) -> Vec<DateSource> {
    if sources.is_empty() {
        return vec![DateSource::Exif];
    }
    if !sources.contains(&DateSource::Exif) {
        sources.insert(0, DateSource::Exif);
    }
    let mut seen = Vec::with_capacity(sources.len());
    sources.retain(|source| {
        if seen.contains(source) {
            false
        } else {
            seen.push(*source);
            true
        }
    });
    sources
}

/// Configuration loaded from TOML file.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    /// Path format template.
    pub format: Option<String>,
    /// Change filename & extension to lowercase.
    pub make_lowercase: Option<bool>,
    /// Recurse subdirectories.
    pub recursive: Option<bool>,
    /// Time at which date wraps to next day.
    pub day_wrap: Option<String>,
    /// Verbose output.
    pub verbose: Option<bool>,
    /// Exit on first error.
    pub halt_on_errors: Option<bool>,
    /// Follow symbolic links.
    pub dereference: Option<bool>,
    /// Use checksum for duplicate detection instead of size.
    pub checksum: Option<bool>,
    /// Sources to try a file's date from, in priority order, when EXIF alone
    /// isn't enough. `exif` is implied first if not listed.
    pub date_from: Option<Vec<DateSource>>,
    /// Folder for files that can't be sorted, keeping their path relative to
    /// the search root.
    pub unclassified: Option<PathBuf>,
}

impl Config {
    /// Load config from the given path, or the default path if `None`.
    /// Returns default config if file doesn't exist.
    pub fn load(path: Option<&PathBuf>) -> Result<Self> {
        if let Some(path) = path {
            Ok(confy::load_path(path)?)
        } else {
            Ok(confy::load(APP_NAME, "config")?)
        }
    }

    /// Returns the format string, using default if not specified.
    pub fn format(&self) -> &str {
        self.format.as_deref().unwrap_or(DEFAULT_FORMAT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config() {
        let toml = r#"
format = "{year}-{month}-{day}/{filename}.{extension}"
make-lowercase = true
day-wrap = "04:00"
verbose = false
unclassified = "unsorted"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            config.format.as_deref(),
            Some("{year}-{month}-{day}/{filename}.{extension}")
        );
        assert_eq!(config.make_lowercase, Some(true));
        assert_eq!(config.day_wrap.as_deref(), Some("04:00"));
        assert_eq!(config.verbose, Some(false));
        assert_eq!(config.unclassified, Some(PathBuf::from("unsorted")));
    }

    #[test]
    fn empty_config() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.format.is_none());
        assert!(config.make_lowercase.is_none());
        assert!(config.date_from.is_none());
        assert!(config.unclassified.is_none());
    }

    #[test]
    fn parse_config_date_from() {
        let toml = r#"date-from = ["folders", "filename"]"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(
            config.date_from,
            Some(vec![DateSource::Folders, DateSource::Filename])
        );
    }

    #[test]
    fn date_sources_orders_and_dedups() {
        assert_eq!(date_sources(vec![]), vec![DateSource::Exif]);
        assert_eq!(
            date_sources(vec![DateSource::Folders]),
            vec![DateSource::Exif, DateSource::Folders]
        );
        assert_eq!(
            date_sources(vec![DateSource::Folders, DateSource::Exif]),
            vec![DateSource::Folders, DateSource::Exif]
        );
        assert_eq!(
            date_sources(vec![
                DateSource::Folders,
                DateSource::Folders,
                DateSource::Exif
            ]),
            vec![DateSource::Folders, DateSource::Exif]
        );
    }
}
