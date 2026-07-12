#!/usr/bin/env bash
# One-time bootstrap: npm trusted publishing (OIDC) cannot create brand-new
# packages, so the very first release of the six ecw2tiff packages is
# published from a maintainer machine (interactive npm login + 2FA).
#
# 1. gh workflow run release.yml            # build binaries for all targets
# 2. npm login                              # in a browser, with 2FA
# 3. npm/first-publish.sh <run-id>          # run id from `gh run list`
# 4. On npmjs.com, add a Trusted Publisher to EACH of the six packages:
#    GitHub Actions / itamarwe / ecw2tiff / release.yml,
#    allowed action "npm publish". Later releases then need no tokens:
#    git tag vX.Y.Z && git push --tags
set -euo pipefail

run_id=${1:?usage: npm/first-publish.sh <workflow-run-id>}
here=$(cd "$(dirname "$0")" && pwd)
version=$(node -p "require('$here/ecw2tiff/package.json').version")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Skip anything already on the registry so the script can be re-run after
# a partial failure (npm versions are immutable; no republish possible).
published() {
  npm view "$1@$version" version >/dev/null 2>&1
}

echo "Downloading binaries from workflow run $run_id ..."
gh run download "$run_id" --dir "$tmp/artifacts"

for pair in "darwin arm64" "darwin x64" "linux x64" "linux arm64" "win32 x64"; do
  set -- $pair
  name="ecw2tiff-$1-$2"
  if published "$name"; then
    echo "$name@$version already published, skipping"
    continue
  fi
  exe=ecw2tiff; [ "$1" = win32 ] && exe=ecw2tiff.exe
  [ -f "$tmp/artifacts/bin-$1-$2/$exe" ] || {
    echo "error: missing binary artifact bin-$1-$2/$exe in run $run_id" >&2
    echo "(did all five build jobs succeed?)" >&2
    exit 1
  }
  node "$here/make-platform-package.mjs" "$1" "$2" "$tmp/artifacts/bin-$1-$2/$exe" "$tmp/dist"
  npm publish --access public "$tmp/dist/$name"
done
if published ecw2tiff; then
  echo "ecw2tiff@$version already published, skipping"
else
  npm publish --access public "$here/ecw2tiff"
fi

cat <<'EOF'

All six packages published. Now finish the OIDC setup so future releases
publish from CI without any tokens:

  For each package (ecw2tiff, ecw2tiff-darwin-arm64, ecw2tiff-darwin-x64,
  ecw2tiff-linux-x64, ecw2tiff-linux-arm64, ecw2tiff-win32-x64):
    npmjs.com -> package -> Settings -> Trusted Publisher
      Publisher:         GitHub Actions
      Organization/user: itamarwe
      Repository:        ecw2tiff
      Workflow filename: release.yml
      Allowed actions:   npm publish

Then a release is just: bump versions, git tag vX.Y.Z && git push --tags
EOF
