//! Career saves: versioned JSON, atomic writes, tolerant loads.
//!
//! Careers go through Hearken's vault, so a release built with
//! `seal = true` keeps them sealed against editing, while developer builds
//! and tests read and write plain JSON. The desk's own settings
//! (`dispatch.json`) stay plain: they are the player's.

use std::io;
use std::path::{Path, PathBuf};

use hearken::{Opened, Vault};
use serde::{Deserialize, Serialize};

use crate::career::Game;

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
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    replace_file(&tmp, &path)
}

/// The longest name the vault takes, in bytes (`hearken-seal`'s rule).
const MAX_NAME: usize = 64;

/// `<slug>-<seed>.json`, with the slug cut short when the whole name would
/// not fit in [`MAX_NAME`] bytes. Names that already fit are unchanged.
fn save_name(slug: &str, seed: &str) -> String {
    let tail = format!("-{seed}.json");
    let slug = slug.trim_matches('-');
    let room = MAX_NAME.saturating_sub(tail.len());
    // The slug is ASCII, so any byte is a character boundary.
    let slug = slug[..slug.len().min(room)].trim_end_matches('-');
    format!("{slug}{tail}")
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
    save_name(&slug, &game.seed.to_string())
}

/// The careers folder, read and written through Hearken's vault: sealed in
/// a release built with `seal = true`, plain JSON in every other build.
pub struct Careers {
    vault: Vault,
}

impl Careers {
    /// The careers in `dir`, with this build's key and the game's
    /// witnesses (see [`crate::paths::careers_witnesses`]).
    pub fn open(dir: &Path) -> Careers {
        let (witnesses, checked) = crate::paths::careers_witnesses();
        Careers::with_vault(
            Vault::new(dir)
                .with_witnesses(witnesses)
                .checking_witnesses(checked),
        )
    }

    /// The careers in a given vault (tests, tools).
    pub fn with_vault(vault: Vault) -> Careers {
        Careers { vault }
    }

    pub fn dir(&self) -> &Path {
        self.vault.dir()
    }

    /// Save the career. A save that was changed outside the game, belongs
    /// to a release this build cannot open, or cannot be read is never
    /// written over.
    pub fn save(&mut self, game: &Game) -> io::Result<PathBuf> {
        self.shorten_long_names();
        let name = file_name(game);
        match self.vault.read(&name) {
            Opened::Tampered(how) => {
                log::warn!("not saving over {name}: {how}");
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "its save file was changed outside the game, so it is left as it is",
                ));
            }
            Opened::Locked => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "its save file is from a released copy of the game and can't be changed here",
                ));
            }
            Opened::Failed(e) => return Err(e),
            Opened::Missing | Opened::Sealed(_) | Opened::Plain(_) | Opened::Legacy(_) => {}
        }
        self.vault.write(&name, &serde_json::to_vec_pretty(game)?)?;
        Ok(self.vault.dir().join(name))
    }

    /// The career saved as `name`.
    pub fn load(&mut self, name: &str) -> Result<Game, String> {
        match self.slot(name) {
            Slot::Ok { game, .. } => Ok(game),
            other => Err(other.label()),
        }
    }

    /// Every career save, sorted by file name. Saves that cannot be opened
    /// are listed, never touched.
    pub fn list(&mut self) -> Vec<Slot> {
        self.shorten_long_names();
        let mut names: Vec<String> = std::fs::read_dir(self.vault.dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json") && !n.starts_with('.'))
            .collect();
        names.sort();
        names.iter().map(|n| self.slot(n)).collect()
    }

    fn slot(&mut self, file: &str) -> Slot {
        let path = self.vault.dir().join(file);
        let name = file.strip_suffix(".json").unwrap_or(file).to_string();
        let bytes = match self.vault.read(file) {
            Opened::Sealed(bytes) | Opened::Plain(bytes) | Opened::Legacy(bytes) => bytes,
            Opened::Tampered(how) => {
                log::warn!(
                    "career save {} was changed outside the game: {how}",
                    path.display()
                );
                return Slot::Changed { path, name };
            }
            Opened::Locked => {
                log::warn!("career save {} is sealed by a release", path.display());
                return Slot::Locked { path, name };
            }
            Opened::Missing => {
                log::warn!("career save {} went missing", path.display());
                return Slot::Corrupt { path, name };
            }
            Opened::Failed(e) => {
                log::warn!("unreadable career save {}: {e}", path.display());
                return Slot::Corrupt { path, name };
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(game) => Slot::Ok { path, game },
            Err(err) => {
                log::warn!("unreadable career save {}: {err}", path.display());
                Slot::Corrupt { path, name }
            }
        }
    }

    /// Older versions named a save after the whole company name, which can
    /// run past the longest name the vault takes. Each such plain save is
    /// renamed to the shorter name a save of it gets now, unless that is
    /// taken, before anything is read or written, so it loads and is
    /// sealed like any other.
    fn shorten_long_names(&self) {
        let dir = self.vault.dir();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(file) = entry.file_name().into_string() else {
                continue;
            };
            if file.len() <= MAX_NAME || !file.is_ascii() {
                continue;
            }
            let Some((slug, seed)) = file
                .strip_suffix(".json")
                .and_then(|stem| stem.rsplit_once('-'))
            else {
                continue;
            };
            if seed.parse::<i64>().is_err() {
                continue;
            }
            let short = dir.join(save_name(slug, seed));
            if std::fs::symlink_metadata(&short).is_ok() {
                continue;
            }
            match std::fs::rename(entry.path(), &short) {
                Ok(()) => log::info!("renamed career save {file} to {}", short.display()),
                Err(e) => log::warn!("could not rename career save {file}: {e}"),
            }
        }
    }
}

/// Save the career in the careers folder `dir` (see [`Careers::save`]).
pub fn save_game(game: &Game, dir: &Path) -> io::Result<PathBuf> {
    Careers::open(dir).save(game)
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

/// The career saved at `path`, through the vault of its folder.
pub fn load_game(path: &Path) -> Result<Game, String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    Careers::open(dir).load(&name)
}

// The brief's interface is `Ok { path, game: Game }`; boxing `Game` would
// change that shape, so the size gap is accepted here instead.
#[allow(clippy::large_enum_variant)]
pub enum Slot {
    Ok {
        path: PathBuf,
        game: Game,
    },
    Corrupt {
        path: PathBuf,
        name: String,
    },
    /// Changed outside the game since a release saved it: listed, never
    /// opened or written over.
    Changed {
        path: PathBuf,
        name: String,
    },
    /// Saved by a release that this build (a developer's) cannot open.
    Locked {
        path: PathBuf,
        name: String,
    },
}

impl Slot {
    pub fn label(&self) -> String {
        match self {
            Slot::Ok { game, .. } => format!(
                "{}, day {}, {}{}.",
                game.name,
                game.day,
                crate::money(game.funds),
                if game.ended.is_some() {
                    ", finished"
                } else {
                    ""
                }
            ),
            Slot::Corrupt { name, .. } => {
                format!("{name}: this save could not be read. It has been left as it is.")
            }
            Slot::Changed { name, .. } => format!(
                "{name}: this save was changed outside the game and can't be opened. It has been left as it is."
            ),
            Slot::Locked { name, .. } => format!(
                "{name}: this save is from a released copy of the game and can't be opened here. It has been left as it is."
            ),
        }
    }
}

/// Every career save in `dir` (see [`Careers::list`]).
pub fn list_slots(dir: &Path) -> Vec<Slot> {
    Careers::open(dir).list()
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

    fn key() -> hearken::seal::Key {
        hearken::seal::Key::from_bytes([42; 32])
    }

    /// A release's careers in `dir`, with its witness at `witness`.
    fn sealed_store(dir: &Path, witness: &Path) -> Careers {
        Careers::with_vault(
            Vault::with_key(dir, Some(key())).with_witnesses(vec![witness.to_path_buf()]),
        )
    }

    fn json(g: &Game) -> String {
        serde_json::to_string(g).unwrap()
    }

    fn only_game(slots: Vec<Slot>) -> Game {
        match <[Slot; 1]>::try_from(slots) {
            Ok([Slot::Ok { game, .. }]) => game,
            Ok([other]) => panic!("expected a career, got {:?}", other.label()),
            Err(slots) => panic!("expected one slot, got {}", slots.len()),
        }
    }

    #[test]
    fn a_release_s_save_round_trips_and_is_not_json_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let witness = elsewhere.path().join(crate::paths::CAREERS_WITNESS);
        let g = game();
        let path = sealed_store(dir.path(), &witness).save(&g).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&raw).is_err());
        assert!(!raw.windows(g.name.len()).any(|w| w == g.name.as_bytes()));
        let back = only_game(sealed_store(dir.path(), &witness).list());
        assert_eq!(json(&back), json(&g));
        let name = path.file_name().unwrap().to_str().unwrap();
        let loaded = sealed_store(dir.path(), &witness).load(name).unwrap();
        assert_eq!(json(&loaded), json(&g));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_release_s_save_copied_to_another_computer_still_loads() {
        let home = tempfile::tempdir().unwrap();
        let home_witness = home.path().join(crate::paths::CAREERS_WITNESS);
        let path = sealed_store(&home.path().join("careers"), &home_witness)
            .save(&game())
            .unwrap();
        assert!(home_witness.is_file());
        let away = tempfile::tempdir().unwrap();
        let careers = away.path().join("careers");
        std::fs::create_dir_all(&careers).unwrap();
        std::fs::copy(&path, careers.join(path.file_name().unwrap())).unwrap();
        let away_witness = away.path().join(crate::paths::CAREERS_WITNESS);
        let back = only_game(sealed_store(&careers, &away_witness).list());
        assert_eq!(json(&back), json(&game()));
    }

    #[test]
    fn an_older_plain_save_loads_and_is_sealed_on_its_next_save() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let witness = elsewhere.path().join(crate::paths::CAREERS_WITNESS);
        // An earlier version (or a developer build) wrote plain JSON.
        let path = Careers::with_vault(Vault::with_key(dir.path(), None))
            .save(&game())
            .unwrap();
        let plain = std::fs::read(&path).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&plain).is_ok());

        let mut store = sealed_store(dir.path(), &witness);
        let mut g = only_game(store.list());
        g.day += 1;
        store.save(&g).unwrap();
        let sealed = std::fs::read(&path).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&sealed).is_err());
        let backup = dir.path().join(format!(
            "{}.plain-backup",
            path.file_name().unwrap().to_str().unwrap()
        ));
        assert_eq!(std::fs::read(&backup).unwrap(), plain);
        assert!(witness.is_file());
        // The backup is not listed as a career.
        let back = only_game(sealed_store(dir.path(), &witness).list());
        assert_eq!(back.day, g.day);
    }

    #[test]
    fn an_edited_release_save_is_listed_as_changed_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let witness = elsewhere.path().join(crate::paths::CAREERS_WITNESS);
        let g = game();
        let path = sealed_store(dir.path(), &witness).save(&g).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        // A hand-written plain career, after the release has saved.
        let forged = dir.path().join("rich-1.json");
        std::fs::write(&forged, serde_json::to_vec(&g).unwrap()).unwrap();

        let mut store = sealed_store(dir.path(), &witness);
        let slots = store.list();
        assert_eq!(slots.len(), 2);
        for slot in &slots {
            assert!(matches!(slot, Slot::Changed { .. }));
            assert!(slot
                .label()
                .ends_with(": this save was changed outside the game and can't be opened. It has been left as it is."));
        }
        let err = store.save(&g).unwrap_err();
        assert_eq!(
            err.to_string(),
            "its save file was changed outside the game, so it is left as it is"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::read(&forged).unwrap(),
            serde_json::to_vec(&g).unwrap()
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_developer_build_lists_a_release_s_save_but_never_changes_it() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let witness = elsewhere.path().join(crate::paths::CAREERS_WITNESS);
        let path = sealed_store(dir.path(), &witness).save(&game()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut dev = Careers::with_vault(Vault::with_key(dir.path(), None));
        let slots = dev.list();
        assert!(matches!(slots.as_slice(), [Slot::Locked { .. }]));
        assert!(slots[0]
            .label()
            .contains("is from a released copy of the game"));
        assert!(dev.save(&game()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn a_long_company_name_still_saves() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let witness = elsewhere.path().join(crate::paths::CAREERS_WITNESS);
        let mut g = game();
        g.name = format!(
            "Northstar Freight Lines with {}",
            "Very Long Driver Name ".repeat(5)
        );
        g.seed = i64::MAX;
        let path = sealed_store(dir.path(), &witness).save(&g).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.len() <= MAX_NAME, "{name}");
        assert!(name.starts_with("northstar-freight-lines-with-"));
        assert!(name.ends_with(&format!("-{}.json", i64::MAX)));
        assert_eq!(
            json(&only_game(sealed_store(dir.path(), &witness).list())),
            json(&g)
        );
        // Names that already fit keep them.
        assert_eq!(file_name(&game()), {
            let g = game();
            let slug: String = g
                .name
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            format!("{}-{}.json", slug.trim_matches('-'), g.seed)
        });
    }

    #[test]
    fn an_older_save_under_a_long_name_is_renamed_and_carries_over() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let witness = elsewhere.path().join(crate::paths::CAREERS_WITNESS);
        let mut g = game();
        g.name = format!("Northstar Freight Lines with {}", "Driver ".repeat(8));
        let slug = g.name.to_ascii_lowercase().replace(' ', "-");
        let old = dir
            .path()
            .join(format!("{}-{}.json", slug.trim_matches('-'), g.seed));
        assert!(old.file_name().unwrap().len() > MAX_NAME);
        std::fs::write(&old, serde_json::to_vec_pretty(&g).unwrap()).unwrap();

        let mut store = sealed_store(dir.path(), &witness);
        let back = only_game(store.list());
        assert_eq!(json(&back), json(&g));
        assert!(!old.exists());
        let path = store.save(&back).unwrap();
        assert!(path.file_name().unwrap().len() <= MAX_NAME);
        assert_eq!(
            json(&only_game(sealed_store(dir.path(), &witness).list())),
            json(&g)
        );
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
