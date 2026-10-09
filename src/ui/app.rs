//! Screens. The App owns the career and today's shift and turns keys into
//! spoken lines. It never touches SDL or Prism.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use ff_core::data::world::World;
use ff_core::models::start_options::START_OPTIONS;

use super::{Item, Key, Menu, MenuEvent, Out};
use crate::audio::Cue;
use crate::board::{load_line, refusal};
use crate::career::{Game, Ring, Shift};
use crate::import::{import_game, list_ff_saves, read_profile};
use crate::online::{self, Identity, Poll};
use crate::saves::{list_slots, load_settings, save_game, save_settings, Settings, Slot};

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
    OnlineBoard,
    Desk,
    Board,
    Drivers(usize),
    Trips,
    Roster,
    Hiring,
    Phone(Ring),
    /// The board for one driver, with their deadhead to each load.
    BoardFor(u32),
}

/// A fresh morning held back one step, so the line before its (blocking)
/// weather fetch is spoken first. It runs on the next tick or key.
enum Fresh {
    Begin(Box<Game>),
    AfterClose,
}

enum SignIn {
    Code(String),
    Done(Result<Identity, String>),
}

pub struct App {
    world: &'static World,
    dirs: Dirs,
    seed: Option<i64>,
    can_go_online: bool,
    online_enabled: bool,
    music_volume: u8,
    stack: Vec<Screen>,
    menu: Menu,
    pub(crate) game: Option<Game>,
    pub(crate) shift: Option<Shift>,
    announced: usize,
    identity: Option<Identity>,
    sign_in: Option<Receiver<SignIn>>,
    sign_in_cancel: Option<Arc<AtomicBool>>,
    online_board: Option<Receiver<Result<Vec<String>, String>>>,
    online_board_lines: Vec<String>,
    /// The last key was an Escape on the desk that warned about leaving.
    leave_armed: bool,
    fresh: Option<Fresh>,
    /// Where the focus was when the phone was answered.
    phone_return: usize,
    /// The sign-in code instruction, while a sign-in is in progress.
    sign_in_code: Option<String>,
}

const DESK_HELP: [&str; 7] = [
    "Assign the day's loads to drivers. Assigning takes 10 minutes.",
    "Trucks on the road. Checking on one takes 2 minutes.",
    "Everyone on the roster and their hours.",
    "Today's applicants. An interview takes 15 minutes.",
    "Hear the morning briefing again.",
    "Let time run to the next call, arrival, or driver coming off a break, and stop there. F2 anywhere at the desk says what needs you now.",
    "End the shift. Trips still rolling finish overnight, and the career saves.",
];

impl App {
    pub fn new(world: &'static World, dirs: Dirs, seed: Option<i64>, can_go_online: bool) -> App {
        let settings = load_settings(&dirs.data);
        let online_enabled = can_go_online && settings.online_services;
        let identity = if online_enabled {
            Identity::load(&dirs.data)
        } else {
            None
        };
        App {
            world,
            dirs,
            seed,
            can_go_online,
            online_enabled,
            music_volume: settings.music_volume.min(100),
            stack: vec![Screen::Start],
            menu: Menu::new("", vec![]),
            game: None,
            shift: None,
            announced: 0,
            identity,
            sign_in: None,
            sign_in_cancel: None,
            online_board: None,
            online_board_lines: Vec::new(),
            leave_armed: false,
            fresh: None,
            phone_return: 0,
            sign_in_code: None,
        }
    }

    pub fn screen_lines(&self) -> Vec<String> {
        self.menu.lines()
    }

    pub fn start(&mut self) -> Vec<Out> {
        self.rebuild();
        let mut out = vec![Out::Music(self.music_volume)];
        out.extend(self.menu.intro());
        out.push(Out::Queue(if self.online_enabled {
            "Online services are on.".into()
        } else {
            "Online services are off. The complete career is available offline.".into()
        }));
        out
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
                let online = match (&self.identity, &self.sign_in_code) {
                    (Some(id), _) => Item::new(
                        signed_in(id),
                        "Your orinks.net account. Dispatch posts your carrier line at the end of each day.",
                    ),
                    (None, Some(code)) => Item::new(code.clone(), ""),
                    (None, None) => Item::new(
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
                        Item::new(
                            "Online drivers board",
                            "Hear which Freight Fate drivers are playing now.",
                        ),
                        online,
                        Item::new(
                            if self.online_enabled {
                                "Online services: on"
                            } else {
                                "Online services: off"
                            },
                            "Optional. Controls live weather, the drivers board, sign-in, and activity posts.",
                        ),
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
            Screen::Import => {
                let mut items: Vec<Item> = list_ff_saves(&self.dirs.ff_saves)
                    .iter()
                    .map(|p| {
                        Item::new(
                            p.file_stem().unwrap_or_default().to_string_lossy(),
                            "Read only. Your Freight Fate save is never changed.",
                        )
                    })
                    .collect();
                if items.is_empty() {
                    let dirs: Vec<String> = self
                        .dirs
                        .ff_saves
                        .iter()
                        .map(|d| d.display().to_string())
                        .collect();
                    items.push(Item::new(
                        format!(
                            "No Freight Fate saves found. Dispatch looked in {}. Set {} to your Freight Fate saves\\profiles folder.",
                            dirs.join(" and "),
                            crate::import::FF_SAVES_ENV
                        ),
                        "",
                    ));
                }
                Menu::new("Import a Freight Fate driver", items)
            }
            Screen::OnlineBoard => Menu::new(
                "Online drivers board",
                self.online_board_lines
                    .iter()
                    .map(|line| Item::new(line, ""))
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
                    "Wait for what's next".to_string(),
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
                let title = match s.board.get(*load) {
                    Some(l) => format!("Who takes it? {}", load_line(&l.job, l.hot, w)),
                    None => "Who takes it?".into(),
                };
                Menu::new(
                    title,
                    g.drivers
                        .iter()
                        .map(|d| {
                            let why = job.and_then(|j| {
                                if matches!(
                                    d.status,
                                    crate::roster::Status::OnLoad | crate::roster::Status::OutOnRun
                                ) {
                                    return Some(
                                        "Rolling. Enter queues this as their next load.".into(),
                                    );
                                }
                                refusal(j, d, w)
                            });
                            Item::new(
                                d.card_at(w, s.now(g.day), g.day),
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
                    .map(|d| {
                        let g = self.game.as_ref().unwrap();
                        let now = self.shift.as_ref().map(|s| s.now(g.day)).unwrap_or(0);
                        Item::new(d.card_at(w, now, g.day), d.pattern().text())
                    })
                    .collect(),
            ),
            Screen::Hiring => {
                let s = self.shift.as_ref().unwrap();
                let items = s
                    .applicants
                    .iter()
                    .flat_map(|a| {
                        [
                            Item::new(
                                format!(
                                    "{} {}",
                                    if a.interviewed {
                                        "Review interview"
                                    } else {
                                        "Interview"
                                    },
                                    a.driver.name
                                ),
                                a.line(),
                            ),
                            Item::new(
                                format!("Hire {}, {}", a.driver.name, crate::money(a.signing)),
                                a.line(),
                            ),
                        ]
                    })
                    .collect();
                Menu::new("Hiring", items)
            }
            Screen::BoardFor(driver_id) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let Some(d) = g.driver(*driver_id) else {
                    return Menu::new("Load board", vec![]);
                };
                Menu::new(
                    format!("Loads for {}", d.name),
                    s.board
                        .iter()
                        .map(|l| {
                            let p = crate::trip::plan(&l.job, d, w);
                            let empty = if p.deadhead_mi >= 1.0 {
                                format!(" {} miles empty to the shipper.", p.deadhead_mi.round())
                            } else {
                                " At the shipper's city.".into()
                            };
                            Item::new(
                                format!("{}{empty}", load_line(&l.job, l.hot, w)),
                                refusal(&l.job, d, w)
                                    .unwrap_or_else(|| "Legal for this driver.".into()),
                            )
                        })
                        .collect(),
                )
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
        let mut out = self.run_fresh();
        out.extend(match key {
            Key::PageUp => self.change_music(10),
            Key::PageDown => self.change_music(-10),
            _ => self.handle(key),
        });
        out
    }

    fn change_music(&mut self, step: i16) -> Vec<Out> {
        let volume = (i16::from(self.music_volume) + step).clamp(0, 100) as u8;
        self.music_volume = volume;
        let line = if volume == 0 {
            "Music off.".to_string()
        } else {
            format!("Music {volume} percent.")
        };
        let mut out = vec![Out::Music(volume), Out::Say(line)];
        if let Err(error) = save_settings(&self.settings(), &self.dirs.data) {
            log::error!("settings save failed: {error}");
            out.push(Out::Queue(format!(
                "The music volume could not be saved: {error}"
            )));
        }
        out
    }

    fn settings(&self) -> Settings {
        Settings {
            online_services: self.online_enabled,
            music_volume: self.music_volume,
        }
    }

    fn handle(&mut self, key: Key) -> Vec<Out> {
        let armed = std::mem::take(&mut self.leave_armed);
        let on_phone = matches!(self.stack.last(), Some(Screen::Phone(_)));
        if let (Some(g), Some(s)) = (self.game.as_ref(), self.shift.as_mut()) {
            let rang = !s.pending.is_empty();
            g.drop_dead_rings(s);
            if rang && s.pending.is_empty() && self.announced > 0 && !on_phone {
                self.announced = 0;
                if key == Key::Enter {
                    return vec![Out::Say("The call dropped.".into())];
                }
            }
        }
        if key == Key::F2 {
            if let (Some(g), Some(s)) = (self.game.as_ref(), self.shift.as_ref()) {
                if on_phone {
                    return vec![Out::Say("The caller is waiting; pick an answer.".into())];
                }
                return vec![Out::Say(g.needs_now(s, self.world))];
            }
        }
        let on_desk = matches!(self.stack.last(), Some(Screen::Desk));
        let ringing = self.shift.as_ref().is_some_and(|s| !s.pending.is_empty());
        if ringing && !on_phone && key == Key::Enter {
            return self.answer();
        }
        let event = self.menu.key(key);
        let cursor = matches!(
            key,
            Key::Up | Key::Down | Key::Home | Key::End | Key::Char(_)
        ) && matches!(&event, MenuEvent::Speak(_));
        let activate = matches!(&event, MenuEvent::Activate(_) | MenuEvent::Back);
        let mut out = match event {
            MenuEvent::Speak(text) => vec![Out::Say(text)],
            MenuEvent::Stop => vec![Out::Stop],
            MenuEvent::Back if on_phone && !self.menu.items.is_empty() => {
                vec![Out::Say("Pick an answer; the caller is waiting.".into())]
            }
            MenuEvent::Back if on_desk && !armed => {
                self.leave_armed = true;
                vec![Out::Say("Leaving the desk loses today's work since this morning's save. Press Escape again to leave, or close the day to save.".into())]
            }
            MenuEvent::Back if on_desk => {
                self.shift = None;
                self.game = None;
                self.back()
            }
            MenuEvent::Back => self.back(),
            MenuEvent::Activate(i) => self.activate(i),
            MenuEvent::None => vec![],
        };
        if cursor {
            out.insert(0, Out::Cue(Cue::Cursor));
        } else if activate {
            out.insert(0, Out::Cue(Cue::Activate));
        }
        out.extend(self.announce_ring());
        out
    }

    fn announce_ring(&mut self) -> Vec<Out> {
        let (Some(g), Some(s)) = (self.game.as_ref(), self.shift.as_mut()) else {
            return vec![];
        };
        g.drop_dead_rings(s);
        let on_phone = matches!(self.stack.last(), Some(Screen::Phone(_)));
        if s.pending.len() > self.announced && !on_phone {
            self.announced = s.pending.len();
            return vec![
                Out::Cue(Cue::Phone),
                Out::Queue("The phone is ringing. Press Enter to answer.".into()),
            ];
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
                self.phone_return = self.menu.index;
                self.open(Screen::Phone(ring))
            }
            Err(crate::career::ActError::NoSuchItem) => {
                self.announced = 0;
                vec![Out::Say("The call dropped.".into())]
            }
            Err(e) => vec![Out::Say(e.spoken())],
        }
    }

    /// Say lines after an action, then refresh the screen underneath.
    fn after(&mut self, lines: Vec<String>) -> Vec<Out> {
        self.rebuild();
        lead(lines.into_iter().map(Out::Queue).collect())
    }

    /// `after`, then say where the focus landed.
    fn landed(&mut self, lines: Vec<String>) -> Vec<Out> {
        let mut out = self.after(lines);
        out.push(Out::Queue(self.menu.current_text()));
        out
    }

    fn activate(&mut self, i: usize) -> Vec<Out> {
        let w = self.world;
        match self.stack.last().cloned().unwrap() {
            Screen::Start => match i {
                0 => self.open(Screen::NewCareer),
                1 => self.open(Screen::Continue),
                2 => self.open(Screen::Import),
                3 => self.begin_online_board(),
                4 if self.sign_in.is_some() => vec![Out::Say(self.menu.items[4].label.clone())],
                4 if self.identity.is_none() => self.begin_sign_in(),
                4 => vec![Out::Say(self.menu.items[4].help.clone())],
                5 => self.toggle_online(),
                _ => vec![Out::Quit],
            },
            Screen::NewCareer => {
                let seed = self.seed.unwrap_or_else(now_seed);
                let game = Game::new(START_OPTIONS[i].key, seed, w);
                self.fresh = Some(Fresh::Begin(Box::new(game)));
                vec![Out::Say("Setting up the desk.".into())]
            }
            Screen::Continue => match list_slots(&self.dirs.careers).into_iter().nth(i) {
                Some(Slot::Ok { game, .. }) => self.begin(game, false),
                Some(slot) => vec![Out::Say(slot.label())],
                None => vec![],
            },
            Screen::Import => {
                let Some(path) = list_ff_saves(&self.dirs.ff_saves).into_iter().nth(i) else {
                    return vec![Out::Say(self.menu.items[i].label.clone())];
                };
                let file = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                match read_profile(&path) {
                    Ok(profile) => {
                        let seed = self.seed.unwrap_or_else(now_seed);
                        let game = import_game(&profile, seed, w);
                        self.fresh = Some(Fresh::Begin(Box::new(game)));
                        vec![Out::Say("Setting up the desk.".into())]
                    }
                    Err(e) => vec![Out::Say(e.spoken(&file))],
                }
            }
            Screen::OnlineBoard => vec![Out::Say(self.menu.items[i].label.clone())],
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
                5 => {
                    let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                    if !s.pending.is_empty() {
                        return vec![Out::Say(crate::career::ActError::PhoneRinging.spoken())];
                    }
                    let lines = g.advance(s, w);
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
                        self.menu.index = 0;
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.after(lines));
                        out.push(Out::Cue(Cue::Cursor));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
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
                    Ok(line) => {
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.after(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::Phone(ring) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let lines = g.resolve(s, &ring, i, w);
                self.stack.pop();
                self.menu.index = self.phone_return;
                if let (Ring::Call(crate::phone::Call::Empty { driver_id }), 0) = (&ring, i) {
                    return self.open(Screen::BoardFor(*driver_id));
                }
                self.landed(lines)
            }
            Screen::BoardFor(driver_id) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                match g.assign(s, i, driver_id, w) {
                    Ok(lines) => {
                        self.stack.pop();
                        self.menu.index = 0;
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.after(lines));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
        }
    }

    /// Start a career day: weather (fresh days only), save, brief, desk.
    fn begin(&mut self, mut game: Game, fresh_day: bool) -> Vec<Out> {
        game.normalize();
        self.game = Some(game);
        self.stack.truncate(1);
        self.stack.push(Screen::Desk);
        self.menu.index = 0;
        let mut out = self.morning(fresh_day);
        self.rebuild();
        out.extend(queued(self.menu.intro()));
        out
    }

    fn run_fresh(&mut self) -> Vec<Out> {
        let out = match self.fresh.take() {
            None => return vec![],
            Some(Fresh::Begin(game)) => self.begin(*game, true),
            Some(Fresh::AfterClose) => {
                let mut out = self.morning(true);
                self.menu.index = 0;
                self.rebuild();
                out.push(Out::Queue(self.menu.current_text()));
                out
            }
        };
        queued(out)
    }

    fn morning(&mut self, fresh_day: bool) -> Vec<Out> {
        let w = self.world;
        let game = self.game.as_mut().unwrap();
        if fresh_day {
            let fetched = if self.online_enabled {
                crate::net::live_weather(&game.todays_routes(w), w)
            } else {
                None
            };
            game.set_weather(fetched);
        }
        let mut out = vec![Out::Cue(Cue::DayStart)];
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
        g.drop_dead_rings(s);
        if !s.pending.is_empty() {
            return vec![Out::Say(crate::career::ActError::PhoneRinging.spoken())];
        }
        // Closing early still runs the rest of the day in order: each event
        // before 6 PM stops the desk first, and Close goes on from there.
        let end = crate::desk::abs_min(g.day, crate::desk::SHIFT_END);
        if !s.clock.closed() && g.next_event_at(s).is_some_and(|t| t <= end) {
            let mut lines = g.advance(s, w);
            lines[0] = format!("Before you close, it's {}", lines[0]);
            let mut out = self.after(lines);
            out.extend(self.announce_ring());
            return out;
        }
        s.clock.minute = crate::desk::SHIFT_END;
        g.ring_due(s);
        if !s.pending.is_empty() {
            self.announced = s.pending.len(); // this line is the announcement
            self.rebuild();
            return vec![Out::Say(
                "Before you close: the phone is ringing. Press Enter to answer.".into(),
            )];
        }
        let mut out: Vec<Out> = vec![
            Out::Cue(Cue::DayClose),
            Out::Say("Closing the desk.".into()),
        ];
        out.extend(g.close_day(s, w).into_iter().map(Out::Queue));
        if online::POST_DAY_JOURNAL {
            if let (Some(id), true) = (&self.identity, self.online_enabled) {
                if let Some(token) = online::load_token(&id.driver_id) {
                    let game = g.clone();
                    let driver_id = id.driver_id.clone();
                    std::thread::spawn(move || {
                        online::post_day(
                            &crate::net::UreqTransport::new(10.0),
                            &online::base_url(),
                            &driver_id,
                            &token,
                            &game,
                        );
                    });
                }
            }
        }
        self.fresh = Some(Fresh::AfterClose);
        out
    }

    fn begin_sign_in(&mut self) -> Vec<Out> {
        if !self.online_enabled {
            return vec![Out::Say(
                "Online services are off. Turn them on from the main menu first.".into(),
            )];
        }
        let (tx, rx) = channel();
        self.sign_in = Some(rx);
        let cancel = Arc::new(AtomicBool::new(false));
        self.sign_in_cancel = Some(cancel.clone());
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
                if cancel.load(Ordering::Acquire) {
                    return;
                }
                let elapsed = started.elapsed().as_secs_f64();
                if elapsed >= a.expires_in as f64 {
                    break Err("The sign-in code expired.".to_string());
                }
                std::thread::sleep(online::poll_interval(elapsed));
                if cancel.load(Ordering::Acquire) {
                    return;
                }
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

    fn begin_online_board(&mut self) -> Vec<Out> {
        if !self.online_enabled {
            return vec![Out::Say(
                "Online services are off. Turn them on from the main menu first.".into(),
            )];
        }
        self.online_board_lines = vec!["Loading.".into()];
        let (tx, rx) = channel();
        self.online_board = Some(rx);
        std::thread::spawn(move || {
            let result = online::board(&crate::net::UreqTransport::new(10.0), &online::base_url())
                .map(|drivers| {
                    if drivers.is_empty() {
                        vec!["No drivers are online right now.".into()]
                    } else {
                        drivers.into_iter().map(|driver| driver.spoken()).collect()
                    }
                })
                .map_err(|_| {
                    "The online drivers board could not be reached. You can keep playing offline."
                        .to_string()
                });
            let _ = tx.send(result);
        });
        self.open(Screen::OnlineBoard)
    }

    fn toggle_online(&mut self) -> Vec<Out> {
        if !self.can_go_online {
            return vec![Out::Say(
                "Online services are unavailable during a scripted run.".into(),
            )];
        }
        self.online_enabled = !self.online_enabled;
        if !self.online_enabled {
            if let Some(cancel) = self.sign_in_cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            self.sign_in = None;
            self.sign_in_code = None;
        }
        self.identity = if self.online_enabled {
            Identity::load(&self.dirs.data)
        } else {
            None
        };
        let settings = self.settings();
        let line = if self.online_enabled {
            "Online services are on."
        } else {
            "Online services are off. Dispatch will not use the network."
        };
        let mut out = vec![Out::Say(line.into())];
        if let Err(error) = save_settings(&settings, &self.dirs.data) {
            log::error!("settings save failed: {error}");
            out.push(Out::Queue(format!(
                "The online setting could not be saved: {error}"
            )));
        }
        self.rebuild();
        out
    }

    /// Background work: a held-back morning, then online results. The window
    /// calls this between key events.
    pub fn tick(&mut self) -> Vec<Out> {
        let mut out = self.run_fresh();
        if let Some(result) = self
            .online_board
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.online_board_lines = match result {
                Ok(lines) => lines,
                Err(error) => vec![error],
            };
            self.online_board = None;
            if matches!(self.stack.last(), Some(Screen::OnlineBoard)) {
                self.rebuild();
                out.push(Out::Say("Online drivers board.".into()));
                out.push(Out::Queue(self.menu.current_text()));
            }
        }
        let msgs: Vec<SignIn> = self
            .sign_in
            .as_ref()
            .map(|receiver| receiver.try_iter().collect())
            .unwrap_or_default();
        for msg in msgs {
            match msg {
                SignIn::Code(text) => {
                    out.push(Out::Say(text.clone()));
                    self.sign_in_code = Some(text);
                    self.rebuild();
                }
                SignIn::Done(result) => {
                    out.push(Out::Cue(if result.is_ok() {
                        Cue::Success
                    } else {
                        Cue::Error
                    }));
                    out.push(Out::Say(match &result {
                        Ok(id) => signed_in(id),
                        Err(e) => e.clone(),
                    }));
                    self.identity = result.ok().or(self.identity.take());
                    self.sign_in = None;
                    self.sign_in_cancel = None;
                    self.sign_in_code = None;
                    self.rebuild();
                    break;
                }
            }
        }
        out
    }
}

/// "Signed in as Name.", or "Signed in." with no display name.
fn signed_in(id: &Identity) -> String {
    match id.display_name.trim() {
        "" => "Signed in.".into(),
        name => format!("Signed in as {name}."),
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
    if let Some((index, first)) = out.iter().enumerate().find_map(|(index, item)| match item {
        Out::Queue(text) => Some((index, text.clone())),
        _ => None,
    }) {
        out[index] = Out::Say(first);
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
        assert!(out.contains("Online services are off."));
        assert!(a.screen_lines().contains(&"New career".to_string()));
        assert!(a
            .screen_lines()
            .contains(&"Online services: off".to_string()));
    }

    #[test]
    fn page_keys_set_the_music_volume_anywhere_and_remember_it() {
        let (mut a, d) = app();
        let start = a.start();
        assert_eq!(start[0], Out::Music(crate::audio::DEFAULT_MUSIC_VOLUME));
        let out = a.key(Key::PageUp);
        assert!(out.contains(&Out::Music(50)));
        assert_eq!(said(&out), "Music 50 percent.");
        for _ in 0..6 {
            a.key(Key::PageDown);
        }
        assert_eq!(said(&a.key(Key::PageDown)), "Music off.");
        assert_eq!(load_settings(d.path()).music_volume, 0);
        assert!(a.screen_lines().contains(&"New career".to_string()));
    }

    #[test]
    fn online_services_require_an_explicit_persisted_opt_in() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            careers: dir.path().join("careers"),
            ff_saves: vec![],
            data: dir.path().to_path_buf(),
        };
        let mut a = App::new(get_world(), dirs, Some(42), true);
        a.start();
        a.menu.index = 5;

        assert!(said(&a.key(Key::Enter)).contains("Online services are on."));
        assert!(load_settings(dir.path()).online_services);
        assert!(a
            .screen_lines()
            .contains(&"Online services: on".to_string()));

        let cancel = Arc::new(AtomicBool::new(false));
        let (_tx, rx) = channel();
        a.sign_in = Some(rx);
        a.sign_in_cancel = Some(cancel.clone());
        a.sign_in_code = Some("pending".into());
        assert!(said(&a.key(Key::Enter))
            .contains("Online services are off. Dispatch will not use the network."));
        assert!(cancel.load(Ordering::Acquire));
        assert!(a.sign_in.is_none());
        assert!(a.sign_in_cancel.is_none());
        assert!(a.sign_in_code.is_none());
        assert!(!load_settings(dir.path()).online_services);
    }

    #[test]
    fn new_career_briefs_saves_and_lands_on_the_desk() {
        let (mut a, d) = app();
        a.start();
        a.key(Key::Enter); // New career
        let first = a.key(Key::Enter); // first carrier
        assert_eq!(
            first,
            vec![
                Out::Cue(Cue::Activate),
                Out::Say("Setting up the desk.".into())
            ]
        );
        let out = said(&a.tick()); // the weather fetch runs after that line
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
        let (mut a, _d) = on_desk();
        a.shift.as_mut().unwrap().schedule = Default::default(); // a quiet day
        a.key(Key::End); // Close the day
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("Closing the desk."), "{out}");
        assert!(
            !out.contains("Day 2"),
            "the morning waits for the next tick"
        );
        let out = said(&a.tick());
        assert!(out.contains("Day 2"), "{out}");
        assert!(out.ends_with("Load board, 8 loads, 1 of 7."), "{out}");
    }

    #[test]
    fn a_ringing_phone_is_announced_and_enter_answers_from_any_menu() {
        let (mut a, _d) = on_desk();
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
        assert!(
            out.ends_with("Trips, 0 rolling, 2 of 7."),
            "focus returns: {out}"
        );
        assert!(a.shift.as_ref().unwrap().pending.is_empty());
    }

    #[test]
    fn a_second_stacked_ring_is_announced_again_after_the_first_resolves() {
        let (mut a, _d) = on_desk();
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
        a.tick();
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
    fn the_driver_picker_names_its_load() {
        let (mut a, _d) = on_desk();
        a.key(Key::Enter); // Load board
        let out = said(&a.key(Key::Enter)); // first load
        let s = a.shift.as_ref().unwrap();
        let line = load_line(&s.board[0].job, false, get_world());
        assert!(out.starts_with(&format!("Who takes it? {line}")), "{out}");
    }

    #[test]
    fn closing_early_rings_what_was_due_by_six_then_closes() {
        let (mut a, _d) = on_desk();
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        let (load, driver) = crate::career::tests::legal_pair(g, s);
        g.assign(s, load, driver, get_world()).unwrap();
        s.schedule = Default::default();
        s.pending.clear();
        let trip_id = g.trips[0].id;
        g.trips[0].dock_hold = true;
        s.schedule.push(
            g.trips[0].depart + 30,
            Ring::Incident(crate::incidents::Incident {
                trip_id,
                kind: crate::incidents::Kind::Detention,
            }),
        );
        a.key(Key::End); // Close the day
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("phone is ringing"), "{out}");
        assert_eq!(a.game.as_ref().unwrap().day, 1);
        let out = said(&a.key(Key::Enter)); // answer
        assert!(out.contains("not ready to load"), "{out}");
        a.key(Key::Enter); // Wait it out

        // Close runs the rest of the day in order; each event stops it.
        let mut closed = false;
        for _ in 0..20 {
            a.key(Key::End);
            let out = said(&a.key(Key::Enter));
            if out.contains("Closing the desk.") {
                closed = true;
                break;
            }
            if out.contains("phone is ringing") {
                a.key(Key::Enter);
                a.key(Key::End);
                a.key(Key::Enter);
            }
        }
        assert!(closed);
        let out = said(&a.tick());
        assert!(out.contains("Day 2 at"), "{out}");
    }

    #[test]
    fn escape_from_the_desk_warns_once_then_leaves() {
        let (mut a, _d) = on_desk();
        let out = said(&a.key(Key::Escape));
        assert!(out.contains("Press Escape again to leave"), "{out}");
        assert!(matches!(a.stack.last(), Some(Screen::Desk)));
        a.key(Key::Down); // any other key disarms
        a.key(Key::Escape);
        assert!(matches!(a.stack.last(), Some(Screen::Desk)));
        let out = said(&a.key(Key::Escape));
        assert!(out.starts_with("Freight Fate: Dispatch"), "{out}");
        assert!(a.shift.is_none() && a.game.is_none());
    }

    #[test]
    fn an_assignment_returns_to_the_desk_without_rereading_it() {
        let (mut a, _d) = on_desk();
        let (g, s) = (a.game.as_ref().unwrap(), a.shift.as_ref().unwrap());
        let (load, driver) = crate::career::tests::legal_pair(g, s);
        let pick = g.drivers.iter().position(|d| d.id == driver).unwrap();
        a.key(Key::Enter); // Load board
        for _ in 0..load {
            a.key(Key::Down);
        }
        a.key(Key::Enter);
        for _ in 0..pick {
            a.key(Key::Down);
        }
        let outs = a.key(Key::Enter);
        let out = said(&outs);
        assert!(out.contains("is headed to the shipper"), "{out}");
        assert!(!out.contains("Load board, 7 loads"), "{out}");
        assert!(matches!(a.stack.last(), Some(Screen::Desk)));
        assert!(
            matches!(outs.last(), Some(Out::Cue(Cue::Cursor))),
            "a focus cue marks the landing"
        );
    }

    fn dead_detention_ring(a: &mut App) {
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        let (load, driver) = crate::career::tests::legal_pair(g, s);
        g.assign(s, load, driver, get_world()).unwrap();
        s.schedule = Default::default();
        let trip_id = g.trips[0].id;
        g.trips[0].dock_hold = false; // the dock call was already settled
        s.pending
            .push_back(Ring::Incident(crate::incidents::Incident {
                trip_id,
                kind: crate::incidents::Kind::Detention,
            }));
    }

    #[test]
    fn a_dead_ring_is_dropped_before_it_is_announced() {
        let (mut a, _d) = on_desk();
        dead_detention_ring(&mut a);
        let out = said(&a.key(Key::Down));
        assert!(!out.contains("phone is ringing"), "{out}");
        assert!(a.shift.as_ref().unwrap().pending.is_empty());
    }

    #[test]
    fn a_ring_that_dies_after_it_was_announced_says_the_call_dropped() {
        let (mut a, _d) = on_desk();
        a.shift
            .as_mut()
            .unwrap()
            .pending
            .push_back(Ring::Call(crate::phone::Call::HotLoad { offer: 0 }));
        a.announced = 1;
        a.shift.as_mut().unwrap().pending.clear();
        dead_detention_ring(&mut a);
        let out = said(&a.key(Key::Enter));
        assert_eq!(out, "The call dropped.");
        assert!(matches!(a.stack.last(), Some(Screen::Desk)));
    }

    #[test]
    fn an_empty_import_menu_says_where_it_looked() {
        let (mut a, d) = app();
        a.dirs.ff_saves = vec![d.path().join("nowhere")];
        a.start();
        a.key(Key::Home);
        a.key(Key::Down);
        a.key(Key::Down);
        let out = said(&a.key(Key::Enter)); // Import
        assert!(out.contains("No Freight Fate saves found"), "{out}");
        assert!(out.contains("nowhere"), "{out}");
        assert!(out.contains(crate::import::FF_SAVES_ENV), "{out}");
        let again = said(&a.key(Key::Enter));
        assert!(again.starts_with("No Freight Fate saves found"), "{again}");
    }

    #[test]
    fn sign_in_code_stays_on_the_start_menu_until_done() {
        let (mut a, _d) = app();
        a.start();
        let (tx, rx) = channel();
        a.sign_in = Some(rx);
        let code = "Go to orinks.net and enter the code A B C.";
        tx.send(SignIn::Code(code.into())).unwrap();
        assert_eq!(said(&a.tick()), code);
        assert!(a.screen_lines().contains(&code.to_string()));
        tx.send(SignIn::Done(Ok(Identity {
            driver_id: "d1".into(),
            display_name: String::new(),
        })))
        .unwrap();
        assert_eq!(said(&a.tick()), "Signed in.");
        assert!(a.screen_lines().contains(&"Signed in.".to_string()));
    }

    #[test]
    fn escape_from_the_start_menu_quits() {
        let (mut a, _d) = app();
        a.start();
        assert!(a.key(Key::Escape).contains(&Out::Quit));
    }
}
