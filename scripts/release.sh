#!/usr/bin/env bash
# Make a release with `hearken release`, check it, play it, and give the
# archive the name the downloads page uses:
#   scripts/release.sh <label>
# writes dist/FreightFateDispatch-<label>-windows-portable.zip,
# -macos-arm64.zip or -linux-x64.tar.gz.
#
# Needs the hearken command at the pin in Cargo.toml (HEARKEN names it, else
# `hearken` on PATH) and the baked world data in freight_fate/data. On CI the
# seal key comes from HEARKEN_SEAL_SECRET.
#
# The scripted game runs from the folder `hearken release` staged, after the
# archive is made, so the saves it writes are never shipped. They go in that
# folder's `saves`, or Application Support on a Mac.
set -euo pipefail

label=$1
hearken=${HEARKEN:-hearken}
name=FreightFateDispatch

for file in world.ffdata carriers.json; do
  if [[ ! -f freight_fate/data/$file ]]; then
    echo "freight_fate/data/$file is missing: put the baked world data there first" >&2
    exit 1
  fi
done

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-x64 ext=tar.gz suffix=linux-x64.tar.gz ;;
  Darwin-arm64) platform=macos-arm64 ext=zip suffix=macos-arm64.zip ;;
  MINGW*-x86_64 | MSYS*-x86_64) platform=windows-x64 ext=zip suffix=windows-portable.zip ;;
  *)
    echo "no release is made for $(uname -s) $(uname -m)" >&2
    exit 1
    ;;
esac

rm -rf dist
"$hearken" release

shopt -s nullglob
archives=(dist/*-"$platform.$ext")
if ((${#archives[@]} != 1)); then
  echo "expected one $platform archive in dist, found: ${archives[*]:-none}" >&2
  exit 1
fi
# Players unpack a new version over the old folder, and their saves live in
# it, so the folder inside the archive keeps the one name it has always had
# and the archive is made again around it.
stage=dist/$name
archive=dist/$name-$label-$suffix
mv "${archives[0]%."$ext"}" "$stage"
rm "${archives[0]}"
case $platform in
  linux-x64) tar -C dist -czf "$archive" "$name" ;;
  # ditto keeps the app whole: its symlinks, permissions and signature.
  macos-arm64) (cd dist && ditto -c -k --keepParent "$name" "../$archive") ;;
  windows-x64) pwsh -NoProfile -Command "Compress-Archive -Path '$stage' -DestinationPath '$archive'" ;;
esac
echo "Archive: $archive"

case $platform in
  linux-x64)
    exe=$stage/$name
    saves=$stage/saves
    if ldd "$exe" | grep -i -e sdl -e 'libstdc++' -e libdbus; then
      echo "SDL2, libstdc++ and libdbus must be linked statically" >&2
      exit 1
    fi
    ;;
  macos-arm64)
    apps=("$stage"/*.app)
    if ((${#apps[@]} != 1)); then
      echo "expected one app in $stage, found: ${apps[*]:-none}" >&2
      exit 1
    fi
    exe=${apps[0]}/Contents/MacOS/$name
    saves="$HOME/Library/Application Support/$name"
    # Only system frameworks may be linked: a brewed SDL would die on players' Macs.
    if otool -L "$exe" | tail -n +2 | grep -v -e '^\s*/System/' -e '^\s*/usr/lib/'; then
      echo "the executable links a library outside macOS" >&2
      exit 1
    fi
    ;;
  windows-x64)
    exe=$stage/$name.exe
    saves=$stage/saves
    ;;
esac
if [[ ! -f $exe ]]; then
  echo "$exe is not in the release" >&2
  exit 1
fi

# The sounds ship packed: a sounds.pak and no loose recordings.
packs=$(find "$stage" -name sounds.pak)
if [[ -z $packs ]]; then
  echo "the release has no sounds.pak" >&2
  exit 1
fi
loose=$(find "$stage" -iname '*.ogg' -o -iname '*.wav')
if [[ -n $loose ]]; then
  echo "the release ships loose sounds:" >&2
  echo "$loose" >&2
  exit 1
fi
echo "Sound pack: $packs"

# A sealed release ignores FF_DISPATCH_DATA_DIR; it is unset so that any
# build saves where a sealed one does. On Windows and Linux the per-user
# folders point at an empty one for the run, so the first-run copy of an
# older version's careers finds none: run on a player's own machine, it
# would otherwise copy their real careers into the staged folder.
before=$(mktemp)
user=$(mktemp -d)
said=$(env -u FF_DISPATCH_DATA_DIR FF_DISPATCH_NO_SPEECH=1 SDL_VIDEODRIVER=dummy \
  SDL_AUDIODRIVER=dummy APPDATA="$user" LOCALAPPDATA="$user" \
  XDG_DATA_HOME="$user" XDG_CONFIG_HOME="$user" \
  "$exe" --seed 7 --script tests/fixtures/first_shift.keys) || {
  echo "$said"
  echo "the packaged game exited with an error" >&2
  exit 1
}
# A scripted run first says what is wrong with the sounds, if anything: a
# pack that is missing, does not open, or lacks a sound
# (crate::audio::sound_problems).
if grep -e 'missing or damaged' -e 'The sound pack has no' <<<"$said"; then
  echo "the packaged game cannot read its sounds" >&2
  exit 1
fi
if ! grep -q "Day 2 at" <<<"$said"; then
  echo "$said"
  echo "packaged smoke failed: the first shift did not reach day 2" >&2
  exit 1
fi

# The career the run saved is sealed, not readable JSON.
careers=$(find "$saves" -path '*careers*' -type f -newer "$before" ! -name '*.tmp' 2>/dev/null || true)
rm -f "$before"
rm -rf "$user"
if [[ -z $careers ]]; then
  echo "the packaged game saved no career under $saves" >&2
  exit 1
fi
while IFS= read -r career; do
  if [[ $(head -c 1 "$career") == "{" ]]; then
    echo "$career is plain JSON: the release does not seal careers" >&2
    exit 1
  fi
  echo "Sealed career: $career"
done <<<"$careers"
echo "Packaged smoke passed."
