use std::process::Command;

#[test]
fn boots_without_speech_and_plays_a_scripted_shift() {
    let dir = tempfile::tempdir().unwrap();
    let keys = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/first_shift.keys"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_ff-dispatch"))
        .args(["--seed", "7", "--script", keys])
        .env("FF_DISPATCH_DATA_DIR", dir.path())
        .env("FF_DISPATCH_NO_SPEECH", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    for line in [
        "Freight Fate: Dispatch",
        "Day 1 at",
        "loads on the board",
        "Assign the day's loads",
        "Load board",
        "Closing the desk.",
        "Day 2 at",
    ] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    assert_eq!(
        std::fs::read_dir(dir.path().join("careers"))
            .unwrap()
            .count(),
        1
    );
}
