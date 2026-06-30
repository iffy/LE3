#!/usr/bin/env bash
# Package release artifacts for CI and local builds.
# Usage: scripts/package-release.sh <linux|macos|windows>
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

version() {
  grep '^version' Cargo.toml | head -1 | cut -d'"' -f2
}

package_linux() {
  local out="bearcad-linux-x86_64.tar.gz"
  mkdir -p dist
  cp target/release/bearcad dist/
  tar czvf "$out" -C dist bearcad
  echo "Created $out"
}

package_macos() {
  local version app_name app_dir dmg
  version="$(version)"
  app_name="BearCAD"
  app_dir="dist/${app_name}.app"
  dmg="bearcad.dmg"

  rm -rf dist
  mkdir -p "${app_dir}/Contents/MacOS" "${app_dir}/Contents/Resources"
  cp target/release/bearcad "${app_dir}/Contents/MacOS/bearcad"
  chmod +x "${app_dir}/Contents/MacOS/bearcad"
  bash scripts/generate-macos-icns.sh dist/AppIcon.icns
  cp dist/AppIcon.icns "${app_dir}/Contents/Resources/AppIcon.icns"

  cat >"${app_dir}/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>bearcad</string>
  <key>CFBundleIdentifier</key>
  <string>com.bearcad.app</string>
  <key>CFBundleName</key>
  <string>${app_name}</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>CFBundleVersion</key>
  <string>${version}</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

  rm -f "$dmg"
  hdiutil create -volname "$app_name" -srcfolder "$app_dir" -ov -format UDZO "$dmg"
  echo "Created $dmg"
}

package_windows() {
  pwsh -NoProfile -File scripts/package-windows.ps1
}

target="${1:-}"
case "$target" in
  linux) package_linux ;;
  macos) package_macos ;;
  windows) package_windows ;;
  *)
    echo "usage: $0 <linux|macos|windows>" >&2
    exit 1
    ;;
esac