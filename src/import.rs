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
    profile_dirs(env_saves_dir(), None, default_profile_dirs())
}

/// `FF_DISPATCH_FREIGHT_FATE_SAVES`, when set and not empty.
pub fn env_saves_dir() -> Option<PathBuf> {
    std::env::var_os(FF_SAVES_ENV)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
}

/// The environment override wins, then the player's saved folder, then
/// the usual places.
pub fn profile_dirs(
    env: Option<PathBuf>,
    setting: Option<&Path>,
    defaults: Vec<PathBuf>,
) -> Vec<PathBuf> {
    if let Some(dir) = env {
        return vec![dir];
    }
    match setting {
        Some(dir) => vec![resolve_profiles_dir(dir).unwrap_or_else(|| dir.to_path_buf())],
        None => defaults,
    }
}

/// The profiles folder for whatever the player pointed at: the profiles
/// folder itself, a saves folder, a Freight Fate folder, or the folder
/// that holds the FreightFate folder. Only reads directory listings.
pub fn resolve_profiles_dir(dir: &Path) -> Option<PathBuf> {
    [
        dir.to_path_buf(),
        dir.join("profiles"),
        dir.join("saves").join("profiles"),
        dir.join("FreightFate").join("saves").join("profiles"),
    ]
    .into_iter()
    .find(|d| !list_ff_saves(std::slice::from_ref(d)).is_empty())
}

/// A typed folder path without surrounding quotes or spaces.
pub fn clean_typed_path(typed: &str) -> PathBuf {
    PathBuf::from(typed.trim().trim_matches('"').trim())
}

/// The profiles folder and how many saves it holds, or a spoken reason.
pub fn check_saves_folder(dir: &Path) -> Result<(PathBuf, usize), String> {
    if !dir.is_dir() {
        return Err(format!(
            "There's no folder at {}. Check the path and type it again.",
            dir.display()
        ));
    }
    match resolve_profiles_dir(dir) {
        Some(found) => {
            let n = list_ff_saves(std::slice::from_ref(&found)).len();
            Ok((found, n))
        }
        None => Err(format!(
            "No Freight Fate saves found in {}. Type the folder that holds your .ffsave files, its saves folder, or your Freight Fate folder.",
            dir.display()
        )),
    }
}

/// "Found 2 Freight Fate saves in C:\ff-tester\FreightFate\saves\profiles."
pub fn found_line(dir: &Path, count: usize) -> String {
    format!(
        "Found {count} Freight Fate save{} in {}.",
        if count == 1 { "" } else { "s" },
        dir.display()
    )
}

/// FF's per-user folder and a portable FreightFate folder beside Dispatch's.
pub fn default_profile_dirs() -> Vec<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn saves_in(dir: &Path, names: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        for n in names {
            std::fs::write(dir.join(n), b"not read").unwrap();
        }
    }

    #[test]
    fn every_folder_shape_resolves_to_the_profiles_folder() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("FreightFate");
        let profiles = root.join("saves").join("profiles");
        saves_in(&profiles, &["Alma.ffsave", "Bo.ffsave", "notes.txt"]);
        for pointed in [
            profiles.clone(),
            root.join("saves"),
            root.clone(),
            t.path().to_path_buf(),
        ] {
            assert_eq!(
                resolve_profiles_dir(&pointed),
                Some(profiles.clone()),
                "{}",
                pointed.display()
            );
            assert_eq!(check_saves_folder(&pointed), Ok((profiles.clone(), 2)));
        }
        assert_eq!(
            found_line(&profiles, 2),
            format!("Found 2 Freight Fate saves in {}.", profiles.display())
        );
        assert!(found_line(&profiles, 1).starts_with("Found 1 Freight Fate save in "));
    }

    #[test]
    fn a_folder_without_saves_or_a_missing_folder_says_so() {
        let t = tempfile::tempdir().unwrap();
        let empty = t.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let err = check_saves_folder(&empty).unwrap_err();
        assert!(err.starts_with("No Freight Fate saves found in "), "{err}");
        let err = check_saves_folder(&t.path().join("nope")).unwrap_err();
        assert!(err.starts_with("There's no folder at "), "{err}");
        assert_eq!(
            clean_typed_path("  \"C:\\ff-tester\" "),
            PathBuf::from("C:\\ff-tester")
        );
    }

    #[test]
    fn the_environment_wins_then_the_setting_then_the_usual_places() {
        let t = tempfile::tempdir().unwrap();
        let profiles = t.path().join("FreightFate").join("saves").join("profiles");
        saves_in(&profiles, &["Alma.ffsave"]);
        let env = t.path().join("env");
        let defaults = vec![t.path().join("default")];
        assert_eq!(
            profile_dirs(Some(env.clone()), Some(t.path()), defaults.clone()),
            vec![env]
        );
        assert_eq!(
            profile_dirs(None, Some(t.path()), defaults.clone()),
            vec![profiles]
        );
        assert_eq!(profile_dirs(None, None, defaults.clone()), defaults);
    }

    #[test]
    fn resolving_a_folder_leaves_the_saves_untouched() {
        let t = tempfile::tempdir().unwrap();
        let profiles = t.path().join("saves").join("profiles");
        saves_in(&profiles, &["Alma.ffsave"]);
        let file = profiles.join("Alma.ffsave");
        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        check_saves_folder(t.path()).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"not read");
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
    }
}
