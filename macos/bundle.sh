#!/bin/sh
# Builds hanstat.app from the release binaries and packs it as
# hanstat-aarch64-apple-darwin.zip (+ .sha256) in the given folder (default
# dist), the asset the in-app updater looks for on macOS. Run on a Mac from
# the repository root after `cargo build --release --locked --bins`.
# Based on linlin56's script in PR #1.
#
# The zip holds hanstat.app at its top level, with hanstat-cli, the README,
# changelog and licences inside the bundle, so an update replaces all of it.
#
# The bundle is ad-hoc signed only (no Apple Developer ID), so a downloaded
# copy needs one confirmation in System Settings → Privacy & Security at first
# launch. Updates installed by the app itself are not quarantined.
set -eu

name=hanstat-aarch64-apple-darwin
out=${1:-dist}
app="$out/hanstat.app"
contents="$app/Contents"

# Cargo.toml's yyyy.Mdd.n: three integers, as CFBundleShortVersionString needs.
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
case "$version" in
    *[!0-9.]* | "") echo "Unexpected Cargo.toml version: $version" >&2; exit 1 ;;
esac

# Only this script's own outputs are removed, never the folder itself.
mkdir -p "$out"
rm -rf "$app" "$out/hanstat.iconset" "$out/$name.zip" "$out/$name.zip.sha256"
mkdir -p "$contents/MacOS" "$contents/Resources"

cp target/release/hanstat target/release/hanstat-cli "$contents/MacOS/"
cp README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE "$contents/Resources/"
cp data/OpenCC-LICENSE.txt "$contents/Resources/LICENSE-OpenCC-data.txt"
cp data/HSK-LICENSE.txt "$contents/Resources/LICENSE-HSK-data.txt"
cp data/CEDICT-LICENSE.txt "$contents/Resources/LICENSE-CEDICT-data.txt"
cp assets/fonts/LXGWWenKai-LICENSE.txt "$contents/Resources/LICENSE-LXGWWenKai-font.txt"
cp assets/fonts/Inter-LICENSE.txt "$contents/Resources/LICENSE-Inter-font.txt"

sed "s/@VERSION@/$version/g" macos/Info.plist > "$contents/Info.plist"
plutil -lint "$contents/Info.plist"

# The icon, from the 256 px PNG that is also the window icon, so the iconset
# stops at 256 px.
iconset="$out/hanstat.iconset"
mkdir -p "$iconset"
for s in 16 32 128 256; do
    sips -z $s $s assets/icon.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
done
for s in 16 32 128; do
    sips -z $((s * 2)) $((s * 2)) assets/icon.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/hanstat.icns"
rm -rf "$iconset"

# Nested code first, then the bundle (which seals the resources too).
codesign --force --sign - "$contents/MacOS/hanstat-cli"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"

# No resource forks or extended attributes: the updater unpacks plain files.
(cd "$out" && ditto -c -k --norsrc --noextattr --keepParent hanstat.app "$name.zip")
(cd "$out" && shasum -a 256 "$name.zip" > "$name.zip.sha256")
echo "$out/$name.zip ($version)"
