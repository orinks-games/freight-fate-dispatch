//! Menu grammar shared with Freight Fate, and what the UI asks the engine
//! to do. Every screen is a Menu, so every screen is also lines.

pub mod app;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Enter,
    Escape,
    /// Help for the current item. The engine reads it through the screen's
    /// help; the key is here for the menu's own tests.
    F1,
    /// What needs the dispatcher now.
    Space,
    Backspace,
    Char(char),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Interrupt and speak.
    Say(String),
    /// Speak after what's already speaking.
    Queue(String),
    Cue(crate::audio::Cue),
    Stop,
    /// The engine's Settings screen, over the desk.
    OpenSettings,
    /// The engine's Help screen, over the desk.
    OpenHelp,
    Quit,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub help: String,
}

impl Item {
    pub fn new(label: impl Into<String>, help: impl Into<String>) -> Item {
        Item {
            label: label.into(),
            help: help.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEvent {
    Speak(String),
    Activate(usize),
    Back,
    None,
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub title: String,
    pub items: Vec<Item>,
    pub index: usize,
}

impl Menu {
    pub fn new(title: impl Into<String>, items: Vec<Item>) -> Menu {
        Menu {
            title: title.into(),
            items,
            index: 0,
        }
    }

    pub fn current_text(&self) -> String {
        match self.items.get(self.index) {
            Some(item) => format!(
                "{}, {} of {}.",
                item.label.trim_end_matches('.'),
                self.index + 1,
                self.items.len()
            ),
            None => "No options available.".into(),
        }
    }

    pub fn intro(&self) -> Vec<Out> {
        vec![
            Out::Say(self.title.clone()),
            Out::Queue(self.current_text()),
        ]
    }

    pub fn lines(&self) -> Vec<String> {
        std::iter::once(self.title.clone())
            .chain(self.items.iter().map(|i| i.label.clone()))
            .collect()
    }

    /// The current item's help, or its label when it has none.
    pub fn help_text(&self) -> String {
        match self.items.get(self.index) {
            Some(i) if !i.help.is_empty() => i.help.clone(),
            Some(i) => format!("{}.", i.label.trim_end_matches('.')),
            None => "No options available.".into(),
        }
    }

    pub fn key(&mut self, key: Key) -> MenuEvent {
        let n = self.items.len();
        match key {
            Key::Escape => return MenuEvent::Back,
            Key::Enter if n > 0 => return MenuEvent::Activate(self.index),
            Key::F1 => return MenuEvent::Speak(self.help_text()),
            _ if n == 0 => return MenuEvent::Speak(self.current_text()),
            Key::Down => self.index = (self.index + 1) % n,
            Key::Up => self.index = (self.index + n - 1) % n,
            Key::Home => self.index = 0,
            Key::End => self.index = n - 1,
            Key::Char(c) if c.is_alphanumeric() => {
                let c = c.to_lowercase().next().unwrap();
                let hit = (1..=n)
                    .map(|off| (self.index + off) % n)
                    .find(|&i| self.items[i].label.to_lowercase().starts_with(c));
                match hit {
                    Some(i) => self.index = i,
                    None => return MenuEvent::None,
                }
            }
            _ => return MenuEvent::None,
        }
        MenuEvent::Speak(self.current_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        Menu::new(
            "Desk",
            vec![
                Item::new("Load board", "The day's loads."),
                Item::new("Trips", ""),
                Item::new("Tally", ""),
            ],
        )
    }

    #[test]
    fn arrows_wrap_and_home_end_jump() {
        let mut m = menu();
        assert_eq!(m.key(Key::Up), MenuEvent::Speak("Tally, 3 of 3.".into()));
        assert_eq!(
            m.key(Key::Down),
            MenuEvent::Speak("Load board, 1 of 3.".into())
        );
        m.key(Key::End);
        assert_eq!(m.index, 2);
        m.key(Key::Home);
        assert_eq!(m.index, 0);
    }

    #[test]
    fn first_letter_cycles_matches_from_the_next_item() {
        let mut m = menu();
        m.key(Key::Char('T'));
        assert_eq!(m.index, 1);
        m.key(Key::Char('t'));
        assert_eq!(m.index, 2);
        assert_eq!(m.key(Key::Char('z')), MenuEvent::None);
    }

    #[test]
    fn enter_escape_and_help() {
        let mut m = menu();
        assert_eq!(m.key(Key::Enter), MenuEvent::Activate(0));
        assert_eq!(m.key(Key::Escape), MenuEvent::Back);
        assert_eq!(m.key(Key::F1), MenuEvent::Speak("The day's loads.".into()));
        m.key(Key::Down);
        assert_eq!(m.key(Key::F1), MenuEvent::Speak("Trips.".into()));
        assert_eq!(m.help_text(), "Trips.");
    }

    #[test]
    fn empty_menu_says_so_and_screens_are_lines() {
        let mut m = Menu::new("Board", vec![]);
        assert_eq!(
            m.key(Key::Enter),
            MenuEvent::Speak("No options available.".into())
        );
        assert_eq!(menu().lines(), vec!["Desk", "Load board", "Trips", "Tally"]);
    }
}
