#!/usr/bin/env bash
# Build, sign and package AuraTerm for the Mac App Store.
#
#   scripts/package-appstore.sh             build, sign, write the .pkg
#   scripts/package-appstore.sh --upload    ...then validate and upload it
#
# The App Store build differs from the direct download: it is sandboxed
# (src-tauri/macos/appstore.entitlements), uses the bundle identifier
# com.auraxlabs.auraterm, has no local shell and keeps logs and downloads in
# ~/Downloads/AuraTerm (src-tauri/src/distribution.rs, src/distribution.ts).
#
# Environment:
#   APPSTORE_PROVISIONING_PROFILE   Mac App Store provisioning profile for
#                                   com.auraxlabs.auraterm (required)
#   APPSTORE_APP_IDENTITY           signing identity for the app; default: the
#                                   keychain's "3rd Party Mac Developer
#                                   Application" (or "Apple Distribution") for
#                                   the team
#   APPSTORE_INSTALLER_IDENTITY     signing identity for the .pkg; default: the
#                                   keychain's "3rd Party Mac Developer Installer"
#                                   for the team
#   APPSTORE_BUILD_NUMBER           CFBundleVersion; must grow with every upload
#                                   of the same version. Default: YYYYMMDDHHMM
#   APP_STORE_CONNECT_API_KEY_ID    with --upload: App Store Connect API key id;
#   APP_STORE_CONNECT_API_ISSUER_ID altool reads the key itself from
#                                   ~/.appstoreconnect/private_keys/AuthKey_<id>.p8
set -euo pipefail

TEAM_ID="EC83H6Z7DX"
BUNDLE_ID="com.auraxlabs.auraterm"

upload=false
for arg in "$@"; do
  case "$arg" in
    --upload) upload=true ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown argument: $arg" >&2; exit 2 ;;
  esac
done

die() { echo "error: $*" >&2; exit 1; }

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

[[ "$(uname -s)" == "Darwin" ]] || die "the Mac App Store package can only be built on macOS"
# AuraTerm ships for Apple silicon only, so the host build is the release build
# (no --target, which would need a rustup-managed toolchain).
[[ "$(uname -m)" == "arm64" ]] || die "build on an Apple silicon Mac; AuraTerm ships arm64 only"

# --- Provisioning profile -------------------------------------------------
profile="${APPSTORE_PROVISIONING_PROFILE:-}"
[[ -n "$profile" ]] || die "set APPSTORE_PROVISIONING_PROFILE to the Mac App Store profile for $BUNDLE_ID"
[[ -f "$profile" ]] || die "provisioning profile not found: $profile"

work_dir="$repo_root/src-tauri/target/appstore"
mkdir -p "$work_dir"
profile_plist="$work_dir/profile.plist"
security cms -D -i "$profile" > "$profile_plist" 2>/dev/null || die "cannot decode provisioning profile: $profile"
profile_app_id="$(plutil -extract Entitlements.com\\.apple\\.application-identifier raw -o - "$profile_plist" 2>/dev/null || true)"
profile_expiry="$(plutil -extract ExpirationDate raw -o - "$profile_plist" 2>/dev/null || true)"
rm -f "$profile_plist"
[[ "$profile_app_id" == "$TEAM_ID.$BUNDLE_ID" ]] \
  || die "profile is for '$profile_app_id', expected '$TEAM_ID.$BUNDLE_ID'"
echo "Provisioning profile: $profile_app_id (expires $profile_expiry)"

# --- Signing identities ---------------------------------------------------
find_identity() { # policy, then name prefixes in order of preference
  local policy="$1"; shift
  local prefix
  for prefix in "$@"; do
    security find-identity -v ${policy:+-p "$policy"} \
      | sed -n "s/.*\"\($prefix: .*($TEAM_ID)\)\"/\1/p" | head -n 1 | grep . && return 0
  done
  return 1
}
app_identity="${APPSTORE_APP_IDENTITY:-$(find_identity codesigning "3rd Party Mac Developer Application" "Apple Distribution" || true)}"
installer_identity="${APPSTORE_INSTALLER_IDENTITY:-$(find_identity "" "3rd Party Mac Developer Installer" "Mac Installer Distribution" || true)}"
[[ -n "$app_identity" ]] || die "no App Store application signing identity for team $TEAM_ID in the keychain"
[[ -n "$installer_identity" ]] || die "no App Store installer signing identity for team $TEAM_ID in the keychain"
echo "App identity:       $app_identity"
echo "Installer identity: $installer_identity"

# --- Build ----------------------------------------------------------------
version="$(node -p 'require("./package.json").version')"
build_number="${APPSTORE_BUILD_NUMBER:-$(date +%Y%m%d%H%M)}"
echo "Version $version, build $build_number"

# Copy the profile so its path has no spaces and the original stays untouched.
embedded_profile="$work_dir/embedded.provisionprofile"
cp "$profile" "$embedded_profile"
build_config="$work_dir/build.conf.json"
EMBEDDED_PROFILE="$embedded_profile" BUILD_NUMBER="$build_number" python3 -c '
import json, os
print(json.dumps({"bundle": {"macOS": {
    "files": {"embedded.provisionprofile": os.environ["EMBEDDED_PROFILE"]},
    "bundleVersion": os.environ["BUILD_NUMBER"],
}}}))' > "$build_config"

# The App Store notarizes on its own; keep Tauri from trying with any
# Developer ID credentials that happen to be in the environment.
unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH

VITE_AURATERM_APP_STORE=1 APPLE_SIGNING_IDENTITY="$app_identity" \
  npm run tauri -- build \
    --bundles app \
    --features app-store \
    --config src-tauri/tauri.appstore.conf.json \
    --config "$build_config"

app="src-tauri/target/release/bundle/macos/AuraTerm.app"
[[ -d "$app" ]] || die "build finished without $app"

# --- Verify ---------------------------------------------------------------
codesign --verify --deep --strict "$app" || die "code signature does not verify"
entitlements="$(codesign -d --entitlements - --xml "$app" 2>/dev/null)"
grep -q "com.apple.security.app-sandbox" <<<"$entitlements" || die "app is not sandboxed"
grep -q "$TEAM_ID.$BUNDLE_ID" <<<"$entitlements" || die "app is not signed for $TEAM_ID.$BUNDLE_ID"
[[ -f "$app/Contents/embedded.provisionprofile" ]] || die "provisioning profile is not embedded"
bundle_id="$(plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")"
[[ "$bundle_id" == "$BUNDLE_ID" ]] || die "bundle identifier is $bundle_id, expected $BUNDLE_ID"

# --- Package --------------------------------------------------------------
pkg="$work_dir/AuraTerm_${version}_${build_number}.pkg"
rm -f "$pkg"
productbuild --sign "$installer_identity" --component "$app" /Applications "$pkg"
echo "Package: $pkg"

if ! $upload; then
  echo "Not uploaded. Re-run with --upload, or open the .pkg in Transporter."
  exit 0
fi

# --- Upload ---------------------------------------------------------------
: "${APP_STORE_CONNECT_API_KEY_ID:?set APP_STORE_CONNECT_API_KEY_ID for --upload}"
: "${APP_STORE_CONNECT_API_ISSUER_ID:?set APP_STORE_CONNECT_API_ISSUER_ID for --upload}"
auth=(--apiKey "$APP_STORE_CONNECT_API_KEY_ID" --apiIssuer "$APP_STORE_CONNECT_API_ISSUER_ID")
xcrun altool --validate-app --type macos --file "$pkg" "${auth[@]}"
xcrun altool --upload-app --type macos --file "$pkg" "${auth[@]}"
echo "Uploaded $pkg; it appears in App Store Connect once processing finishes."
