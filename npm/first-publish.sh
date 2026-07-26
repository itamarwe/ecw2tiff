#!/usr/bin/env bash
# One-time bootstrap: npm trusted publishing (OIDC) cannot create a brand-new
# package, so the very first release of `ecw2tiff` is published from a
# maintainer machine (interactive npm login + 2FA).
#
# 1. gh workflow run release.yml       # build binaries for all targets
# 2. npm login                         # in a browser, with 2FA
# 3. npm/first-publish.sh <run-id>     # run id from `gh run list`
# 4. On npmjs.com add a Trusted Publisher to the package:
#    GitHub Actions / itamarwe / ecw2tiff / release.yml, allowed action
#    "npm publish". Later releases then need no tokens:
#    git tag vX.Y.Z && git push --tags
#
# All platform binaries ship inside the one package, so there is a single
# name to publish (and nothing to partially fail).
set -euo pipefail

run_id=${1:?usage: npm/first-publish.sh <workflow-run-id>}
here=$(cd "$(dirname "$0")" && pwd)
version=$(node -p "require('$here/ecw2tiff/package.json').version")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if npm view "ecw2tiff@$version" version >/dev/null 2>&1; then
  echo "ecw2tiff@$version is already published — nothing to do."
  exit 0
fi

echo "Downloading binaries from workflow run $run_id ..."
gh run download "$run_id" --dir "$tmp/artifacts"

node "$here/assemble.mjs" "$tmp/artifacts"
npm publish --access public "$here/ecw2tiff"

cat <<'EOF'

Published. Now finish the OIDC setup so future releases publish from CI
without any tokens:

  npmjs.com -> ecw2tiff -> Settings -> Trusted Publisher
    Publisher:         GitHub Actions
    Organization/user: itamarwe
    Repository:        ecw2tiff
    Workflow filename: release.yml
    Allowed actions:   npm publish

Then a release is just: bump versions, git tag vX.Y.Z && git push --tags
EOF
