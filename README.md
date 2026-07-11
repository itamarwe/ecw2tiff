# ecw2tiff

A from-scratch ECW decoder in Rust that converts ECW imagery (v2 and v3) to
tiled BigTIFF with JPEG or Deflate compression. No ECW SDK, GDAL, or other
imaging libraries are used for decoding — the wavelet codec is implemented
directly from the on-disk format.

## Build

```bash
cargo build --release
```

By default JPEG tiles are encoded with libjpeg-turbo (SIMD), which must be
present on the system (`brew install jpeg-turbo` on macOS,
`apt install libturbojpeg0-dev` on Debian/Ubuntu). For a pure-Rust binary
with no native dependencies, build with:

```bash
cargo build --release --no-default-features
```

(JPEG encoding is then ~20% slower end to end; all other codecs are
unaffected.)

## Usage

```bash
# inspect a file
ecw2tiff --info input.ecw

# full conversion (tiled BigTIFF, JPEG quality 87, GeoTIFF tags included)
ecw2tiff --progress input.ecw output.tif

# quick preview at 1/256 resolution
ecw2tiff --scale 256 input.ecw preview.tif

# crop a full-resolution region (x,y,width,height in pixels)
ecw2tiff --region 51200,76800,4096,4096 input.ecw crop.tif

# other codecs
ecw2tiff --compress jpeg:92    input.ecw out.tif
ecw2tiff --compress deflate:6  input.ecw out.tif   # lossless
ecw2tiff --compress none       input.ecw out.tif

# limit decoding to 4 threads (default: all cores)
ecw2tiff --threads 4 input.ecw out.tif
```

Output is always little-endian BigTIFF (safe for >4 GB images), tiled
(default 512x512), with `ModelPixelScale`/`ModelTiepoint` and a GeoKey
directory carrying the EPSG code when it is known (stated by the file for
ECW v3; mapped from `GEODETIC`/`NUTM*`/`SUTM*` + `WGS84` names for v2).
GDAL/QGIS read the result directly.

Decoding is parallelized across horizontal strips (one per core, tile
compression and file I/O overlapped). On an Apple-silicon Mac (M1,
4P+4E) region decodes reach ~190 Mpixel/s and a sustained full
conversion of a 59-gigapixel mosaic runs at ~130 Mpixel/s — under
8 minutes end to end, versus ~15 minutes single-threaded (50–70
Mpixel/s). Strip-parallel output is byte-identical to a
single-threaded decode.
Peak memory is about `image_width * tile * 3 * threads` bytes (~4 GB for
a 276k-pixel-wide mosaic on 8 cores); reduce `--threads` or `--tile` on
memory-constrained machines.

## How it works

ECW is a wavelet codec: a multi-resolution QMF pyramid where each level
stores LH/HL/HH sidebands (plus LL at the smallest level) as quantized
int16 coefficients, in 64x64 (v2) or 128x128 (v3) blocks, each sideband
entropy-coded independently as one of:

| id | encoding |
|----|----------|
| 1  | raw int16 |
| 2  | Huffman (serialized tree + LSB-first bitstream, zero-run symbols) |
| 3  | range coder, 257-symbol quasistatic model, 2 symbols per coefficient |
| 4  | range coder over 8-bit deltas |
| 5  | all zeros |
| 6  | 16-bit zero-run RLE |

Reconstruction runs line-by-line with two-line ring buffers per level: an
output line of level L combines two sideband lines of level L with a 1-2-1
interpolation filter whose taps depend on output x/y parity; the LL input
of level L is produced recursively by level L-1. Color is stored as
JPEG-style YCbCr ("YUV" in ECW terms); RGB = Y + 1.402 V, etc.

File-format quirks worth knowing (v2): header integers are big-endian
while doubles, the block table, and all block payloads are little-endian;
the per-level block offset table is itself entropy-coded and ends with a
sentinel entry.

ECW v3 wraps essentially the same codestream in ISO-BMFF-style boxes
(`ecwh`/`ihdr` header, `gtif` embedded GeoTIFF tags, `ecwc` codestream),
adds an opacity band (RGB+alpha as 4-band YUV+A), and replaces the block
table with raw `(u64 offset, u32 length)` entries where zero length means
an empty block. The `ihdr` box stores image height before width. The
opacity band is currently decoded but not written to the TIFF.

Format reference: the ECW v2 structure follows the publicly released
libecwj2-3.3 sources (Earth Resource Mapping, 2006); the v3 container was
reverse-engineered from real files. All code here is an independent
implementation.

## License

MIT — see [LICENSE](LICENSE).
