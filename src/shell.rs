//! Dispatch on Hearken: what the engine needs to know about the game, the
//! one screen that hosts the desk's menus, and the scripted runner that
//! plays a key file with no window or voice and prints what was said.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ff_core::data::world::World;
use hearken::audio::{Audio, ClipSource};
use hearken::screens::{HelpScreen, SettingsScreen};
use hearken::sound::{SoundConfig, UiSound};
use hearken::speech::{CaptureSpeech, SharedCapture};
use hearken::SoundtrackScreen;
use hearken::{Bus, Ctx, Game, InputEvent, Key, State};

use crate::audio::{Cue, Music, DEFAULT_MUSIC_VOLUME};
use crate::ui::app::{App, Dirs};
use crate::ui::{Key as DeskKey, Out};

/// The help document, compiled in.
pub const HELP: &str = include_str!("../help.md");

pub struct Dispatch;

/// What the desk needs to start: the world, where files live, and the
/// seed and online allowance a scripted run fixes.
pub struct Data {
    pub world: &'static World,
    pub dirs: Dirs,
    pub seed: Option<i64>,
    pub can_go_online: bool,
}

impl hearken::Game for Dispatch {
    const TITLE: &'static str = "Freight Fate: Dispatch";
    const DATA_DIR: &'static str = "FreightFateDispatch";
    const ENV_PREFIX: &'static str = "FF_DISPATCH";
    type Event = ();
    type Data = Data;
    type Settings = ();
    type Action = hearken::NoAction;
    type Category = hearken::NoCategory;

    fn builtin_clips() -> Option<Box<dyn ClipSource>> {
        Some(Box::new(crate::audio::clips()))
    }

    /// The three pieces, unlocked as they are heard at the desk.
    fn soundtrack() -> Vec<hearken::Track> {
        crate::audio::tracks()
    }

    /// The desk's own earcons on the engine's menus (Settings, Help), and
    /// music under speech from the first launch.
    fn sound() -> SoundConfig {
        let mut sound = SoundConfig::default();
        sound.ui.set(UiSound::Open, None);
        sound.ui.set(UiSound::Move, Some(Cue::Cursor.key()));
        sound.ui.set(UiSound::Select, Some(Cue::Activate.key()));
        sound.ui.set(UiSound::Back, Some(Cue::Back.key()));
        sound.ui.set(UiSound::Tick, Some(Cue::Type.key()));
        sound.ui.set(UiSound::Error, Some(Cue::Error.key()));
        sound.ui.set(UiSound::Notify, Some(Cue::Notify.key()));
        sound.default_volume = |bus| {
            if bus == Bus::MUSIC {
                DEFAULT_MUSIC_VOLUME
            } else {
                hearken::settings::default_volume(bus)
            }
        };
        sound
    }
}

/// The desk: every Dispatch menu, from the start screen to the phone, as
/// one engine screen. The `App` turns keys into lines; this screen turns
/// the lines into speech and sound, and opens the engine's Settings, Help
/// and soundtrack screens over itself.
pub struct Desk {
    app: App,
    music: Music,
}

impl Desk {
    pub fn new(data: &Data) -> Desk {
        Desk {
            app: App::new(data.world, data.dirs.clone(), data.seed, data.can_go_online),
            music: Music::new(),
        }
    }

    fn emit(&mut self, ctx: &mut Ctx<Dispatch>, out: Vec<Out>) {
        for o in out {
            match o {
                Out::Say(text) => ctx.say(text),
                Out::Queue(text) => ctx.say_queued(text),
                Out::Cue(cue) => {
                    ctx.audio.play(cue.key(), cue.bus());
                    // The day's music starts with the day.
                    if cue == Cue::DayStart {
                        self.music.change(ctx);
                    }
                }
                Out::Stop => ctx.stop_speech(),
                Out::OpenSettings => ctx.push(SettingsScreen::<Dispatch>::new()),
                Out::OpenHelp => ctx.push(HelpScreen::<Dispatch>::new(HELP)),
                Out::OpenSoundtrack => ctx.push(SoundtrackScreen::<Dispatch>::new()),
                Out::Quit => ctx.quit(),
            }
        }
    }

    /// The desk keys an input event stands for: none for a key the desk
    /// has no use for. Typed text reaches the saves-folder field with its
    /// case and symbols; elsewhere a letter jumps the menu.
    fn keys(&self, event: &InputEvent) -> Vec<DeskKey> {
        let Some(key) = event.pressed() else {
            return Vec::new();
        };
        let (mods, text) = match event.key_down() {
            Some((_, mods, text)) => (mods, text),
            None => (hearken::Mods::NONE, None),
        };
        let typing = self.app.wants_text();
        let desk = match key {
            Key::Up => DeskKey::Up,
            Key::Down => DeskKey::Down,
            Key::Home => DeskKey::Home,
            Key::End => DeskKey::End,
            Key::Return | Key::KpEnter => DeskKey::Enter,
            Key::Escape => DeskKey::Escape,
            Key::Backspace => DeskKey::Backspace,
            Key::Space if !typing => DeskKey::Space,
            Key::V if typing && mods.ctrl => {
                return clipboard_text().chars().map(DeskKey::Char).collect();
            }
            _ if mods.ctrl || mods.alt => return Vec::new(),
            _ => match text {
                Some(c) if typing => DeskKey::Char(c),
                Some(c) if c.is_alphanumeric() => DeskKey::Char(c.to_ascii_lowercase()),
                _ => return Vec::new(),
            },
        };
        vec![desk]
    }
}

impl State<Dispatch> for Desk {
    fn name(&self) -> &'static str {
        "desk"
    }

    fn enter(&mut self, ctx: &mut Ctx<Dispatch>) {
        log::info!("{}", crate::paths::location_note());
        let out = self.app.start();
        self.emit(ctx, out);
    }

    fn exit(&mut self, ctx: &mut Ctx<Dispatch>) {
        self.music.stop(ctx);
    }

    /// Back from Settings, Help or the soundtrack: the screen underneath
    /// says where the player is.
    fn resume(&mut self, ctx: &mut Ctx<Dispatch>) {
        let out = self.app.announce();
        self.emit(ctx, out);
    }

    fn handle_input(&mut self, ctx: &mut Ctx<Dispatch>, event: &InputEvent) {
        if event.is_repeat() {
            return;
        }
        for key in self.keys(event) {
            let out = self.app.key(key);
            self.emit(ctx, out);
        }
    }

    /// Background work between keys: a held-back morning, online results,
    /// and the music.
    fn update(&mut self, ctx: &mut Ctx<Dispatch>, dt: f64) {
        self.music.update(ctx, dt, self.app.desk_minute());
        let out = self.app.tick();
        self.emit(ctx, out);
    }

    fn lines(&self, _ctx: &Ctx<Dispatch>) -> Vec<String> {
        self.app.screen_lines()
    }

    fn help(&self, _ctx: &Ctx<Dispatch>) -> String {
        self.app.help_line()
    }
}

/// What the player has copied, for Ctrl+V in the saves-folder field. Empty
/// with no window (a scripted run) or nothing copied.
fn clipboard_text() -> String {
    // SAFETY: SDL_GetClipboardText returns a NUL-terminated string the
    // caller frees with SDL_free, or NULL; nothing of ours is passed in. With
    // no video subsystem it returns an empty string, never a dangling one.
    unsafe {
        let p = sdl2::sys::SDL_GetClipboardText();
        if p.is_null() {
            return String::new();
        }
        let text = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
        sdl2::sys::SDL_free(p.cast());
        text
    }
}

/// One line of a key script as input events: `type <text>` types the text
/// as written, anything else is one key word.
pub fn script_line(line: &str) -> Vec<InputEvent> {
    if let Some(text) = line.trim_start().strip_prefix("type ") {
        return text.trim_end().chars().map(InputEvent::typed).collect();
    }
    let word = line.trim().to_ascii_lowercase();
    let key = match word.as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "home" => Key::Home,
        "end" => Key::End,
        "enter" => Key::Return,
        "escape" => Key::Escape,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        w if w.chars().count() == 1 => return vec![InputEvent::typed(w.chars().next().unwrap())],
        _ => return Vec::new(),
    };
    vec![InputEvent::key(key)]
}

/// Play `script` with no window, sound device or voice, printing every
/// line the game speaks. Saves go where a real run's would. Returns the
/// process exit code.
pub fn run_script(data: Data, script: &Path) -> i32 {
    let text = match std::fs::read_to_string(script) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("{}: {e}", script.display());
            return 2;
        }
    };
    let speech = Rc::new(RefCell::new(CaptureSpeech::new()));
    let paths = hearken::paths::Paths::discover(Dispatch::DATA_DIR, Dispatch::ENV_PREFIX);
    let (settings, _) = hearken::settings::Settings::load(&paths);
    let desk = Desk::new(&data);
    let mut ctx = Ctx::new(
        Box::new(SharedCapture(speech.clone())),
        settings,
        paths,
        data,
    );
    ctx.install_audio(Audio::silent());
    ctx.apply_language();
    let mut app = hearken::App::new(ctx, Box::new(desk));
    let print = |speech: &Rc<RefCell<CaptureSpeech>>| {
        let mut speech = speech.borrow_mut();
        for line in speech.main_lines() {
            println!("{line}");
        }
        speech.clear();
    };
    print(&speech);
    for event in text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .flat_map(script_line)
    {
        app.step(vec![event], 0.0);
        print(&speech);
        app.step(Vec::new(), 0.1);
        print(&speech);
        if !app.ctx.running {
            break;
        }
    }
    0
}

/// `--seed N` and `--script FILE` from the command line.
pub fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

pub fn script_arg(args: &[String]) -> Option<PathBuf> {
    arg(args, "--script").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_words_become_events() {
        assert_eq!(script_line("enter"), [InputEvent::key(Key::Return)]);
        assert_eq!(script_line("F1"), [InputEvent::key(Key::F1)]);
        assert_eq!(script_line("c"), [InputEvent::typed('c')]);
        assert_eq!(
            script_line("type C:\\ff"),
            ['C', ':', '\\', 'f', 'f'].map(InputEvent::typed)
        );
        assert!(script_line("nonsense").is_empty());
    }
}
