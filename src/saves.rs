//! Career saves: versioned JSON, atomic writes, tolerant loads.
//!
//! A save is the career's fields at the top level, plus `desk`: today's
//! shift when a day is in progress. A save without one, or with one that
//! no longer reads, starts that day's morning again.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::career::{Game, Shift};

pub use crate::paths::{careers_dir, data_dir, DATA_DIR_ENV};

/// The desk's own settings. The engine keeps speech, volumes and the
/// controller in its `settings.json`; these live beside it in
/// `dispatch.json`.
#[derive(Debug, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default)]
    pub online_services: bool,
    /// Where the player pointed Dispatch at their Freight Fate saves.
    #[serde(default)]
    pub ff_saves_folder: Option<PathBuf>,
    /// Tips play the first time the player reaches something. On for
    /// every career until switched off.
    #[serde(default = "tips_default")]
    pub tips: bool,
}

fn tips_default() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            online_services: false,
            ff_saves_folder: None,
            tips: tips_default(),
        }
    }
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
        tips: field(&fields, "tips", defaults.tips),
    }
}

pub fn save_settings(settings: &Settings, dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(SETTINGS_FILE);
    write_atomic(&path, &serde_json::to_vec_pretty(settings)?)
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

/// The career's fields, then `desk` when a day is in progress.
#[derive(Serialize)]
struct SaveFile<'a> {
    #[serde(flatten)]
    game: &'a Game,
    #[serde(skip_serializing_if = "Option::is_none")]
    desk: Option<&'a Shift>,
}

/// The save file's bytes for a career and, mid-day, its desk.
pub fn save_bytes(game: &Game, desk: Option<&Shift>) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec_pretty(&SaveFile { game, desk })
}

/// Where a career's save goes in `dir`.
pub fn save_path(game: &Game, dir: &Path) -> PathBuf {
    dir.join(file_name(game))
}

/// Write save bytes atomically to `path`, creating its folder.
pub fn write_save(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_atomic(path, bytes)
}

/// Write the career, and its desk mid-day, atomically.
pub fn save_game(game: &Game, desk: Option<&Shift>, dir: &Path) -> io::Result<PathBuf> {
    let path = save_path(game, dir);
    write_save(&path, &save_bytes(game, desk)?)?;
    Ok(path)
}

/// A temp file written and flushed to disk, then renamed over `path`. A
/// failure at any step leaves the old file as it was and no temp file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fn flushed(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = std::fs::File::create(tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    }
    let tmp = path.with_extension("json.tmp");
    let written = flushed(&tmp, bytes).and_then(|()| replace_file(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    sync_dir(path);
    Ok(())
}

/// The rename itself reaches the disk once the folder is flushed.
#[cfg(unix)]
fn sync_dir(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
}

/// MoveFileExW's write-through already flushes the rename.
#[cfg(not(unix))]
fn sync_dir(_: &Path) {}

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

/// A career as loaded, with the day at the desk when it was saved mid-day.
pub struct Saved {
    pub game: Game,
    pub desk: Option<Shift>,
}

/// The career, or why it can't be read. A desk that doesn't read is
/// dropped with a warning, and that day starts from its morning.
pub fn load_game(path: &Path) -> Result<Saved, String> {
    /// The file's `desk` alone; everything else is skipped.
    #[derive(Deserialize)]
    struct DeskOnly {
        #[serde(default)]
        desk: Option<Shift>,
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    // Both read straight from the bytes: a `serde_json::Value` in between
    // would sort map keys, and some of the career's maps keep their order.
    let game: Game = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let desk = match serde_json::from_slice::<DeskOnly>(&bytes) {
        Ok(file) => file.desk,
        Err(err) => {
            log::warn!(
                "the day in progress in {} could not be read, so it starts from the morning: {err}",
                path.display()
            );
            None
        }
    };
    Ok(Saved { game, desk })
}

// Boxing `Game` would change the shape every caller matches on, so the
// size gap is accepted here instead.
#[allow(clippy::large_enum_variant)]
pub enum Slot {
    Ok {
        path: PathBuf,
        game: Game,
        desk: Option<Shift>,
    },
    Corrupt {
        path: PathBuf,
        name: String,
    },
}

impl Slot {
    pub fn label(&self) -> String {
        match self {
            Slot::Ok { game, desk, .. } => {
                // A day in progress past the desk's opening says the time.
                let at = match desk {
                    Some(shift)
                        if game.ended.is_none() && shift.clock.minute != game.desk_opens() =>
                    {
                        format!(", {}", shift.clock.spoken())
                    }
                    _ => String::new(),
                };
                format!(
                    "{}, day {}{at}, {}{}.",
                    game.name,
                    game.day,
                    crate::money(game.funds),
                    if game.ended.is_some() {
                        ", finished"
                    } else {
                        ""
                    }
                )
            }
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
            Ok(Saved { game, desk }) => Slot::Ok { path, game, desk },
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
        let path = save_game(&g, None, dir.path()).unwrap();
        let back = load_game(&path).unwrap();
        assert_eq!(
            serde_json::to_string(&back.game).unwrap(),
            serde_json::to_string(&g).unwrap()
        );
        assert!(back.desk.is_none());
        let s = g.morning(get_world());
        save_game(&g, Some(&s), dir.path()).unwrap();
        save_game(&g, Some(&s), dir.path()).unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_day_in_progress_round_trips_beside_the_career() {
        let w = get_world();
        let dir = tempfile::tempdir().unwrap();
        let mut g = game();
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        let path = save_game(&g, Some(&s), dir.path()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let top: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            top["name"],
            serde_json::json!(g.name),
            "the career stays at the top"
        );
        assert!(top["desk"].is_object());
        let back = load_game(&path).unwrap();
        assert_eq!(
            serde_json::to_string(&back.game).unwrap(),
            serde_json::to_string(&g).unwrap()
        );
        let desk = back.desk.expect("the desk loads");
        assert_eq!(
            serde_json::to_string(&desk).unwrap(),
            serde_json::to_string(&s).unwrap()
        );
        assert_eq!(
            save_bytes(&back.game, Some(&desk)).unwrap(),
            text.as_bytes()
        );
    }

    #[test]
    fn a_save_from_before_the_desk_loads_at_its_morning() {
        let dir = tempfile::tempdir().unwrap();
        let g = game();
        // What older releases wrote: the career alone.
        let path = dir.path().join("old-42.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&g).unwrap()).unwrap();
        let back = load_game(&path).unwrap();
        assert!(back.desk.is_none());
        assert_eq!(back.game.day, g.day);
        let slots = list_slots(dir.path());
        assert!(matches!(&slots[0], Slot::Ok { desk: None, .. }));
        assert_eq!(slots[0].label(), format!("{}, day 1, $25,000.", g.name));
    }

    #[test]
    fn a_desk_that_does_not_read_drops_to_the_morning_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let g = game();
        let mut v = serde_json::to_value(&g).unwrap();
        v.as_object_mut().unwrap().insert(
            "desk".into(),
            serde_json::json!({"clock": "noon", "board": 3}),
        );
        let path = dir.path().join("bad-desk-42.json");
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        let back = load_game(&path).unwrap();
        assert!(back.desk.is_none());
        assert_eq!(back.game.name, g.name);
        assert!(matches!(
            &list_slots(dir.path())[0],
            Slot::Ok { desk: None, .. }
        ));
    }

    #[test]
    fn the_label_says_the_time_once_the_day_is_under_way() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let label = |g: &Game, s: Shift| {
            Slot::Ok {
                path: PathBuf::new(),
                game: g.clone(),
                desk: Some(s),
            }
            .label()
        };
        let at_opening = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(
            label(&g, at_opening),
            format!("{}, day 1, $25,000.", g.name)
        );
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        let clock = s.clock.spoken();
        assert_ne!(clock, "6 AM");
        assert_eq!(
            label(&g, s),
            format!("{}, day 1, {clock}, {}.", g.name, crate::money(g.funds))
        );
    }

    #[test]
    fn a_failed_save_leaves_the_last_one_whole() {
        let dir = tempfile::tempdir().unwrap();
        let g = game();
        let path = save_game(&g, None, dir.path()).unwrap();
        let before = std::fs::read(&path).unwrap();
        // The temp file's name is taken by a folder, so the write fails.
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        let mut later = g.clone();
        later.funds += 1.0;
        assert!(save_game(&later, None, dir.path()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(load_game(&path).unwrap().game.funds, g.funds);
    }

    #[test]
    fn corrupt_saves_are_listed_by_name_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        save_game(&game(), None, dir.path()).unwrap();
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
        let path = save_game(&g1, None, dir.path()).unwrap();
        let mut s1 = g1.morning(w);
        let mut g2 = load_game(&path).unwrap().game;
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
