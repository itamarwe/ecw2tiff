//! ECW v2 header, level table and block offset table parsing.
//!
//! On-disk quirk (documented in the original SDK): integers in the header are
//! MSB-first, while doubles, the block table and all block payload data are
//! LSB-first.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::Path;

pub const MAX_SIDEBAND: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressFormat {
    None,
    Grey,     // COMPRESS_UINT8
    Yuv,      // COMPRESS_YUV (JPEG YCbCr)
    Multi,    // COMPRESS_MULTI
    Rgb,      // COMPRESS_RGB (unused in practice)
}

impl CompressFormat {
    pub fn from_u8(v: u8) -> io::Result<Self> {
        Ok(match v {
            0 => CompressFormat::None,
            1 => CompressFormat::Grey,
            2 => CompressFormat::Yuv,
            3 => CompressFormat::Multi,
            4 => CompressFormat::Rgb,
            _ => return Err(bad(format!("unknown compress format {v}"))),
        })
    }
}

#[derive(Debug, Clone)]
pub struct LevelInfo {
    pub level: usize,
    pub x_size: u32,
    pub y_size: u32,
    pub bin_sizes: Vec<u32>, // one per band
    pub nr_x_blocks: u32,
    pub nr_y_blocks: u32,
    pub first_block_number: u64,
}

#[derive(Debug, Clone)]
pub struct EcwHeader {
    pub version: u8,
    pub compress_format: CompressFormat,
    pub num_levels: usize,
    pub nr_sidebands: usize,
    pub x_size: u32,
    pub y_size: u32,
    pub nr_bands: usize,
    pub scale_factor: f32, // meaningful for v1 files only; 1.0 for v2
    pub x_block_size: u32,
    pub y_block_size: u32,
    pub compression_rate: u16,
    pub cell_units: u8,
    pub cell_inc_x: f64,
    pub cell_inc_y: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    pub datum: String,
    pub projection: String,
    /// EPSG code when the file states one directly (ECW v3 gtif box).
    pub epsg: Option<u16>,
}

pub struct EcwFile {
    file: File,
    pub header: EcwHeader,
    pub levels: Vec<LevelInfo>,
    /// per-block absolute file offset and byte length; a zero length marks an
    /// empty (all-zero) block (ECW v3 writes those for nodata areas).
    block_table: Vec<(u64, u32)>,
}

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

struct Rd<'a> {
    f: &'a mut File,
}

impl<'a> Rd<'a> {
    fn u8(&mut self) -> io::Result<u8> {
        let mut b = [0u8; 1];
        self.f.read_exact(&mut b)?;
        Ok(b[0])
    }
    fn u16_be(&mut self) -> io::Result<u16> {
        let mut b = [0u8; 2];
        self.f.read_exact(&mut b)?;
        Ok(u16::from_be_bytes(b))
    }
    fn u32_be(&mut self) -> io::Result<u32> {
        let mut b = [0u8; 4];
        self.f.read_exact(&mut b)?;
        Ok(u32::from_be_bytes(b))
    }
    fn f64_le(&mut self) -> io::Result<f64> {
        let mut b = [0u8; 8];
        self.f.read_exact(&mut b)?;
        Ok(f64::from_le_bytes(b))
    }
    fn cstr(&mut self, len: usize) -> io::Result<String> {
        let mut b = vec![0u8; len];
        self.f.read_exact(&mut b)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(len);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

impl EcwFile {
    pub fn open(path: &Path) -> io::Result<EcwFile> {
        let mut file = File::open(path)?;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        file.seek(SeekFrom::Start(0))?;
        if magic[0] == b'e' {
            Self::open_v2(file)
        } else if &magic == b"\0\0\0\x10" {
            crate::ecw3::open_v3(file)
        } else {
            Err(bad("not an ECW file (bad magic)".into()))
        }
    }

    fn open_v2(mut file: File) -> io::Result<EcwFile> {
        let file_len = file.metadata()?.len();
        let mut r = Rd { f: &mut file };
        if r.u8()? != b'e' {
            return Err(bad("not an ECW v2 file".into()));
        }
        let version = r.u8()?;
        if version == 0 || version > 2 {
            return Err(bad(format!("unsupported ECW version {version}")));
        }
        let _blocking = r.u8()?;
        let compress_format = CompressFormat::from_u8(r.u8()?)?;
        let num_levels = r.u8()? as usize;
        let nr_sidebands = r.u8()? as usize;
        let x_size = r.u32_be()?;
        let y_size = r.u32_be()?;
        let nr_bands = r.u16_be()? as usize;
        let scale_factor = r.u16_be()? as f32;
        let x_block_size = r.u16_be()? as u32;
        let y_block_size = r.u16_be()? as u32;
        if num_levels == 0 || nr_bands == 0 || scale_factor == 0.0 {
            return Err(bad("invalid ECW header".into()));
        }
        if x_block_size == 0 || y_block_size == 0 {
            return Err(bad("zero block size".into()));
        }
        if nr_sidebands != MAX_SIDEBAND {
            return Err(bad(format!("unsupported sideband count {nr_sidebands}")));
        }

        let mut h = EcwHeader {
            version,
            compress_format,
            num_levels,
            nr_sidebands,
            x_size,
            y_size,
            nr_bands,
            scale_factor,
            x_block_size,
            y_block_size,
            compression_rate: 1,
            cell_units: 0,
            cell_inc_x: 1.0,
            cell_inc_y: 1.0,
            origin_x: 0.0,
            origin_y: 0.0,
            datum: "RAW".into(),
            projection: "RAW".into(),
            epsg: None,
        };
        if version > 1 {
            h.compression_rate = r.u16_be()?;
            h.cell_units = r.u8()?;
            h.cell_inc_x = r.f64_le()?;
            h.cell_inc_y = r.f64_le()?;
            h.origin_x = r.f64_le()?;
            h.origin_y = r.f64_le()?;
            h.datum = r.cstr(16)?;
            h.projection = r.cstr(16)?;
        }

        // Per-level info
        let mut levels = Vec::with_capacity(num_levels);
        let mut first_block_number = 0u64;
        for level in 0..num_levels {
            let l = r.u8()? as usize;
            let lx = r.u32_be()?;
            let ly = r.u32_be()?;
            if l != level || lx >= x_size || ly >= y_size {
                return Err(bad(format!("corrupt level header at level {level}")));
            }
            let mut bin_sizes = Vec::with_capacity(nr_bands);
            for _ in 0..nr_bands {
                bin_sizes.push(r.u32_be()?);
            }
            let nr_x_blocks = lx.div_ceil(x_block_size);
            let nr_y_blocks = ly.div_ceil(y_block_size);
            levels.push(LevelInfo {
                level,
                x_size: lx,
                y_size: ly,
                bin_sizes,
                nr_x_blocks,
                nr_y_blocks,
                first_block_number,
            });
            first_block_number += nr_x_blocks as u64 * nr_y_blocks as u64;
        }
        let total_blocks = first_block_number;
        // Loose sanity cap: a real table packs to roughly a byte per block at
        // best, so more blocks than file bytes means a forged header (and an
        // unbounded allocation below).
        if total_blocks > file_len {
            return Err(bad("block count exceeds file size".into()));
        }

        // Block offset table: u32(BE) packed length, u8 encode format,
        // then (packed_length - 1) bytes of payload.
        let packed_length = r.u32_be()? as usize;
        let table_entries = total_blocks as usize + 1;
        let unpacked_len = table_entries * 8;
        if packed_length == 0
            || packed_length as u64 > file_len
            || packed_length > unpacked_len + 1
        {
            return Err(bad("bad block table size".into()));
        }
        let fmt = r.u8()?;
        let mut payload = vec![0u8; packed_length - 1];
        file.read_exact(&mut payload)?;
        let raw = crate::entropy::unpack_data(fmt, &payload, unpacked_len)
            .map_err(|e| bad(format!("block table decode: {e}")))?;
        let offsets: Vec<u64> = raw
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        let first_block_offset = file.stream_position()?;
        // The sentinel (last) offset is the total block data length; with the
        // monotonicity check below this keeps every block inside the file.
        let last = *offsets.last().unwrap();
        if first_block_offset.checked_add(last).is_none_or(|end| end > file_len) {
            return Err(bad("block data extends past end of file".into()));
        }
        // Convert sentinel-style offsets into absolute (offset, length) pairs.
        let mut block_table = Vec::with_capacity(total_blocks as usize);
        for w in offsets.windows(2) {
            let (start, end) = (w[0], w[1]);
            if end < start {
                return Err(bad("non-monotonic block table".into()));
            }
            block_table.push((first_block_offset + start, (end - start) as u32));
        }

        Ok(EcwFile {
            file,
            header: h,
            levels,
            block_table,
        })
    }

    pub(crate) fn from_parts(
        file: File,
        header: EcwHeader,
        levels: Vec<LevelInfo>,
        block_table: Vec<(u64, u32)>,
    ) -> EcwFile {
        EcwFile {
            file,
            header,
            levels,
            block_table,
        }
    }

    /// Read one packed block for (level, bx, by). An empty Vec means an
    /// all-zero block.
    pub fn read_block(&self, level: usize, bx: u32, by: u32) -> io::Result<Vec<u8>> {
        let li = &self.levels[level];
        debug_assert!(bx < li.nr_x_blocks && by < li.nr_y_blocks);
        let n = (li.first_block_number + by as u64 * li.nr_x_blocks as u64 + bx as u64) as usize;
        let (start, len) = self.block_table[n];
        let mut buf = vec![0u8; len as usize];
        self.file.read_exact_at(&mut buf, start)?;
        Ok(buf)
    }
}
