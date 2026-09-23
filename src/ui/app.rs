//! Screens. The App owns the career and today's shift and turns keys into
//! spoken lines. It never touches SDL or Prism.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use ff_core::data::world::World;
use ff_core::models::start_options::START_OPTIONS;

use super::{Item, Key, Menu, MenuEvent, Out};
use crate::board::{load_line, refusal};
use crate::career::{Game, Ring, Shift};
use crate::import::{import_game, list_ff_saves, read_profile};
use crate::online::{self, Identity, Poll};
use crate::saves::{list_slots, save_game, Slot};

pub struct Dirs {
    pub careers: PathBuf,
    pub ff_saves: Vec<PathBuf>,
    pub data: PathBuf,
}

#[derive(Clone)]
enum Screen {
    Start,
    NewCareer,
    Continue,
    Import,
    Desk,
    Board,
    Drivers(usize),
    Trips,
    Roster,
    Hiring,
    Phone(Ring),
}

enum SignIn {
    Code(String),
    Done(Result<Identity, String>),
}

pub struct App {
    world: &'static World,
    dirs: Dirs,
    seed: Option<i64>,
    live: bool,
    stack: Vec<Screen>,
    menu: Menu,
    pub(crate) game: Option<Game>,
    pub(crate) shift: Option<Shift>,
    announced: usize,
    identity: Option<Identity>,
    sign_in: Option<Receiver<SignIn>>,
}

const DESK_HELP: [&str; 6] = [
    "Assign the day's loads to drivers. Assigning takes 10 minutes.",
    "Trucks on the road. Checking on one takes 2 minutes.",
    "Everyone on the roster and their hours.",
    "Today's applicants. An interview takes 15 minutes.",
    "Hear the morning briefing again.",
    "End the shift. Trips still rolling finish overnight, and the career saves.",
];

impl App {
    pub fn new(world: &'static World, dirs: Dirs, seed: Option<i64>, live: bool) -> App {
        let identity = if live {
            Identity::load(&dirs.data)
        } else {
            None
        };
        App {
            world,
            dirs,
            seed,
            live,
            stack: vec![Screen::Start],
            menu: Menu::new("", vec![]),
            game: None,
            shift: None,
            announced: 0,
            identity,
            sign_in: None,
        }
    }

    pub fn screen_lines(&self) -> Vec<String> {
        self.menu.lines()
    }

    pub fn start(&mut self) -> Vec<Out> {
        self.rebuild();
        self.menu.intro()
    }

    fn rebuild(&mut self) {
        let index = self.menu.index;
        let top = self.stack.last().cloned().unwrap_or(Screen::Start);
        self.menu = self.build(&top);
        self.menu.index = index.min(self.menu.items.len().saturating_sub(1));
    }

    fn open(&mut self, screen: Screen) -> Vec<Out> {
        self.stack.push(screen);
        self.menu.index = 0;
        self.rebuild();
        self.menu.intro()
    }

    fn back(&mut self) -> Vec<Out> {
        if self.stack.len() <= 1 {
            return vec![Out::Quit];
        }
        self.stack.pop();
        self.menu.index = 0;
        self.rebuild();
        self.menu.intro()
    }

    fn build(&self, screen: &Screen) -> Menu {
        let w = self.world;
        match screen {
            Screen::Start => {
                let online = match &self.identity {
                    Some(id) => Item::new(
                        format!("Signed in as {}", id.display_name),
                        "Your orinks.net account. Dispatch posts your carrier line at the end of each day.",
                    ),
                    None => Item::new(
                        "Sign in to orinks.net",
                        "Optional. Uses your Freight Fate account.",
                    ),
                };
                Menu::new(
                    "Freight Fate: Dispatch",
                    vec![
                        Item::new("New career", "Pick a carrier and start with three drivers."),
                        Item::new("Continue", "Saved careers. Each saves at the start of a day."),
                        Item::new(
                            "Import a Freight Fate driver",
                            "Bring your Freight Fate driver onto the roster. Your Freight Fate save is only read.",
                        ),
                        online,
                        Item::new("Quit", ""),
                    ],
                )
            }
            Screen::NewCareer => Menu::new(
                "Pick a carrier",
                START_OPTIONS
                    .iter()
                    .map(|o| Item::new(o.label, o.help_text))
                    .collect(),
            ),
            Screen::Continue => Menu::new(
                "Continue",
                list_slots(&self.dirs.careers)
                    .iter()
                    .map(|s| Item::new(s.label(), ""))
                    .collect(),
            ),
            Screen::Import => Menu::new(
                "Import a Freight Fate driver",
                list_ff_saves(&self.dirs.ff_saves)
                    .iter()
                    .map(|p| {
                        Item::new(
                            p.file_stem().unwrap_or_default().to_string_lossy(),
                            "Read only. Your Freight Fate save is never changed.",
                        )
                    })
                    .collect(),
            ),
            Screen::Desk => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let labels = [
                    format!("Load board, {} loads", s.board.len()),
                    format!("Trips, {} rolling", g.trips.len()),
                    format!("Roster, {} drivers", g.drivers.len()),
                    format!("Hiring, {} applicants", s.applicants.len()),
                    "Briefing".to_string(),
                    "Close the day".to_string(),
                ];
                Menu::new(
                    format!("Desk, {}", s.clock.spoken()),
                    labels
                        .into_iter()
                        .zip(DESK_HELP)
                        .map(|(l, h)| Item::new(l, h))
                        .collect(),
                )
            }
            Screen::Board => {
                let s = self.shift.as_ref().unwrap();
                Menu::new(
                    "Load board",
                    s.board
                        .iter()
                        .map(|l| Item::new(load_line(&l.job, l.hot, w), "Enter picks a driver."))
                        .collect(),
                )
            }
            Screen::Drivers(load) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let job = s.board.get(*load).map(|l| &l.job);
                Menu::new(
                    "Who takes it?",
                    g.drivers
                        .iter()
                        .map(|d| {
                            let why = job.and_then(|j| refusal(j, d, w));
                            Item::new(
                                d.card(),
                                why.unwrap_or_else(|| "Legal for this load.".into()),
                            )
                        })
                        .collect(),
                )
            }
            Screen::Trips => {
                let g = self.game.as_ref().unwrap();
                let name = |id: u32| g.driver(id).map(|d| d.name.clone()).unwrap_or_default();
                Menu::new(
                    "Trips",
                    g.trips
                        .iter()
                        .map(|t| {
                            Item::new(
                                format!(
                                    "{}, {} to {}",
                                    name(t.driver_id),
                                    t.job.cargo.label,
                                    w.spoken_city(&t.job.destination, None)
                                ),
                                "Enter checks on the truck.",
                            )
                        })
                        .collect(),
                )
            }
            Screen::Roster => Menu::new(
                "Roster",
                self.game
                    .as_ref()
                    .unwrap()
                    .drivers
                    .iter()
                    .map(|d| Item::new(d.card(), ""))
                    .collect(),
            ),
            Screen::Hiring => {
                let s = self.shift.as_ref().unwrap();
                let items = s
                    .applicants
                    .iter()
                    .flat_map(|a| {
                        [
                            Item::new(format!("Interview {}", a.driver.name), a.line()),
                            Item::new(
                                format!("Hire {}, {}", a.driver.name, crate::money(a.signing)),
                                a.line(),
                            ),
                        ]
                    })
                    .collect();
                Menu::new("Hiring", items)
            }
            Screen::Phone(ring) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                Menu::new(
                    ring.title(g, s, w),
                    ring.labels(g, s, w)
                        .into_iter()
                        .map(|l| Item::new(l, ""))
                        .collect(),
                )
            }
        }
    }

    pub fn key(&mut self, key: Key) -> Vec<Out> {
        let ringing = self.shift.as_ref().is_some_and(|s| !s.pending.is_empty());
        let on_phone = matches!(self.stack.last(), Some(Screen::Phone(_)));
        if ringing && !on_phone && key == Key::Enter {
            return self.answer();
        }
        let mut out = match self.menu.key(key) {
            MenuEvent::Speak(text) => vec![Out::Say(text)],
            MenuEvent::Stop => vec![Out::Stop],
            MenuEvent::Back if on_phone && !self.menu.items.is_empty() => {
                vec![Out::Say("Pick an answer; the caller is waiting.".into())]
            }
            MenuEvent::Back => self.back(),
            MenuEvent::Activate(i) => self.activate(i),
            MenuEvent::None => vec![],
        };
        out.extend(self.announce_ring());
        out
    }

    fn announce_ring(&mut self) -> Vec<Out> {
        let Some(s) = self.shift.as_ref() else {
            return vec![];
        };
        let on_phone = matches!(self.stack.last(), Some(Screen::Phone(_)));
        if s.pending.len() > self.announced && !on_phone {
            self.announced = s.pending.len();
            return vec![Out::Queue(
                "The phone is ringing. Press Enter to answer.".into(),
            )];
        }
        self.announced = self.announced.min(s.pending.len());
        vec![]
    }

    fn answer(&mut self) -> Vec<Out> {
        let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
        match g.answer(s) {
            Ok(ring) => {
                // A ring still pending after this one is resolved must be
                // announced again on return, so it is not left silent.
                self.announced = 0;
                self.open(Screen::Phone(ring))
            }
            Err(e) => vec![Out::Say(e.spoken())],
        }
    }

    /// Say lines after an action, then refresh the screen underneath.
    fn after(&mut self, lines: Vec<String>) -> Vec<Out> {
        self.rebuild();
        lead(lines.into_iter().map(Out::Queue).collect())
    }

    fn activate(&mut self, i: usize) -> Vec<Out> {
        let w = self.world;
        match self.stack.last().cloned().unwrap() {
            Screen::Start => match i {
                0 => self.open(Screen::NewCareer),
                1 => self.open(Screen::Continue),
                2 => self.open(Screen::Import),
                3 if self.identity.is_none() => self.begin_sign_in(),
                3 => vec![Out::Say(self.menu.items[3].help.clone())],
                _ => vec![Out::Quit],
            },
            Screen::NewCareer => {
                let seed = self.seed.unwrap_or_else(now_seed);
                self.begin(Game::new(START_OPTIONS[i].key, seed, w), true)
            }
            Screen::Continue => match list_slots(&self.dirs.careers).into_iter().nth(i) {
                Some(Slot::Ok { game, .. }) => self.begin(game, false),
                Some(slot) => vec![Out::Say(slot.label())],
                None => vec![],
            },
            Screen::Import => {
                let Some(path) = list_ff_saves(&self.dirs.ff_saves).into_iter().nth(i) else {
                    return vec![];
                };
                let file = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                match read_profile(&path) {
                    Ok(profile) => {
                        let seed = self.seed.unwrap_or_else(now_seed);
                        self.begin(import_game(&profile, seed, w), true)
                    }
                    Err(e) => vec![Out::Say(e.spoken(&file))],
                }
            }
            Screen::Desk => match i {
                0 => self.open(Screen::Board),
                1 => self.open(Screen::Trips),
                2 => self.open(Screen::Roster),
                3 => self.open(Screen::Hiring),
                4 => {
                    let lines = self
                        .game
                        .as_ref()
                        .unwrap()
                        .briefing(self.shift.as_ref().unwrap(), w);
                    self.after(lines)
                }
                _ => self.close_day(),
            },
            Screen::Board => self.open(Screen::Drivers(i)),
            Screen::Drivers(load) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let id = g.drivers[i].id;
                match g.assign(s, load, id, w) {
                    Ok(lines) => {
                        self.stack.pop();
                        self.stack.pop();
                        self.after(lines)
                    }
                    Err(e) => vec![Out::Say(e.spoken())],
                }
            }
            Screen::Trips => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let id = g.trips[i].id;
                match g.check_truck(s, id, w) {
                    Ok(line) => self.after(vec![line]),
                    Err(e) => vec![Out::Say(e.spoken())],
                }
            }
            Screen::Roster => vec![Out::Say(self.menu.items[i].label.clone())],
            Screen::Hiring => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let r = if i.is_multiple_of(2) {
                    g.interview(s, i / 2)
                } else {
                    g.hire(s, i / 2)
                };
                match r {
                    Ok(line) => self.after(vec![line]),
                    Err(e) => vec![Out::Say(e.spoken())],
                }
            }
            Screen::Phone(ring) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let lines = g.resolve(s, &ring, i, w);
                self.stack.pop();
                self.after(lines)
            }
        }
    }

    /// Start a career day: weather (fresh days only), save, brief, desk.
    fn begin(&mut self, game: Game, fresh_day: bool) -> Vec<Out> {
        self.game = Some(game);
        self.stack.truncate(1);
        self.stack.push(Screen::Desk);
        self.menu.index = 0;
        let mut out = self.morning(fresh_day);
        self.rebuild();
        out.extend(queued(self.menu.intro()));
        out
    }

    fn morning(&mut self, fresh_day: bool) -> Vec<Out> {
        let w = self.world;
        let game = self.game.as_mut().unwrap();
        if fresh_day {
            let fetched = if self.live {
                crate::net::live_weather(&game.todays_routes(w), w)
            } else {
                None
            };
            game.set_weather(fetched);
        }
        let mut out = Vec::new();
        if let Err(e) = save_game(game, &self.dirs.careers) {
            log::error!("save failed: {e}");
            out.push(Out::Say(format!("The career could not be saved: {e}")));
        }
        let shift = game.morning(w);
        let lines = game.briefing(&shift, w);
        self.shift = Some(shift);
        self.announced = 0;
        out.extend(lines.into_iter().map(Out::Queue));
        lead(out)
    }

    fn close_day(&mut self) -> Vec<Out> {
        let w = self.world;
        let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
        if !s.pending.is_empty() {
            return vec![Out::Say(crate::career::ActError::PhoneRinging.spoken())];
        }
        let mut out: Vec<Out> = vec![Out::Say("Closing the desk.".into())];
        out.extend(g.close_day(s, w).into_iter().map(Out::Queue));
        if let (Some(id), true) = (&self.identity, self.live) {
            if let Some(token) = online::load_token(&id.driver_id) {
                let game = g.clone();
                std::thread::spawn(move || {
                    online::post_day(
                        &crate::net::UreqTransport::new(10.0),
                        &online::base_url(),
                        &token,
                        &game,
                    );
                });
            }
        }
        let morning = self.morning(true);
        out.extend(queued(morning));
        self.rebuild();
        out
    }

    fn begin_sign_in(&mut self) -> Vec<Out> {
        if !self.live {
            return vec![Out::Say("Online is off in this mode.".into())];
        }
        let (tx, rx) = channel();
        self.sign_in = Some(rx);
        let data = self.dirs.data.clone();
        std::thread::spawn(move || {
            let t = crate::net::UreqTransport::new(10.0);
            let base = online::base_url();
            let Some(a) = online::start(&t, &base, &online::machine_key()) else {
                let _ = tx.send(SignIn::Done(Err(
                    "orinks.net could not be reached. You can keep playing offline.".into(),
                )));
                return;
            };
            let _ = tx.send(SignIn::Code(format!(
                "Go to {} and enter the code {}.",
                a.verification_uri,
                a.user_code
                    .chars()
                    .map(String::from)
                    .collect::<Vec<_>>()
                    .join(" ")
            )));
            let started = Instant::now();
            let result = loop {
                let elapsed = started.elapsed().as_secs_f64();
                if elapsed >= a.expires_in as f64 {
                    break Err("The sign-in code expired.".to_string());
                }
                std::thread::sleep(online::poll_interval(elapsed));
                match online::poll(&t, &base, &a.device_code) {
                    Poll::Ready {
                        driver_id,
                        token,
                        display_name,
                    } => {
                        if !online::store_token(&driver_id, &token) {
                            break Err("The sign-in worked, but the token could not be stored in the system keyring.".into());
                        }
                        let id = Identity {
                            driver_id,
                            display_name,
                        };
                        if let Err(e) = id.save(&data) {
                            log::warn!(
                                "online: identity save failed: {e}; the token is in the keyring, but you will need to sign in again next launch"
                            );
                        }
                        break Ok(id);
                    }
                    Poll::Expired => break Err("The sign-in code expired.".into()),
                    Poll::Error => break Err("orinks.net refused the sign-in.".into()),
                    Poll::Pending | Poll::Retry => {}
                }
            };
            let _ = tx.send(SignIn::Done(result));
        });
        vec![Out::Say("Contacting orinks.net.".into())]
    }

    /// Background results (sign-in). The window calls this between key events.
    pub fn tick(&mut self) -> Vec<Out> {
        let Some(rx) = &self.sign_in else {
            return vec![];
        };
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                SignIn::Code(text) => out.push(Out::Say(text)),
                SignIn::Done(Ok(id)) => {
                    out.push(Out::Say(format!("Signed in as {}.", id.display_name)));
                    self.identity = Some(id);
                    self.sign_in = None;
                    self.rebuild();
                    break;
                }
                SignIn::Done(Err(e)) => {
                    out.push(Out::Say(e));
                    self.sign_in = None;
                    break;
                }
            }
        }
        out
    }
}

fn now_seed() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64 & 0xFFFF_FFFF_FFFF)
        .unwrap_or(1)
}

/// Promote the first `Queue` to a `Say`, so it interrupts. Any later item
/// already `Say` is left alone.
fn lead(mut out: Vec<Out>) -> Vec<Out> {
    if let Some(Out::Queue(first)) = out.first().cloned() {
        out[0] = Out::Say(first);
    }
    out
}

/// Demote every `Say` to a `Queue`, so it speaks after what's already
/// speaking rather than interrupting it.
fn queued(out: Vec<Out>) -> Vec<Out> {
    out.into_iter()
        .map(|o| match o {
            Out::Say(s) => Out::Queue(s),
            o => o,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_core::data::world::get_world;

    fn app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            careers: dir.path().join("careers"),
            ff_saves: vec![],
            data: dir.path().to_path_buf(),
        };
        (App::new(get_world(), dirs, Some(42), false), dir)
    }

    fn said(out: &[Out]) -> String {
        out.iter()
            .filter_map(|o| match o {
                Out::Say(s) | Out::Queue(s) => Some(s.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[test]
    fn start_menu_opens_with_the_title() {
        let (mut a, _d) = app();
        let out = said(&a.start());
        assert!(out.starts_with("Freight Fate: Dispatch"));
        assert!(a.screen_lines().contains(&"New career".to_string()));
    }

    #[test]
    fn new_career_briefs_saves_and_lands_on_the_desk() {
        let (mut a, d) = app();
        a.start();
        a.key(Key::Enter); // New career
        let out = said(&a.key(Key::Enter)); // first carrier
        assert!(out.contains("Day 1"));
        assert!(out.contains("loads on the board"));
        assert!(a.screen_lines()[0].starts_with("Desk"));
        assert_eq!(
            std::fs::read_dir(d.path().join("careers")).unwrap().count(),
            1
        );
    }

    #[test]
    fn closing_the_day_rolls_to_day_two_and_saves() {
        let (mut a, _d) = app();
        a.start();
        a.key(Key::Enter);
        a.key(Key::Enter);
        a.key(Key::End); // Close the day
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("Day 2"), "{out}");
    }

    #[test]
    fn a_ringing_phone_is_announced_and_enter_answers_from_any_menu() {
        let (mut a, _d) = app();
        a.start();
        a.key(Key::Enter);
        a.key(Key::Enter);
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        let id = g.drivers[0].id;
        s.pending
            .push_back(crate::career::Ring::Call(crate::phone::Call::PersonalDay {
                driver_id: id,
            }));
        let out = said(&a.key(Key::Down));
        assert!(out.contains("phone is ringing"), "{out}");
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("personal day"), "{out}");
        let out = said(&a.key(Key::Enter)); // Give them the day
        assert!(out.contains("Day granted"));
        assert!(a.shift.as_ref().unwrap().pending.is_empty());
    }

    #[test]
    fn a_second_stacked_ring_is_announced_again_after_the_first_resolves() {
        let (mut a, _d) = app();
        a.start();
        a.key(Key::Enter);
        a.key(Key::Enter);
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        let (id1, id2) = (g.drivers[0].id, g.drivers[1].id);
        s.pending
            .push_back(crate::career::Ring::Call(crate::phone::Call::PersonalDay {
                driver_id: id1,
            }));
        s.pending
            .push_back(crate::career::Ring::Call(crate::phone::Call::PersonalDay {
                driver_id: id2,
            }));
        a.key(Key::Down); // announced
        a.key(Key::Enter); // answer the first
        let out = said(&a.key(Key::Enter)); // resolve it (Give them the day)
        assert!(out.contains("phone is ringing"), "{out}");
    }

    /// A new career's first morning, on the desk.
    fn on_desk() -> (App, tempfile::TempDir) {
        let (mut a, d) = app();
        a.start();
        a.key(Key::Enter);
        a.key(Key::Enter);
        (a, d)
    }

    #[test]
    fn escape_leaves_a_phone_screen_with_nothing_to_answer() {
        let (mut a, _d) = on_desk();
        a.stack
            .push(Screen::Phone(Ring::Incident(crate::incidents::Incident {
                trip_id: 999,
                kind: crate::incidents::Kind::Breakdown,
            })));
        a.rebuild();
        let out = said(&a.key(Key::Escape));
        assert!(matches!(a.stack.last(), Some(Screen::Desk)));
        assert!(out.starts_with("Desk"), "{out}");
    }

    #[test]
    fn closing_the_day_with_a_call_pending_is_refused() {
        let (mut a, _d) = on_desk();
        let id = a.game.as_ref().unwrap().drivers[0].id;
        a.shift
            .as_mut()
            .unwrap()
            .pending
            .push_back(Ring::Call(crate::phone::Call::PersonalDay {
                driver_id: id,
            }));
        let out = said(&a.close_day());
        assert!(out.contains("The phone is ringing"), "{out}");
        assert_eq!(a.game.as_ref().unwrap().day, 1);
    }

    #[test]
    fn escape_from_the_start_menu_quits() {
        let (mut a, _d) = app();
        a.start();
        assert!(a.key(Key::Escape).contains(&Out::Quit));
    }
}
