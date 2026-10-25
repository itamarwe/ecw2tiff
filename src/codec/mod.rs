#![forbid(unsafe_code)]
//! Decoder for ECW (Enhanced Compression Wavelet) raster images, versions 1–3.
//!
//! Usage: open a file with [`EcwImage::open`], inspect [`EcwImage::info`],
//! then pull output rows of a [`Window`] through [`EcwImage::rows`]. Several
//! [`RowReader`]s may run concurrently on one shared image; each reads the
//! file with positional reads and keeps only a few rows per pyramid level in
//! memory.

mod entropy;
mod header;
mod io;
mod reader;
mod recon;

use std::fmt;
use std::path::Path;

pub use reader::RowReader;

/// Broad classification of an [`Error`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The operating system reported an I/O failure.
    Io,
    /// The file is structurally invalid, truncated or corrupt.
    Malformed,
    /// The file uses a feature this decoder does not implement.
    Unsupported,
    /// The caller asked for something invalid (bad window, exhausted reader…).
    InvalidRequest,
}

/// Error returned by every fallible operation of this module.
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn malformed(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Malformed, message)
    }

    pub(crate) fn unsupported(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Unsupported, message)
    }

    pub(crate) fn request(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::InvalidRequest, message)
    }

    pub(crate) fn io(err: &std::io::Error) -> Error {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::malformed(format!("unexpected end of file: {err}"))
        } else {
            Error::new(ErrorKind::Io, format!("I/O error: {err}"))
        }
    }

    /// The category of this error.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error({:?}: {})", self.kind, self.message)
    }
}

impl std::error::Error for Error {}

/// Result alias used throughout this module.
pub type Result<T> = std::result::Result<T, Error>;

/// Colour model stored in the header (codes 0..=4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorModel {
    Unspecified,
    Greyscale,
    YCbCr,
    Multiband,
    Rgb,
}

impl ColorModel {
    pub(crate) fn from_code(code: u8) -> Result<ColorModel> {
        Ok(match code {
            0 => ColorModel::Unspecified,
            1 => ColorModel::Greyscale,
            2 => ColorModel::YCbCr,
            3 => ColorModel::Multiband,
            4 => ColorModel::Rgb,
            other => return Err(Error::unsupported(format!("unknown colour model {other}"))),
        })
    }
}

/// Global description of an image.
#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub format_version: u8,
    pub width: u32,
    pub height: u32,
    pub bands: usize,
    pub color_model: ColorModel,
    pub stages: usize,
    pub tile_width: u32,
    pub tile_height: u32,
    pub target_ratio: u16,
    pub cell_unit_code: u8,
    pub cell_size_x: f64,
    pub cell_size_y: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub datum: String,
    pub projection: String,
    pub epsg: Option<u16>,
}

impl ImageInfo {
    /// Number of interleaved 8-bit samples per output pixel.
    pub fn channels(&self) -> usize {
        match self.color_model {
            ColorModel::Greyscale => 1,
            ColorModel::YCbCr => 3,
            ColorModel::Unspecified | ColorModel::Multiband | ColorModel::Rgb => self.bands.min(3),
        }
    }
}

/// Geometry and quantization of one pyramid stage.
#[derive(Clone, Debug)]
pub struct StageInfo {
    pub width: u32,
    pub height: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub quant_steps: Vec<u32>,
}

/// Inclusive full-resolution pixel window plus output size.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub out_width: u32,
    pub out_height: u32,
}

/// An opened ECW file. Cheap to share between threads.
pub struct EcwImage {
    file: io::FileHandle,
    layout: header::Layout,
}

impl EcwImage {
    /// Open and validate an ECW file. Only the header and tile index are read.
    pub fn open(path: &Path) -> Result<EcwImage> {
        let file = io::FileHandle::open(path)?;
        let layout = header::parse(&file)?;
        Ok(EcwImage { file, layout })
    }

    /// Image description.
    pub fn info(&self) -> &ImageInfo {
        &self.layout.info
    }

    /// Per-stage geometry, coarsest stage first.
    pub fn stages(&self) -> &[StageInfo] {
        &self.layout.stages
    }

    /// Reader producing output rows `first_row .. first_row + row_count` of `window`.
    pub fn rows(&self, window: Window, first_row: u32, row_count: u32) -> Result<RowReader<'_>> {
        RowReader::new(&self.file, &self.layout, window, first_row, row_count)
    }
}
