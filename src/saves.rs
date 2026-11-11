//! Career saves: versioned JSON, atomic writes, tolerant loads.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::career::Game;

pub use crate::paths::{careers_dir, data_dir, DATA_DIR_ENV};

/// The desk's own settings. The engine keeps speech, volumes and the
/// controller in its `settings.json`; these live beside it in
/// `dispatch.json`.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub online_services: bool,
    /// Where the player pointed Dispatch at their Freight Fate saves.
    #[serde(default)]
    pub ff_saves_folder: Option<PathBuf>,
}

pub const SETTINGS_FILE: &str = "dispatch.json";
/// Where releases before Hearken kept these, as top-level fields.
const LEGACY_SETTINGS_FILE: &str = "settings.json";

/// Each setting falls back to its default on its own, so one bad value
/// doesn't reset the rest. With no `dispatch.json` yet, the fields an
/// older release left in `settings.json` are read once.
pub fn load_settings(dir: &Path) -> Settings {
    let defaults = Settings::default();
    let file = if dir.join(SETTINGS_FILE).is_file() {
        SETTINGS_FILE
    } else {
        LEGACY_SETTINGS_FILE
    };
    let Some(serde_json::Value::Object(fields)) = std::fs::read(dir.join(file))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    else {
        return defaults;
    };
    fn field<T: serde::de::DeserializeOwned>(
        fields: &serde_json::Map<String, serde_json::Value>,
        name: &str,
        default: T,
    ) -> T {
        fields
            .get(name)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(default)
    }
    Settings {
        online_services: field(&fields, "online_services", defaults.online_services),
        ff_saves_folder: field(&fields, "ff_saves_folder", defaults.ff_saves_folder),
    }
}

pub fn save_settings(settings: &Settings, dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(SETTINGS_FILE);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    replace_file(&tmp, &path)
}

fn file_name(game: &Game) -> String {
    let slug: String = game
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("{}-{}.json", slug.trim_matches('-'), game.seed)
}

/// Write the career atomically: a temp file, then a rename over the old save.
pub fn save_game(game: &Game, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(game));
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(game)?)?;
    replace_file(&tmp, &path)?;
    Ok(path)
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // Both pointers remain valid, NUL-terminated UTF-16 strings for this call.
    let moved = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn load_game(path: &Path) -> Result<Game, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

// The brief's interface is `Ok { path, game: Game }`; boxing `Game` would
// change that shape, so the size gap is accepted here instead.
#[allow(clippy::large_enum_variant)]
pub enum Slot {
    Ok { path: PathBuf, game: Game },
    Corrupt { path: PathBuf, name: String },
}

impl Slot {
    pub fn label(&self) -> String {
        match self {
            Slot::Ok { game, .. } => format!(
                "{}, day {}, {}.",
                game.name,
                game.day,
                crate::money(game.funds)
            ),
            Slot::Corrupt { name, .. } => {
                format!("{name}: this save could not be read. It has been left as it is.")
            }
        }
    }
}

/// Every career save in `dir`, sorted by file name. Corrupt ones are listed, never touched.
pub fn list_slots(dir: &Path) -> Vec<Slot> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| match load_game(&path) {
            Ok(game) => Slot::Ok { path, game },
            Err(err) => {
                log::warn!("unreadable career save {}: {err}", path.display());
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                Slot::Corrupt { path, name }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::tests::{game, legal_pair};
    use ff_core::data::world::get_world;

    #[test]
    fn save_and_load_round_trip_without_leaving_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let g = game();
        let path = save_game(&g, dir.path()).unwrap();
        let back = load_game(&path).unwrap();
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            serde_json::to_string(&g).unwrap()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn corrupt_saves_are_listed_by_name_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        save_game(&game(), dir.path()).unwrap();
        let bad = dir.path().join("broken-1.json");
        std::fs::write(&bad, b"{not json").unwrap();
        let slots = list_slots(dir.path());
        assert_eq!(slots.len(), 2);
        assert!(slots
            .iter()
            .any(|s| matches!(s, Slot::Corrupt { name, .. } if name == "broken-1")));
        assert_eq!(std::fs::read(&bad).unwrap(), b"{not json");
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let mut v = serde_json::to_value(game()).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.insert("from_the_future".into(), serde_json::json!({"x": 1}));
        for field in [
            "record",
            "trips",
            "market",
            "shipper_goodwill",
            "weather",
            "weather_live",
            "news",
            "next_id",
        ] {
            obj.remove(field);
        }
        let g: Game = serde_json::from_value(v).unwrap();
        assert_eq!(g.record.delivered, 0);
    }

    #[test]
    fn a_reloaded_morning_replays_board_calls_applicants_and_incidents() {
        let w = get_world();
        let dir = tempfile::tempdir().unwrap();
        let mut g1 = game();
        g1.set_weather(None);
        let path = save_game(&g1, dir.path()).unwrap();
        let mut s1 = g1.morning(w);
        let mut g2 = load_game(&path).unwrap();
        let mut s2 = g2.morning(w);
        let jobs = |s: &crate::career::Shift| {
            s.board
                .iter()
                .map(|l| format!("{:?}", l.job))
                .collect::<Vec<_>>()
        };
        assert_eq!(jobs(&s1), jobs(&s2));
        assert_eq!(format!("{:?}", s1.schedule), format!("{:?}", s2.schedule));
        let names = |s: &crate::career::Shift| {
            s.applicants
                .iter()
                .map(|a| a.driver.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&s1), names(&s2));
        let (load, driver) = legal_pair(&g1, &s1);
        g1.assign(&mut s1, load, driver, w).unwrap();
        g2.assign(&mut s2, load, driver, w).unwrap();
        assert_eq!(format!("{:?}", s1.schedule), format!("{:?}", s2.schedule));
    }

    #[test]
    fn online_services_are_off_until_the_player_enables_them() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!load_settings(dir.path()).online_services);

        save_settings(
            &Settings {
                online_services: true,
                ..Settings::default()
            },
            dir.path(),
        )
        .unwrap();

        assert!(load_settings(dir.path()).online_services);
    }

    #[test]
    fn an_older_release_s_settings_file_is_read_until_the_new_one_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"online_services":true,"music_volume":40}"#,
        )
        .unwrap();
        let settings = load_settings(dir.path());
        assert!(settings.online_services);
        assert_eq!(settings.ff_saves_folder, None);
        save_settings(&Settings::default(), dir.path()).unwrap();
        assert!(dir.path().join(SETTINGS_FILE).is_file());
        assert!(!load_settings(dir.path()).online_services);
    }

    #[test]
    fn one_bad_setting_keeps_the_others() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(SETTINGS_FILE),
            r#"{"online_services":true,"ff_saves_folder":7}"#,
        )
        .unwrap();
        let settings = load_settings(dir.path());
        assert!(settings.online_services);
        assert_eq!(settings.ff_saves_folder, None);
    }
}
