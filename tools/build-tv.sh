#!/usr/bin/env bash
# Builds the TV app (dist-tv/) and packages it:
#   tools/build-tv.sh                 LG webOS: build/lu.webcms.phototek_<version>_all.ipk
#   tools/build-tv.sh install         … and installs it on the LG TV set up as "tv"
#   tools/build-tv.sh tizen           Samsung: build/tizen/ (and a .wgt with Tizen Studio)
# Android TV is the Android app itself (tools/build-android.sh): on a TV it
# shows this interface.
#
# LG's tools, once:      npm install -g @webos-tools/cli
# Samsung's tools:       Tizen Studio with the TV extension, and a Samsung
#                        certificate profile (docs/tv.md)
set -euo pipefail
cd "$(dirname "$0")/.."

npm run build:tv
mkdir -p build

if [ "${1:-}" = "tizen" ]; then
  rm -rf build/tizen
  cp -R dist-tv build/tizen
  rm -f build/tizen/appinfo.json build/tizen/icon.png
  cp tv/tizen/config.xml build/tizen/
  if command -v tizen >/dev/null; then
    tizen package -t wgt -s "${TIZEN_PROFILE:-phototek}" -- build/tizen
    echo "Packaged: $(ls -t build/tizen/*.wgt | head -1)"
  else
    echo "The Samsung app is in build/tizen/. To package it: Tizen Studio's tizen CLI (docs/tv.md)."
  fi
  exit 0
fi

if ! command -v ares-package >/dev/null; then
  echo "The TV app is built in dist-tv/. To package it, install LG's tools: npm install -g @webos-tools/cli"
  exit 0
fi
ares-package dist-tv -o build
ipk=$(ls -t build/lu.webcms.phototek_*.ipk | head -1)
echo "Packaged: $ipk"
if [ "${1:-}" = "install" ]; then
  ares-install --device tv "$ipk"
  ares-launch --device tv lu.webcms.phototek
fi
