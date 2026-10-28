#!/bin/sh
# The desktop app for macOS (Magnum BI.app in a .dmg, both chips), from a universal desktop build at
# target/release/magnum-bi, into target/packages:
#   packaging/macos.sh 0.1.7
# Unsigned: signed ad hoc only (Apple Silicon runs nothing without a signature), so macOS asks the
# first time it opens (docs: "Opening an unsigned app").
set -eu
version=$1
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
out=$root/target/packages
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/dmg/Magnum BI.app"
mkdir -p "$out" "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$root/target/release/magnum-bi" "$app/Contents/MacOS/magnum-bi"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Magnum BI</string>
  <key>CFBundleDisplayName</key><string>Magnum BI</string>
  <key>CFBundleIdentifier</key><string>io.github.giobinda.magnumbi</string>
  <key>CFBundleExecutable</key><string>magnum-bi</string>
  <key>CFBundleIconFile</key><string>magnum-bi</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <!-- the window shows the app at http://localhost -->
  <key>NSAppTransportSecurity</key><dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict>
</plist>
EOF

# the icon, at every size macOS asks for
set=$work/magnum-bi.iconset
mkdir -p "$set"
for s in 16 32 128 256 512; do
    sips -z $s $s "$here/icon.png" --out "$set/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    if [ $d -le 512 ]; then
        sips -z $d $d "$here/icon.png" --out "$set/icon_${s}x${s}@2x.png" >/dev/null
    else
        cp "$here/icon.png" "$set/icon_${s}x${s}@2x.png"
    fi
done
iconutil -c icns "$set" -o "$app/Contents/Resources/magnum-bi.icns"

codesign --force --sign - "$app"
ln -s /Applications "$work/dmg/Applications"
cp "$root/LICENSE" "$work/dmg/LICENSE"
hdiutil create -quiet -volname "Magnum BI" -srcfolder "$work/dmg" -format UDZO -ov "$out/magnum-bi-macos.dmg"
ls "$out"
