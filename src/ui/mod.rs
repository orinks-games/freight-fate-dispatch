//! Menu grammar shared with Freight Fate, and what the UI asks the
//! speaker to do. Every screen is a Menu, so every screen is also lines.

pub mod app;
pub mod speech;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Enter,
    Escape,
    F1,
    Ctrl,
    PageUp,
    PageDown,
    Backspace,
    Char(char),
}

impl Key {
    /// Script words for the key-script mode and tests.
    pub fn parse(word: &str) -> Option<Key> {
        Some(match word.trim().to_ascii_lowercase().as_str() {
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "enter" => Key::Enter,
            "escape" => Key::Escape,
            "f1" => Key::F1,
            "ctrl" => Key::Ctrl,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "backspace" => Key::Backspace,
            w if w.chars().count() == 1 => Key::Char(w.chars().next()?),
            _ => return None,
        })
    }

    /// One script line: `type <text>` types the text as written, anything
    /// else is one key word.
    pub fn parse_line(line: &str) -> Vec<Key> {
        match line.trim_start().strip_prefix("type ") {
            Some(text) => text.trim_end().chars().map(Key::Char).collect(),
            None => Key::parse(line).into_iter().collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Interrupt and speak.
    Say(String),
    /// Speak after what's already speaking.
    Queue(String),
    Cue(crate::audio::Cue),
    /// Music volume in percent.
    Music(u8),
    Stop,
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
    Stop,
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

    pub fn key(&mut self, key: Key) -> MenuEvent {
        let n = self.items.len();
        match key {
            Key::Ctrl => return MenuEvent::Stop,
            Key::Escape => return MenuEvent::Back,
            Key::Enter if n > 0 => return MenuEvent::Activate(self.index),
            Key::F1 => {
                return MenuEvent::Speak(match self.items.get(self.index) {
                    Some(i) if !i.help.is_empty() => i.help.clone(),
                    Some(i) => format!("{}.", i.label.trim_end_matches('.')),
                    None => "No options available.".into(),
                })
            }
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
    fn enter_escape_help_and_stop() {
        let mut m = menu();
        assert_eq!(m.key(Key::Enter), MenuEvent::Activate(0));
        assert_eq!(m.key(Key::Escape), MenuEvent::Back);
        assert_eq!(m.key(Key::F1), MenuEvent::Speak("The day's loads.".into()));
        m.key(Key::Down);
        assert_eq!(m.key(Key::F1), MenuEvent::Speak("Trips.".into()));
        assert_eq!(m.key(Key::Ctrl), MenuEvent::Stop);
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

    #[test]
    fn script_words_parse() {
        assert_eq!(Key::parse("enter"), Some(Key::Enter));
        assert_eq!(Key::parse("F1"), Some(Key::F1));
        assert_eq!(Key::parse("c"), Some(Key::Char('c')));
        assert_eq!(Key::parse("nonsense"), None);
    }
}
