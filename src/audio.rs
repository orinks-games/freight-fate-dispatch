//! The game's sounds: the interface clicks, the desk and road effects, and
//! four pieces of music that play as looping stems the engine arranges
//! anew each time. They are files in the repository's `sounds` folder,
//! played through Hearken's sound pool by key: `sounds/sfx/horn.ogg` is
//! `sfx/horn`. A developer build reads that folder from the current
//! directory (run from the repository root, as `cargo run` and `hearken
//! play` do) or from the folder `FF_DISPATCH_SOUNDS` names. A release made
//! by `hearken release` ships it as one encrypted `sounds.pak` and reads
//! nothing else. Gameplay never depends on audio being available.
//!
//! Only the seven interface cues are also compiled in ([`clips`]), so the
//! menus still click if a release's pack is missing or damaged.
//!
//! The sounds are made by `audio/effects.py` and `audio/soundtrack.py`;
//! see `audio/README.md`.

use hearken::audio::{ClipSource, MemoryClips};
use hearken::{Bus, Ctx, Performer, Stems, Track, Variation, Variety};

/// The music bus before the player changes it: well under speech.
pub const DEFAULT_MUSIC_VOLUME: u8 = 40;

macro_rules! cues {
    ($($name:ident => $key:literal,)*) => {
        /// Every one-shot sound the game plays, by the moment it marks.
        #[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
        pub enum Cue {
            $($name,)*
        }

        impl Cue {
            pub const ALL: &'static [Cue] = &[$(Cue::$name,)*];

            /// The sound key, which is also the file under `sounds`.
            pub const fn key(self) -> &'static str {
                match self {
                    $(Cue::$name => $key,)*
                }
            }
        }
    };
}

cues! {
    // Interface: the menus and the typed field.
    Cursor => "ui/cursor",
    Type => "ui/type",
    Activate => "ui/activate",
    Back => "ui/back",
    Error => "ui/error",
    Success => "ui/success",
    Notify => "ui/notify",
    // The phone.
    PhoneRing => "sfx/phone_ring",
    PhonePickup => "sfx/phone_pickup",
    PhoneHangup => "sfx/phone_hangup",
    // Trucks and the radio.
    Radio => "sfx/radio",
    TruckIdle => "sfx/truck_idle",
    AirBrakes => "sfx/air_brakes",
    TruckDepart => "sfx/truck_depart",
    Horn => "sfx/horn",
    // Paperwork and money.
    Paper => "sfx/paper",
    Stamp => "sfx/stamp",
    Delivered => "sfx/delivered",
    Late => "sfx/late",
    Clock => "sfx/clock",
    Cash => "sfx/cash",
    Bill => "sfx/bill",
    Hire => "sfx/hire",
    Door => "sfx/door",
    // What the road throws back.
    Alert => "sfx/alert",
    Hours => "sfx/hours",
    Breakdown => "sfx/breakdown",
    Weather => "sfx/weather",
    Dock => "sfx/dock",
    Inspection => "sfx/inspection",
    Citation => "sfx/citation",
    OutOfService => "sfx/out_of_service",
    // Stings for the shape of the day and the week.
    DayStart => "sfx/day_start",
    DayClose => "sfx/day_close",
    Goal => "sfx/goal",
    WeekClose => "sfx/week_close",
    Lose => "sfx/lose",
}

impl Cue {
    /// Interface clicks go on the UI bus; everything else is game sound.
    pub fn bus(self) -> Bus {
        if self.key().starts_with("ui/") {
            Bus::UI
        } else {
            Bus::EFFECTS
        }
    }
}

/// One piece of the soundtrack: layers recorded to loop together, which
/// the engine arranges anew for every play (see [`hearken::Stems`]).
pub struct Piece {
    /// The save id and the folder under `sounds/music`.
    pub id: &'static str,
    pub title: &'static str,
    /// Read on F1 in the soundtrack menu.
    pub about: &'static str,
    /// Where it is heard, for the soundtrack menu while it is locked.
    pub hint: &'static str,
    /// Seconds in one loop of the stems.
    pub loop_s: f64,
    /// Stems that play the whole way through: the title theme keeps its
    /// tune in the bed, so the start menu always opens with it.
    pub bed: &'static [&'static str],
    /// Stems that come and go, in the order they best enter.
    pub voices: &'static [&'static str],
    /// About how long one play runs.
    pub length_s: f64,
}

/// Rendered by `audio/soundtrack.py`: 16 bars each, and every stem of a
/// piece the same length to the sample. The first is the start menu's;
/// the rest follow the desk clock.
pub const PIECES: [Piece; 4] = [
    Piece {
        id: "title",
        title: "Freight Fate",
        about: "The start menu: strings and Rhodes, upright bass, brushes, a picked steel guitar, piano on the tune and a French horn answering it. 84 beats a minute in D.",
        hint: "Heard on the start menu.",
        loop_s: 2_016_000.0 / 44_100.0,
        bed: &["music/title/keys", "music/title/piano"],
        voices: &[
            "music/title/bass",
            "music/title/drums",
            "music/title/guitar",
            "music/title/horn",
        ],
        length_s: 170.0,
    },
    Piece {
        id: "first_light",
        title: "First Light",
        about: "The morning at the desk: Rhodes and organ under brushes, upright bass, a picked guitar, vibes on the tune and a jazz guitar answering it. 98 beats a minute in G.",
        hint: "Heard at the desk.",
        loop_s: 1_728_000.0 / 44_100.0,
        bed: &["music/first_light/keys"],
        voices: &[
            "music/first_light/bass",
            "music/first_light/drums",
            "music/first_light/guitar",
            "music/first_light/vibes",
            "music/first_light/answers",
        ],
        length_s: 180.0,
    },
    Piece {
        id: "rolling",
        title: "Rolling",
        about: "The afternoon with trucks moving: a train beat, strummed steel guitar, picked bass, clarinet on the tune and a clean electric answering in sixths. 100 beats a minute in A.",
        hint: "Heard at the desk.",
        loop_s: 1_693_440.0 / 44_100.0,
        bed: &["music/rolling/organ"],
        voices: &[
            "music/rolling/bass",
            "music/rolling/drums",
            "music/rolling/guitar",
            "music/rolling/clarinet",
            "music/rolling/sixths",
        ],
        length_s: 170.0,
    },
    Piece {
        id: "night_desk",
        title: "Night Desk",
        about: "The evening with the board nearly cleared: Rhodes and slow strings, upright bass, brushes, a nylon guitar, piano on the tune and a French horn and clarinet far back, trading the answers. 75 beats a minute in E minor.",
        hint: "Heard at the desk.",
        loop_s: 2_257_920.0 / 44_100.0,
        bed: &["music/night_desk/keys"],
        voices: &[
            "music/night_desk/bass",
            "music/night_desk/drums",
            "music/night_desk/nylon",
            "music/night_desk/piano",
            "music/night_desk/answers",
        ],
        length_s: 200.0,
    },
];

/// How far a play may stray from the recording: the voices' levels within
/// 2 dB and a little apart in the stereo field, always at the written
/// pitch and tempo.
const VARIATION: Variation = Variation {
    semitones: 0,
    level_db: 2.0,
    pan: 0.3,
};

/// The interface cues, compiled in. The engine asks them after the
/// `sounds` folder or pack, so they play only when that has no such key:
/// a release whose pack is missing or damaged still clicks through its
/// menus.
const BUILT_IN: [(Cue, &[u8]); 7] = [
    (Cue::Cursor, include_bytes!("../sounds/ui/cursor.wav")),
    (Cue::Type, include_bytes!("../sounds/ui/type.wav")),
    (Cue::Activate, include_bytes!("../sounds/ui/activate.wav")),
    (Cue::Back, include_bytes!("../sounds/ui/back.wav")),
    (Cue::Error, include_bytes!("../sounds/ui/error.wav")),
    (Cue::Success, include_bytes!("../sounds/ui/success.wav")),
    (Cue::Notify, include_bytes!("../sounds/ui/notify.wav")),
];

/// The sounds the game carries inside itself: the interface cues.
pub fn clips() -> MemoryClips {
    BUILT_IN
        .iter()
        .fold(MemoryClips::default(), |clips, (cue, bytes)| {
            clips.with(cue.key(), *bytes)
        })
}

/// Every key the game plays: the cues, then every piece's stems.
pub fn keys() -> impl Iterator<Item = &'static str> {
    let stems = PIECES
        .iter()
        .flat_map(|p| p.bed.iter().chain(p.voices.iter()).copied());
    Cue::ALL.iter().map(|c| c.key()).chain(stems)
}

/// What a scripted run says about the game's sounds, which it never plays:
/// the engine's notice when a release's `sounds.pak` is missing or does not
/// open with this program's key, and, in a release made by `hearken
/// release`, each key the pack does not hold. Empty when all is well, so a
/// packaged smoke test fails on any line it prints.
pub fn sound_problems(env_prefix: &str) -> Vec<String> {
    let found = hearken::audio::discover_clips(env_prefix);
    if let Some(notice) = found.notice {
        return vec![notice];
    }
    if option_env!("HEARKEN_RELEASE") != Some("1") {
        return Vec::new();
    }
    let mut clips = found.clips;
    keys()
        .filter(|key| clips.load(key).is_none())
        .map(|key| format!("The sound pack has no {key}."))
        .collect()
}

/// The soundtrack as the engine lists it: each piece unlocks the first
/// time it is heard.
pub fn tracks() -> Vec<Track> {
    PIECES
        .iter()
        .map(|p| {
            let stems = Stems::new(p.loop_s)
                .bed(p.bed.iter().copied())
                .voices(p.voices.iter().copied())
                .length_s(p.length_s);
            Track::stems(p.id, p.title, stems)
                .about(p.about)
                .hint(p.hint)
                .unlocked_by(p.bed.iter().chain(p.voices.iter()).copied())
                .variation(VARIATION)
                .varied()
        })
        .collect()
}

/// The start menu's piece, in [`PIECES`].
pub const TITLE: usize = 0;

/// The piece for a time of day on the desk clock (minutes since
/// midnight): the morning until noon, the road through the afternoon, and
/// the night piece from five.
pub fn piece_for(minute: u32) -> usize {
    match minute {
        m if m < 12 * 60 => 1,
        m if m < 17 * 60 => 2,
        _ => 3,
    }
}

/// How long a performance that ended almost at once (no sound device, a
/// pool with no room) waits before the next is tried, in seconds.
const RETRY_S: f64 = 30.0;

/// The music: one performance after another for as long as the desk is
/// up, each a fresh arrangement of the title theme on the start menu or
/// of the piece for the time of day once a career is open.
pub struct Music {
    playing: Option<Box<dyn Performer>>,
    started_at: f64,
    retry_at: f64,
    last: Option<usize>,
}

impl Music {
    pub fn new() -> Music {
        Music {
            playing: None,
            started_at: f64::NEG_INFINITY,
            retry_at: f64::NEG_INFINITY,
            last: None,
        }
    }

    /// Keeps the performance going, and starts the next once it has played
    /// out: the piece for `minute` on the desk clock, or the title theme
    /// when no career is open. Crossing between the two, as the desk opens
    /// or closes, ends the performance under way.
    pub fn update<G: hearken::Game>(&mut self, ctx: &mut Ctx<G>, dt: f64, minute: Option<u32>) {
        if self.playing.is_some() && self.last.map(|i| i == TITLE) != Some(minute.is_none()) {
            self.change(ctx);
        }
        if let Some(performer) = self.playing.as_mut() {
            if performer.update(&mut ctx.audio, dt) {
                return;
            }
            self.playing = None;
            if ctx.clock - self.started_at < 5.0 {
                self.retry_at = ctx.clock + RETRY_S;
                return;
            }
        }
        if ctx.clock < self.retry_at {
            return;
        }
        let index = minute.map_or(TITLE, piece_for);
        let tracks = tracks();
        let mut performer = tracks[index].perform(Variety::fresh());
        performer.start(&mut ctx.audio);
        self.last = Some(index);
        self.started_at = ctx.clock;
        self.playing = Some(performer);
    }

    pub fn stop<G: hearken::Game>(&mut self, ctx: &mut Ctx<G>) {
        if let Some(mut performer) = self.playing.take() {
            performer.stop(&mut ctx.audio);
        }
    }

    /// Lets the current performance go, so the next update starts the
    /// piece for where the player is.
    pub fn change<G: hearken::Game>(&mut self, ctx: &mut Ctx<G>) {
        self.stop(ctx);
        self.retry_at = f64::NEG_INFINITY;
    }
}

impl Default for Music {
    fn default() -> Self {
        Music::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hearken::audio::{discover_clips_from, LooseClips};
    use std::path::{Path, PathBuf};

    fn sounds_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("sounds")
    }

    fn every_key() -> Vec<&'static str> {
        keys().collect()
    }

    fn is_wav_or_ogg(bytes: &[u8]) -> bool {
        bytes.len() > 4 && (&bytes[..4] == b"RIFF" || &bytes[..4] == b"OggS")
    }

    #[test]
    fn every_cue_and_stem_is_a_file_the_engine_loads_and_decodes() {
        let mut clips = LooseClips::at(sounds_dir());
        for key in every_key() {
            let bytes = clips
                .load(key)
                .unwrap_or_else(|| panic!("no file for {key}"));
            assert!(is_wav_or_ogg(&bytes), "{key} is neither WAV nor Ogg");
            if key.starts_with("music/") {
                assert_eq!(&bytes[..4], b"OggS", "{key} is not an Ogg");
            }
        }
        assert!(clips.load("music/first_light/kazoo").is_none());
    }

    /// What a developer build does: with no pack key and no
    /// `FF_DISPATCH_SOUNDS`, the engine takes the `sounds` folder of the
    /// current directory, which is the repository root under `cargo test`
    /// and `cargo run`.
    #[test]
    fn a_developer_build_run_from_the_repository_finds_every_sound() {
        let found = discover_clips_from("FF_DISPATCH_TEST_UNSET", None, None);
        assert!(found.notice.is_none());
        let mut clips = found.clips;
        for key in every_key() {
            assert!(clips.load(key).is_some(), "{key} was not found");
        }
    }

    #[test]
    fn every_file_in_the_sounds_folder_is_a_sound_the_game_plays() {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    let rel = path.strip_prefix(root).unwrap().with_extension("");
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut files = Vec::new();
        walk(&sounds_dir(), &sounds_dir(), &mut files);
        let keys = every_key();
        for file in &files {
            assert!(keys.contains(&file.as_str()), "{file} is never played");
        }
        assert_eq!(files.len(), keys.len(), "two files share a key");
    }

    #[test]
    fn only_the_interface_cues_are_built_in_and_they_match_their_files() {
        let mut built_in = clips();
        let mut files = LooseClips::at(sounds_dir());
        for cue in Cue::ALL {
            let compiled = built_in.load(cue.key());
            assert_eq!(compiled.is_some(), cue.bus() == Bus::UI, "{cue:?}");
            if let Some(bytes) = compiled {
                assert_eq!(Some(bytes), files.load(cue.key()), "{cue:?}");
            }
        }
        assert!(built_in.load(PIECES[TITLE].bed[0]).is_none());
    }

    #[test]
    fn every_piece_names_stems_of_its_own_and_can_be_arranged() {
        for piece in &PIECES {
            for key in piece.bed.iter().chain(piece.voices.iter()) {
                assert!(key.starts_with(&format!("music/{}/", piece.id)), "{key}");
            }
            let stems = Stems::new(piece.loop_s)
                .bed(piece.bed.iter().copied())
                .voices(piece.voices.iter().copied());
            assert!(
                stems.arranges(),
                "{} has too few voices to arrange",
                piece.id
            );
            assert!(stems.loop_is_valid(), "{}", piece.id);
        }
        assert_eq!(tracks().len(), PIECES.len());
    }

    #[test]
    fn the_piece_follows_the_desk_clock() {
        assert_eq!(PIECES[TITLE].id, "title");
        assert!((1..PIECES.len()).all(|i| i != TITLE));
        assert_eq!(PIECES[piece_for(6 * 60)].id, "first_light");
        assert_eq!(PIECES[piece_for(11 * 60 + 59)].id, "first_light");
        assert_eq!(PIECES[piece_for(12 * 60)].id, "rolling");
        assert_eq!(PIECES[piece_for(16 * 60 + 59)].id, "rolling");
        assert_eq!(PIECES[piece_for(17 * 60)].id, "night_desk");
        assert_eq!(PIECES[piece_for(23 * 60)].id, "night_desk");
    }

    #[test]
    fn interface_cues_are_on_the_ui_bus_and_the_rest_are_effects() {
        assert_eq!(Cue::Cursor.bus(), Bus::UI);
        assert_eq!(Cue::Success.bus(), Bus::UI);
        assert_eq!(Cue::PhoneRing.bus(), Bus::EFFECTS);
        assert_eq!(Cue::DayStart.bus(), Bus::EFFECTS);
    }
}
