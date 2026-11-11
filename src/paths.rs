//! Where Dispatch keeps its own files, laid out the way Freight Fate does.
//!
//! On Windows and Linux Dispatch is portable: careers, settings and logs
//! live in a `saves` folder inside the game's own folder, next to the
//! executable in a packaged build and at the repository root when running
//! from source. Copying the folder copies the careers with it.
//!
//! macOS apps must not write beside themselves in `/Applications`, so on
//! macOS saves go in `~/Library/Application Support/FreightFateDispatch`.
//! The same per-user folder catches Windows and Linux installs in a
//! read-only place such as `Program Files`, instead of failing on the first
//! save.
//!
//! Before the portable layout every platform used that per-user folder. The
//! first run of a portable copy that has no `saves` folder yet copies it in
//! once, and leaves the original where it was.
//!
//! `FF_DISPATCH_DATA_DIR` overrides all of it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DATA_DIR_ENV: &str = "FF_DISPATCH_DATA_DIR";

const APP_FOLDER: &str = "FreightFateDispatch";

static LEGACY_CHECKED: AtomicBool = AtomicBool::new(false);

/// The roots the save location is derived from. [`SaveRoots::current`]
/// reads the real process; tests build one by hand.
#[derive(Debug, Clone)]
pub struct SaveRoots {
    /// `FF_DISPATCH_DATA_DIR`, when set and not empty.
    pub override_dir: Option<PathBuf>,
    /// [`game_root`].
    pub game_root: PathBuf,
    /// [`user_data_dir`]: the per-user folder.
    pub user_dir: PathBuf,
    pub macos: bool,
    /// Force the writable-probe answer (tests); `None` probes the disk.
    pub writable: Option<bool>,
}

impl SaveRoots {
    pub fn current() -> SaveRoots {
        SaveRoots {
            override_dir: std::env::var_os(DATA_DIR_ENV)
                .filter(|d| !d.is_empty())
                .map(PathBuf::from),
            game_root: game_root(),
            user_dir: user_data_dir(),
            macos: cfg!(target_os = "macos"),
            writable: None,
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The per-user folder: the macOS save location, the Windows and Linux
/// fallback, and where every platform saved before the portable layout.
pub fn user_data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        home_dir().map(|h| h.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local/share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join(APP_FOLDER)
}

/// The running executable's folder, absolute but not canonical (on Windows
/// `canonicalize` yields a `\\?\` path that would leak into every save path).
fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::path::absolute(&exe).unwrap_or(exe);
    exe.parent().map(Path::to_path_buf)
}

/// The game's own folder: the executable's folder in a packaged build
/// (`freight_fate/data` ships beside it), the repository root from source.
pub fn game_root() -> PathBuf {
    let exe = exe_dir();
    if let Some(dir) = &exe {
        if dir.join("freight_fate").join("data").is_dir() {
            return dir.clone();
        }
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR"));
    if source.join("Cargo.toml").is_file() {
        return source.to_path_buf();
    }
    exe.unwrap_or_else(|| PathBuf::from("."))
}

/// Whether `path` exists (or can be created) and accepts a write.
pub fn is_writable_dir(path: &Path) -> bool {
    if std::fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(".dispatch-write-test");
    std::fs::write(&probe, b"").is_ok() && std::fs::remove_file(&probe).is_ok()
}

/// The active save folder, before the override.
pub fn save_root_in(roots: &SaveRoots) -> PathBuf {
    if roots.macos {
        return roots.user_dir.clone();
    }
    let writable = roots
        .writable
        .unwrap_or_else(|| is_writable_dir(&roots.game_root));
    if writable {
        roots.game_root.join("saves")
    } else {
        roots.user_dir.clone()
    }
}

/// Copy the per-user folder into a portable `saves` folder that does not
/// exist yet. Never blocks startup: on an error the old files stay put.
fn migrate_legacy(roots: &SaveRoots, target: &Path) {
    let legacy = &roots.user_dir;
    if target.exists() || legacy == target || !legacy.is_dir() {
        return;
    }
    log::info!(
        "Save migration: copying {} into {}",
        legacy.display(),
        target.display()
    );
    if let Err(e) = copy_dir_recursive(legacy, target) {
        log::warn!("Save migration failed: {e}");
    }
}

fn copy_dir_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let dest = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// [`data_dir`] against explicit roots and an explicit "already migrated"
/// flag.
pub fn data_dir_in(roots: &SaveRoots, legacy_checked: &AtomicBool) -> PathBuf {
    if let Some(dir) = &roots.override_dir {
        return dir.clone();
    }
    let target = save_root_in(roots);
    if !legacy_checked.swap(true, Ordering::SeqCst) {
        migrate_legacy(roots, &target);
    }
    target
}

/// Where Dispatch keeps its own files (never Freight Fate's folder): the
/// override, else the portable save folder, copying the per-user folder in
/// on the first call of the process.
pub fn data_dir() -> PathBuf {
    let dir = data_dir_in(&SaveRoots::current(), &LEGACY_CHECKED);
    // An agent playtest (a debug build run with `--agent-server`) keeps its
    // careers and settings apart from the player's, as Hearken's own files do.
    if cfg!(debug_assertions) && std::env::args().any(|a| a == "--agent-server") {
        return dir.join("agent-sandbox");
    }
    dir
}

pub fn careers_dir() -> PathBuf {
    data_dir().join("careers")
}

/// The line to log once logging is up: where saves went, and why when the
/// game folder could not take them.
pub fn location_note() -> String {
    let roots = SaveRoots::current();
    let dir = data_dir_in(&roots, &AtomicBool::new(true));
    if roots.override_dir.is_none() && !roots.macos && dir == roots.user_dir {
        format!(
            "The game folder {} is not writable; saving to {} instead. Move Dispatch out of a protected folder such as Program Files to keep saves beside the game.",
            roots.game_root.display(),
            dir.display()
        )
    } else {
        format!("Saves folder: {}", dir.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn roots(tmp: &Path) -> SaveRoots {
        let game = tmp.join("FreightFateDispatch");
        std::fs::create_dir_all(&game).unwrap();
        SaveRoots {
            override_dir: None,
            game_root: game,
            user_dir: tmp.join("appdata").join(APP_FOLDER),
            macos: false,
            writable: None,
        }
    }

    fn data_dir_of(roots: &SaveRoots) -> PathBuf {
        data_dir_in(roots, &AtomicBool::new(false))
    }

    #[test]
    fn saves_live_inside_the_game_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let r = roots(tmp.path());
        assert_eq!(data_dir_of(&r), r.game_root.join("saves"));
        assert!(!r.user_dir.exists());
    }

    #[test]
    fn the_override_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = roots(tmp.path());
        r.override_dir = Some(tmp.path().join("custom"));
        assert_eq!(data_dir_of(&r), tmp.path().join("custom"));
    }

    #[test]
    fn a_read_only_game_folder_falls_back_to_the_user_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = roots(tmp.path());
        r.writable = Some(false);
        assert_eq!(data_dir_of(&r), r.user_dir);
    }

    #[test]
    fn macos_saves_in_application_support() {
        let tmp = tempfile::tempdir().unwrap();
        let mut r = roots(tmp.path());
        r.macos = true;
        write(&r.user_dir.join("careers/a.json"), "{}");
        assert_eq!(data_dir_of(&r), r.user_dir);
        assert!(!r.game_root.join("saves").exists());
    }

    #[test]
    fn old_per_user_saves_are_copied_in_once() {
        let tmp = tempfile::tempdir().unwrap();
        let r = roots(tmp.path());
        write(&r.user_dir.join("careers/day.json"), "career");
        write(&r.user_dir.join("settings.json"), "settings");
        let dir = data_dir_of(&r);
        assert_eq!(
            std::fs::read_to_string(dir.join("careers/day.json")).unwrap(),
            "career"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("settings.json")).unwrap(),
            "settings"
        );
        // Copied, not moved.
        assert!(r.user_dir.join("careers/day.json").is_file());
    }

    #[test]
    fn existing_portable_saves_are_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let r = roots(tmp.path());
        write(&r.user_dir.join("careers/day.json"), "old");
        write(&r.game_root.join("saves/careers/day.json"), "new");
        let dir = data_dir_of(&r);
        assert_eq!(
            std::fs::read_to_string(dir.join("careers/day.json")).unwrap(),
            "new"
        );
    }

    #[test]
    fn the_migration_runs_once_per_process() {
        let tmp = tempfile::tempdir().unwrap();
        let r = roots(tmp.path());
        let checked = AtomicBool::new(false);
        data_dir_in(&r, &checked);
        write(&r.user_dir.join("careers/day.json"), "late");
        std::fs::remove_dir_all(r.game_root.join("saves")).ok();
        data_dir_in(&r, &checked);
        assert!(!r.game_root.join("saves/careers/day.json").exists());
    }

    #[test]
    fn the_source_checkout_keeps_saves_beside_the_project() {
        assert!(game_root().join("Cargo.toml").is_file());
    }

    #[test]
    fn the_writable_probe_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fresh");
        assert!(is_writable_dir(&dir));
        assert!(!dir.join(".dispatch-write-test").exists());
        let blocked = tmp.path().join("blocked");
        std::fs::write(&blocked, b"x").unwrap();
        assert!(!is_writable_dir(&blocked));
    }
}
