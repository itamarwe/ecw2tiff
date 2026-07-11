mod ecw3;
mod entropy;
mod header;
mod idwt;
mod tiff;

use header::EcwFile;
use idwt::Region;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::exit;
use tiff::{Codec, GeoInfo, TiffWriter};

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
                o.scale = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage());
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
                o.tile = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage());
                if o.tile == 0 || o.tile % 16 != 0 {
                    eprintln!("error: --tile must be a positive multiple of 16");
                    exit(2);
                }
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

    let mut region = match Region::start(
        &ecw,
        rx,
        ry,
        rx + rw - 1,
        ry + rh - 1,
        number_x,
        number_y,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: cannot start region: {e}");
            exit(1);
        }
    };

    let ch = region.channels();
    // Map common ER Mapper projection/datum names to EPSG codes.
    let epsg_from_names: Option<u16> = if h.datum == "WGS84" {
        if h.projection == "GEODETIC" {
            Some(4326)
        } else if let Some(zone) = h.projection.strip_prefix("NUTM") {
            zone.parse::<u16>().ok().filter(|z| (1..=60).contains(z)).map(|z| 32600 + z)
        } else if let Some(zone) = h.projection.strip_prefix("SUTM") {
            zone.parse::<u16>().ok().filter(|z| (1..=60).contains(z)).map(|z| 32700 + z)
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

    let mut line = vec![0u8; number_x as usize * ch];
    let t0 = std::time::Instant::now();
    for y in 0..number_y {
        if let Err(e) = region.read_scanline(&ecw, &mut line) {
            eprintln!("error: decode failed at row {y}: {e}");
            exit(1);
        }
        if let Err(e) = writer.push_scanline(&line) {
            eprintln!("error: write failed at row {y}: {e}");
            exit(1);
        }
        if opts.progress && (y % 1024 == 0 || y + 1 == number_y) {
            let pct = (y + 1) as f64 * 100.0 / number_y as f64;
            let rate = (y + 1) as f64 * number_x as f64 / t0.elapsed().as_secs_f64() / 1e6;
            eprint!("\r{pct:6.2}%  {rate:7.1} Mpx/s ");
            let _ = std::io::stderr().flush();
        }
    }
    if opts.progress {
        eprintln!();
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
