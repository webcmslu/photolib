#!/usr/bin/env bash
# Builds the Android app bundle (.aab) for Google Play. Run from the app
# folder:  tools/build-android.sh
# Adapted from CasaFlow's scripts/build-android.sh, for Tauri.
#
# Before: bump bundle.android.versionCode (+1 for every upload) in
# src-tauri/tauri.android.conf.json, and "version" there for a new public
# version.
#
# Signing needs the upload key, made once and BACKED UP (Play refuses an app
# signed with another key):
#   keytool -genkey -v -keystore ~/.android/phototek-upload.jks -keyalg RSA \
#     -keysize 2048 -validity 10000 -alias upload
# then src-tauri/gen/android/keystore.properties (never committed):
#   storeFile=/Users/<you>/.android/phototek-upload.jks
#   keyAlias=upload
#   password=<the keystore's password>
set -euo pipefail
cd "$(dirname "$0")/.."

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
err() { echo -e "${RED}✖ $*${NC}" >&2; }
ok()  { echo -e "${GREEN}✔ $*${NC}"; }

if [ -n "${VITE_PHOTOTEK_SERVER:-}" ]; then
  err "VITE_PHOTOTEK_SERVER is set: it is for development builds only. Unset it."
  exit 1
fi
if [ ! -f src-tauri/gen/android/keystore.properties ]; then
  err "No src-tauri/gen/android/keystore.properties: see the top of this script."
  exit 1
fi

export JAVA_HOME="${JAVA_HOME:-/Applications/Android Studio.app/Contents/jbr/Contents/Home}"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
if [ -z "${NDK_HOME:-}" ]; then
  NDK_HOME=$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)
  export NDK_HOME
fi
[ -d "$JAVA_HOME" ] || { err "Java not found: install Android Studio."; exit 1; }
[ -d "${NDK_HOME:-}" ] || { err "Android NDK not found: install it from Android Studio's SDK Manager."; exit 1; }
ok "Java $JAVA_HOME"
ok "NDK $NDK_HOME"

VERSION=$(node -p 'require("./src-tauri/tauri.android.conf.json").version')
CODE=$(node -p 'require("./src-tauri/tauri.android.conf.json").bundle.android.versionCode')
ok "Version $VERSION, versionCode $CODE"

echo "==> Building the app bundle…"
npx tauri android build --aab

AAB=$(ls src-tauri/gen/android/app/build/outputs/bundle/universalRelease/*.aab | head -1)
ok "Built $AAB: upload it in the Play Console (Testing → Internal testing → Create new release)."
