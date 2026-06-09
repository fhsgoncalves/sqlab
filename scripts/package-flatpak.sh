#!/usr/bin/env bash
set -euo pipefail

APP_ID="io.github.fhsgoncalves.sqlab"
APP_NAME="sqlab"

if [[ "$#" -eq 0 ]]; then
  set -- x86_64-unknown-linux-gnu
fi

for TARGET in "$@"; do
  ARCHIVE="target/distrib/${APP_NAME}-${TARGET}.tar.xz"
  WORK_DIR="target/flatpak/${TARGET}"
  INPUT_DIR="${WORK_DIR}/input"
  REPO_DIR="${WORK_DIR}/repo"
  BUILD_DIR="${WORK_DIR}/build"
  MANIFEST="${WORK_DIR}/${APP_ID}.yml"
  FLATPAK_PATH="target/distrib/${APP_NAME}-${TARGET}.flatpak"

  if [[ ! -f "${ARCHIVE}" ]]; then
    echo "Missing ${ARCHIVE}; run dist build for ${TARGET} first" >&2
    exit 1
  fi

  rm -rf "${WORK_DIR}" "${FLATPAK_PATH}" "${FLATPAK_PATH}.sha256"
  mkdir -p "${INPUT_DIR}" "${REPO_DIR}"

  tar -xf "${ARCHIVE}" --strip-components 1 -C "${INPUT_DIR}"
  install -Dm644 packaging/flatpak/${APP_ID}.desktop "${INPUT_DIR}/${APP_ID}.desktop"
  install -Dm644 packaging/flatpak/${APP_ID}.metainfo.xml "${INPUT_DIR}/${APP_ID}.metainfo.xml"
  install -Dm644 packaging/flatpak/${APP_ID}.png "${INPUT_DIR}/${APP_ID}.png"

  cat > "${MANIFEST}" <<EOF
app-id: ${APP_ID}
runtime: org.freedesktop.Platform
runtime-version: "24.08"
sdk: org.freedesktop.Sdk
command: ${APP_NAME}
# Standalone bundles do not need a repository AppStream index.
appstream-compose: false

finish-args:
  - --share=ipc
  - --socket=wayland
  - --socket=fallback-x11
  - --device=dri
  - --share=network
  - --filesystem=home

modules:
  - name: ${APP_NAME}
    buildsystem: simple
    build-commands:
      - install -Dm755 ${APP_NAME} /app/bin/${APP_NAME}
      - install -Dm644 ${APP_ID}.desktop /app/share/applications/${APP_ID}.desktop
      - install -Dm644 ${APP_ID}.metainfo.xml /app/share/metainfo/${APP_ID}.metainfo.xml
      - install -Dm644 ${APP_ID}.png /app/share/icons/hicolor/512x512/apps/${APP_ID}.png
    sources:
      - type: dir
        path: input
EOF

  flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
  flatpak-builder --user --force-clean --repo="${REPO_DIR}" --install-deps-from=flathub "${BUILD_DIR}" "${MANIFEST}"
  flatpak build-bundle "${REPO_DIR}" "${FLATPAK_PATH}" "${APP_ID}" --runtime-repo=https://flathub.org/repo/flathub.flatpakrepo
  (
    cd "$(dirname "${FLATPAK_PATH}")"
    sha256sum "$(basename "${FLATPAK_PATH}")" > "$(basename "${FLATPAK_PATH}").sha256"
  )
done
