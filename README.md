# ecw2tiff

A from-scratch ECW decoder in Rust that converts ECW imagery (v2 and v3) to
tiled BigTIFF with JPEG or Deflate compression. No ECW SDK, GDAL, or other
imaging libraries are used for decoding — the wavelet codec is implemented
directly from the on-disk format.

## Install

Prebuilt binaries for macOS (arm64/x64), Linux (x64/arm64) and Windows
(x64) ship on npm:

```bash
npx ecw2tiff --info input.ecw    # run without installing
npm install -g ecw2tiff          # or install globally
```

The npm package has no dependencies and no install scripts: every
platform binary ships inside it (1.7 MB for all five), and the `bin` shim
picks the right one at run time. The binaries are dependency-free
pure-Rust builds (JPEG encoding is ~20% slower than a source build with
libjpeg-turbo — see below). Static binaries are also attached to each
[GitHub release](https://github.com/itamarwe/ecw2tiff/releases).

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

## Python binding

For servers that decode many tiles from the same file (e.g. a map tile
server), the crate also builds an in-process Python extension via
[PyO3](https://pyo3.rs) + [maturin](https://www.maturin.rs). A resident
`EcwReader` opens the file and its pyramid **once** and decodes tiles
directly — no per-tile process spawn, no intermediate file, and the tile
is JPEG-encoded a single time from the decoded pixels.

```bash
pip install maturin
maturin develop --release        # build + install into the active venv
# (needs libjpeg-turbo present; add --no-default-features for pure Rust,
#  or --features python,webp to enable WebP tile output)
```

```python
from ecw2tiff import EcwReader

r = EcwReader("arzi_3_26.ecw")          # opens header + pyramid once; reuse it
r.info                                   # {'width':…, 'height':…, 'bands':…, 'levels':…,
                                         #  'cell_size':(…), 'origin':(…), 'projection':…, 'epsg':…}

# Decode a full-resolution pixel rectangle at 1/scale (scale a power of two;
# the decoder walks the ECW pyramid so coarse tiles stay cheap).
jpeg = r.tile(x, y, 512, 512, scale=1, encode="jpeg", quality=85)   # -> bytes
webp = r.tile(x, y, 512, 512, encode="webp", quality=80)            # -> bytes (needs webp feature)
pixels, w, h, ch = r.tile(x, y, 512, 512, encode="raw")            # interleaved RGB bytes + dims
```

`encode` defaults to `"raw"` (interleaved 8-bit samples, returned with
`width, height, channels`); `"jpeg"` returns **baseline** JPEG with YCbCr
4:2:0 subsampling (3-band RGB; a 4th/alpha band is dropped); `"webp"`
returns lossy WebP. `quality` (1–100) applies to jpeg/webp.

**Threading:** reads are thread-safe — the file is read with positional
I/O and each `tile()` builds its own decode state — so a multi-threaded
server can share one `EcwReader` across threads, and the GIL is released
for the whole decode+encode. On an M1 (4P+4E) tile decoding scales ~4.7×
across 8 threads, and an in-process tile is ~100× faster than spawning
the CLI and re-encoding with GDAL per tile.

## Encoding (experimental)

`img2ecw` writes ECW v2 files — the inverse of everything above. It takes a
binary PGM/PPM (`gdal_translate -of PNM`, ImageMagick, ffmpeg all produce
one) and builds the wavelet pyramid, quantizes it and writes the container:

```bash
img2ecw input.ppm output.ecw
img2ecw --levels 5 --projection GEODETIC --datum WGS84 input.ppm out.ecw
```

The forward transform is an exact inverse of the decoder's synthesis, so at
the default scale factor the round trip is **bit-exact** for synthetic images
(gradients, checkerboards, 1-pixel alternating patterns, RGB through the
YCbCr transform) and within one count on real aerial photography.

For lossy output, ask for a compression ratio:

```bash
img2ecw --ratio 20 input.ppm output.ecw
```

Each sideband is encoded every way the decoder can read — raw int16,
all-zeros, zero-run RLE, Huffman, and both range-coder forms — and the
smallest is kept, so a format can only ever shrink output. `--ratio`
searches for the quantization strength that lands nearest the requested
ratio, quantizing per level with the same curve real ECW files use (luma
bins halving per level, chroma about 3x luma). On a 512x512 aerial crop:

| mode | size | ratio | PSNR |
|------|------|-------|------|
| lossless | 387 KB | 2.0x | exact |
| `--ratio 5` | 149 KB | 5.2x | 58.4 dB |
| `--ratio 10` | 74 KB | 10.4x | 57.0 dB |
| `--ratio 20` | 38 KB | 20.2x | 29.8 dB |
| `--ratio 100` | 7.6 KB | 101x | 25.3 dB |

Quantization is chosen, not fixed. For a requested ratio the encoder tries
several zero-bin widths and keeps whichever predicts the least error at that
size — predicted in the pixel domain, by scaling each level's coefficient
error with the measured gain from that level to the image
(`fdwt::detail_gain`), so no decoding is needed to compare candidates. That
is worth up to +2.5 dB over using a fixed rounding rule. `--strength` sets
quantization directly if you would rather not search, and `--deadzone`
pins the zero-bin width.

One negative result worth recording, since it is easy to assume otherwise:
allocating bins by measured band gain — the textbook MSE-optimal rule,
bins proportional to `1 / gain` — measured *worse* than the simple halving
curve real files use, by 1-3 dB at matched sizes. That rule assumes a
high-rate regime; at these ratios most coefficients quantize to zero and the
assumption does not hold. The halving curve is kept because it measures
better.

Which format wins is content-dependent, and worth knowing if you extend
this: Huffman dominates quantized detail bands, because it codes a run of
zeros as a *single* symbol from the run/value alphabet, while the range
formats spend one or two adaptively-coded byte-symbols per coefficient. The
range coder earns its place on the smooth, large-magnitude `LL` band and on
streams with coefficients beyond +/-16383, which no other format can
compress. Adding it bought about 1% overall — see
`examples/format_choice.rs` for the per-format comparison.

Remaining gaps: rate-distortion is still below a mature ECW encoder at a
given ratio (the gap is in rate allocation across levels, not entropy
coding); the whole image is held in memory, so this suits modest rasters
rather than the gigapixel mosaics the decoder streams; and dimensions must
be divisible by `2^levels`.

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

## Releasing

npm publishing uses [trusted publishing](https://docs.npmjs.com/trusted-publishers/)
(GitHub Actions OIDC) — no npm tokens are stored anywhere. The packages
also contain no install scripts, so they are unaffected by npm's
install-time security defaults (lifecycle scripts disabled).

Regular release: bump the version in `Cargo.toml` and
`npm/ecw2tiff/package.json`, then push a matching tag:

```bash
git tag v0.1.1 && git push --tags
```

The release workflow builds binaries for all five targets, bundles them
into the single `ecw2tiff` npm package, publishes it via OIDC and
attaches the binaries to a GitHub release.

One-time bootstrap (trusted publishing cannot create a brand-new
package): trigger `gh workflow run release.yml` to build the binaries,
`npm login`, then run `npm/first-publish.sh <run-id>` to publish the
first version from your machine, and configure the Trusted Publisher as
the script prints.

The Python wheels publish to [PyPI](https://pypi.org/project/ecw2tiff/)
the same tokenless way (`.github/workflows/pypi.yml`, OIDC trusted
publishing) — the same `v*` tag triggers it. Wheels are built pure-Rust
(`--no-default-features --features python`) so they install without a
system libjpeg-turbo. PyPI supports *pending* publishers, so no bootstrap
is needed: once, on pypi.org (Account → Publishing → add a pending
publisher) register project `ecw2tiff`, owner `itamarwe`, repo
`ecw2tiff`, workflow `pypi.yml`, environment `pypi`. Then `pip install
ecw2tiff` works after the first tag.

## License

MIT — see [LICENSE](LICENSE).
