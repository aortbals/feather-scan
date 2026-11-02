#!/usr/bin/env bash
# Regenerates macos/Resources/AppIcon.icns and docs/feather.svg from the menu bar feather.
# Usage: scripts/make-icon.sh [preview.png]
set -euo pipefail
cd "$(dirname "$0")/.."
TMP="$(mktemp -d)"
swiftc -O -parse-as-library -o "$TMP/make-icon" macos/Sources/FeatherScan/FeatherIcon.swift scripts/make-icon.swift
"$TMP/make-icon" "$PWD" "$@"
rm -rf "$TMP"
