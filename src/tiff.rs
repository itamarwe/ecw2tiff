//! Minimal streaming BigTIFF writer: tiled, little-endian, with per-tile
//! JPEG (Compression=7), Deflate (Compression=8) or no compression.
//! Scanlines are pushed top-to-bottom; a row of tiles is compressed (in
//! parallel) and flushed each time `tile_size` rows accumulate.

use flate2::write::ZlibEncoder;
use jpeg_encoder::{ColorType, Encoder as JpegEncoder};
use rayon::prelude::*;
use std::io::{self, Seek, SeekFrom, Write};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Codec {
    Jpeg(u8),    // quality 1..=100
    Deflate(u32), // level 1..=9
    None,
}

pub struct GeoInfo {
    pub cell_inc_x: f64,
    pub cell_inc_y: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub citation: String,
    pub is_degrees: bool,
    /// EPSG code if the ER Mapper projection/datum pair maps to one.
    pub epsg: Option<u16>,
}

pub struct TiffWriter<W: Write + Seek> {
    w: W,
    width: u64,
    height: u64,
    channels: usize,
    codec: Codec,
    tile: usize,
    geo: Option<GeoInfo>,
    tiles_x: usize,
    // indexed by tile number (row-major), filled as tile rows are written
    tile_offsets: Vec<u64>,
    tile_bytecounts: Vec<u64>,
}

const TAG_IMAGE_WIDTH: u16 = 256;
const TAG_IMAGE_LENGTH: u16 = 257;
const TAG_BITS_PER_SAMPLE: u16 = 258;
const TAG_COMPRESSION: u16 = 259;
const TAG_PHOTOMETRIC: u16 = 262;
const TAG_IMAGE_DESCRIPTION: u16 = 270;
const TAG_SOFTWARE: u16 = 305;
const TAG_SAMPLES_PER_PIXEL: u16 = 277;
const TAG_PLANAR_CONFIG: u16 = 284;
const TAG_TILE_WIDTH: u16 = 322;
const TAG_TILE_LENGTH: u16 = 323;
const TAG_TILE_OFFSETS: u16 = 324;
const TAG_TILE_BYTECOUNTS: u16 = 325;
const TAG_SAMPLE_FORMAT: u16 = 339;
const TAG_YCBCR_SUBSAMPLING: u16 = 530;
const TAG_MODEL_PIXEL_SCALE: u16 = 33550;
const TAG_MODEL_TIEPOINT: u16 = 33922;
const TAG_GEO_KEY_DIRECTORY: u16 = 34735;
const TAG_GEO_ASCII_PARAMS: u16 = 34737;

const TYPE_SHORT: u16 = 3;
const TYPE_ASCII: u16 = 2;
const TYPE_DOUBLE: u16 = 12;
const TYPE_LONG8: u16 = 16;

struct IfdEntry {
    tag: u16,
    typ: u16,
    count: u64,
    /// either inline value bytes (<= 8) or raw bytes to be written out-of-line
    data: Vec<u8>,
}

/// Assemble and compress one row of tiles from `rows` scanlines
/// (`width * channels` bytes each) in `row_buf`. Tiles are compressed in
/// parallel; the returned vector is ordered left to right for
/// `TiffWriter::write_tile_row`.
pub fn encode_tile_row(
    row_buf: &[u8],
    rows: usize,
    width: usize,
    channels: usize,
    tile: usize,
    codec: Codec,
) -> Vec<Vec<u8>> {
    let ch = channels;
    let tiles_x = width.div_ceil(tile);
    (0..tiles_x)
        .into_par_iter()
        .map(|tx| {
            // Assemble the (edge-padded) tile.
            let mut t = vec![0u8; tile * tile * ch];
            let x0 = tx * tile;
            for ty in 0..tile {
                let sy = ty.min(rows - 1); // pad bottom by replication
                let src_row = &row_buf[sy * width * ch..(sy + 1) * width * ch];
                let dst = &mut t[ty * tile * ch..(ty + 1) * tile * ch];
                for x in 0..tile {
                    let sx = (x0 + x).min(width - 1); // pad right by replication
                    dst[x * ch..(x + 1) * ch]
                        .copy_from_slice(&src_row[sx * ch..(sx + 1) * ch]);
                }
            }
            match codec {
                Codec::Jpeg(q) => encode_jpeg(&t, tile, ch, q),
                Codec::Deflate(level) => {
                    let mut z = ZlibEncoder::new(Vec::new(), flate2::Compression::new(level));
                    z.write_all(&t).unwrap();
                    z.finish().unwrap()
                }
                Codec::None => t,
            }
        })
        .collect()
}

fn encode_jpeg(t: &[u8], tile: usize, ch: usize, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = JpegEncoder::new(&mut out, quality);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::F_2_2);
    let color = if ch == 1 {
        ColorType::Luma
    } else {
        ColorType::Rgb
    };
    enc.encode(t, tile as u16, tile as u16, color)
        .expect("jpeg encode");
    out
}

impl<W: Write + Seek> TiffWriter<W> {
    pub fn new(
        mut w: W,
        width: u64,
        height: u64,
        channels: usize,
        codec: Codec,
        tile: usize,
        geo: Option<GeoInfo>,
    ) -> io::Result<Self> {
        assert!(tile % 16 == 0, "tile size must be a multiple of 16");
        // BigTIFF header: II, 43, bytesize-of-offsets 8, 0, offset to first IFD
        // (patched at finish time).
        w.write_all(b"II")?;
        w.write_all(&43u16.to_le_bytes())?;
        w.write_all(&8u16.to_le_bytes())?;
        w.write_all(&0u16.to_le_bytes())?;
        w.write_all(&0u64.to_le_bytes())?; // IFD offset placeholder
        let tiles_x = width.div_ceil(tile as u64) as usize;
        let tiles_y = height.div_ceil(tile as u64) as usize;
        Ok(TiffWriter {
            w,
            width,
            height,
            channels,
            codec,
            tile,
            geo,
            tiles_x,
            tile_offsets: vec![0; tiles_x * tiles_y],
            tile_bytecounts: vec![0; tiles_x * tiles_y],
        })
    }

    /// Append one row of pre-encoded tiles (from `encode_tile_row`) to the
    /// file. Rows may be written in any order; offsets are recorded by index.
    pub fn write_tile_row(&mut self, tile_row: usize, encoded: &[Vec<u8>]) -> io::Result<()> {
        assert_eq!(encoded.len(), self.tiles_x);
        for (i, e) in encoded.iter().enumerate() {
            let pos = self.w.stream_position()?;
            self.w.write_all(e)?;
            let idx = tile_row * self.tiles_x + i;
            self.tile_offsets[idx] = pos;
            self.tile_bytecounts[idx] = e.len() as u64;
        }
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        let mut entries: Vec<IfdEntry> = Vec::new();
        let push_shorts = |tag: u16, vals: &[u16]| IfdEntry {
            tag,
            typ: TYPE_SHORT,
            count: vals.len() as u64,
            data: vals.iter().flat_map(|v| v.to_le_bytes()).collect(),
        };
        let push_long = |tag: u16, v: u64| IfdEntry {
            tag,
            typ: TYPE_LONG8,
            count: 1,
            data: v.to_le_bytes().to_vec(),
        };

        entries.push(push_long(TAG_IMAGE_WIDTH, self.width));
        entries.push(push_long(TAG_IMAGE_LENGTH, self.height));
        entries.push(push_shorts(
            TAG_BITS_PER_SAMPLE,
            &vec![8u16; self.channels],
        ));
        let compression: u16 = match self.codec {
            Codec::Jpeg(_) => 7,
            Codec::Deflate(_) => 8,
            Codec::None => 1,
        };
        entries.push(push_shorts(TAG_COMPRESSION, &[compression]));
        // Note: with Compression=7 each tile is a self-contained JPEG stream;
        // its internal color space is YCbCr but readers key off the JPEG
        // headers, so Photometric stays RGB unless we write raw YCbCr.
        let photometric: u16 = if self.channels == 1 {
            1 // BlackIsZero
        } else {
            match self.codec {
                Codec::Jpeg(_) => 6, // YCbCr (as produced by the JPEG encoder)
                _ => 2,              // RGB
            }
        };
        entries.push(push_shorts(TAG_PHOTOMETRIC, &[photometric]));
        entries.push(IfdEntry {
            tag: TAG_IMAGE_DESCRIPTION,
            typ: TYPE_ASCII,
            count: 0, // fixed below
            data: Vec::new(),
        });
        let desc = format!(
            "Converted from ECW by ecw2tiff{}\0",
            self.geo
                .as_ref()
                .map(|g| format!(" ({})", g.citation))
                .unwrap_or_default()
        );
        {
            let e = entries.last_mut().unwrap();
            e.data = desc.into_bytes();
            e.count = e.data.len() as u64;
        }
        entries.push(push_shorts(TAG_SAMPLES_PER_PIXEL, &[self.channels as u16]));
        entries.push(push_shorts(TAG_PLANAR_CONFIG, &[1]));
        entries.push(IfdEntry {
            tag: TAG_SOFTWARE,
            typ: TYPE_ASCII,
            count: 9,
            data: b"ecw2tiff\0".to_vec(),
        });
        entries.push(push_shorts(TAG_TILE_WIDTH, &[self.tile as u16]));
        entries.push(push_shorts(TAG_TILE_LENGTH, &[self.tile as u16]));
        entries.push(IfdEntry {
            tag: TAG_TILE_OFFSETS,
            typ: TYPE_LONG8,
            count: self.tile_offsets.len() as u64,
            data: self
                .tile_offsets
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        });
        entries.push(IfdEntry {
            tag: TAG_TILE_BYTECOUNTS,
            typ: TYPE_LONG8,
            count: self.tile_bytecounts.len() as u64,
            data: self
                .tile_bytecounts
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        });
        entries.push(push_shorts(TAG_SAMPLE_FORMAT, &vec![1u16; self.channels]));
        if matches!(self.codec, Codec::Jpeg(_)) && self.channels == 3 {
            entries.push(push_shorts(TAG_YCBCR_SUBSAMPLING, &[2, 2]));
        }

        if let Some(geo) = &self.geo {
            let scale = [geo.cell_inc_x.abs(), geo.cell_inc_y.abs(), 0.0];
            entries.push(IfdEntry {
                tag: TAG_MODEL_PIXEL_SCALE,
                typ: TYPE_DOUBLE,
                count: 3,
                data: scale.iter().flat_map(|v| v.to_le_bytes()).collect(),
            });
            let tie = [0.0, 0.0, 0.0, geo.origin_x, geo.origin_y, 0.0];
            entries.push(IfdEntry {
                tag: TAG_MODEL_TIEPOINT,
                typ: TYPE_DOUBLE,
                count: 6,
                data: tie.iter().flat_map(|v| v.to_le_bytes()).collect(),
            });
            // Minimal GeoTIFF keys: model type + citation. The ECW header only
            // carries ER Mapper datum/projection names, so we pass them along
            // as a citation instead of guessing an EPSG code.
            let ascii = format!("{}\0", geo.citation);
            let model_type: u16 = if geo.is_degrees { 2 } else { 1 };
            let mut keys: Vec<u16> = vec![
                1024, 0, 1, model_type, // GTModelTypeGeoKey
                1025, 0, 1, 1, // GTRasterTypeGeoKey = PixelIsArea
                1026, 34737, ascii.len() as u16, 0, // GTCitationGeoKey
            ];
            if let Some(epsg) = geo.epsg {
                if geo.is_degrees {
                    keys.extend_from_slice(&[2048, 0, 1, epsg]); // GeographicTypeGeoKey
                } else {
                    keys.extend_from_slice(&[3072, 0, 1, epsg]); // ProjectedCSTypeGeoKey
                    keys.extend_from_slice(&[3076, 0, 1, 9001]); // ProjLinearUnitsGeoKey = metre
                }
            }
            // key ids above are already in the required ascending order
            let n_keys = (keys.len() / 4) as u16;
            let mut hdr = vec![1u16, 1, 0, n_keys];
            hdr.extend_from_slice(&keys);
            let keys = hdr;
            entries.push(IfdEntry {
                tag: TAG_GEO_KEY_DIRECTORY,
                typ: TYPE_SHORT,
                count: keys.len() as u64,
                data: keys.iter().flat_map(|v| v.to_le_bytes()).collect(),
            });
            entries.push(IfdEntry {
                tag: TAG_GEO_ASCII_PARAMS,
                typ: TYPE_ASCII,
                count: ascii.len() as u64,
                data: ascii.into_bytes(),
            });
        }

        entries.sort_by_key(|e| e.tag);

        // Write out-of-line data first, remembering offsets.
        let mut value_offsets: Vec<Option<u64>> = Vec::with_capacity(entries.len());
        for e in &entries {
            if e.data.len() > 8 {
                let pos = self.w.stream_position()?;
                self.w.write_all(&e.data)?;
                value_offsets.push(Some(pos));
            } else {
                value_offsets.push(None);
            }
        }

        // IFD
        let ifd_pos = self.w.stream_position()?;
        self.w
            .write_all(&(entries.len() as u64).to_le_bytes())?;
        for (e, voff) in entries.iter().zip(&value_offsets) {
            self.w.write_all(&e.tag.to_le_bytes())?;
            self.w.write_all(&e.typ.to_le_bytes())?;
            self.w.write_all(&e.count.to_le_bytes())?;
            match voff {
                Some(off) => self.w.write_all(&off.to_le_bytes())?,
                None => {
                    let mut v = [0u8; 8];
                    v[..e.data.len()].copy_from_slice(&e.data);
                    self.w.write_all(&v)?;
                }
            }
        }
        self.w.write_all(&0u64.to_le_bytes())?; // next IFD = none

        // Patch the header's IFD offset.
        self.w.seek(SeekFrom::Start(8))?;
        self.w.write_all(&ifd_pos.to_le_bytes())?;
        self.w.flush()?;
        Ok(self.w)
    }
}
