// Dispatch is a windowed game, so the binary is linked into the Windows GUI
// subsystem: no terminal window ever opens beside it. For a screen reader a
// console is a second window competing for focus, not a cosmetic leftover.
// `attach_parent_console` gives `--script` and no-speech runs their output
// back when started from a shell. tests/windows_subsystem.rs pins this.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! Freight Fate: Dispatch. The window, the keyboard and speech; all the
//! game is in the library.

use std::path::PathBuf;
use std::time::Duration;

use ff_dispatch::audio::Player;
use ff_dispatch::import::ff_profile_dirs;
use ff_dispatch::saves::{careers_dir, data_dir};
use ff_dispatch::ui::app::{App, Dirs};
use ff_dispatch::ui::speech::{speaker, Console, Speaker};
use ff_dispatch::ui::{Key, Out};
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    attach_parent_console(&args);
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let seed = arg("--seed").and_then(|s| s.parse().ok());
    let script = arg("--script").map(PathBuf::from);
    configure_logging(script.is_some());

    let mut voice: Box<dyn Speaker> = if script.is_some() {
        Box::new(Console {
            lines: Vec::new(),
            echo: true,
        })
    } else {
        speaker()
    };
    let root = ff_core::data::data_resources::data_root();
    if !root.join("world.ffdata").is_file() && !root.join("world_data/index.json").is_file() {
        let msg = format!(
            "World data is missing. Dispatch expected it at {}.",
            root.join("world.ffdata").display()
        );
        voice.say(&msg, true);
        eprintln!("{msg}");
        std::thread::sleep(Duration::from_secs(4)); // let the speech finish
        std::process::exit(1);
    }
    let world = ff_core::data::world::get_world();
    let dirs = Dirs {
        careers: careers_dir(),
        ff_saves: ff_profile_dirs(),
        data: data_dir(),
    };
    let mut app = App::new(world, dirs, seed, script.is_none());

    match script {
        Some(path) => {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            if flush(app.start(), voice.as_mut(), None) {
                return;
            }
            for key in text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .filter_map(Key::parse)
            {
                if flush(app.key(key), voice.as_mut(), None)
                    || flush(app.tick(), voice.as_mut(), None)
                {
                    return;
                }
            }
        }
        None => {
            if let Err(e) = run_window(&mut app, voice.as_mut()) {
                log::error!("window: {e}");
                eprintln!("{e}");
            }
        }
    }
}

/// Speak what the app said; `true` means quit.
fn flush(out: Vec<Out>, voice: &mut dyn Speaker, audio: Option<&Player>) -> bool {
    for o in out {
        match o {
            Out::Say(t) => voice.say(&t, true),
            Out::Queue(t) => voice.say(&t, false),
            Out::Cue(cue) => {
                if let Some(audio) = audio {
                    audio.play(cue);
                }
            }
            Out::Stop => voice.stop(),
            Out::Quit => return true,
        }
    }
    false
}

fn translate(keycode: Keycode, keymod: Mod) -> Option<Key> {
    // sdl2 0.38: Keycode is a struct with associated consts (FF matches the same way).
    Some(match keycode {
        Keycode::UP => Key::Up,
        Keycode::DOWN => Key::Down,
        Keycode::HOME => Key::Home,
        Keycode::END => Key::End,
        Keycode::RETURN | Keycode::KP_ENTER => Key::Enter,
        Keycode::ESCAPE => Key::Escape,
        Keycode::F1 => Key::F1,
        Keycode::LCTRL | Keycode::RCTRL => Key::Ctrl,
        _ if keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) => return None,
        k => {
            let name = k.name();
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

fn run_window(app: &mut App, voice: &mut dyn Speaker) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let _window = video
        .window("Freight Fate: Dispatch", 640, 480)
        .position_centered()
        .build()
        .map_err(|e| e.to_string())?;
    let mut pump = sdl.event_pump()?;
    let audio = if std::env::var_os("FF_DISPATCH_NO_AUDIO").is_some() {
        None
    } else {
        Player::new(&sdl)
            .map_err(|error| log::warn!("audio disabled: {error}"))
            .ok()
    };
    if flush(app.start(), voice, audio.as_ref()) {
        return Ok(());
    }
    loop {
        if let Some(event) = pump.wait_event_timeout(100) {
            for event in std::iter::once(event).chain(pump.poll_iter()) {
                match event {
                    Event::Quit { .. } => return Ok(()),
                    Event::KeyDown {
                        keycode: Some(k),
                        keymod,
                        repeat: false,
                        ..
                    } => {
                        if let Some(key) = translate(k, keymod) {
                            if flush(app.key(key), voice, audio.as_ref()) {
                                return Ok(());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if flush(app.tick(), voice, audio.as_ref()) {
            return Ok(());
        }
    }
}

/// Borrow the parent shell's console so a terminal run still prints.
///
/// Only with an argument or `FF_DISPATCH_NO_SPEECH=1`: a player's launch
/// (double-click, shortcut) stays detached, so a Ctrl-C or a closing shell
/// cannot kill it. `AttachConsole` never creates a console, so nothing here
/// can put a window on screen. Redirected handles are restored afterwards,
/// because the attach overwrites them (Freight Fate measured this: a
/// redirected run otherwise writes an empty file).
#[cfg(windows)]
fn attach_parent_console(args: &[String]) {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    let no_speech = std::env::var(ff_dispatch::ui::speech::NO_SPEECH_ENV).is_ok_and(|v| v == "1");
    if args.len() < 2 && !no_speech {
        return;
    }
    // SAFETY: kernel32 calls on the process's own standard handles, which are
    // read and put back, never closed; no pointers of ours are passed.
    unsafe {
        let saved: [(u32, HANDLE); 3] = [
            (STD_OUTPUT_HANDLE, GetStdHandle(STD_OUTPUT_HANDLE)),
            (STD_ERROR_HANDLE, GetStdHandle(STD_ERROR_HANDLE)),
            (STD_INPUT_HANDLE, GetStdHandle(STD_INPUT_HANDLE)),
        ];
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        for (slot, previous) in saved {
            if !previous.is_null() && previous != INVALID_HANDLE_VALUE {
                SetStdHandle(slot, previous);
            }
        }
    }
}

#[cfg(not(windows))]
fn attach_parent_console(_args: &[String]) {}

fn configure_logging(script: bool) {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if !script {
        let dir = data_dir().join("logs");
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(file) = std::fs::File::create(dir.join("dispatch.log")) {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
        }
    }
    let _ = builder.try_init();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("Fatal error: {info}");
        default_hook(info);
    }));
}
