//! Embedded interface sounds and the endless desk music, mixed on one SDL
//! device. Gameplay never depends on audio being available; failure to open a
//! device leaves Dispatch speech-only.

use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

use sdl2::audio::{AudioCallback, AudioDevice, AudioFormat, AudioSpecDesired, AudioSpecWAV};
use sdl2::rwops::RWops;

pub const SAMPLE_RATE: i32 = 44_100;
/// Every music segment is 8 bars of 4/4 at 84 BPM: 32 beats, sample-exact.
pub const SEGMENT_SAMPLES: usize = 1_008_000;
/// How many recent segments the shuffle avoids repeating.
const RECENT: usize = 4;
pub const DEFAULT_MUSIC_VOLUME: u8 = 40;

/// Rendered by genny from `audio/music.json`; see `audio/README.md`.
const SEGMENTS: [&[u8]; 12] = [
    include_bytes!("../assets/music/desk_1.ogg"),
    include_bytes!("../assets/music/desk_2.ogg"),
    include_bytes!("../assets/music/desk_3.ogg"),
    include_bytes!("../assets/music/desk_4.ogg"),
    include_bytes!("../assets/music/desk_5.ogg"),
    include_bytes!("../assets/music/desk_6.ogg"),
    include_bytes!("../assets/music/road_1.ogg"),
    include_bytes!("../assets/music/road_2.ogg"),
    include_bytes!("../assets/music/road_3.ogg"),
    include_bytes!("../assets/music/road_4.ogg"),
    include_bytes!("../assets/music/night_1.ogg"),
    include_bytes!("../assets/music/night_2.ogg"),
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
    const ALL: [Cue; 7] = [
        Cue::Cursor,
        Cue::Activate,
        Cue::Success,
        Cue::Error,
        Cue::Phone,
        Cue::DayStart,
        Cue::DayClose,
    ];

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

pub struct Player {
    device: AudioDevice<Mixer>,
    cues: HashMap<Cue, Arc<[i16]>>,
}

impl Player {
    pub fn new(sdl: &sdl2::Sdl, music_volume: u8) -> Result<Player, String> {
        let audio = sdl.audio()?;
        let desired = AudioSpecDesired {
            freq: Some(SAMPLE_RATE),
            channels: Some(1),
            samples: Some(1024),
        };
        let mut cues = HashMap::new();
        for cue in Cue::ALL {
            cues.insert(cue, decode_wav(cue.wav())?.into());
        }
        let (tx, rx) = sync_channel(1);
        let device = audio.open_playback(None, &desired, |spec| {
            if spec.freq != SAMPLE_RATE || spec.channels != 1 {
                log::warn!(
                    "audio device opened at {} Hz, {} channels",
                    spec.freq,
                    spec.channels
                );
            }
            Mixer::new(rx, music_gain(music_volume))
        })?;
        spawn_music(tx);
        device.resume();
        Ok(Player { device, cues })
    }

    /// Play a cue over the music, replacing any cue still sounding.
    pub fn play(&mut self, cue: Cue) {
        if let Some(samples) = self.cues.get(&cue) {
            self.device.lock().cue = Some((samples.clone(), 0));
        }
    }

    pub fn set_music_volume(&mut self, percent: u8) {
        self.device.lock().music_gain = music_gain(percent);
    }
}

/// Perceptual volume curve: 50 percent is about -12 dB, 0 is silent.
pub fn music_gain(percent: u8) -> f32 {
    let v = f32::from(percent.min(100)) / 100.0;
    v * v
}

pub struct Mixer {
    cue: Option<(Arc<[i16]>, usize)>,
    music: Option<(Vec<i16>, usize)>,
    next: Receiver<Vec<i16>>,
    music_gain: f32,
}

impl Mixer {
    pub fn new(next: Receiver<Vec<i16>>, music_gain: f32) -> Mixer {
        Mixer {
            cue: None,
            music: None,
            next,
            music_gain,
        }
    }

    fn mix(&mut self, out: &mut [i16]) {
        for sample in out {
            if self.music.as_ref().is_none_or(|(s, pos)| *pos >= s.len()) {
                self.music = self.next.try_recv().ok().map(|s| (s, 0));
            }
            let mut mixed = 0.0;
            if let Some((segment, pos)) = &mut self.music {
                mixed += f32::from(segment[*pos]) * self.music_gain;
                *pos += 1;
            }
            if let Some((cue, pos)) = &mut self.cue {
                mixed += f32::from(cue[*pos]);
                *pos += 1;
                if *pos >= cue.len() {
                    self.cue = None;
                }
            }
            *sample = mixed.clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        }
    }
}

impl AudioCallback for Mixer {
    type Channel = i16;

    fn callback(&mut self, out: &mut [i16]) {
        self.mix(out);
    }
}

/// Decode segments in shuffled order, one ahead of the mixer, so they play
/// back to back without a gap for as long as the device is open.
fn spawn_music(tx: SyncSender<Vec<i16>>) {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64);
    let spawned = std::thread::Builder::new()
        .name("music".into())
        .spawn(move || {
            let mut shuffle = Shuffle::new(seed);
            loop {
                let index = shuffle.next(SEGMENTS.len());
                match decode_segment(SEGMENTS[index]) {
                    Ok(samples) => {
                        if tx.send(samples).is_err() {
                            return; // the player was dropped
                        }
                    }
                    Err(error) => {
                        log::warn!("music segment {index} failed to decode: {error}");
                        return;
                    }
                }
            }
        });
    if let Err(error) = spawned {
        log::warn!("music disabled: {error}");
    }
}

/// Random order that never repeats one of the last few segments.
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
            // xorshift64
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

pub fn decode_segment(bytes: &[u8]) -> Result<Vec<i16>, String> {
    let mut reader =
        lewton::inside_ogg::OggStreamReader::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let header = &reader.ident_hdr;
    if header.audio_channels != 1 || header.audio_sample_rate != SAMPLE_RATE as u32 {
        return Err(format!(
            "music must be mono {SAMPLE_RATE} Hz, got {} Hz, {} channels",
            header.audio_sample_rate, header.audio_channels
        ));
    }
    let mut samples = Vec::with_capacity(SEGMENT_SAMPLES);
    while let Some(packet) = reader.read_dec_packet_itl().map_err(|e| e.to_string())? {
        samples.extend(packet);
    }
    if samples.len() != SEGMENT_SAMPLES {
        return Err(format!(
            "music segment is {} samples, expected {SEGMENT_SAMPLES}",
            samples.len()
        ));
    }
    Ok(samples)
}

fn decode_wav(bytes: &'static [u8]) -> Result<Vec<i16>, String> {
    let mut rw = RWops::from_bytes(bytes)?;
    let wav = AudioSpecWAV::load_wav_rw(&mut rw)?;
    if wav.freq != SAMPLE_RATE || wav.channels != 1 || wav.format != AudioFormat::S16LSB {
        return Err(format!(
            "embedded WAV must be mono 44100 Hz S16LSB, got {} Hz, {} channels, {:?}",
            wav.freq, wav.channels, wav.format
        ));
    }
    Ok(wav
        .buffer()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_cue_has_the_runtime_format() {
        for cue in Cue::ALL {
            assert!(!decode_wav(cue.wav()).unwrap().is_empty(), "{cue:?}");
        }
    }

    #[test]
    fn every_music_segment_is_exactly_32_beats() {
        for (i, bytes) in SEGMENTS.iter().enumerate() {
            let samples = decode_segment(bytes).unwrap_or_else(|e| panic!("segment {i}: {e}"));
            assert!(samples.iter().any(|&s| s != 0), "segment {i} is silent");
        }
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

    #[test]
    fn mixer_chains_segments_without_a_gap_and_lays_cues_on_top() {
        let (tx, rx) = sync_channel(2);
        tx.send(vec![1000; 3]).unwrap();
        tx.send(vec![2000; 3]).unwrap();
        let mut mixer = Mixer::new(rx, 0.5);
        mixer.cue = Some((vec![100, 32_767].into(), 0));
        let mut out = [0; 8];
        mixer.mix(&mut out);
        assert_eq!(out, [600, 32_767, 500, 1000, 1000, 1000, 0, 0]);
        assert!(mixer.cue.is_none());
    }

    #[test]
    fn music_volume_curve() {
        assert_eq!(music_gain(0), 0.0);
        assert_eq!(music_gain(100), 1.0);
        assert_eq!(music_gain(250), 1.0);
        assert!((music_gain(50) - 0.25).abs() < 1e-6);
    }
}
