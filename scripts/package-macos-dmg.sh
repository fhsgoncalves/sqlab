#!/usr/bin/env bash
set -euo pipefail

TARGET="${1:-aarch64-apple-darwin}"
APP_NAME="sqlab"
DMG_PATH="target/distrib/${APP_NAME}-${TARGET}.dmg"
DMG_ROOT="target/dmg-root-${TARGET}"

cargo bundle --package "${APP_NAME}" --profile dist --format osx --target "${TARGET}"

APP_PATH="$(find "target/${TARGET}/dist" "target/dist" "target" -path "*/bundle/osx/${APP_NAME}.app" -type d -print -quit 2>/dev/null || true)"
if [[ -z "${APP_PATH}" ]]; then
  echo "Could not find ${APP_NAME}.app after cargo bundle" >&2
  exit 1
fi

rm -rf "${DMG_ROOT}" "${DMG_PATH}" "${DMG_PATH}.sha256"
mkdir -p "${DMG_ROOT}"
ditto "${APP_PATH}" "${DMG_ROOT}/${APP_NAME}.app"
ln -s /Applications "${DMG_ROOT}/Applications"

hdiutil create \
  -volname "sqlab" \
  -srcfolder "${DMG_ROOT}" \
  -ov \
  -format UDZO \
  "${DMG_PATH}"

shasum -a 256 "${DMG_PATH}" > "${DMG_PATH}.sha256"
