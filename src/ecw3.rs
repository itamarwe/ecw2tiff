//! ECW version 3 container (ISO-BMFF style boxes written by ERDAS ECW SDK 5+).
//!
//! Layout (reverse engineered from real files):
//!   box "ecw "  signature
//!   box "ftyp"  major brand "ecw ", compatible "ECW3"
//!   box "ecwh"  header, containing:
//!     box "ihdr":  u8 ver?, u8 ?, u8 compress_format,
//!                  u32be y_size, u32be x_size, u16be nr_bands,
//!                  u8 num_levels, u8 nr_sidebands,
//!                  u16be x_block_size, u16be y_block_size,
//!                  u8 bits-1?, u16be target_ratio, ... (unknown trailing)
//!     box "bpcc":  bits per channel - 1, one byte per band
//!     box "cdef":  JP2-style channel definitions (marks the opacity band)
//!   box "gtif"  embedded GeoTIFF tags:
//!                u32be total_len, u8 ?, then TLVs of
//!                [u16be tiff_tag][u32be byte_len][payload]
//!                (GeoKeyDirectory as u16le quads, tiepoint/scale as f64le)
//!   box "ecwc"  codestream: per-level table in the v2 on-disk format
//!               (u8 level, u32be x, u32be y, u32be binsize per band), then
//!               u32be table_len, u8 format(=RAW), then n_blocks entries of
//!               [u64le absolute file offset][u32le byte length]; zero length
//!               means an empty (all-zero) block. Block payloads are identical
//!               to ECW v2 blocks.

use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use crate::header::read_exact_at;

use crate::header::{CompressFormat, EcwFile, EcwHeader, LevelInfo};

fn bad(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

struct BoxHdr {
    typ: [u8; 4],
    payload_start: u64,
    payload_end: u64,
}

fn read_box(file: &mut File, at: u64, end: u64) -> io::Result<Option<BoxHdr>> {
    if at + 8 > end {
        return Ok(None);
    }
    let mut hdr = [0u8; 8];
    read_exact_at(file, &mut hdr, at)?;
    let mut len = u32::from_be_bytes(hdr[0..4].try_into().unwrap()) as u64;
    let typ: [u8; 4] = hdr[4..8].try_into().unwrap();
    let mut payload_start = at + 8;
    if len == 1 {
        let mut ext = [0u8; 8];
        read_exact_at(file, &mut ext, at + 8)?;
        len = u64::from_be_bytes(ext);
        payload_start = at + 16;
    } else if len == 0 {
        len = end - at;
    }
    Ok(Some(BoxHdr {
        typ,
        payload_start,
        payload_end: at + len,
    }))
}

struct GtifInfo {
    origin_x: f64,
    origin_y: f64,
    cell_inc_x: f64,
    cell_inc_y: f64,
    epsg: Option<u16>,
    is_degrees: bool,
}

fn parse_gtif(data: &[u8]) -> GtifInfo {
    let mut g = GtifInfo {
        origin_x: 0.0,
        origin_y: 0.0,
        cell_inc_x: 1.0,
        cell_inc_y: -1.0,
        epsg: None,
        is_degrees: false,
    };
    // u32be total_len, u8 unknown, then TLVs
    let mut pos = 5usize;
    while pos + 6 <= data.len() {
        let tag = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let len = u32::from_be_bytes(data[pos + 2..pos + 6].try_into().unwrap()) as usize;
        pos += 6;
        if pos + len > data.len() {
            break;
        }
        let payload = &data[pos..pos + len];
        match tag {
            34735 => {
                // GeoKeyDirectory: u16le quads
                let vals: Vec<u16> = payload
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                for q in vals.chunks_exact(4).skip(1) {
                    match q[0] {
                        1024 => g.is_degrees = q[3] == 2, // GTModelType: 2 = geographic
                        2048 | 3072 => {
                            if q[1] == 0 && q[3] != 0 && q[3] != 32767 {
                                g.epsg = Some(q[3]);
                            }
                        }
                        _ => {}
                    }
                }
            }
            33922 => {
                // ModelTiepoint: 6 doubles (i, j, k, x, y, z)
                if payload.len() >= 48 {
                    g.origin_x =
                        f64::from_le_bytes(payload[24..32].try_into().unwrap());
                    g.origin_y =
                        f64::from_le_bytes(payload[32..40].try_into().unwrap());
                }
            }
            33550 => {
                // ModelPixelScale: 3 doubles
                if payload.len() >= 16 {
                    g.cell_inc_x = f64::from_le_bytes(payload[0..8].try_into().unwrap());
                    g.cell_inc_y = f64::from_le_bytes(payload[8..16].try_into().unwrap());
                }
            }
            _ => {}
        }
        pos += len;
    }
    g
}

pub fn open_v3(mut file: File) -> io::Result<EcwFile> {
    let file_len = file.seek(SeekFrom::End(0))?;

    let mut ihdr: Option<Vec<u8>> = None;
    let mut gtif: Option<Vec<u8>> = None;
    let mut ecwc: Option<(u64, u64)> = None;

    let mut at = 0u64;
    while let Some(b) = read_box(&mut file, at, file_len)? {
        match &b.typ {
            b"ecwh" => {
                let mut sub_at = b.payload_start;
                while let Some(s) = read_box(&mut file, sub_at, b.payload_end)? {
                    if &s.typ == b"ihdr" {
                        let mut buf = vec![0u8; (s.payload_end - s.payload_start) as usize];
                        read_exact_at(&file, &mut buf, s.payload_start)?;
                        ihdr = Some(buf);
                    }
                    sub_at = s.payload_end;
                }
            }
            b"gtif" => {
                let mut buf = vec![0u8; (b.payload_end - b.payload_start) as usize];
                read_exact_at(&file, &mut buf, b.payload_start)?;
                gtif = Some(buf);
            }
            b"ecwc" => {
                ecwc = Some((b.payload_start, b.payload_end.min(file_len)));
            }
            _ => {}
        }
        if b.payload_end <= at {
            return Err(bad("corrupt box structure".into()));
        }
        at = b.payload_end;
    }

    let ihdr = ihdr.ok_or_else(|| bad("missing ihdr box".into()))?;
    let (ecwc_start, ecwc_end) = ecwc.ok_or_else(|| bad("missing ecwc box".into()))?;
    if ihdr.len() < 21 {
        return Err(bad("ihdr too short".into()));
    }

    let compress_format = CompressFormat::from_u8(ihdr[2])?;
    let y_size = u32::from_be_bytes(ihdr[3..7].try_into().unwrap());
    let x_size = u32::from_be_bytes(ihdr[7..11].try_into().unwrap());
    let nr_bands = u16::from_be_bytes(ihdr[11..13].try_into().unwrap()) as usize;
    let num_levels = ihdr[13] as usize;
    let nr_sidebands = ihdr[14] as usize;
    let x_block_size = u16::from_be_bytes(ihdr[15..17].try_into().unwrap()) as u32;
    let y_block_size = u16::from_be_bytes(ihdr[17..19].try_into().unwrap()) as u32;
    let compression_rate = u16::from_be_bytes(ihdr[20..22].try_into().unwrap());
    if num_levels == 0 || nr_bands == 0 || nr_sidebands != 4 {
        return Err(bad("invalid ECW v3 ihdr".into()));
    }
    if x_block_size == 0 || y_block_size == 0 {
        return Err(bad("zero block size".into()));
    }

    let g = gtif
        .map(|d| parse_gtif(&d))
        .unwrap_or_else(|| parse_gtif(&[]));

    let header = EcwHeader {
        version: 3,
        compress_format,
        num_levels,
        x_size,
        y_size,
        nr_bands,
        scale_factor: 1.0,
        x_block_size,
        y_block_size,
        compression_rate,
        cell_units: if g.is_degrees { 2 } else { 1 },
        cell_inc_x: g.cell_inc_x,
        cell_inc_y: if g.cell_inc_y > 0.0 {
            -g.cell_inc_y
        } else {
            g.cell_inc_y
        },
        origin_x: g.origin_x,
        origin_y: g.origin_y,
        datum: String::new(),
        projection: g
            .epsg
            .map(|e| format!("EPSG:{e}"))
            .unwrap_or_else(|| "RAW".into()),
        epsg: g.epsg,
    };

    // Level table (v2 on-disk format) at the start of the ecwc payload.
    let mut pos = ecwc_start;
    let mut levels = Vec::with_capacity(num_levels);
    let mut first_block_number = 0u64;
    let rec_len = 1 + 4 + 4 + 4 * nr_bands;
    let mut rec = vec![0u8; rec_len];
    for level in 0..num_levels {
        read_exact_at(&file, &mut rec, pos)?;
        pos += rec_len as u64;
        if rec[0] as usize != level {
            return Err(bad(format!("bad level record {level}")));
        }
        let lx = u32::from_be_bytes(rec[1..5].try_into().unwrap());
        let ly = u32::from_be_bytes(rec[5..9].try_into().unwrap());
        if lx >= x_size || ly >= y_size {
            return Err(bad(format!("bad level dimensions at level {level}")));
        }
        let mut bin_sizes = Vec::with_capacity(nr_bands);
        for b in 0..nr_bands {
            bin_sizes.push(u32::from_be_bytes(
                rec[9 + b * 4..13 + b * 4].try_into().unwrap(),
            ));
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
    let total_blocks = first_block_number as usize;

    // Block table: u32be length, u8 format (RAW), then 12-byte entries.
    let mut tbl_hdr = [0u8; 5];
    read_exact_at(&file, &mut tbl_hdr, pos)?;
    pos += 5;
    let fmt = tbl_hdr[4];
    if fmt != crate::entropy::ENCODE_RAW {
        return Err(bad(format!("unsupported v3 block table encoding {fmt}")));
    }
    let need = total_blocks * 12;
    if pos + need as u64 > ecwc_end {
        return Err(bad("v3 block table extends past codestream".into()));
    }
    let mut raw = vec![0u8; need];
    read_exact_at(&file, &mut raw, pos)?;
    let mut block_table = Vec::with_capacity(total_blocks);
    for e in raw.chunks_exact(12) {
        let off = u64::from_le_bytes(e[0..8].try_into().unwrap());
        let len = u32::from_le_bytes(e[8..12].try_into().unwrap());
        if len != 0 && off.checked_add(len as u64).is_none_or(|end| end > file_len) {
            return Err(bad("v3 block entry out of range".into()));
        }
        block_table.push((off, len));
    }

    Ok(EcwFile::from_parts(file, header, levels, block_table))
}
