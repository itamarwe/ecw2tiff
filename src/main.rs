use ecw2tiff::header::EcwFile;
use ecw2tiff::idwt::Region;
use ecw2tiff::tiff::{self, Codec, GeoInfo, TiffWriter};
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::exit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};

fn usage() -> ! {
    eprintln!(
        "ecw2tiff - convert ECW imagery to tiled BigTIFF (from-scratch decoder)

USAGE:
    ecw2tiff [OPTIONS] <input.ecw> <output.tif>
    ecw2tiff --info <input.ecw>

OPTIONS:
    --info                 print header information and exit
    --compress <c>         jpeg[:quality] | deflate[:level] | none  (default jpeg:87)
    --scale <n>            decode at 1/n resolution, n a power of two (default 1)
    --region <x,y,w,h>     decode only this pixel rectangle (full-res coordinates)
    --tile <n>             output tile size, multiple of 16 (default 512)
    --threads <n>          decode threads (default: all cores)
    --progress             print progress to stderr
"
    );
    exit(2);
}

struct Opts {
    input: PathBuf,
    output: Option<PathBuf>,
    info: bool,
    codec: Codec,
    scale: u32,
    region: Option<(u32, u32, u32, u32)>,
    tile: usize,
    threads: Option<usize>,
    progress: bool,
}

fn parse_args() -> Opts {
    let mut o = Opts {
        input: PathBuf::new(),
        output: None,
        info: false,
        codec: Codec::Jpeg(87),
        scale: 1,
        region: None,
        tile: 512,
        threads: None,
        progress: false,
    };
    let mut pos: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--info" => o.info = true,
            "--progress" => o.progress = true,
            "--compress" => {
                let v = args.next().unwrap_or_else(|| usage());
                let (name, param) = v.split_once(':').unwrap_or((v.as_str(), ""));
                o.codec = match name {
                    "jpeg" => Codec::Jpeg(param.parse().unwrap_or(87)),
                    "deflate" => Codec::Deflate(param.parse().unwrap_or(6)),
                    "none" => Codec::None,
                    _ => usage(),
                };
            }
            "--scale" => {
                o.scale = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage());
                if o.scale == 0 || !o.scale.is_power_of_two() {
                    eprintln!("error: --scale must be a power of two");
                    exit(2);
                }
            }
            "--region" => {
                let v = args.next().unwrap_or_else(|| usage());
                let parts: Vec<u32> = v.split(',').filter_map(|p| p.parse().ok()).collect();
                if parts.len() != 4 {
                    usage();
                }
                o.region = Some((parts[0], parts[1], parts[2], parts[3]));
            }
            "--tile" => {
                o.tile = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage());
                if o.tile == 0 || !o.tile.is_multiple_of(16) || o.tile > 65520 {
                    eprintln!("error: --tile must be a multiple of 16 between 16 and 65520");
                    exit(2);
                }
            }
            "--threads" => {
                let n: usize = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| usage());
                if n == 0 {
                    eprintln!("error: --threads must be at least 1");
                    exit(2);
                }
                o.threads = Some(n);
            }
            "-h" | "--help" => usage(),
            _ => pos.push(a),
        }
    }
    match pos.len() {
        1 if o.info => o.input = PathBuf::from(&pos[0]),
        2 => {
            o.input = PathBuf::from(&pos[0]);
            o.output = Some(PathBuf::from(&pos[1]));
        }
        _ => usage(),
    }
    o
}

fn print_info(ecw: &EcwFile) {
    let h = &ecw.header;
    println!("ECW version:      {}", h.version);
    println!("Size:             {} x {}", h.x_size, h.y_size);
    println!("Bands:            {}", h.nr_bands);
    println!("Format:           {:?}", h.compress_format);
    println!("Levels:           {}", h.num_levels);
    println!("Block size:       {} x {}", h.x_block_size, h.y_block_size);
    println!("Target ratio:     {}:1", h.compression_rate);
    println!(
        "Cell size:        {} x {} (units {})",
        h.cell_inc_x, h.cell_inc_y, h.cell_units
    );
    println!("Origin:           {}, {}", h.origin_x, h.origin_y);
    println!("Datum:            {}", h.datum);
    println!("Projection:       {}", h.projection);
    for l in &ecw.levels {
        println!(
            "  level {:2}: {:7} x {:7}  blocks {:5} x {:5}  binsizes {:?}",
            l.level, l.x_size, l.y_size, l.nr_x_blocks, l.nr_y_blocks, l.bin_sizes
        );
    }
}

fn main() {
    let opts = parse_args();
    let ecw = match EcwFile::open(&opts.input) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: cannot open {}: {e}", opts.input.display());
            exit(1);
        }
    };
    if opts.info {
        print_info(&ecw);
        return;
    }

    let h = &ecw.header;
    let (rx, ry, rw, rh) = opts.region.unwrap_or((0, 0, h.x_size, h.y_size));
    if rx + rw > h.x_size || ry + rh > h.y_size || rw == 0 || rh == 0 {
        eprintln!("error: region out of bounds");
        exit(2);
    }
    let number_x = rw.div_ceil(opts.scale);
    let number_y = rh.div_ceil(opts.scale);

    let ch = h.output_channels();
    // Map common ER Mapper projection/datum names to EPSG codes.
    let epsg_from_names: Option<u16> = if h.datum == "WGS84" {
        if h.projection == "GEODETIC" {
            Some(4326)
        } else if let Some(zone) = h.projection.strip_prefix("NUTM") {
            zone.parse::<u16>()
                .ok()
                .filter(|z| (1..=60).contains(z))
                .map(|z| 32600 + z)
        } else if let Some(zone) = h.projection.strip_prefix("SUTM") {
            zone.parse::<u16>()
                .ok()
                .filter(|z| (1..=60).contains(z))
                .map(|z| 32700 + z)
        } else {
            None
        }
    } else {
        None
    };
    let epsg = h.epsg.or(epsg_from_names);
    let geo = Some(GeoInfo {
        cell_inc_x: h.cell_inc_x * (rw as f64 / number_x as f64),
        cell_inc_y: h.cell_inc_y * (rh as f64 / number_y as f64),
        origin_x: h.origin_x + rx as f64 * h.cell_inc_x,
        origin_y: h.origin_y + ry as f64 * h.cell_inc_y,
        citation: format!("{} / {}", h.projection, h.datum),
        is_degrees: h.cell_units == 2,
        epsg,
    });

    let out_path = opts.output.as_ref().unwrap();
    let file = match std::fs::File::create(out_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: cannot create {}: {e}", out_path.display());
            exit(1);
        }
    };
    let mut writer = TiffWriter::new(
        BufWriter::with_capacity(1 << 20, file),
        number_x as u64,
        number_y as u64,
        ch,
        opts.codec,
        opts.tile,
        geo,
    )
    .unwrap_or_else(|e| {
        eprintln!("error: tiff init: {e}");
        exit(1);
    });

    // Strip-parallel decode: the output is split into contiguous runs of
    // tile rows, handed out dynamically to decoder threads. Each thread
    // primes its own Region at the strip start (block reads use positioned
    // I/O, so the EcwFile is shared), decodes scanlines, and compresses each
    // completed tile row; the main thread writes rows to the file in order,
    // overlapping decode, compression and I/O.
    let tile = opts.tile;
    let row_bytes = number_x as usize * ch;
    let tiles_y = (number_y as usize).div_ceil(tile);
    let n_threads = opts
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
        .min(tiles_y);
    // ~4 units per thread balances load without paying the strip-priming
    // cost (re-skipping into each level's entropy streams) too often.
    let n_units = (n_threads * 4).clamp(1, tiles_y);
    let next_unit = AtomicUsize::new(0);
    let decode_err: Mutex<Option<String>> = Mutex::new(None);
    let (tx, encoded_rows) = mpsc::sync_channel::<(usize, Vec<Vec<u8>>)>(n_threads * 2);
    let ecw_ref = &ecw;
    let codec = opts.codec;

    let t0 = std::time::Instant::now();
    let mut rows_done = 0u64;
    let io_result: std::io::Result<()> = std::thread::scope(|s| {
        for _ in 0..n_threads {
            let tx = tx.clone();
            let next_unit = &next_unit;
            let decode_err = &decode_err;
            s.spawn(move || {
                let mut row_buf = vec![0u8; tile * row_bytes];
                loop {
                    let u = next_unit.fetch_add(1, Ordering::Relaxed);
                    if u >= n_units || decode_err.lock().unwrap().is_some() {
                        return;
                    }
                    let tr0 = u * tiles_y / n_units;
                    let tr1 = (u + 1) * tiles_y / n_units;
                    let row0 = (tr0 * tile) as u32;
                    let row1 = ((tr1 * tile).min(number_y as usize)) as u32;
                    let mut region = match Region::start_rows(
                        ecw_ref,
                        rx,
                        ry,
                        rx + rw - 1,
                        ry + rh - 1,
                        number_x,
                        number_y,
                        row0,
                        row1 - row0,
                    ) {
                        Ok(r) => r,
                        Err(e) => {
                            decode_err.lock().unwrap().get_or_insert(e);
                            return;
                        }
                    };
                    for tr in tr0..tr1 {
                        let rows = tile.min(number_y as usize - tr * tile);
                        for r in 0..rows {
                            let line = &mut row_buf[r * row_bytes..(r + 1) * row_bytes];
                            if let Err(e) = region.read_scanline(ecw_ref, line) {
                                let msg = format!("row {}: {e}", tr * tile + r);
                                decode_err.lock().unwrap().get_or_insert(msg);
                                return;
                            }
                        }
                        let enc = tiff::encode_tile_row(
                            &row_buf,
                            rows,
                            number_x as usize,
                            ch,
                            tile,
                            codec,
                        );
                        if tx.send((tr, enc)).is_err() {
                            return; // writer bailed
                        }
                    }
                }
            });
        }
        drop(tx);

        // Write tile rows in order as they arrive.
        let mut pending: BTreeMap<usize, Vec<Vec<u8>>> = BTreeMap::new();
        let mut next_row = 0usize;
        for (tr, enc) in encoded_rows {
            pending.insert(tr, enc);
            while let Some(enc) = pending.remove(&next_row) {
                writer.write_tile_row(next_row, &enc)?;
                rows_done += tile.min(number_y as usize - next_row * tile) as u64;
                next_row += 1;
                if opts.progress {
                    let pct = rows_done as f64 * 100.0 / number_y as f64;
                    let rate =
                        rows_done as f64 * number_x as f64 / t0.elapsed().as_secs_f64() / 1e6;
                    eprint!("\r{pct:6.2}%  {rate:7.1} Mpx/s ");
                    let _ = std::io::stderr().flush();
                }
            }
        }
        Ok(())
    });
    if opts.progress {
        eprintln!();
    }
    if let Some(e) = decode_err.into_inner().unwrap() {
        eprintln!("error: decode failed: {e}");
        exit(1);
    }
    if let Err(e) = io_result {
        eprintln!("error: write failed: {e}");
        exit(1);
    }
    match writer.finish() {
        Ok(_) => {
            if opts.progress {
                eprintln!(
                    "done: {}x{} in {:.1}s",
                    number_x,
                    number_y,
                    t0.elapsed().as_secs_f64()
                );
            }
        }
        Err(e) => {
            eprintln!("error: finalize failed: {e}");
            exit(1);
        }
    }
}
