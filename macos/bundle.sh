#!/bin/sh
# Builds hanstat.app from the release binaries and packs it as
# hanstat-aarch64-apple-darwin.zip (+ .sha256), the asset the in-app updater
# looks for on macOS. Run from the repository root after
# `cargo build --release --locked --bins`.
#
# The bundle is ad-hoc signed only (no Apple Developer ID), so a downloaded
# copy needs one confirmation in System Settings → Privacy & Security at first
# launch. Updates installed by the app itself are not quarantined.
set -eu

name=hanstat-aarch64-apple-darwin
app=hanstat.app
out=${1:-dist}

# Cargo.toml's yyyy.Mdd.n as the release's yyyy.mm.dd.n (see src/update.rs).
cargo_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
version=$(echo "$cargo_version" | awk -F. '{ printf "%s.%02d.%02d.%s", $1, int($2 / 100), $2 % 100, $3 }')

rm -rf "$out"
mkdir -p "$out/$app/Contents/MacOS" "$out/$app/Contents/Resources"
contents="$out/$app/Contents"

cp target/release/hanstat target/release/hanstat-cli "$contents/MacOS/"
cp README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE "$contents/Resources/"
cp data/OpenCC-LICENSE.txt "$contents/Resources/LICENSE-OpenCC-data.txt"
cp data/HSK-LICENSE.txt "$contents/Resources/LICENSE-HSK-data.txt"
cp data/JLPT-LICENSE.txt "$contents/Resources/LICENSE-JLPT-data.txt"

# The icon, from the 256 px PNG that is also the window icon.
iconset="$out/hanstat.iconset"
mkdir -p "$iconset"
for s in 16 32 128 256; do
    sips -z $s $s assets/icon.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
done
for s in 16 128; do
    sips -z $((s * 2)) $((s * 2)) assets/icon.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$contents/Resources/hanstat.icns"
rm -rf "$iconset"

cat > "$contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>hanstat</string>
    <key>CFBundleDisplayName</key>
    <string>hanstat</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.caocaochan.hanstat</string>
    <key>CFBundleExecutable</key>
    <string>hanstat</string>
    <key>CFBundleIconFile</key>
    <string>hanstat</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

codesign --force --sign - "$contents/MacOS/hanstat-cli"
codesign --force --sign - "$out/$app"

# No resource forks or extended attributes: the updater unpacks plain files.
(cd "$out" && ditto -c -k --norsrc --noextattr --keepParent "$app" "$name.zip")
(cd "$out" && shasum -a 256 "$name.zip" > "$name.zip.sha256")
echo "$out/$name.zip ($version)"
