# `exifmv`

![build](https://github.com/virtualritz/exifmv/workflows/build/badge.svg)
![Maintenance](https://img.shields.io/badge/maintenance-passively--maintained-yellowgreen.svg)

Moves images into a folder hierarchy based on EXIF tags.

XMP sidecar files are also moved, if present.

The folder hierarchy is configurable via a template string
(`-f`/`--format`). The default template is:

`{year}/{month}/{day}/{filename}.{extension}`

Available template variables: `year`, `month`, `day`, `hour`, `minute`,
`second`, `filename`, `extension`, `camera_make`, `camera_model`, `lens`,
`iso`, `focal_length`, `album`.

`album` is taken from the image's folder name, for libraries that encode
events in the path, like old iPhoto exports: an image in
`2011-07-14--Iceland/Originals` gets the album `Iceland`. Container folders
(`Originals`, `Modified`, `Masters`, …) and purely numeric/date folders
(`2011`) are skipped, and a leading `YYYY-MM-DD` date is stripped.

Run `exifmv --help` for full variable descriptions and examples.

## Files Without EXIF

Movies, and images whose EXIF has been stripped, have no `DateTimeOriginal`
to sort by. `--date-from` names sources to try instead, in priority order:
`filename` (a date, and optionally a time, encoded in the file name, e.g.
`IMG_20190310_123456.jpg`) and `folders` (a date encoded in the names of
the containing folders, as for `album` above). `exif` is implied first if
not listed, so `--date-from folders` tries EXIF, then folder dates; listing
`exif` explicitly lets a path date override it, e.g.
`--date-from folders,exif`.

A `filename` or `folders` date is only trusted for a file whose contents
are recognized as an image or movie, checked independently of its
extension. If the destination template uses `{album}` and none can be
found, the file is skipped rather than filed under `unknown`. See
`--unclassified` below for filing these files instead of skipping them.

## Unclassifiable Files

By default a file `exifmv` can't sort — no date from any `--date-from`
source, a filename/folders date on something not recognized as media, or
a missing `{album}` — is left in place, reported, and (with
`--halt-on-errors`) treated as a failure.

`--unclassified DIR` files such files into `DIR` instead, keeping each
one's path relative to SOURCE (e.g. `SOURCE/2011 trip/misc/clip.avi` ends
up at `DIR/2011 trip/misc/clip.avi`). This also covers any other,
non-hidden file under SOURCE that isn't recognized as an image or movie
by extension — so a run with `--unclassified` set can be pointed at a
folder of mixed content and sort out everything it recognizes. An XMP
sidecar always follows whichever path its image ends up at, sorted or
not.

## Example

If you have an image shot on _Aug. 15 2020_ named
`Foo1234.ARW` it will e.g. end up in a folder hierarchy like so:

```
2020
├── 08
│   ├── 15
│   │   ├── foo1234.arw
│   │   ├── …
```

## Safety

With default settings `exifmv` uses move/rename only for organizing files.
The only thing you risk is having files end up somewhere you didn’t intend.

But – if you specify the `--remove-source` it will _remove the original_.

> **In this case the original is permanently deleted!**

Alternatively you can use the `--trash-source` which will move source files
to the user’s trash folder from where they can be restored to their original
location on most operating systems.

Before doing any deletion or moving-to-trash `exifmv` checks that the file
size matches. Use `--checksum` to verify file contents instead, eliminating
false positives from same-size different-content files.

## Name Collisions

When two different photos would land on the same destination name, the
second one is moved aside as `IMG_1234_1.jpg`, `IMG_1234_2.jpg` and so on;
an existing file is never overwritten. A source that matches a file already
at any of those names is treated as a duplicate instead, so re-running
`exifmv` over the same photos does not pile up extra copies. XMP sidecars
follow the name their image ended up with.

Note that "different" is judged by file size unless `--checksum` is given,
so same-size different-content photos are still taken for duplicates by
default.

`--dry-run` predicts these names: it keeps track of the names it hands
out, so colliding files are reported under the names a real run would give
them, and a photo matching one already accounted for is reported as a
duplicate. Files are processed in parallel, so which photo gets which
number can differ between runs; the set of names does not.

## Configuration File

`exifmv` supports a TOML configuration file. The default location is
platform-specific (e.g., `~/.config/exifmv/config.toml` on Linux).

```toml
format = "{year}/{month}/{day}/{filename}.{extension}"
make-lowercase = true
recursive = true
day-wrap = "04:00"
verbose = false
halt-on-errors = false
dereference = false
checksum = false
date-from = ["folders", "filename"]
unclassified = "unsorted"
```

CLI arguments override config file settings.

## Features

- **color** (default): Enables colored CLI help output. Disable with
  `--no-default-features`.

## History

This is based on a Python script that did more or less the same thing and
which served me well for 15 years. When I started to learn Rust in 2018 I
decided to port the Python code to Rust as CLI app learning experience.

As such this app may not be the prettiest code you’ve come across lately.
It may also contain non-idiomatic (aka: non-Rust) ways of doing stuff. If
you feel like fixing any of those or add some nice features, I look forward
to merge your PRs. Beers!

Current version: 0.5.3

## License

Apache-2.0 OR BSD-3-Clause OR MIT OR Zlib
