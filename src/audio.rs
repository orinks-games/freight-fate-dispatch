//! Small embedded interface sounds. Gameplay never depends on audio being
//! available; failure to open a device leaves Dispatch speech-only.

use std::collections::HashMap;

use sdl2::audio::{AudioFormat, AudioQueue, AudioSpecDesired, AudioSpecWAV};
use sdl2::rwops::RWops;

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
    queue: AudioQueue<i16>,
    samples: HashMap<Cue, Vec<i16>>,
}

impl Player {
    pub fn new(sdl: &sdl2::Sdl) -> Result<Player, String> {
        let audio = sdl.audio()?;
        let desired = AudioSpecDesired {
            freq: Some(44_100),
            channels: Some(1),
            samples: Some(1024),
        };
        let queue = AudioQueue::<i16>::open_queue(&audio, None, &desired)?;
        let mut samples = HashMap::new();
        for cue in Cue::ALL {
            samples.insert(cue, decode(cue.wav())?);
        }
        queue.resume();
        Ok(Player { queue, samples })
    }

    pub fn play(&self, cue: Cue) {
        let Some(samples) = self.samples.get(&cue) else {
            return;
        };
        self.queue.clear();
        if let Err(error) = self.queue.queue_audio(samples) {
            log::warn!("audio cue {cue:?} could not be queued: {error}");
        }
    }
}

fn decode(bytes: &'static [u8]) -> Result<Vec<i16>, String> {
    let mut rw = RWops::from_bytes(bytes)?;
    let wav = AudioSpecWAV::load_wav_rw(&mut rw)?;
    if wav.freq != 44_100 || wav.channels != 1 || wav.format != AudioFormat::S16LSB {
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
            assert!(!decode(cue.wav()).unwrap().is_empty(), "{cue:?}");
        }
    }
}
