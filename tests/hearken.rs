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
            "New career, 1 of 9.",
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
    h.clear();
    h.press(Key::Return);
    assert_eq!(h.app.top_name(), "settings");
    assert_eq!(h.spoken()[0], "Settings.");
    h.press(Key::Escape);
    assert_eq!(h.app.top_name(), "desk");
    assert!(h.spoken().contains(&"Settings, 7 of 9.".to_string()));
    h.press(Key::Down);
    h.clear();
    h.press(Key::Return);
    assert_eq!(h.app.top_name(), "help");
    assert_eq!(h.spoken()[0], "Help.");
    h.press(Key::Escape);
    assert_eq!(h.app.top_name(), "desk");
}

#[test]
fn f1_reads_the_current_item_s_help_and_the_cursor_clicks() {
    let (mut h, _dir) = start();
    h.clear();
    h.press(Key::F1);
    assert_eq!(h.spoken(), ["Pick a carrier and start with three drivers."]);
    h.clear();
    h.press(Key::Down);
    assert_eq!(h.spoken(), ["Continue, 2 of 9."]);
    assert_eq!(h.played(), ["ui/cursor"]);
    h.clear();
    h.send(InputEvent::typed('q'));
    assert_eq!(h.spoken(), ["Quit, 9 of 9."]);
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
    assert!(h.played().contains(&"desk/day_start".to_string()));
    h.clear();
    h.press(Key::Space);
    assert_eq!(h.spoken().len(), 1);
}
