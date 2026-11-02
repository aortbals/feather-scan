#!/usr/bin/env bash
# Builds "Feather Scan.app" into ./build (Apple silicon).
#
# Signing settings come from the environment or from scripts/signing.env
# (gitignored; copy scripts/signing.env.example). In order of preference:
#   SIGN_IDENTITY=…            explicit identity (name or SHA-1), "-" for ad-hoc
#   Developer ID Application   for TEAM_ID (distributable; notarized if NOTARY_PROFILE is set)
#   Apple Development          for TEAM_ID (runs on the build machine only)
#   ad-hoc                     if nothing else is available
#
# Notarization: create a profile once with
#   xcrun notarytool store-credentials <profile> --apple-id … --team-id …
# and set NOTARY_PROFILE=<profile>.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -f scripts/signing.env ]]; then
  # shellcheck disable=SC1091
  source scripts/signing.env
fi
TEAM_ID="${TEAM_ID:-}"

cargo build --release -p feather-core
(cd macos && swift build -c release --arch arm64)
BIN="$(cd macos && swift build -c release --arch arm64 --show-bin-path)/FeatherScan"

APP="build/Feather Scan.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/FeatherScan"
cp macos/Resources/Info.plist "$APP/Contents/Info.plist"
cp macos/Resources/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

# Also compile the icon into an asset catalog (Assets.car). Newer macOS looks the
# app icon up there (CFBundleIconName) in places like Notification Center, and shows
# a blank tile when only the .icns exists.
ICONTMP="$(mktemp -d)"
iconutil -c iconset -o "$ICONTMP/AppIcon.iconset" macos/Resources/AppIcon.icns
SET="$ICONTMP/Assets.xcassets/AppIcon.appiconset"
mkdir -p "$SET"
cp "$ICONTMP"/AppIcon.iconset/*.png "$SET/"
{
  echo '{"images":['
  sep=""
  for pt in 16 32 128 256 512; do
    for sc in 1 2; do
      suffix=""
      if [[ $sc == 2 ]]; then suffix="@2x"; fi
      name="icon_${pt}x${pt}${suffix}.png"
      printf '%s{"idiom":"mac","size":"%sx%s","scale":"%sx","filename":"%s"}' "$sep" "$pt" "$pt" "$sc" "$name"
      sep=","
    done
  done
  echo '],"info":{"version":1,"author":"xcode"}}'
} >"$SET/Contents.json"
xcrun actool "$ICONTMP/Assets.xcassets" --compile "$APP/Contents/Resources" \
  --platform macosx --minimum-deployment-target 14.0 --app-icon AppIcon \
  --output-partial-info-plist "$ICONTMP/partial.plist" >/dev/null
rm -rf "$ICONTMP"
strip -x "$APP/Contents/MacOS/FeatherScan" 2>/dev/null || true

# SHA-1 of the first valid identity of a kind whose certificate belongs to TEAM_ID.
find_identity() {
  local kind="$1"
  security find-identity -v -p codesigning | grep "\"$kind: " | awk '{print $2}' | while read -r sha; do
    if security find-certificate -a -Z -p | awk -v h="$sha" 'BEGIN{RS="-----END CERTIFICATE-----"} $0 ~ h {print $0 RS; exit}' |
      sed -n '/BEGIN/,$p' | openssl x509 -noout -subject 2>/dev/null | grep -q "OU=$TEAM_ID"; then
      echo "$sha"
      return
    fi
  done
}

IDENTITY="${SIGN_IDENTITY:-}"
KIND="explicit"
if [[ -z "$IDENTITY" ]]; then
  if [[ -n "$TEAM_ID" ]]; then
    IDENTITY="$(find_identity "Developer ID Application")" && KIND="Developer ID"
    [[ -z "$IDENTITY" ]] && IDENTITY="$(find_identity "Apple Development")" && KIND="Apple Development"
  fi
  [[ -z "$IDENTITY" ]] && IDENTITY="-" && KIND="ad-hoc"
fi

if [[ "$IDENTITY" == "-" ]]; then
  codesign --force --sign - --options runtime "$APP"
else
  codesign --force --sign "$IDENTITY" --options runtime --timestamp "$APP"
fi
codesign --verify --strict --verbose=1 "$APP"
echo "Signed ($KIND): $(codesign -dv --verbose=2 "$APP" 2>&1 | grep -E '^Authority=' | head -1 | cut -d= -f2)"

if [[ "$KIND" == "Developer ID" && -n "${NOTARY_PROFILE:-}" ]]; then
  ZIP="build/Feather Scan.zip"
  ditto -c -k --keepParent "$APP" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  ditto -c -k --keepParent "$APP" "$ZIP"
  spctl --assess --type execute --verbose "$APP"
fi

echo "Built $APP ($(du -sh "$APP" | cut -f1), binary $(du -h "$APP/Contents/MacOS/FeatherScan" | cut -f1))"
