//! In-process Python binding (PyO3 + maturin), built with `--features python`.
//!
//! Exposes a resident `EcwReader` that opens an ECW (file header + pyramid /
//! block table) exactly once and decodes many tiles from it, replacing a
//! per-tile CLI spawn. Reads are thread-safe — the underlying file is read
//! with positional I/O and each `tile()` call builds its own decode state —
//! so a multi-threaded server can share one reader across threads, and the
//! GIL is released for the whole decode+encode so those threads run in
//! parallel.

// pyo3's #[pymethods] expansion trips useless_conversion on the generated
// error path; the lint points at our fn signatures, not our code.
#![allow(clippy::useless_conversion)]

use std::path::Path;
use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

use crate::header::EcwFile;
use crate::idwt::Region;
use crate::tiff;

/// A resident ECW reader. Open once, decode many tiles.
#[pyclass(frozen)]
struct EcwReader {
    ecw: Arc<EcwFile>,
}

/// Decode the full-resolution rectangle `(x, y, w, h)` at `1/scale`, returning
/// interleaved 8-bit samples plus the produced `(width, height, channels)`.
/// The decoder walks the ECW's own pyramid, so a large `scale` reads few
/// coefficients and stays cheap.
fn decode_tile(
    ecw: &EcwFile,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    scale: u32,
) -> Result<(Vec<u8>, u32, u32, usize), String> {
    let hdr = &ecw.header;
    if w == 0 || h == 0 {
        return Err("tile width and height must be non-zero".into());
    }
    let x_end = x.checked_add(w).ok_or("x + w overflows")?;
    let y_end = y.checked_add(h).ok_or("y + h overflows")?;
    if x_end > hdr.x_size || y_end > hdr.y_size {
        return Err(format!(
            "tile ({x},{y},{w},{h}) is outside the {}x{} image",
            hdr.x_size, hdr.y_size
        ));
    }
    let number_x = w.div_ceil(scale);
    let number_y = h.div_ceil(scale);
    let ch = hdr.output_channels();

    let mut region = Region::start_rows(
        ecw,
        x,
        y,
        x_end - 1,
        y_end - 1,
        number_x,
        number_y,
        0,
        number_y,
    )?;
    let row_bytes = number_x as usize * ch;
    let mut buf = vec![0u8; row_bytes * number_y as usize];
    for row in 0..number_y as usize {
        region.read_scanline(ecw, &mut buf[row * row_bytes..(row + 1) * row_bytes])?;
    }
    Ok((buf, number_x, number_y, ch))
}

enum TileOut {
    Raw(Vec<u8>, u32, u32, usize),
    Bytes(Vec<u8>),
}

#[pymethods]
impl EcwReader {
    /// Open an ECW file, reading its header and block table once.
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        let ecw = EcwFile::open(Path::new(path))
            .map_err(|e| PyValueError::new_err(format!("cannot open {path}: {e}")))?;
        Ok(EcwReader { ecw: Arc::new(ecw) })
    }

    /// Header fields, mirroring `ecw2tiff --info`.
    #[getter]
    fn info<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let h = &self.ecw.header;
        let d = PyDict::new_bound(py);
        d.set_item("version", h.version)?;
        d.set_item("width", h.x_size)?;
        d.set_item("height", h.y_size)?;
        d.set_item("bands", h.nr_bands)?;
        d.set_item("levels", h.num_levels)?;
        d.set_item("block_size", (h.x_block_size, h.y_block_size))?;
        d.set_item("cell_size", (h.cell_inc_x, h.cell_inc_y))?;
        d.set_item("origin", (h.origin_x, h.origin_y))?;
        d.set_item("datum", h.datum.as_str())?;
        d.set_item("projection", h.projection.as_str())?;
        d.set_item("epsg", h.epsg)?;
        Ok(d)
    }

    /// Decode the full-resolution pixel rectangle `(x, y, w, h)` at `1/scale`
    /// (`scale` a power of two).
    ///
    /// `encode`:
    ///   - `"raw"`  (default): returns `(bytes, width, height, channels)` of
    ///     interleaved 8-bit samples.
    ///   - `"jpeg"`: returns baseline JPEG bytes (YCbCr 4:2:0 for RGB).
    ///   - `"webp"`: returns lossy WebP bytes (needs the `webp` build feature).
    ///
    /// `quality` (1..=100) applies to jpeg/webp and is ignored for raw.
    #[pyo3(signature = (x, y, w, h, scale=1, encode="raw", quality=85))]
    #[allow(clippy::too_many_arguments)] // the tile() call shape is the API
    fn tile(
        &self,
        py: Python<'_>,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        scale: u32,
        encode: &str,
        quality: u8,
    ) -> PyResult<PyObject> {
        if scale == 0 || !scale.is_power_of_two() {
            return Err(PyValueError::new_err("scale must be a power of two"));
        }
        let ecw = self.ecw.clone();
        let encode = encode.to_ascii_lowercase();

        let out = py
            .allow_threads(move || -> Result<TileOut, String> {
                let (pixels, ow, oh, ch) = decode_tile(&ecw, x, y, w, h, scale)?;
                match encode.as_str() {
                    "raw" => Ok(TileOut::Raw(pixels, ow, oh, ch)),
                    "jpeg" => Ok(TileOut::Bytes(tiff::encode_jpeg_rgb(
                        &pixels,
                        ow as usize,
                        oh as usize,
                        ch,
                        quality,
                    ))),
                    "webp" => Ok(TileOut::Bytes(tiff::encode_webp(
                        &pixels,
                        ow as usize,
                        oh as usize,
                        ch,
                        quality,
                    )?)),
                    other => Err(format!(
                        "unknown encode {other:?} (expected \"raw\", \"jpeg\", or \"webp\")"
                    )),
                }
            })
            .map_err(PyValueError::new_err)?;

        Ok(match out {
            TileOut::Raw(pixels, ow, oh, ch) => {
                (PyBytes::new_bound(py, &pixels), ow, oh, ch).into_py(py)
            }
            TileOut::Bytes(b) => PyBytes::new_bound(py, &b).into_py(py),
        })
    }

    fn __repr__(&self) -> String {
        let h = &self.ecw.header;
        format!(
            "EcwReader(v{}, {}x{}, {} bands, {} levels)",
            h.version, h.x_size, h.y_size, h.nr_bands, h.num_levels
        )
    }
}

#[pymodule]
fn ecw2tiff(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<EcwReader>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
