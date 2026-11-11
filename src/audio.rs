//! The game's sounds: the interface clicks, the desk and road effects, and
//! three pieces of music that play as looping stems the engine arranges
//! anew each time. Everything is compiled into the executable and played
//! through Hearken's sound pool by key; a file of the same key in a
//! `sounds` folder beside the game replaces the built-in one. Gameplay
//! never depends on audio being available.
//!
//! The sounds are made by `audio/effects.py` and `audio/soundtrack.py`;
//! see `audio/README.md`.

use hearken::audio::MemoryClips;
use hearken::{Bus, Ctx, Performer, Stems, Track, Variation, Variety};

/// The music bus before the player changes it: well under speech.
pub const DEFAULT_MUSIC_VOLUME: u8 = 40;

macro_rules! cues {
    ($($name:ident => $key:literal . $ext:literal,)*) => {
        /// Every one-shot sound the game plays, by the moment it marks.
        #[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
        pub enum Cue {
            $($name,)*
        }

        impl Cue {
            pub const ALL: &'static [Cue] = &[$(Cue::$name,)*];

            /// The sound key, which is also the file under `assets/sounds`.
            pub const fn key(self) -> &'static str {
                match self {
                    $(Cue::$name => $key,)*
                }
            }

            fn bytes(self) -> &'static [u8] {
                match self {
                    $(Cue::$name => include_bytes!(concat!("../assets/sounds/", $key, ".", $ext)),)*
                }
            }
        }
    };
}

cues! {
    // Interface: the menus and the typed field.
    Cursor => "ui/cursor"."wav",
    Type => "ui/type"."wav",
    Activate => "ui/activate"."wav",
    Back => "ui/back"."wav",
    Error => "ui/error"."wav",
    Success => "ui/success"."wav",
    Notify => "ui/notify"."wav",
    // The phone.
    PhoneRing => "sfx/phone_ring"."ogg",
    PhonePickup => "sfx/phone_pickup"."wav",
    PhoneHangup => "sfx/phone_hangup"."wav",
    // Trucks and the radio.
    Radio => "sfx/radio"."wav",
    TruckIdle => "sfx/truck_idle"."ogg",
    AirBrakes => "sfx/air_brakes"."wav",
    TruckDepart => "sfx/truck_depart"."ogg",
    Horn => "sfx/horn"."ogg",
    // Paperwork and money.
    Paper => "sfx/paper"."wav",
    Stamp => "sfx/stamp"."wav",
    Delivered => "sfx/delivered"."wav",
    Late => "sfx/late"."wav",
    Clock => "sfx/clock"."wav",
    Cash => "sfx/cash"."wav",
    Bill => "sfx/bill"."wav",
    Hire => "sfx/hire"."wav",
    Door => "sfx/door"."wav",
    // What the road throws back.
    Alert => "sfx/alert"."wav",
    Hours => "sfx/hours"."ogg",
    Breakdown => "sfx/breakdown"."ogg",
    Weather => "sfx/weather"."ogg",
    Dock => "sfx/dock"."ogg",
    Inspection => "sfx/inspection"."wav",
    Citation => "sfx/citation"."wav",
    OutOfService => "sfx/out_of_service"."ogg",
    // Stings for the shape of the day and the week.
    DayStart => "sfx/day_start"."ogg",
    DayClose => "sfx/day_close"."ogg",
    Goal => "sfx/goal"."ogg",
    WeekClose => "sfx/week_close"."ogg",
    Lose => "sfx/lose"."ogg",
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
    /// The save id and the folder under `assets/sounds/music`.
    pub id: &'static str,
    pub title: &'static str,
    /// Read on F1 in the soundtrack menu.
    pub about: &'static str,
    /// Seconds in one loop of the stems.
    pub loop_s: f64,
    /// Stems that play the whole way through.
    pub bed: &'static [&'static str],
    /// Stems that come and go, in the order they best enter.
    pub voices: &'static [&'static str],
    /// About how long one play runs.
    pub length_s: f64,
}

/// Rendered by `audio/soundtrack.py`: 16 bars each, and every stem of a
/// piece the same length to the sample.
pub const PIECES: [Piece; 3] = [
    Piece {
        id: "first_light",
        title: "First Light",
        about: "The morning at the desk: Rhodes and organ under brushes, upright bass, a picked guitar, vibes on the tune and a jazz guitar answering it. 98 beats a minute in G.",
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
        about: "The afternoon with trucks moving: a train beat, strummed steel guitar, picked bass, harmonica on the tune and a clean electric answering in sixths. 100 beats a minute in A.",
        loop_s: 1_693_440.0 / 44_100.0,
        bed: &["music/rolling/organ"],
        voices: &[
            "music/rolling/bass",
            "music/rolling/drums",
            "music/rolling/guitar",
            "music/rolling/harmonica",
            "music/rolling/sixths",
        ],
        length_s: 170.0,
    },
    Piece {
        id: "night_desk",
        title: "Night Desk",
        about: "The evening with the board nearly cleared: Rhodes and slow strings, upright bass, brushes, a nylon guitar, piano on the tune and a muted trumpet far back. 75 beats a minute in E minor.",
        loop_s: 2_257_920.0 / 44_100.0,
        bed: &["music/night_desk/keys"],
        voices: &[
            "music/night_desk/bass",
            "music/night_desk/drums",
            "music/night_desk/nylon",
            "music/night_desk/piano",
            "music/night_desk/trumpet",
        ],
        length_s: 200.0,
    },
];

/// Every stem, by key.
const STEMS: [(&str, &[u8]); 18] = [
    (
        "music/first_light/keys",
        include_bytes!("../assets/sounds/music/first_light/keys.ogg"),
    ),
    (
        "music/first_light/bass",
        include_bytes!("../assets/sounds/music/first_light/bass.ogg"),
    ),
    (
        "music/first_light/drums",
        include_bytes!("../assets/sounds/music/first_light/drums.ogg"),
    ),
    (
        "music/first_light/guitar",
        include_bytes!("../assets/sounds/music/first_light/guitar.ogg"),
    ),
    (
        "music/first_light/vibes",
        include_bytes!("../assets/sounds/music/first_light/vibes.ogg"),
    ),
    (
        "music/first_light/answers",
        include_bytes!("../assets/sounds/music/first_light/answers.ogg"),
    ),
    (
        "music/rolling/organ",
        include_bytes!("../assets/sounds/music/rolling/organ.ogg"),
    ),
    (
        "music/rolling/bass",
        include_bytes!("../assets/sounds/music/rolling/bass.ogg"),
    ),
    (
        "music/rolling/drums",
        include_bytes!("../assets/sounds/music/rolling/drums.ogg"),
    ),
    (
        "music/rolling/guitar",
        include_bytes!("../assets/sounds/music/rolling/guitar.ogg"),
    ),
    (
        "music/rolling/harmonica",
        include_bytes!("../assets/sounds/music/rolling/harmonica.ogg"),
    ),
    (
        "music/rolling/sixths",
        include_bytes!("../assets/sounds/music/rolling/sixths.ogg"),
    ),
    (
        "music/night_desk/keys",
        include_bytes!("../assets/sounds/music/night_desk/keys.ogg"),
    ),
    (
        "music/night_desk/bass",
        include_bytes!("../assets/sounds/music/night_desk/bass.ogg"),
    ),
    (
        "music/night_desk/drums",
        include_bytes!("../assets/sounds/music/night_desk/drums.ogg"),
    ),
    (
        "music/night_desk/nylon",
        include_bytes!("../assets/sounds/music/night_desk/nylon.ogg"),
    ),
    (
        "music/night_desk/piano",
        include_bytes!("../assets/sounds/music/night_desk/piano.ogg"),
    ),
    (
        "music/night_desk/trumpet",
        include_bytes!("../assets/sounds/music/night_desk/trumpet.ogg"),
    ),
];

/// How far a play may stray from the recording: the voices' levels within
/// 2 dB and a little apart in the stereo field, always at the written
/// pitch and tempo.
const VARIATION: Variation = Variation {
    semitones: 0,
    level_db: 2.0,
    pan: 0.3,
};

/// Every sound the game carries inside itself, by key.
pub fn clips() -> MemoryClips {
    let mut clips = MemoryClips::default();
    for cue in Cue::ALL {
        clips = clips.with(cue.key(), cue.bytes());
    }
    for (key, bytes) in STEMS {
        clips = clips.with(key, bytes);
    }
    clips
}

/// The soundtrack as the engine lists it: each piece unlocks the first
/// time it is heard at the desk.
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
                .hint("Heard at the desk.")
                .unlocked_by(p.bed.iter().chain(p.voices.iter()).copied())
                .variation(VARIATION)
                .varied()
        })
        .collect()
}

/// The piece for a time of day on the desk clock (minutes since
/// midnight): the morning until noon, the road through the afternoon, and
/// the night piece from five.
pub fn piece_for(minute: u32) -> usize {
    match minute {
        m if m < 12 * 60 => 0,
        m if m < 17 * 60 => 1,
        _ => 2,
    }
}

/// How long a performance that ended almost at once (no sound device, a
/// pool with no room) waits before the next is tried, in seconds.
const RETRY_S: f64 = 30.0;

/// The desk music: one performance after another for as long as the desk
/// is up, each a fresh arrangement of a piece chosen by the time of day.
pub struct Music {
    playing: Option<Box<dyn Performer>>,
    started_at: f64,
    retry_at: f64,
    last: Option<usize>,
    shuffle: Shuffle,
}

impl Music {
    pub fn new() -> Music {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Music {
            playing: None,
            started_at: f64::NEG_INFINITY,
            retry_at: f64::NEG_INFINITY,
            last: None,
            shuffle: Shuffle::new(seed),
        }
    }

    /// Keeps the performance going, and starts the next once it has played
    /// out: the piece for `minute` on the desk clock, or any piece but the
    /// last one when no career is open.
    pub fn update<G: hearken::Game>(&mut self, ctx: &mut Ctx<G>, dt: f64, minute: Option<u32>) {
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
        let index = match minute {
            Some(m) => piece_for(m),
            None => {
                let mut pick = self.shuffle.next(PIECES.len());
                if Some(pick) == self.last && PIECES.len() > 1 {
                    pick = (pick + 1) % PIECES.len();
                }
                pick
            }
        };
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
    /// piece for the time of day: for the moment the desk opens.
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

pub struct Shuffle {
    state: u64,
}

impl Shuffle {
    pub fn new(seed: u64) -> Shuffle {
        Shuffle { state: seed | 1 }
    }

    pub fn next(&mut self, count: usize) -> usize {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state % count as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hearken::audio::ClipSource;

    #[test]
    fn every_cue_and_stem_is_a_clip_in_a_format_the_engine_decodes() {
        let mut clips = clips();
        for cue in Cue::ALL {
            let bytes = clips.load(cue.key()).unwrap_or_else(|| panic!("{cue:?}"));
            let riff = &bytes[..4] == b"RIFF";
            let ogg = &bytes[..4] == b"OggS";
            assert!(riff || ogg, "{cue:?} is neither WAV nor Ogg");
        }
        for (key, _) in STEMS {
            let bytes = clips.load(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!(&bytes[..4], b"OggS", "{key} is not an Ogg");
        }
        assert!(clips.load("music/first_light/kazoo").is_none());
    }

    #[test]
    fn every_piece_names_stems_that_exist_and_can_be_arranged() {
        for piece in &PIECES {
            for key in piece.bed.iter().chain(piece.voices.iter()) {
                assert!(STEMS.iter().any(|(k, _)| k == key), "{key} is not a stem");
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
