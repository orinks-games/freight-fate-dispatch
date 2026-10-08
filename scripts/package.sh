#!/usr/bin/env bash
# Stage and archive a macOS or Linux build after `cargo build --release`.
#   scripts/package.sh <label> <world.ffdata> <carriers.json>
# Writes dist/FreightFateDispatch-<label>-macos-arm64.zip or -linux-x64.tar.gz
# and prints the staged executable's path on the last line.
set -euo pipefail

label=$1 world=$2 carriers=$3
name=FreightFateDispatch
exe=target/release/ff-dispatch
rm -rf stage dist
mkdir -p stage/$name dist

case "$(uname -s)" in
  Darwin)
    app=stage/$name/$name.app
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/freight_fate/data"
    cp "$exe" "$app/Contents/MacOS/$name"
    cp "$world" "$app/Contents/Resources/freight_fate/data/world.ffdata"
    cp "$carriers" "$app/Contents/Resources/freight_fate/data/carriers.json"
    version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>Freight Fate Dispatch</string>
  <key>CFBundleExecutable</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>net.orinks.freight-fate-dispatch</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Freight Fate Dispatch</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    # Only system frameworks may be linked: a brewed SDL would die on players' Macs.
    if otool -L "$app/Contents/MacOS/$name" | tail -n +2 | grep -v -e '^\s*/System/' -e '^\s*/usr/lib/'; then
      echo "the executable links a library outside macOS" >&2
      exit 1
    fi
    codesign --force --deep --sign - "$app"
    codesign --verify --deep --strict "$app"
    cp README.md LICENSE stage/$name/
    (cd stage && ditto -c -k --keepParent $name "../dist/$name-$label-macos-arm64.zip")
    echo "$app/Contents/MacOS/$name"
    ;;
  Linux)
    dir=stage/$name
    mkdir -p "$dir/freight_fate/data"
    cp "$exe" "$dir/$name"
    cp "$world" "$dir/freight_fate/data/world.ffdata"
    cp "$carriers" "$dir/freight_fate/data/carriers.json"
    cp README.md LICENSE "$dir/"
    if ldd "$dir/$name" | grep -i -e sdl -e 'libstdc++'; then
      echo "SDL2 and libstdc++ must be linked statically" >&2
      exit 1
    fi
    tar -C stage -czf "dist/$name-$label-linux-x64.tar.gz" $name
    echo "$dir/$name"
    ;;
  *)
    echo "unsupported OS; Windows packages in ci.yml" >&2
    exit 1
    ;;
esac
