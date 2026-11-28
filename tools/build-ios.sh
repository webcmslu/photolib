#!/usr/bin/env bash
# Builds the iOS app for the App Store and uploads it to App Store Connect
# (TestFlight). Run from the app folder:  tools/build-ios.sh
# Adapted from CasaFlow's scripts/build-ios.sh, for Tauri.
#
# Before: bump bundle.iOS.bundleVersion (+1 for every upload) in
# src-tauri/tauri.ios.conf.json, and "version" there for a new public version.
#
# Signing is automatic with team 7LMT79E5E4 (tauri.conf.json → bundle.iOS).
# The upload logs in with an App Store Connect API key, as for CasaFlow: the
# AuthKey_<KEYID>.p8 in ~/.appstoreconnect/private_keys/, and ASC_ISSUER_ID
# (plus ASC_KEY_ID if there are several keys) in asc-api-key.env here
# (gitignored) or in ~/.appstoreconnect/phototek.env.
set -euo pipefail
cd "$(dirname "$0")/.."

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
err() { echo -e "${RED}✖ $*${NC}" >&2; }
ok()  { echo -e "${GREEN}✔ $*${NC}"; }

if [ -n "${VITE_PHOTOTEK_SERVER:-}" ]; then
  err "VITE_PHOTOTEK_SERVER is set: it is for development builds only. Unset it."
  exit 1
fi

for f in asc-api-key.env "$HOME/.appstoreconnect/phototek.env"; do
  # shellcheck disable=SC1090
  [ -f "$f" ] && source "$f" && break
done
KEY_FILE=$(ls "$HOME/.appstoreconnect/private_keys/AuthKey_${ASC_KEY_ID:-}"*.p8 2>/dev/null | head -1 || true)
if [ -z "${ASC_KEY_ID:-}" ] && [ -n "$KEY_FILE" ]; then
  ASC_KEY_ID=$(basename "$KEY_FILE" | sed -E 's/AuthKey_(.*)\.p8/\1/')
fi
if [ -z "${ASC_KEY_ID:-}" ] || [ -z "${ASC_ISSUER_ID:-}" ] || [ -z "$KEY_FILE" ]; then
  err "No App Store Connect API key: see the top of this script."
  exit 1
fi
ok "App Store Connect API key $ASC_KEY_ID"
# Tauri passes these to xcodebuild, to make the provisioning profiles.
export APPLE_API_KEY="$ASC_KEY_ID" APPLE_API_ISSUER="$ASC_ISSUER_ID" APPLE_API_KEY_PATH="$KEY_FILE"

VERSION=$(node -p 'require("./src-tauri/tauri.ios.conf.json").version')
BUILD=$(node -p 'require("./src-tauri/tauri.ios.conf.json").bundle.iOS.bundleVersion')
ok "Version $VERSION, build $BUILD"

echo "==> Building (Release, App Store)…"
npx tauri ios build --export-method app-store-connect

IPA=$(ls src-tauri/gen/apple/build/arm64/*.ipa | head -1)
ok "Built $IPA"

echo "==> Uploading to App Store Connect…"
xcrun altool --upload-app --type ios --file "$IPA" \
  --apiKey "$ASC_KEY_ID" --apiIssuer "$ASC_ISSUER_ID"

ok "Done: build $BUILD is processing in App Store Connect → TestFlight (about 15 minutes)."
echo "Then free the space Xcode used: rm -rf ~/Library/Developer/Xcode/DerivedData/*"
