//! Read-only import of a Freight Fate driver. The save is read into memory
//! and never opened for writing. Profile::load is not used: it re-signs
//! and rewrites saves.

use std::path::{Path, PathBuf};

use ff_core::data::world::World;
use ff_core::models::profile::{decode_save_bytes, Profile, FIRST_1_9_SAVE_VERSION, SAVE_VERSION};
use ff_core::models::start_options::START_OPTIONS;

use crate::career::Game;
use crate::roster::Driver;

pub const FF_SAVES_ENV: &str = "FF_DISPATCH_FREIGHT_FATE_SAVES";

/// Where FF keeps profiles: the override, FF's per-user folder, and a
/// portable FreightFate folder unzipped beside Dispatch's.
pub fn ff_profile_dirs() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os(FF_SAVES_ENV).filter(|d| !d.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = vec![ff_core::settings::paths::legacy_data_dir().join("profiles")];
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent()?.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("FreightFate").join("saves").join("profiles"));
    }
    dirs
}

pub fn list_ff_saves(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ffsave"))
        .collect();
    out.sort();
    out
}

#[derive(Debug, PartialEq)]
pub enum ImportError {
    Unreadable(String),
    Newer { found: i64, supported: i64 },
    TooOld,
}

impl ImportError {
    pub fn spoken(&self, file: &str) -> String {
        match self {
            ImportError::Unreadable(_) => {
                format!("{file} could not be read. It has been left as it is.")
            }
            ImportError::Newer { found, supported } => format!(
                "{file} is from a newer Freight Fate than this Dispatch understands (save format {found}; Dispatch reads up to {supported}). Update Dispatch to import it."
            ),
            ImportError::TooOld => format!(
                "{file} is from before Freight Fate 1.9. Open it in Freight Fate once to update it, then import it."
            ),
        }
    }
}

pub fn read_profile(path: &Path) -> Result<Profile, ImportError> {
    let raw = std::fs::read(path).map_err(|e| ImportError::Unreadable(e.to_string()))?;
    let (map, _packed) =
        decode_save_bytes(&raw).map_err(|e| ImportError::Unreadable(format!("{e:?}")))?;
    let version = map
        .get("version")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    if version > SAVE_VERSION {
        return Err(ImportError::Newer {
            found: version,
            supported: SAVE_VERSION,
        });
    }
    if version < FIRST_1_9_SAVE_VERSION {
        return Err(ImportError::TooOld);
    }
    // The integrity flag is ignored on purpose: this is a local continuation.
    Ok(Profile::from_dict(&map))
}

/// A career at the driver's carrier, with the driver on the roster as they are in FF.
pub fn import_game(profile: &Profile, seed: i64, world: &World) -> Game {
    let carrier = START_OPTIONS
        .iter()
        .find(|o| o.key == profile.carrier_key)
        .map(|o| o.key)
        .unwrap_or("northstar");
    let mut game = Game::new(carrier, seed, world);
    let mut driver = Driver::new(game.drivers[0].id, &profile.name, profile.career.clone());
    driver.hos = profile.hos.clone();
    let keep = driver.id;
    game.drivers[0] = driver;
    game.drivers
        .retain(|d| d.id == keep || d.name != profile.name);
    game.reputation = profile.standing();
    game.name = format!("{} with {}", game.name, profile.name);
    game
}
