"""Minimal slippy-map tile server backed by the in-process ecw2tiff binding.

Run:
    maturin develop --release          # build the extension into your venv
    pip install flask
    python examples/tile_server.py path/to/file.ecw

Then open  http://localhost:8000/0/0/0.jpg  (z/x/y), or point a Leaflet /
OpenLayers XYZ layer at http://localhost:8000/{z}/{x}/{y}.jpg

The ECW is opened once at startup; every request decodes straight from the
resident reader and JPEG-encodes a single time. Flask serves requests on
threads, and EcwReader is thread-safe, so no per-request locking is needed.
"""

import sys
from flask import Flask, Response
from ecw2tiff import EcwReader

TILE = 256  # output tile size in pixels

reader = EcwReader(sys.argv[1] if len(sys.argv) > 1 else "input.ecw")
info = reader.info
app = Flask(__name__)


@app.route("/<int:z>/<int:x>/<int:y>.jpg")
def tile(z, x, y):
    # Coarser zoom-out levels read at 1/scale from the ECW's own pyramid.
    # Here z is treated as "zoom out by z levels"; adapt to your tiling scheme.
    scale = 1 << z
    px, py = x * TILE * scale, y * TILE * scale
    if px >= info["width"] or py >= info["height"]:
        return Response(status=404)
    w = min(TILE * scale, info["width"] - px)
    h = min(TILE * scale, info["height"] - py)
    jpeg = reader.tile(px, py, w, h, scale=scale, encode="jpeg", quality=85)
    return Response(jpeg, mimetype="image/jpeg")


if __name__ == "__main__":
    print(f"serving {info['width']}x{info['height']} ECW on http://localhost:8000")
    app.run(host="0.0.0.0", port=8000, threaded=True)
