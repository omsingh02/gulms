#!/usr/bin/env bash
# Package the release binary: scripts/package.sh <tag> <target-triple> <tar.gz|zip>
# Writes dist/gulms-<tag>-<target>.<ext> (used by the release workflow and the installer CI job).
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 <tag> <target-triple> <tar.gz|zip>" >&2
  exit 2
fi
TAG=$1
TARGET=$2
KIND=$3

NAME="gulms-${TAG}-${TARGET}"
BIN=target/release/gulms
[ -f "${BIN}.exe" ] && BIN="${BIN}.exe"

rm -rf "dist/${NAME}"
mkdir -p "dist/${NAME}/completions"
cp "$BIN" LICENSE-MIT LICENSE-APACHE README.md CHANGELOG.md "dist/${NAME}/"
for shell in bash zsh fish powershell; do
  "$BIN" completions "$shell" > "dist/${NAME}/completions/gulms.${shell}"
done

cd dist
if [ "$KIND" = zip ]; then
  7z a "${NAME}.zip" "${NAME}" > /dev/null
else
  tar czf "${NAME}.tar.gz" "${NAME}"
fi
rm -rf "${NAME}"
ls -la
