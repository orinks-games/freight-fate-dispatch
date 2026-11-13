# Freight Fate: Dispatch

Run the desk at a Freight Fate carrier. Answer customers' tenders, work
the brokers' spot board, assign loads, then handle what the road throws
back over the phone: hours running out, breakdowns, weather and detention.
Time only moves when you act, every Sunday closes with the week's
scorecard, and the money comes in weeks after the work, the way it does.

## Keys

| Key | Effect |
|---|---|
| Up / Down | Move, with wraparound |
| Home / End | First / last item |
| Letter | Jump to the next item starting with it |
| Enter | Select, or answer a ringing phone |
| Escape | Back |
| F1 | Help for the current item |
| Space | What needs you now, anywhere at the desk |
| F2 | Stop speech |

Settings on the start screen opens on Speech, Sound and Display, each with
its own list, the game controller, and Discord status, which shows your
friends the carrier, the day, the roster size and the trips on the road.
Soundtrack plays the pieces you have heard: the start screen's theme and the desk's three. Help reads the
manual a section at a time.

On macOS the app is not notarized yet: the first time, Control-click it and
choose Open.

## Build from source

Dispatch runs on [Hearken](https://github.com/orinks-games/hearken). Install the tools in
Hearken's Getting started, then in this folder:

```sh
cargo run --release
```

Run it from this folder: that is where the game finds its `sounds` folder.

On Linux build with gcc 13 (`CC=gcc-13 CXX=g++-13`) and set
`CMAKE_POLICY_VERSION_MINIMUM=3.5` for the bundled SDL2.

## Releases

Releases are made by Hearken's `hearken release` command, with the settings in
`[package.metadata.hearken]` in `Cargo.toml`: the `sounds` folder ships as one
encrypted `sounds.pak`, careers are sealed, and the Windows and Linux builds keep
their saves beside the game. Before running it, put the baked world data
(`world.ffdata` and `carriers.json`, made by Freight Fate's `ff-bake` at the
ff-core pin) in `freight_fate/data`. CI does all of this on a `v*` tag and nightly;
`.github/workflows/ci.yml` has the steps.

## Files

- Dispatch is portable, like Freight Fate. Careers, settings and logs live in a `saves` folder
  inside the `FreightFateDispatch` folder, so moving or copying that folder takes your careers
  with it. Careers save in `saves/careers` after everything you do, so Escape at the desk goes
  back to the start screen with nothing lost.
  - On macOS, saves go in `~/Library/Application Support/FreightFateDispatch` instead, because
    apps can't write beside themselves in Applications.
  - Keep the game in a folder you can write to, not under `Program Files`: a release always saves
    beside itself and can't save anywhere else.
  - Careers from an earlier version in those per-user folders are copied into the new `saves`
    folder the first time it is created. The originals are left in place.
  - In a release, careers are stored in a form that can't be edited by hand. A career changed
    outside the game is listed as changed and left as it is. Careers from an earlier version load
    as they are and are converted the next time they save; the old file stays beside the new one
    with `.plain-backup` added to its name. Settings stay plain JSON.
  - `FF_DISPATCH_DATA_DIR` puts saves in another folder when running from source. Releases
    ignore it.
- Importing a Freight Fate driver only reads the Freight Fate save.
  - Dispatch looks in Freight Fate's own `profiles` folder (on Windows,
    `%APPDATA%\FreightFate\profiles`) and in a `FreightFate` folder unzipped beside this one.
  - To use another install, pick **Freight Fate saves folder** on the import screen and
    type its path. The profiles folder, its `saves` folder, the `FreightFate` folder, or the
    folder that holds it (for example `C:\ff-tester`) all work. `%APPDATA%` and `~` are expanded,
    and Ctrl+V (Cmd+V on a Mac) pastes. Dispatch remembers it.
  - `FF_DISPATCH_FREIGHT_FATE_SAVES` still wins over that setting when it is set, and accepts the
    same folders.
- `FF_DISPATCH_NO_SPEECH=1` starts with speech off; `FF_DISPATCH_STREAM_TRANSCRIPT=1` prints
  every spoken line to the console. `FF_DISPATCH_AUDIO_BACKEND=silent` turns sound off without
  touching speech. The session log is `session.log` in the saves folder.
- `--seed N --script FILE` plays a key file with no window or voice and prints what was said;
  `tests/fixtures/first_shift.keys` is one.
- The sounds are files in the `sounds` folder, named by key: `ui/cursor.wav`,
  `sfx/phone_ring.ogg`, `music/rolling/bass.ogg` and so on. Built from source, the game reads
  them from the folder it is started in, or from the folder `FF_DISPATCH_SOUNDS` names; a
  release carries them in `sounds.pak`. `audio/README.md` lists the keys and how the sounds are
  made.
- Online services are off by default. The complete career works without an
  account or internet connection.
- The public online drivers board works without signing in.
- Sign-in to orinks.net is optional, uses your Freight Fate account, and posts
  an idempotent road-journal line when a dispatch day closes.
