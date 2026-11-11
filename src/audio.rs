//! The interface sounds and the endless desk music, compiled into the game
//! and played through Hearken's sound pool. Gameplay never depends on audio
//! being available: without a sound device the engine keeps the game
//! running speech-only.

use std::collections::VecDeque;

use hearken::audio::MemoryClips;
use hearken::{Bus, Ctx, Sound};

/// Every music segment is 8 bars of 4/4 at 84 BPM: 32 beats.
pub const SEGMENT_SECONDS: f64 = 1_008_000.0 / 44_100.0;
/// How many recent segments the shuffle avoids repeating.
const RECENT: usize = 4;
/// The music bus before the player changes it: well under speech.
pub const DEFAULT_MUSIC_VOLUME: u8 = 40;

/// Rendered by genny from `audio/music.json`; see `audio/README.md`.
pub const SEGMENTS: [(&str, &[u8]); 12] = [
    ("music/desk_1", include_bytes!("../assets/music/desk_1.ogg")),
    ("music/desk_2", include_bytes!("../assets/music/desk_2.ogg")),
    ("music/desk_3", include_bytes!("../assets/music/desk_3.ogg")),
    ("music/desk_4", include_bytes!("../assets/music/desk_4.ogg")),
    ("music/desk_5", include_bytes!("../assets/music/desk_5.ogg")),
    ("music/desk_6", include_bytes!("../assets/music/desk_6.ogg")),
    ("music/road_1", include_bytes!("../assets/music/road_1.ogg")),
    ("music/road_2", include_bytes!("../assets/music/road_2.ogg")),
    ("music/road_3", include_bytes!("../assets/music/road_3.ogg")),
    ("music/road_4", include_bytes!("../assets/music/road_4.ogg")),
    (
        "music/night_1",
        include_bytes!("../assets/music/night_1.ogg"),
    ),
    (
        "music/night_2",
        include_bytes!("../assets/music/night_2.ogg"),
    ),
];

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Cue {
    Cursor,
    Activate,
    Success,
    Error,
    Phone,
    DayStart,
    DayClose,
}

impl Cue {
    pub const ALL: [Cue; 7] = [
        Cue::Cursor,
        Cue::Activate,
        Cue::Success,
        Cue::Error,
        Cue::Phone,
        Cue::DayStart,
        Cue::DayClose,
    ];

    /// The sound key: a file of the same name in a `sounds` folder beside
    /// the game replaces the built-in recording.
    pub const fn key(self) -> &'static str {
        match self {
            Cue::Cursor => "ui/cursor",
            Cue::Activate => "ui/activate",
            Cue::Success => "ui/success",
            Cue::Error => "ui/error",
            Cue::Phone => "desk/phone",
            Cue::DayStart => "desk/day_start",
            Cue::DayClose => "desk/day_close",
        }
    }

    /// Phone and day cues are game sound; the rest are interface clicks.
    pub const fn bus(self) -> Bus {
        match self {
            Cue::Cursor | Cue::Activate | Cue::Success | Cue::Error => Bus::UI,
            Cue::Phone | Cue::DayStart | Cue::DayClose => Bus::EFFECTS,
        }
    }

    fn wav(self) -> &'static [u8] {
        match self {
            Cue::Cursor => include_bytes!("../assets/audio/cursor.wav"),
            Cue::Activate => include_bytes!("../assets/audio/activate.wav"),
            Cue::Success => include_bytes!("../assets/audio/success.wav"),
            Cue::Error => include_bytes!("../assets/audio/error.wav"),
            Cue::Phone => include_bytes!("../assets/audio/phone.wav"),
            Cue::DayStart => include_bytes!("../assets/audio/day_start.wav"),
            Cue::DayClose => include_bytes!("../assets/audio/day_close.wav"),
        }
    }
}

/// Every sound the game carries inside itself, by key.
pub fn clips() -> MemoryClips {
    let mut clips = MemoryClips::default();
    for cue in Cue::ALL {
        clips = clips.with(cue.key(), cue.wav());
    }
    for (key, bytes) in SEGMENTS {
        clips = clips.with(key, bytes);
    }
    clips
}

/// The desk music: one shuffled segment after another, for as long as the
/// desk screen is up.
pub struct Music {
    shuffle: Shuffle,
    playing: Sound,
    started_at: f64,
}

impl Music {
    pub fn new() -> Music {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
        Music {
            shuffle: Shuffle::new(seed),
            playing: Sound::NONE,
            started_at: f64::NEG_INFINITY,
        }
    }

    /// Start the next segment once the last one has played out. A segment
    /// the pool dropped (no device yet) is waited out, so a device that
    /// opens late is never flooded with starts.
    pub fn update<G: hearken::Game>(&mut self, ctx: &mut Ctx<G>) {
        let elapsed = ctx.clock - self.started_at;
        if ctx.audio.is_playing(self.playing) || elapsed < SEGMENT_SECONDS {
            return;
        }
        let (key, _) = SEGMENTS[self.shuffle.next(SEGMENTS.len())];
        self.playing = ctx.audio.play(key, Bus::MUSIC);
        self.started_at = ctx.clock;
    }

    pub fn stop<G: hearken::Game>(&mut self, ctx: &mut Ctx<G>) {
        ctx.audio.stop(self.playing, 500);
        self.playing = Sound::NONE;
    }
}

impl Default for Music {
    fn default() -> Self {
        Music::new()
    }
}

pub struct Shuffle {
    state: u64,
    recent: VecDeque<usize>,
}

impl Shuffle {
    pub fn new(seed: u64) -> Shuffle {
        Shuffle {
            state: seed | 1,
            recent: VecDeque::new(),
        }
    }

    pub fn next(&mut self, count: usize) -> usize {
        let window = RECENT.min(count.saturating_sub(1));
        loop {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            let pick = (self.state % count as u64) as usize;
            if !self.recent.contains(&pick) {
                self.recent.push_back(pick);
                if self.recent.len() > window {
                    self.recent.pop_front();
                }
                return pick;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hearken::audio::ClipSource;

    #[test]
    fn every_cue_and_segment_is_a_clip() {
        let mut clips = clips();
        for cue in Cue::ALL {
            let bytes = clips.load(cue.key()).unwrap_or_else(|| panic!("{cue:?}"));
            assert_eq!(&bytes[..4], b"RIFF", "{cue:?} is not a WAV");
        }
        for (key, _) in SEGMENTS {
            let bytes = clips.load(key).unwrap_or_else(|| panic!("{key}"));
            assert_eq!(&bytes[..4], b"OggS", "{key} is not an Ogg");
        }
        assert!(clips.load("music/desk_99").is_none());
    }

    #[test]
    fn shuffle_avoids_recent_segments() {
        let mut shuffle = Shuffle::new(7);
        let picks: Vec<usize> = (0..500).map(|_| shuffle.next(12)).collect();
        for window in picks.windows(RECENT + 1) {
            let mut sorted = window.to_vec();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), window.len(), "{window:?}");
        }
        for segment in 0..12 {
            assert!(picks.contains(&segment), "segment {segment} never played");
        }
    }
}
