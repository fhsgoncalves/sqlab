#!/usr/bin/env bash
set -euo pipefail

TAG="${1:?usage: generate-homebrew-cask.sh <tag> <dmg-sha256-file> <output-cask>}"
CHECKSUM_FILE="${2:?usage: generate-homebrew-cask.sh <tag> <dmg-sha256-file> <output-cask>}"
OUTPUT_CASK="${3:?usage: generate-homebrew-cask.sh <tag> <dmg-sha256-file> <output-cask>}"

APP_NAME="sqlab"
TARGET="aarch64-apple-darwin"
DMG_NAME="${APP_NAME}-${TARGET}.dmg"
VERSION="${TAG#v}"
SHA256="$(awk '{print $1}' "${CHECKSUM_FILE}")"

if [[ -z "${SHA256}" ]]; then
  echo "Could not read checksum from ${CHECKSUM_FILE}" >&2
  exit 1
fi

mkdir -p "$(dirname "${OUTPUT_CASK}")"

cat > "${OUTPUT_CASK}" <<EOF
cask "sqlab" do
  version "${VERSION}"
  sha256 "${SHA256}"

  url "https://github.com/fhsgoncalves/sqlab/releases/download/${TAG}/${DMG_NAME}",
      verified: "github.com/fhsgoncalves/sqlab/"
  name "sq/lab"
  desc "SQL editor written in Rust using GPUI"
  homepage "https://github.com/fhsgoncalves/sqlab"

  depends_on arch: :arm64

  app "sqlab.app"
  binary "#{appdir}/sqlab.app/Contents/MacOS/sqlab", target: "sqlab"

  caveats <<~EOS
    sq/lab is not currently signed or notarized.
    If macOS reports that sqlab is damaged, reinstall without quarantine:

      brew reinstall --cask --no-quarantine fhsgoncalves/tap/sqlab

    Or remove the quarantine attribute after installation:

      xattr -dr com.apple.quarantine /Applications/sqlab.app
  EOS

  zap trash: [
    "~/.sqlab",
    "~/Library/Saved Application State/io.github.fhsgoncalves.sqlab.savedState",
  ]
end
EOF
