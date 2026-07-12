#!/usr/bin/env node
// Launcher: runs the prebuilt ecw2tiff binary for this platform, installed
// via the matching optional dependency (esbuild-style distribution).
"use strict";
const { spawnSync } = require("child_process");

const PLATFORMS = {
  "darwin arm64": "ecw2tiff-darwin-arm64",
  "darwin x64": "ecw2tiff-darwin-x64",
  "linux x64": "ecw2tiff-linux-x64",
  "linux arm64": "ecw2tiff-linux-arm64",
  "win32 x64": "ecw2tiff-win32-x64",
};

const key = `${process.platform} ${process.arch}`;
const pkg = PLATFORMS[key];
if (!pkg) {
  console.error(
    `ecw2tiff: no prebuilt binary for ${key}.\n` +
      "Build from source instead: cargo install --git https://github.com/itamarwe/ecw2tiff"
  );
  process.exit(1);
}

const exe = process.platform === "win32" ? "ecw2tiff.exe" : "ecw2tiff";
let bin;
try {
  bin = require.resolve(`${pkg}/bin/${exe}`);
} catch {
  console.error(
    `ecw2tiff: platform package ${pkg} is missing.\n` +
      "It is installed as an optionalDependency; reinstall without " +
      "--no-optional / --omit=optional."
  );
  process.exit(1);
}

const r = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (r.error) {
  console.error(`ecw2tiff: failed to run ${bin}: ${r.error.message}`);
  process.exit(1);
}
process.exit(r.status === null ? 1 : r.status);
