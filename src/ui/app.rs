//! Screens. The App owns the career and today's shift and turns keys into
//! spoken lines. It never touches SDL or Prism.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, sync_channel, Receiver};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ff_core::data::world::World;
use ff_core::models::start_options::START_OPTIONS;

use super::{Item, Key, Menu, MenuEvent, Out};
use crate::audio::Cue;
use crate::board::{load_line, refusal};
use crate::career::{Game, Ring, Shift};
use crate::desk::Action;
use crate::import::{import_game, list_ff_saves, read_profile};
use crate::online::{self, Identity, Poll};
use crate::saves::{list_slots, load_settings, save_game, save_settings, Settings, Slot};

const REMOTE_POLL_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, PartialEq, Eq)]
enum RemotePhase {
    Ringing,
    Claiming,
    Choosing,
    Answering,
}

struct RemoteCall {
    call: online::OpenCall,
    phase: RemotePhase,
}

#[derive(Clone, Copy)]
enum RemoteTaskKind {
    Poll,
    Claim,
    Answer,
}

enum RemoteEvent {
    Open(Result<Vec<online::OpenCall>, online::CallError>),
    Claim(Result<(), online::CallError>),
    Answer(Result<(), online::CallError>),
}

struct RemoteWorker {
    kind: RemoteTaskKind,
    receiver: Receiver<RemoteEvent>,
    cancel: Arc<AtomicBool>,
}

fn run_remote_task(
    transport: &dyn ff_core::sim::real_traffic::HttpTransport,
    base: &str,
    identity: &Identity,
    token: &str,
    kind: RemoteTaskKind,
    call_id: Option<&str>,
    decision: Option<&str>,
) -> RemoteEvent {
    match kind {
        RemoteTaskKind::Poll => {
            RemoteEvent::Open(online::open_calls(transport, base, identity, token))
        }
        RemoteTaskKind::Claim => RemoteEvent::Claim(online::claim_call(
            transport,
            base,
            identity,
            token,
            call_id.unwrap_or_default(),
        )),
        RemoteTaskKind::Answer => RemoteEvent::Answer(online::answer_call(
            transport,
            base,
            identity,
            token,
            call_id.unwrap_or_default(),
            decision.unwrap_or_default(),
        )),
    }
}

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
    RemoteCall,
    Desk,
    Board,
    Drivers(usize),
    Trips,
    Roster,
    Hiring,
    Phone(Ring),
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
    take_driver_calls: bool,
    remote_call: Option<RemoteCall>,
    remote_worker: Option<RemoteWorker>,
    remote_next_poll: Instant,
    remote_poll_stopped: bool,
    remote_not_listed_announced: bool,
    seen_remote_calls: HashSet<String>,
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
            take_driver_calls: settings.take_driver_calls,
            remote_call: None,
            remote_worker: None,
            remote_next_poll: Instant::now(),
            remote_poll_stopped: false,
            remote_not_listed_announced: false,
            seen_remote_calls: HashSet::new(),
        }
    }

    pub fn screen_lines(&self) -> Vec<String> {
        self.menu.lines()
    }

    pub fn start(&mut self) -> Vec<Out> {
        self.rebuild();
        let mut out = self.menu.intro();
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
                        Item::new(
                            if self.take_driver_calls {
                                "Take Freight Fate drivers' calls: on"
                            } else {
                                "Take Freight Fate drivers' calls: off"
                            },
                            "A remote dispatcher may answer, and the office answers if nobody does within a minute.",
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
            Screen::RemoteCall => {
                let Some(call) = self.remote_call.as_ref() else {
                    return Menu::new("Remote call", vec![]);
                };
                let mut items: Vec<Item> = call
                    .call
                    .kind
                    .choices()
                    .iter()
                    .map(|(label, _)| Item::new(*label, ""))
                    .collect();
                items.push(Item::new("Let the office handle it", ""));
                Menu::new("Remote call", items)
            }
        }
    }

    pub fn key(&mut self, key: Key) -> Vec<Out> {
        let mut out = self.run_fresh();
        out.extend(self.handle(key));
        out
    }

    fn handle(&mut self, key: Key) -> Vec<Out> {
        if let Some(call) = &self.remote_call {
            match call.phase {
                RemotePhase::Ringing => match key {
                    Key::Enter => return self.claim_remote_call(),
                    Key::Escape => return self.let_remote_call_go(true),
                    _ => return vec![],
                },
                RemotePhase::Claiming | RemotePhase::Answering => {
                    if key == Key::Escape {
                        self.cancel_remote_work();
                        self.clear_remote_screen();
                        return vec![Out::Say("The call dropped.".into())];
                    }
                    return vec![];
                }
                RemotePhase::Choosing if key == Key::Escape => {
                    return self.let_remote_call_go(false)
                }
                RemotePhase::Choosing => {}
            }
        }
        let armed = std::mem::take(&mut self.leave_armed);
        let on_desk = matches!(self.stack.last(), Some(Screen::Desk));
        let ringing = self.shift.as_ref().is_some_and(|s| !s.pending.is_empty());
        let on_phone = matches!(self.stack.last(), Some(Screen::Phone(_)));
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
        let Some(s) = self.shift.as_ref() else {
            return vec![];
        };
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
                6 => self.toggle_take_driver_calls(),
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
            Screen::RemoteCall => self.choose_remote_call(i),
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
                        self.menu.index = 0;
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.landed(lines));
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
                self.landed(lines)
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
        if !s.pending.is_empty() {
            return vec![Out::Say(crate::career::ActError::PhoneRinging.spoken())];
        }
        // Closing runs the desk clock to 6 PM: whatever was due by then rings
        // first, and the day closes on the next Close with nothing pending.
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
            self.cancel_remote_work();
            self.clear_remote_call();
            if let Some(cancel) = self.sign_in_cancel.take() {
                cancel.store(true, Ordering::Release);
            }
            self.sign_in = None;
            self.sign_in_code = None;
        } else {
            self.remote_poll_stopped = false;
            self.remote_next_poll = Instant::now();
        }
        self.identity = if self.online_enabled {
            Identity::load(&self.dirs.data)
        } else {
            None
        };
        let settings = Settings {
            online_services: self.online_enabled,
            take_driver_calls: self.take_driver_calls,
        };
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

    fn toggle_take_driver_calls(&mut self) -> Vec<Out> {
        self.take_driver_calls = !self.take_driver_calls;
        if self.take_driver_calls {
            self.remote_poll_stopped = false;
            self.remote_next_poll = Instant::now();
        } else {
            self.cancel_remote_work();
            self.clear_remote_call();
        }
        let settings = Settings {
            online_services: self.online_enabled,
            take_driver_calls: self.take_driver_calls,
        };
        let line = if self.take_driver_calls {
            "Taking Freight Fate drivers' calls is on."
        } else {
            "Taking Freight Fate drivers' calls is off."
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

    fn remote_enabled(&self) -> bool {
        self.online_enabled && self.take_driver_calls && self.identity.is_some()
    }

    fn cancel_remote_work(&mut self) {
        if let Some(worker) = &self.remote_worker {
            worker.cancel.store(true, Ordering::Release);
        }
    }

    fn clear_remote_screen(&mut self) {
        self.clear_remote_call();
    }

    fn clear_remote_call(&mut self) {
        self.remote_call = None;
        if matches!(self.stack.last(), Some(Screen::RemoteCall)) {
            self.stack.pop();
            self.menu.index = 0;
            self.rebuild();
        }
    }

    fn start_remote_worker(
        &mut self,
        kind: RemoteTaskKind,
        call_id: Option<String>,
        decision: Option<String>,
    ) -> bool {
        let Some(identity) = self.identity.clone() else {
            return false;
        };
        let base = online::base_url();
        let (tx, receiver) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let Ok(_join) = std::thread::Builder::new().spawn(move || {
            if worker_cancel.load(Ordering::Acquire) {
                return;
            }
            let Some(token) = online::load_token(&identity.driver_id) else {
                let event = match kind {
                    RemoteTaskKind::Poll => RemoteEvent::Open(Err(online::CallError::Unauthorized)),
                    RemoteTaskKind::Claim => RemoteEvent::Claim(Err(online::CallError::Other)),
                    RemoteTaskKind::Answer => RemoteEvent::Answer(Err(online::CallError::Other)),
                };
                if !worker_cancel.load(Ordering::Acquire) {
                    let _ = tx.send(event);
                }
                return;
            };
            let transport = crate::net::UreqTransport::new(10.0);
            let event = run_remote_task(
                &transport,
                &base,
                &identity,
                &token,
                kind,
                call_id.as_deref(),
                decision.as_deref(),
            );
            if !worker_cancel.load(Ordering::Acquire) {
                let _ = tx.send(event);
            }
        }) else {
            return false;
        };
        self.remote_worker = Some(RemoteWorker {
            kind,
            receiver,
            cancel,
        });
        true
    }

    fn claim_remote_call(&mut self) -> Vec<Out> {
        let Some(call) = self.remote_call.as_mut() else {
            return vec![];
        };
        if call.phase != RemotePhase::Ringing {
            return vec![];
        }
        call.phase = RemotePhase::Claiming;
        let id = call.call.call_id.clone();
        if self.start_remote_worker(RemoteTaskKind::Claim, Some(id), None) {
            vec![Out::Say("Connecting to the dispatcher.".into())]
        } else {
            self.clear_remote_call();
            vec![Out::Say("The call dropped.".into())]
        }
    }

    fn choose_remote_call(&mut self, index: usize) -> Vec<Out> {
        let Some(call) = self.remote_call.as_ref() else {
            return vec![];
        };
        if call.phase != RemotePhase::Choosing {
            return vec![];
        }
        let choices = call.call.kind.choices();
        if index == choices.len() {
            return self.let_remote_call_go(false);
        }
        let Some((_, decision)) = choices.get(index) else {
            return vec![];
        };
        let call_id = call.call.call_id.clone();
        if let Some(shift) = self.shift.as_mut() {
            shift.clock.spend(Action::TakeCall);
        }
        if let Some(call) = self.remote_call.as_mut() {
            call.phase = RemotePhase::Answering;
        }
        if self.start_remote_worker(
            RemoteTaskKind::Answer,
            Some(call_id),
            Some((*decision).to_string()),
        ) {
            vec![Out::Say("Sending your answer.".into())]
        } else {
            self.clear_remote_call();
            vec![Out::Say("The call dropped.".into())]
        }
    }

    fn let_remote_call_go(&mut self, ring: bool) -> Vec<Out> {
        self.clear_remote_call();
        vec![Out::Say(if ring {
            "You let the call go.".into()
        } else {
            "The office will handle it.".into()
        })]
    }

    fn process_remote_event(
        &mut self,
        kind: RemoteTaskKind,
        event: RemoteEvent,
        wall_ms: u64,
    ) -> Vec<Out> {
        match (kind, event) {
            (RemoteTaskKind::Poll, RemoteEvent::Open(Ok(calls))) => {
                if !matches!(self.stack.last(), Some(Screen::Desk))
                    || self
                        .shift
                        .as_ref()
                        .is_some_and(|shift| !shift.pending.is_empty())
                    || self.remote_call.is_some()
                {
                    return vec![];
                }
                let Some(call) = calls.into_iter().find(|call| {
                    call.ring_until > wall_ms && !self.seen_remote_calls.contains(&call.call_id)
                }) else {
                    return vec![];
                };
                self.seen_remote_calls.insert(call.call_id.clone());
                let line = format!(
                    "A Freight Fate driver, {}, is calling about {}. Press Enter to answer.",
                    call.caller_name,
                    call.kind.words()
                );
                self.remote_call = Some(RemoteCall {
                    call,
                    phase: RemotePhase::Ringing,
                });
                vec![Out::Cue(Cue::Phone), Out::Say(line)]
            }
            (RemoteTaskKind::Poll, RemoteEvent::Open(Err(online::CallError::NotListed))) => {
                self.remote_poll_stopped = true;
                if std::mem::replace(&mut self.remote_not_listed_announced, true) {
                    vec![]
                } else {
                    vec![Out::Say(
                        "Taking drivers' calls needs a public driver profile with sharing turned on."
                            .into(),
                    )]
                }
            }
            (RemoteTaskKind::Poll, RemoteEvent::Open(Err(online::CallError::Unauthorized))) => {
                self.remote_poll_stopped = true;
                vec![]
            }
            (RemoteTaskKind::Poll, RemoteEvent::Open(Err(_))) => vec![],
            (RemoteTaskKind::Claim, RemoteEvent::Claim(result)) => {
                let Some(call) = self.remote_call.as_mut() else {
                    return vec![];
                };
                if call.phase != RemotePhase::Claiming {
                    return vec![];
                }
                match result {
                    Ok(()) => {
                        call.phase = RemotePhase::Choosing;
                        let summary = call_facts_summary(&call.call.facts);
                        let mut out: Vec<Out> = summary.into_iter().map(Out::Queue).collect();
                        self.stack.push(Screen::RemoteCall);
                        self.menu.index = 0;
                        self.rebuild();
                        out.push(Out::Queue("Remote call".into()));
                        out.push(Out::Queue(self.menu.current_text()));
                        out
                    }
                    Err(online::CallError::Taken) => {
                        self.clear_remote_call();
                        vec![Out::Say(
                            "Another dispatcher already took that call.".into(),
                        )]
                    }
                    Err(_) => {
                        self.clear_remote_call();
                        vec![Out::Say("The call dropped.".into())]
                    }
                }
            }
            (RemoteTaskKind::Answer, RemoteEvent::Answer(result)) => {
                let caller = self
                    .remote_call
                    .as_ref()
                    .map(|call| call.call.caller_name.clone())
                    .unwrap_or_else(|| "the driver".into());
                self.clear_remote_call();
                vec![Out::Say(match result {
                    Ok(()) => format!("Your answer is on its way to {caller}."),
                    Err(_) => "The call dropped.".into(),
                })]
            }
            _ => vec![],
        }
    }

    fn remote_tick(&mut self, now: Instant, wall_ms: u64) -> Vec<Out> {
        let mut out = Vec::new();
        if self.remote_call.as_ref().is_some_and(|call| {
            call.phase == RemotePhase::Ringing && wall_ms >= call.call.ring_until
        }) {
            self.clear_remote_call();
            out.push(Out::Say("The driver's call stopped ringing.".into()));
        }
        let result =
            self.remote_worker
                .as_ref()
                .and_then(|worker| match worker.receiver.try_recv() {
                    Ok(event) => Some((
                        worker.kind,
                        Some(event),
                        worker.cancel.load(Ordering::Acquire),
                    )),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some((worker.kind, None, worker.cancel.load(Ordering::Acquire)))
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                });
        if let Some((kind, event, cancelled)) = result {
            self.remote_worker = None;
            if !cancelled {
                if let Some(event) = event {
                    out.extend(self.process_remote_event(kind, event, wall_ms));
                } else if matches!(kind, RemoteTaskKind::Claim | RemoteTaskKind::Answer) {
                    self.clear_remote_call();
                    out.push(Out::Say("The call dropped.".into()));
                }
            }
        }
        let local_ring = self
            .shift
            .as_ref()
            .is_some_and(|shift| !shift.pending.is_empty());
        if self.remote_enabled()
            && !self.remote_poll_stopped
            && self.remote_worker.is_none()
            && self.remote_call.is_none()
            && !local_ring
            && matches!(self.stack.last(), Some(Screen::Desk))
            && now >= self.remote_next_poll
        {
            self.remote_next_poll = now + REMOTE_POLL_INTERVAL;
            self.start_remote_worker(RemoteTaskKind::Poll, None, None);
        }
        out
    }

    /// Background work: a held-back morning, then online results. The window
    /// calls this between key events.
    pub fn tick(&mut self) -> Vec<Out> {
        self.tick_at(Instant::now(), unix_millis())
    }

    fn tick_at(&mut self, now: Instant, wall_ms: u64) -> Vec<Out> {
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
        out.extend(self.remote_tick(now, wall_ms));
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

fn call_facts_summary(facts: &online::CallFacts) -> Vec<String> {
    let miles = facts.remaining_miles.round() as u64;
    let rounded_hours = facts.hours_left.abs().round() as u64;
    let hour_text = if rounded_hours == 1 {
        "1 hour".to_string()
    } else {
        format!("{rounded_hours} hours")
    };
    let mut lines = vec![
        format!("{miles} miles to go."),
        if facts.hours_left < 0.0 {
            format!("Running {hour_text} late.")
        } else {
            format!("{hour_text} before the appointment.")
        },
        format!(
            "Truck damage {} percent. Freight damage {} percent.",
            facts.truck_damage_pct.round() as u64,
            facts.cargo_damage_pct.round() as u64
        ),
    ];
    if let Some(minutes) = facts.hos_remaining_minutes {
        lines.push(format!("Next hours limit in {}.", crate::hours(minutes)));
    }
    if facts.weather_alerts > 0 {
        lines.push(if facts.weather_alerts == 1 {
            "1 weather warning.".into()
        } else {
            format!("{} weather warnings.", facts.weather_alerts)
        });
    }
    lines
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
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
        assert!(out.ends_with("Load board, 8 loads, 1 of 6."), "{out}");
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
            out.ends_with("Trips, 0 rolling, 2 of 6."),
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
        s.schedule.push(
            g.trips[0].depart + 30,
            Ring::Incident(crate::incidents::Incident {
                trip_id,
                kind: crate::incidents::Kind::Detention,
            }),
        );
        a.key(Key::End); // Close the day
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("the phone is ringing"), "{out}");
        assert_eq!(a.game.as_ref().unwrap().day, 1);
        let out = said(&a.key(Key::Enter)); // answer
        assert!(out.contains("not ready to load"), "{out}");
        a.key(Key::Enter); // Wait it out
        a.key(Key::End);
        a.key(Key::Enter);
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
    fn an_assignment_lands_on_the_load_board_item() {
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
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("is rolling"), "{out}");
        assert!(out.ends_with("Load board, 7 loads, 1 of 6."), "{out}");
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

    fn remote_call(ring_until: u64, phase: RemotePhase) -> RemoteCall {
        RemoteCall {
            call: online::OpenCall {
                call_id: "call-1".into(),
                caller_name: "Casey".into(),
                kind: online::CallKind::Delay,
                facts: online::CallFacts {
                    remaining_miles: 12.4,
                    hours_left: 1.3,
                    truck_damage_pct: 34.5,
                    cargo_damage_pct: 0.4,
                    hos_remaining_minutes: Some(90.0),
                    weather_alerts: 1,
                },
                ring_until,
            },
            phase,
        }
    }

    #[test]
    fn taking_calls_off_or_online_services_off_never_starts_polling() {
        let (mut app, _dir) = on_desk();
        app.online_enabled = true;
        app.identity = Some(Identity {
            driver_id: "driver-1".into(),
            display_name: String::new(),
        });
        app.take_driver_calls = false;
        app.remote_tick(Instant::now() + REMOTE_POLL_INTERVAL, 0);
        assert!(app.remote_worker.is_none());

        app.take_driver_calls = true;
        app.online_enabled = false;
        app.remote_tick(Instant::now() + REMOTE_POLL_INTERVAL, 0);
        assert!(app.remote_worker.is_none());
    }

    #[test]
    fn a_local_phone_ring_prevents_remote_polling() {
        let (mut app, _dir) = on_desk();
        app.online_enabled = true;
        app.take_driver_calls = true;
        app.identity = Some(Identity {
            driver_id: "driver-1".into(),
            display_name: String::new(),
        });
        let driver_id = app.game.as_ref().unwrap().drivers[0].id;
        app.shift
            .as_mut()
            .unwrap()
            .pending
            .push_back(Ring::Call(crate::phone::Call::PersonalDay { driver_id }));
        app.remote_tick(Instant::now() + REMOTE_POLL_INTERVAL, 0);
        assert!(app.remote_worker.is_none());
    }

    #[test]
    fn either_off_toggle_cancels_work_and_clears_a_pending_ring() {
        let (mut app, _dir) = on_desk();
        let (_tx, rx) = sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        app.remote_worker = Some(RemoteWorker {
            kind: RemoteTaskKind::Poll,
            receiver: rx,
            cancel: cancel.clone(),
        });
        app.remote_call = Some(remote_call(100, RemotePhase::Ringing));
        app.take_driver_calls = true;
        app.toggle_take_driver_calls();
        assert!(cancel.load(Ordering::Acquire));
        assert!(app.remote_call.is_none());

        app.remote_call = Some(remote_call(100, RemotePhase::Ringing));
        app.can_go_online = true;
        app.online_enabled = true;
        app.identity = Some(Identity {
            driver_id: "driver-1".into(),
            display_name: String::new(),
        });
        app.toggle_online();
        assert!(app.remote_call.is_none());
    }

    #[test]
    fn not_listed_is_announced_once_and_stops_polling_until_toggled() {
        let (mut app, _dir) = on_desk();
        let line = "Taking drivers' calls needs a public driver profile with sharing turned on.";
        assert_eq!(
            said(&app.process_remote_event(
                RemoteTaskKind::Poll,
                RemoteEvent::Open(Err(online::CallError::NotListed)),
                0,
            )),
            line
        );
        assert!(app.remote_poll_stopped);
        assert!(said(&app.process_remote_event(
            RemoteTaskKind::Poll,
            RemoteEvent::Open(Err(online::CallError::NotListed)),
            0,
        ))
        .is_empty());
        assert!(app.remote_poll_stopped);
        app.toggle_take_driver_calls();
        app.toggle_take_driver_calls();
        assert!(!app.remote_poll_stopped);
        assert!(said(&app.process_remote_event(
            RemoteTaskKind::Poll,
            RemoteEvent::Open(Err(online::CallError::NotListed)),
            0,
        ))
        .is_empty());
    }

    #[test]
    fn sign_in_loss_stops_polling_without_speech() {
        let (mut app, _dir) = on_desk();
        assert!(said(&app.process_remote_event(
            RemoteTaskKind::Poll,
            RemoteEvent::Open(Err(online::CallError::Unauthorized)),
            0,
        ))
        .is_empty());
        assert!(app.remote_poll_stopped);
    }

    #[test]
    fn remote_ring_speaks_the_prompt_and_escape_lets_it_go() {
        let (mut app, _dir) = on_desk();
        let out = app.process_remote_event(
            RemoteTaskKind::Poll,
            RemoteEvent::Open(Ok(vec![remote_call(10_000, RemotePhase::Ringing).call])),
            0,
        );
        assert_eq!(out[0], Out::Cue(Cue::Phone));
        assert_eq!(
            said(&out),
            "A Freight Fate driver, Casey, is calling about a delay. Press Enter to answer."
        );
        assert_eq!(said(&app.key(Key::Escape)), "You let the call go.");
        assert!(app.remote_call.is_none());
        assert!(app.remote_worker.is_none());
    }

    #[test]
    fn frame_tick_drains_remote_worker_results() {
        let (mut app, _dir) = on_desk();
        let (sender, receiver) = sync_channel(1);
        sender
            .send(RemoteEvent::Open(Ok(vec![
                remote_call(10_000, RemotePhase::Ringing).call,
            ])))
            .unwrap();
        app.remote_worker = Some(RemoteWorker {
            kind: RemoteTaskKind::Poll,
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let out = app.tick_at(Instant::now(), 0);
        assert!(said(&out).contains("A Freight Fate driver, Casey"));
        assert!(app.remote_worker.is_none());
        assert!(app.remote_call.is_some());
    }

    #[test]
    fn remote_ring_expires_at_the_server_timestamp() {
        let (mut app, _dir) = on_desk();
        app.remote_call = Some(remote_call(1234, RemotePhase::Ringing));
        assert_eq!(
            said(&app.remote_tick(Instant::now(), 1234)),
            "The driver's call stopped ringing."
        );
        assert!(app.remote_call.is_none());
    }

    #[test]
    fn claim_conflicts_speak_the_already_taken_line() {
        for error in [online::CallError::Taken, online::CallError::Other] {
            let (mut app, _dir) = on_desk();
            app.remote_call = Some(remote_call(100, RemotePhase::Claiming));
            let line = said(&app.process_remote_event(
                RemoteTaskKind::Claim,
                RemoteEvent::Claim(Err(error)),
                0,
            ));
            assert_eq!(
                line,
                if error == online::CallError::Taken {
                    "Another dispatcher already took that call."
                } else {
                    "The call dropped."
                }
            );
        }
    }

    #[test]
    fn successful_claim_speaks_the_facts_and_offers_design_choices() {
        let (mut app, _dir) = on_desk();
        app.remote_call = Some(remote_call(100, RemotePhase::Claiming));
        let out = app.process_remote_event(RemoteTaskKind::Claim, RemoteEvent::Claim(Ok(())), 0);
        let text = said(&out);
        assert!(text.contains("12 miles to go."));
        assert!(text.contains("1 hour before the appointment."));
        assert!(text.contains("Truck damage 35 percent. Freight damage 0 percent."));
        assert!(text.contains("Next hours limit in 1.5 hours."));
        assert!(text.contains("1 weather warning."));
        assert!(app
            .screen_lines()
            .contains(&"Tell them to keep going".to_string()));
        assert!(app
            .screen_lines()
            .contains(&"Let the office handle it".to_string()));
        assert_eq!(app.menu.items.len(), 4);
    }

    #[test]
    fn choosing_an_answer_spends_five_desk_minutes() {
        let (mut app, _dir) = on_desk();
        app.remote_call = Some(remote_call(100, RemotePhase::Choosing));
        app.stack.push(Screen::RemoteCall);
        app.rebuild();
        let before = app.shift.as_ref().unwrap().clock.minute;
        app.choose_remote_call(0);
        assert_eq!(app.shift.as_ref().unwrap().clock.minute, before + 5);
    }

    #[test]
    fn letting_the_office_handle_it_sends_nothing_and_costs_no_time() {
        let (mut app, _dir) = on_desk();
        app.remote_call = Some(remote_call(100, RemotePhase::Choosing));
        app.stack.push(Screen::RemoteCall);
        app.rebuild();
        let before = app.shift.as_ref().unwrap().clock.minute;
        let out = app.choose_remote_call(online::CallKind::Delay.choices().len());
        assert_eq!(said(&out), "The office will handle it.");
        assert_eq!(app.shift.as_ref().unwrap().clock.minute, before);
        assert!(app.remote_worker.is_none());
    }

    #[test]
    fn answer_completion_speaks_the_caller_name() {
        let (mut app, _dir) = on_desk();
        app.remote_call = Some(remote_call(100, RemotePhase::Answering));
        app.stack.push(Screen::RemoteCall);
        app.rebuild();
        let out = app.process_remote_event(RemoteTaskKind::Answer, RemoteEvent::Answer(Ok(())), 0);
        assert_eq!(said(&out), "Your answer is on its way to Casey.");
        assert!(app.remote_call.is_none());
        assert!(matches!(app.stack.last(), Some(Screen::Desk)));
    }

    #[test]
    fn facts_summary_handles_late_and_singular_warning_lines() {
        let mut call = remote_call(0, RemotePhase::Choosing).call;
        call.facts.hours_left = -3.2;
        assert_eq!(
            call_facts_summary(&call.facts),
            vec![
                "12 miles to go.",
                "Running 3 hours late.",
                "Truck damage 35 percent. Freight damage 0 percent.",
                "Next hours limit in 1.5 hours.",
                "1 weather warning."
            ]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
        );
        call.facts.weather_alerts = 2;
        assert!(call_facts_summary(&call.facts).contains(&"2 weather warnings.".to_string()));
    }

    #[test]
    fn escape_from_the_start_menu_quits() {
        let (mut a, _d) = app();
        a.start();
        assert!(a.key(Key::Escape).contains(&Out::Quit));
    }
}
