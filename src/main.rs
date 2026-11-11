// Dispatch is a windowed game, so the binary is linked into the Windows GUI
// subsystem: no terminal window ever opens beside it. For a screen reader a
// console is a second window competing for focus, not a cosmetic leftover.
// `attach_parent_console` gives `--script` and no-speech runs their output
// back when started from a shell. tests/windows_subsystem.rs pins this.
#![cfg_attr(windows, windows_subsystem = "windows")]

//! Freight Fate: Dispatch. Finds the world data and the save folder, then
//! hands the desk to Hearken; `--script FILE` plays a key file with no
//! window instead.

use ff_dispatch::import::default_profile_dirs;
use ff_dispatch::saves::{careers_dir, data_dir, DATA_DIR_ENV};
use ff_dispatch::shell::{self, Data, Desk, Dispatch};
use ff_dispatch::ui::app::Dirs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    attach_parent_console(&args);
    let seed = shell::arg(&args, "--seed").and_then(|s| s.parse().ok());
    let script = shell::script_arg(&args);
    // The engine keeps its settings and log in the game's data folder, so
    // the folder Dispatch works out (portable beside the game, or per
    // user) is handed to it the one way it takes: the environment.
    let data = data_dir();
    if std::env::var_os(DATA_DIR_ENV).is_none_or(|v| v.is_empty()) {
        std::env::set_var(DATA_DIR_ENV, &data);
    }
    let root = ff_core::data::data_resources::data_root();
    if !root.join("world.ffdata").is_file() && !root.join("world_data/index.json").is_file() {
        fatal(&format!(
            "World data is missing. Dispatch expected it at {}.",
            root.join("world.ffdata").display()
        ));
        std::process::exit(1);
    }
    let world = ff_core::data::world::get_world();
    let dirs = Dirs {
        careers: careers_dir(),
        ff_saves: default_profile_dirs(),
        data,
    };
    let data = Data {
        world,
        dirs,
        seed,
        can_go_online: script.is_none(),
    };
    let code = match script {
        Some(path) => shell::run_script(data, &path),
        None => {
            let desk = Desk::new(&data);
            hearken::run::<Dispatch>(data, move || Box::new(desk))
        }
    };
    std::process::exit(code);
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

    let no_speech = std::env::var("FF_DISPATCH_NO_SPEECH").is_ok_and(|v| !v.is_empty());
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

/// A windowed build has no console, so a startup failure is shown in a
/// dialog the screen reader reads, and printed for anyone running a script.
fn fatal(msg: &str) {
    eprintln!("{msg}");
    let _ = sdl2::messagebox::show_simple_message_box(
        sdl2::messagebox::MessageBoxFlag::ERROR,
        "Freight Fate: Dispatch",
        msg,
        None,
    );
}
