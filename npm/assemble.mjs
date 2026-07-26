#!/usr/bin/env node
// Copy the CI-built binaries into npm/ecw2tiff/bin/ so the single npm package
// carries every supported platform.
//
// Usage: node npm/assemble.mjs <artifacts-dir>
// where <artifacts-dir> holds the release workflow's artifacts, i.e.
//   <artifacts-dir>/bin-<os>-<cpu>/ecw2tiff[.exe]
import { copyFileSync, chmodSync, existsSync, mkdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const TARGETS = [
  ["darwin", "arm64"],
  ["darwin", "x64"],
  ["linux", "arm64"],
  ["linux", "x64"],
  ["win32", "x64"],
];

const artifacts = process.argv[2];
if (!artifacts) {
  console.error("usage: assemble.mjs <artifacts-dir>");
  process.exit(2);
}

const here = dirname(fileURLToPath(import.meta.url));
const binDir = join(here, "ecw2tiff", "bin");
mkdirSync(binDir, { recursive: true });

let total = 0;
for (const [os, cpu] of TARGETS) {
  const exe = os === "win32" ? "ecw2tiff.exe" : "ecw2tiff";
  const src = join(artifacts, `bin-${os}-${cpu}`, exe);
  if (!existsSync(src)) {
    console.error(`error: missing artifact ${src} (did all build jobs succeed?)`);
    process.exit(1);
  }
  const dst = join(binDir, `ecw2tiff-${os}-${cpu}${os === "win32" ? ".exe" : ""}`);
  copyFileSync(src, dst);
  if (os !== "win32") chmodSync(dst, 0o755);
  total += 1;
}
console.log(`assembled ${total} binaries into ${binDir}`);
