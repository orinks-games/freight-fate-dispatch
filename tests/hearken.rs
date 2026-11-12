//! The desk on the engine, with no window: what the start screen says, and
//! that Settings and Help open over it and come back to where the player
//! was.

use ff_dispatch::shell::{Data, Desk, Dispatch};
use ff_dispatch::ui::app::Dirs;
use hearken::{Headless, InputEvent, Key};

fn start() -> (Headless<Dispatch>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data = Data {
        world: ff_core::data::world::get_world(),
        dirs: Dirs {
            careers: dir.path().join("careers"),
            ff_saves: vec![],
            data: dir.path().to_path_buf(),
        },
        seed: Some(7),
        can_go_online: false,
    };
    let desk = Desk::new(&data);
    (Headless::new(data, desk), dir)
}

#[test]
fn the_start_screen_announces_itself() {
    let (h, _dir) = start();
    assert_eq!(
        h.spoken(),
        [
            "Freight Fate: Dispatch",
            "New career, 1 of 10.",
            "Online services are off. The complete career is available offline."
        ]
    );
    assert_eq!(h.app.top_name(), "desk");
    assert!(h.app.top_lines().contains(&"Settings".to_string()));
}

#[test]
fn settings_and_help_open_over_the_desk_and_come_back() {
    let (mut h, _dir) = start();
    h.press(Key::End);
    h.press(Key::Up);
    h.press(Key::Up);
    h.press(Key::Up);
    h.clear();
    h.press(Key::Return);
    assert_eq!(h.app.top_name(), "settings");
    assert_eq!(h.spoken()[0], "Settings.");
    h.press(Key::Escape);
    assert_eq!(h.app.top_name(), "desk");
    assert!(h.spoken().contains(&"Settings, 7 of 10.".to_string()));
    h.press(Key::Down);
    h.press(Key::Down);
    h.clear();
    h.press(Key::Return);
    assert_eq!(h.app.top_name(), "help");
    assert_eq!(h.spoken()[0], "Help.");
    h.press(Key::Escape);
    assert_eq!(h.app.top_name(), "desk");
}

#[test]
fn the_soundtrack_screen_lists_the_pieces_locked_until_heard() {
    let (mut h, _dir) = start();
    h.press(Key::End);
    h.press(Key::Up);
    h.press(Key::Up);
    h.clear();
    h.press(Key::Return);
    assert_eq!(h.app.top_name(), "soundtrack");
    assert!(h.spoken()[0].starts_with("Soundtrack."), "{:?}", h.spoken());
    // Four tracks between the title and Back; the start screen's music
    // has unlocked the title theme, and the desk pieces read as locked.
    let lines = h.app.top_lines();
    assert_eq!(lines.len(), 6, "{lines:?}");
    let locked = lines.iter().filter(|l| l.contains("Locked track")).count();
    assert_eq!(locked, 3, "{lines:?}");
    let title = &ff_dispatch::audio::PIECES[ff_dispatch::audio::TITLE];
    assert!(lines.iter().any(|l| l.contains(title.title)), "{lines:?}");
    h.press(Key::Escape);
    assert_eq!(h.app.top_name(), "desk");
}

#[test]
fn playing_a_track_from_the_soundtrack_screen_does_not_double_the_menu_music() {
    let (mut h, _dir) = start();
    h.frame(0.1);
    let music = |h: &hearken::Headless<Dispatch>| {
        h.app
            .ctx
            .audio
            .live()
            .iter()
            .filter(|l| l.key.starts_with("music/"))
            .count()
    };
    // The start screen's theme: one performance, six stems.
    assert_eq!(music(&h), 6);
    h.press(Key::End);
    h.press(Key::Up);
    h.press(Key::Up);
    h.press(Key::Return);
    assert_eq!(h.app.top_name(), "soundtrack");
    h.press(Key::Return);
    h.frame(0.1);
    // The track plays on its own, not over the theme already playing.
    assert_eq!(music(&h), 6, "{:?}", h.app.ctx.audio.live());
    h.press(Key::Escape);
    assert_eq!(h.app.top_name(), "desk");
    h.frame(0.1);
    // Back on the start screen, the theme starts again.
    assert_eq!(music(&h), 6, "{:?}", h.app.ctx.audio.live());
}

#[test]
fn f1_reads_the_current_item_s_help_and_the_cursor_clicks() {
    let (mut h, _dir) = start();
    h.clear();
    h.press(Key::F1);
    assert_eq!(h.spoken(), ["Pick a carrier and start with three drivers."]);
    h.clear();
    h.press(Key::Down);
    assert_eq!(h.spoken(), ["Continue, 2 of 10."]);
    assert_eq!(h.played(), ["ui/cursor"]);
    h.clear();
    h.send(InputEvent::typed('q'));
    assert_eq!(h.spoken(), ["Quit, 10 of 10."]);
}

#[test]
fn a_new_career_reaches_the_desk_and_space_says_what_needs_you() {
    let (mut h, _dir) = start();
    h.press(Key::Return);
    h.press(Key::Return);
    h.frame(0.1);
    let said = h.spoken().join(" | ");
    assert!(said.contains("Day 1 at"), "{said}");
    assert!(said.contains("loads on the board"), "{said}");
    let played = h.played();
    assert!(played.contains(&"sfx/day_start".to_string()), "{played:?}");
    // The morning piece starts with the desk: every stem of it together.
    let morning = &ff_dispatch::audio::PIECES[ff_dispatch::audio::piece_for(6 * 60)];
    for stem in morning.bed.iter().chain(morning.voices.iter()) {
        assert!(
            played.contains(&stem.to_string()),
            "{stem} missing from {played:?}"
        );
    }
    h.clear();
    h.press(Key::Space);
    assert_eq!(h.spoken().len(), 1);
}
