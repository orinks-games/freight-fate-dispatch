use std::path::PathBuf;

use ff_core::models::career::Career;
use ff_core::models::profile::{encode_save_bytes, Profile, SAVE_VERSION};
use ff_dispatch::import::{import_game, list_ff_saves, read_profile, ImportError};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ff")
}

/// Regenerate the committed fixtures with the pinned ff-core's own encoder.
/// Run after every ff-core pin bump:
/// `cargo test --test import write_fixtures -- --ignored`
#[test]
#[ignore]
fn write_fixtures() {
    let tmp = tempfile::tempdir().unwrap();
    ff_core::settings::set_thread_data_dir(Some(tmp.path().to_path_buf()));
    let mut p = Profile::named("Casey Rivers");
    p.career = Career::with_xp(4_000.0);
    p.career.deliveries = 40;
    p.career.on_time_deliveries = 36;
    p.carrier_key = "prairie_link".into();
    p.hos.drive(240.0);
    p.integrity_modified = true;
    let mut dict = p.to_dict();
    std::fs::create_dir_all(fixtures()).unwrap();
    std::fs::write(
        fixtures().join("Casey Rivers.ffsave"),
        encode_save_bytes(&dict),
    )
    .unwrap();
    dict.insert("version".into(), (SAVE_VERSION + 1).into());
    std::fs::write(fixtures().join("Newer.ffsave"), encode_save_bytes(&dict)).unwrap();
}

#[test]
fn a_real_ffsave_imports_with_career_hours_and_carrier_intact() {
    let path = fixtures().join("Casey Rivers.ffsave");
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let profile = read_profile(&path).unwrap();
    assert!(
        profile.integrity_modified,
        "integrity flag set, and it still imports"
    );
    let game = import_game(&profile, 9, ff_core::data::world::get_world());
    let d = &game.drivers[0];
    assert_eq!(d.name, "Casey Rivers");
    assert_eq!(d.career.on_time_deliveries, 36);
    assert_eq!(d.hos.driving_min, 240.0);
    assert_eq!(game.carrier_key, "prairie_link");
    assert_eq!(game.drivers.len(), 3);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the FF save is never written"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn a_newer_save_is_refused_plainly_and_untouched() {
    let path = fixtures().join("Newer.ffsave");
    let before = std::fs::read(&path).unwrap();
    let err = read_profile(&path).unwrap_err();
    assert_eq!(
        err,
        ImportError::Newer {
            found: SAVE_VERSION + 1,
            supported: SAVE_VERSION
        }
    );
    assert!(err.spoken("Newer").contains("newer Freight Fate"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_corrupt_ffsave_is_refused_not_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("Bad.ffsave");
    std::fs::write(&bad, b"garbage").unwrap();
    assert!(matches!(
        read_profile(&bad),
        Err(ImportError::Unreadable(_))
    ));
    assert!(bad.exists(), "left where it was");
    assert_eq!(list_ff_saves(&[dir.path().to_path_buf()]), vec![bad]);
}
