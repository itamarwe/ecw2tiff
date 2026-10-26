# ecw2tiff

A clean-room ECW decoder in Rust that converts ECW imagery (v1, v2 and v3) to
tiled BigTIFF with JPEG or Deflate compression. No ECW SDK, GDAL, or other
imaging libraries are used for decoding. See [PROVENANCE.md](PROVENANCE.md).

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

`--compress deflate` uses the pure-Rust miniz_oxide backend by default.
Deflate is by far the most expensive codec here — on an M1 it is where
roughly 90% of the CPU goes, against about 10% for the actual decode — so
there is a faster option:

```bash
cargo build --release --features fast-deflate
```

That switches the backend to zlib-ng, whose NEON match-finder and
checksums cut deflate CPU by about 27% and raise end-to-end throughput by
roughly 22% (measured on an M1, 67 Mpixel region: 44 -> 54 Mpixel/s).
The trade is output about 4-5% larger at the same level, and a build that
needs a C toolchain and cmake, which is why it is opt-in. Decoded pixels
are identical either way.

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

## Testing

`cargo test` decodes the synthetic fixtures in `tests/data` and compares the
output against recorded SHA-256 digests, checks that strip-parallel and
concurrent decodes match a full decode, and feeds in corrupted files.

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

## How it works

ECW is a wavelet codec. An image is stored as a pyramid of stages, coarsest
first; each stage holds three quantized 16-bit detail planes per band (the
coarsest also holds a base plane), cut into tiles. Each plane within a tile
is entropy-coded independently with one of six encodings: raw, empty,
zero-run words, a prefix code, or an adaptive arithmetic code over bytes or
over byte deltas. Reconstructing a stage combines its base and detail planes
into a grid twice the size, which becomes the next stage's base.

The decoder streams: it produces output rows on demand, computes only the
columns the requested window and scale actually sample, and keeps two input
rows per stage in memory. A reader can start at any output row, touching only
the tile rows that row depends on, which is what makes strip-parallel
conversion and cheap map-tile requests possible. YCbCr images are converted
to RGB with the JPEG coefficients.

ECW v3 wraps the same codestream in ISO-BMFF-style boxes (`ecwh`/`ihdr`
header, `gtif` embedded GeoTIFF tags, `ecwc` codestream) and can carry a
fourth, opacity band.

The full format description is in [docs/SPEC.md](docs/SPEC.md).

## Limitations

- The opacity band of ECW v3 RGBA files is decoded but not written to the
  output.
- ECW v3 files whose block table is not stored raw are rejected as unsupported.
- EPSG codes for v2 files are derived only from `GEODETIC` / `NUTM*` /
  `SUTM*` projection names with the `WGS84` datum.
- No JPEG 2000, and no ECWP streaming.
- The decoder contains no `unsafe` code and is tested against truncated and
  randomly corrupted files, but it has not been externally fuzzed or audited.

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
