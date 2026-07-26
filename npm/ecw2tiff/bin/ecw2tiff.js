#!/usr/bin/env node
// Launcher: runs the prebuilt ecw2tiff binary for this platform. All
// supported binaries ship inside this package (they total under 2 MB), so
// there are no optional dependencies to resolve and no install scripts.
"use strict";
const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const SUPPORTED = new Set([
  "darwin-arm64",
  "darwin-x64",
  "linux-arm64",
  "linux-x64",
  "win32-x64",
]);

const key = `${process.platform}-${process.arch}`;
if (!SUPPORTED.has(key)) {
  console.error(
    `ecw2tiff: no prebuilt binary for ${key}.\n` +
      "Build from source instead: cargo install --git https://github.com/itamarwe/ecw2tiff"
  );
  process.exit(1);
}

const bin = path.join(
  __dirname,
  `ecw2tiff-${key}${process.platform === "win32" ? ".exe" : ""}`
);
if (!fs.existsSync(bin)) {
  console.error(`ecw2tiff: bundled binary is missing (${bin}). Try reinstalling.`);
  process.exit(1);
}

function run() {
  return spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
}

let r = run();
// npm normally preserves the executable bit from the published tarball; if
// something stripped it, fix it once and retry rather than failing.
if (r.error && r.error.code === "EACCES" && process.platform !== "win32") {
  try {
    fs.chmodSync(bin, 0o755);
    r = run();
  } catch {
    /* fall through to the error report below */
  }
}
if (r.error) {
  console.error(`ecw2tiff: failed to run ${bin}: ${r.error.message}`);
  process.exit(1);
}
process.exit(r.status === null ? 1 : r.status);
