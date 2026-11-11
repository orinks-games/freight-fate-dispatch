# Freight Fate: Dispatch (beta)

Run the desk at a Freight Fate carrier. Answer customers' tenders, work
the brokers' spot board, assign loads, then handle what the road throws
back over the phone: hours running out, breakdowns, weather and detention.
Time only moves when you act, and every Sunday closes with the week's
scorecard.

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

Settings on the start screen has the speech rate, a volume for each kind of
sound including the music, menu positions, and the game controller. Help
reads the manual a section at a time.

On macOS the app is not notarized yet: the first time, Control-click it and
choose Open.

## Build from source

Dispatch runs on [Hearken](https://github.com/orinks-games/hearken). Install the tools in
Hearken's Getting started, then in this folder:

```sh
cargo run --release
```

On Linux build with gcc 13 (`CC=gcc-13 CXX=g++-13`) and set
`CMAKE_POLICY_VERSION_MINIMUM=3.5` for the bundled SDL2.

## Files

- Dispatch is portable, like Freight Fate. Careers, settings and logs live in a `saves` folder
  inside the `FreightFateDispatch` folder, so moving or copying that folder takes your careers
  with it. Careers save at the start of each day in `saves/careers`.
  - On macOS, saves go in `~/Library/Application Support/FreightFateDispatch` instead, because
    apps can't write beside themselves in Applications.
  - If the game folder can't be written to (for example under `Program Files`), Dispatch saves to
    `%APPDATA%\FreightFateDispatch` on Windows or `~/.local/share/FreightFateDispatch` on Linux.
  - Careers from an earlier version in those per-user folders are copied into the new `saves`
    folder the first time it is created. The originals are left in place.
  - `FF_DISPATCH_DATA_DIR` puts saves in another folder.
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
- Recordings dropped in a `sounds` folder beside the game replace the built-in ones, named by
  key: `ui/cursor.ogg`, `desk/phone.wav`, `music/desk_1.ogg` and so on.
- Online services are off by default. The complete career works without an
  account or internet connection.
- The public online drivers board works without signing in.
- Sign-in to orinks.net is optional, uses your Freight Fate account, and posts
  an idempotent road-journal line when a dispatch day closes.
