#!/usr/bin/env bash
# Builds the TV app and packages it for LG webOS (.ipk). Run from the app
# folder:  tools/build-tv.sh            → dist-tv/ and lu.webcms.phototek_<version>_all.ipk
#          tools/build-tv.sh install    → also installs it on the TV set up as "tv"
#
# Needs LG's webOS CLI, once:  npm install -g @webos-tools/cli
# and, to install, the TV in Developer Mode (docs/tv.md).
set -euo pipefail
cd "$(dirname "$0")/.."

npm run build:tv
if ! command -v ares-package >/dev/null; then
  echo "The TV app is built in dist-tv/. To package it, install LG's tools: npm install -g @webos-tools/cli"
  exit 0
fi
mkdir -p build
ares-package dist-tv -o build
ipk=$(ls -t build/lu.webcms.phototek_*.ipk | head -1)
echo "Packaged: $ipk"
if [ "${1:-}" = "install" ]; then
  ares-install --device tv "$ipk"
  ares-launch --device tv lu.webcms.phototek
fi
