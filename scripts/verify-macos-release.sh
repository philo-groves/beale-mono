#!/usr/bin/env bash
set -euo pipefail

if [[ ! "${MACOS_SIGNING_TEAM_ID:-}" =~ ^[A-Z0-9]{10}$ ]]; then
  echo "MACOS_SIGNING_TEAM_ID must be a ten-character Apple team identifier." >&2
  exit 1
fi

release_dir=apps/desktop/release
source_app="$release_dir/mac-arm64/Beale.app"
shopt -s nullglob
images=("$release_dir"/*.dmg)
archives=("$release_dir"/*.zip)
if [[ ${#images[@]} -ne 1 || ${#archives[@]} -ne 1 || ! -d "$source_app" ]]; then
  echo "Expected one DMG, one ZIP, and the packaged Beale.app." >&2
  exit 1
fi

temporary_dir=$(/usr/bin/mktemp -d)
mounted=false
cleanup() {
  if [[ "$mounted" == true ]]; then
    /usr/bin/hdiutil detach "$temporary_dir/dmg" -quiet || true
  fi
  /bin/rm -rf "$temporary_dir"
}
trap cleanup EXIT
/bin/mkdir "$temporary_dir/dmg" "$temporary_dir/zip"

requirement="anchor apple generic and certificate leaf[subject.OU] = \"$MACOS_SIGNING_TEAM_ID\" and identifier \"com.beale.app\""
verify_app() {
  local app=$1
  /usr/bin/codesign --verify --deep --strict --verbose=2 -R="$requirement" "$app"
  /usr/bin/xcrun stapler validate "$app"
  /usr/sbin/spctl --assess --type execute --verbose=2 "$app"
}
code_hash() {
  /usr/bin/codesign -d --verbose=4 "$1" 2>&1 |
    /usr/bin/sed -n 's/^CDHash=//p'
}

verify_app "$source_app"
/usr/bin/hdiutil verify "${images[0]}" -quiet
/usr/bin/hdiutil attach "${images[0]}" -readonly -nobrowse -quiet -mountpoint "$temporary_dir/dmg"
mounted=true
/usr/bin/ditto -x -k "${archives[0]}" "$temporary_dir/zip"

for app in "$temporary_dir/dmg/Beale.app" "$temporary_dir/zip/Beale.app"; do
  if [[ ! -d "$app" ]]; then
    echo "Release artifact does not contain Beale.app: $app" >&2
    exit 1
  fi
  verify_app "$app"
  if [[ -z "$(code_hash "$source_app")" || "$(code_hash "$app")" != "$(code_hash "$source_app")" ]]; then
    echo "Release artifact contains a different Beale.app signature." >&2
    exit 1
  fi
done
