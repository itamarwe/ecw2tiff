#!/usr/bin/env node
// Generate one platform binary package (ecw2tiff-<os>-<cpu>) for npm.
// Usage: node make-platform-package.mjs <npm-os> <npm-cpu> <binary-path> <out-dir>
// The version is taken from npm/ecw2tiff/package.json so everything stays in
// lockstep with the launcher package.
import { mkdirSync, copyFileSync, writeFileSync, readFileSync, chmodSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const [os, cpu, binaryPath, outDir] = process.argv.slice(2);
if (!os || !cpu || !binaryPath || !outDir) {
  console.error("usage: make-platform-package.mjs <os> <cpu> <binary-path> <out-dir>");
  process.exit(2);
}

const here = dirname(fileURLToPath(import.meta.url));
const { version } = JSON.parse(readFileSync(join(here, "ecw2tiff/package.json"), "utf8"));

const name = `ecw2tiff-${os}-${cpu}`;
const exe = os === "win32" ? "ecw2tiff.exe" : "ecw2tiff";
const pkgDir = join(outDir, name);
mkdirSync(join(pkgDir, "bin"), { recursive: true });

writeFileSync(
  join(pkgDir, "package.json"),
  JSON.stringify(
    {
      name,
      version,
      description: `Prebuilt ecw2tiff binary for ${os}-${cpu}`,
      license: "MIT",
      repository: "github:itamarwe/ecw2tiff",
      os: [os],
      cpu: [cpu],
      files: ["bin"],
    },
    null,
    2
  ) + "\n"
);
copyFileSync(binaryPath, join(pkgDir, "bin", exe));
if (os !== "win32") chmodSync(join(pkgDir, "bin", exe), 0o755);
console.log(`${pkgDir} (version ${version})`);
