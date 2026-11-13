//! Screens. The App owns the career and today's shift and turns keys into
//! spoken lines. It never touches the window, the voice or the sound pool;
//! `crate::shell::Desk` does that.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Instant;

use ff_core::data::world::World;
use ff_core::models::start_options::pay_plan_for_key;

use super::{Item, Key, Menu, MenuEvent, Out};
use crate::audio::Cue;
use crate::board::refusal_or_warning;
use crate::career::{Game, Ring, Shift};
use crate::customers::tender_line;
use crate::import::{
    check_saves_folder, clean_typed_path, env_saves_dir, found_line, import_game, list_ff_saves,
    profile_dirs, read_profile,
};
use crate::online::{self, Identity, Poll};
use crate::saves::{
    list_slots, load_settings, save_bytes, save_path, save_settings, write_save, Settings, Slot,
};
use crate::tips::Tip;

#[derive(Clone, Debug)]
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
    /// Typing the Freight Fate saves folder.
    SavesFolder,
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
    /// One driver's card, with finding them a load or sending them home.
    Driver(u32),
    /// Open tenders, and the sales call.
    Tenders,
    /// Accept or decline one tender.
    Tender(usize),
    /// Pick tonight who takes an accepted tender in the morning; the index
    /// is into `Game::coming_tenders`.
    PrePlan(usize),
    /// Cash, what's owed, the bills, factoring, and the trucks.
    Money,
    Trucks,
    /// The tier, the score, the accounts, the terminals, the milestones.
    Company,
    /// Where to open the second terminal.
    TerminalPick,
    /// The tips heard so far, the on or off switch, and forgetting them.
    Tips,
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
    /// For each screen on the stack, the row it was on when the next one
    /// opened, so Escape lands back there.
    return_index: Vec<usize>,
    menu: Menu,
    pub(crate) game: Option<Game>,
    pub(crate) shift: Option<Shift>,
    announced: usize,
    identity: Option<Identity>,
    sign_in: Option<Receiver<SignIn>>,
    sign_in_cancel: Option<Arc<AtomicBool>>,
    online_board: Option<Receiver<Result<Vec<String>, String>>>,
    online_board_lines: Vec<String>,
    /// The last key was an Escape that asked first: quitting from the
    /// start screen, or leaving a desk whose save just failed.
    leave_armed: bool,
    /// Close the day has warned today about contract loads or tenders left
    /// open; it doesn't warn again until tomorrow.
    close_armed: bool,
    /// The player asked to close the day; the close resumes after each call.
    closing: bool,
    /// The last key was Enter on something that asks first: a driver short
    /// on hours (board index, driver id), or a truck to service
    /// (`usize::MAX`, truck id). A second Enter goes ahead.
    confirm_over: Option<(usize, u32)>,
    fresh: Option<Fresh>,
    /// Where the focus was when the phone was answered.
    phone_return: usize,
    /// The sign-in code instruction, while a sign-in is in progress.
    sign_in_code: Option<String>,
    /// The player's Freight Fate saves folder setting, as typed.
    ff_saves_folder: Option<PathBuf>,
    /// The usual places to look, before the env var and the setting.
    ff_saves_defaults: Vec<PathBuf>,
    /// The folder path being typed.
    typed: String,
    /// The record as last cued, so a delivery or a goal met since the
    /// last action gets its sound once.
    heard: Heard,
    /// Tips play the first time the player reaches something.
    tips_on: bool,
    /// Tips this key press reached, in the order they came up. The first
    /// one not heard yet plays; the rest wait for their next time.
    tip_due: Vec<Tip>,
    /// A screen opened during this key press.
    opened_now: bool,
    /// The cursor moved during this key press.
    cursor_moved: bool,
    /// The last key was Enter on "Hear all tips again"; a second forgets.
    forget_armed: bool,
    /// The save file's bytes as last written, so an action that changed
    /// nothing writes nothing.
    last_saved: Option<Vec<u8>>,
    /// The last save failed and the player has heard so; the next failure
    /// in a row isn't said again.
    save_failing: bool,
}

/// What the desk has already made a sound for.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Heard {
    delivered: u32,
    on_time: u32,
    goal_met: bool,
}

/// The carriers a new career can run, by Freight Fate start key, as a
/// dispatcher hears them: the terminal and the freight.
const CARRIERS: [(&str, &str, &str); 4] = [
    (
        "northstar",
        "Northstar Freight Lines, Chicago: mixed freight",
        "A balanced board out of Chicago, short runs and long ones.",
    ),
    (
        "great_lakes_training",
        "Great Lakes Training Transport, Milwaukee: short lanes with slack",
        "More short loads and a little extra time on appointments. The gentlest start.",
    ),
    (
        "prairie_link",
        "Prairie Link Regional, Kansas City: regional grain and bulk",
        "Same-region grain, farm and bulk lanes, and few long hauls.",
    ),
    (
        "summit_value",
        "Summit Value Logistics, Denver: long, high-value lanes",
        "Longer lanes and freight like electronics and auto parts, with a bigger on-time bonus for drivers.",
    ),
];

const DESK_HELP: [&str; 11] = [
    "Assign the day's loads to drivers. Assigning takes 10 minutes.",
    "Customers' loads waiting for yes or no. Open at the close is declined. Accepted loads for a coming day follow; Enter on one plans its driver tonight. The last row makes a sales call.",
    "Trucks on the road. Checking on one takes 2 minutes.",
    "Everyone on the roster and their hours. Enter on a driver finds them a load or sends them home.",
    "Today's applicants. An interview takes 15 minutes.",
    "Cash, what customers and brokers owe, Friday's settlement, the first's bills, factoring, and the trucks.",
    "Reputation tier and what it opens, the career score, dedicated accounts, the terminals, and milestones. The last row opens a second terminal.",
    "Hear the morning briefing again.",
    "The tips you have heard, to hear again or switch off.",
    "Let time run to the next call, arrival, or driver coming off a break, and stop there. Space anywhere at the desk says what needs you now.",
    "End the shift. Trips still rolling finish overnight.",
];

impl App {
    pub fn new(world: &'static World, dirs: Dirs, seed: Option<i64>, can_go_online: bool) -> App {
        let settings = load_settings(&dirs.data);
        let ff_saves_defaults = dirs.ff_saves.clone();
        let dirs = Dirs {
            ff_saves: profile_dirs(
                env_saves_dir(),
                settings.ff_saves_folder.as_deref(),
                dirs.ff_saves,
            ),
            ..dirs
        };
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
            return_index: Vec::new(),
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
            close_armed: false,
            closing: false,
            confirm_over: None,
            fresh: None,
            phone_return: 0,
            sign_in_code: None,
            ff_saves_folder: settings.ff_saves_folder,
            ff_saves_defaults,
            typed: String::new(),
            heard: Heard::default(),
            tips_on: settings.tips,
            tip_due: Vec::new(),
            opened_now: false,
            cursor_moved: false,
            forget_armed: false,
            last_saved: None,
            save_failing: false,
        }
    }

    /// Typed keys are text, not menu letters: the window feeds it
    /// TextInput characters with their case and symbols.
    pub fn wants_text(&self) -> bool {
        matches!(self.stack.last(), Some(Screen::SavesFolder))
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

    /// The current screen again, for coming back from Settings or Help.
    pub fn announce(&mut self) -> Vec<Out> {
        self.rebuild();
        self.menu.intro()
    }

    /// F1: help for the current item.
    pub fn help_line(&self) -> String {
        self.menu.help_text()
    }

    fn rebuild(&mut self) {
        let index = self.menu.index;
        let top = self.stack.last().cloned().unwrap_or(Screen::Start);
        self.menu = self.build(&top);
        self.menu.index = index.min(self.menu.items.len().saturating_sub(1));
    }

    fn open(&mut self, screen: Screen) -> Vec<Out> {
        self.opened_now = true;
        let desk_screen = match &screen {
            Screen::Tenders => Some("Tenders"),
            Screen::Trips => Some("Trips"),
            Screen::Roster => Some("Roster"),
            Screen::Hiring => Some("Hiring"),
            Screen::Money => Some("Money"),
            Screen::Company => Some("Company"),
            _ => None,
        };
        if let (Some(name), Some(g)) = (desk_screen, self.game.as_mut()) {
            g.tips.open(name);
        }
        let depth = self.stack.len();
        self.return_index.resize(depth, 0);
        self.return_index[depth - 1] = self.menu.index;
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
        self.menu.index = self
            .return_index
            .get(self.stack.len() - 1)
            .copied()
            .unwrap_or(0);
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
                        Item::new(
                            "Continue",
                            "Saved careers. Each picks up where you left the desk.",
                        ),
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
                        Item::new("Settings", "Speech, sound volumes, and the controller."),
                        Item::new("Soundtrack", "The music, once you have heard it at the desk."),
                        Item::new("Help", "The desk, the keys, and where files go."),
                        Item::new("Quit", ""),
                    ],
                )
            }
            Screen::NewCareer => Menu::new(
                "Pick a carrier",
                CARRIERS
                    .iter()
                    .map(|(key, label, help)| {
                        let share = pay_plan_for_key(Some(key)).pay_share * 100.0;
                        Item::new(
                            *label,
                            format!("{help} Drivers take {} percent of each load.", share.round()),
                        )
                    })
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
                            "No Freight Fate saves found. Dispatch looked in {}. Set the Freight Fate saves folder below.",
                            dirs.join(" and "),
                        ),
                        "",
                    ));
                }
                items.push(Item::new(
                    match &self.ff_saves_folder {
                        Some(dir) => format!("Freight Fate saves folder: {}", dir.display()),
                        None => "Freight Fate saves folder: not set".into(),
                    },
                    "Type the path to your Freight Fate saves. The profiles folder, the saves folder, the Freight Fate folder, or the folder that holds it all work.",
                ));
                if self.ff_saves_folder.is_some() {
                    items.push(Item::new(
                        "Clear the saves folder setting",
                        "Go back to looking in the usual places.",
                    ));
                }
                Menu::new("Import a Freight Fate driver", items)
            }
            Screen::SavesFolder => Menu::new(
                "Freight Fate saves folder. Type the folder path, then press Enter. Escape cancels.",
                vec![Item::new(
                    if self.typed.is_empty() {
                        "Empty".to_string()
                    } else {
                        self.typed.clone()
                    },
                    "For example C:\\ff-tester. Dispatch finds the profiles folder inside it.",
                )],
            ),
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
                    format!("Tenders, {} open", s.tenders.len()),
                    format!("Trips, {} rolling", g.trips.len()),
                    format!("Roster, {} drivers", g.drivers.len()),
                    format!("Hiring, {} applicants", s.applicants.len()),
                    format!("Money, {}", crate::money(g.funds)),
                    format!(
                        "Company, {}",
                        crate::growth::Tier::of(g.reputation)
                            .name()
                            .trim_start_matches("a ")
                            .trim_start_matches("an ")
                    ),
                    "Briefing".to_string(),
                    format!("Tips, {} heard", g.tips.heard_tips().len()),
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
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                Menu::new(
                    "Load board",
                    s.board
                        .iter()
                        .map(|l| {
                            let broker = match &l.source {
                                crate::customers::Source::Spot { broker } => g.broker_note(broker),
                                _ => String::new(),
                            };
                            Item::new(
                                g.load_text(l, w),
                                format!(
                                    "Enter picks a driver. From {}.{broker} {}",
                                    l.owner(g),
                                    l.terms.help()
                                )
                                .trim_end()
                                .to_string(),
                            )
                        })
                        .collect(),
                )
            }
            Screen::Drivers(load) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let job = s.board.get(*load).map(|l| &l.job);
                let title = match s.board.get(*load) {
                    Some(l) => format!("Who takes it? {}", g.load_text(l, w)),
                    None => "Who takes it?".into(),
                };
                let counter = s.board.get(*load).filter(|l| l.source.is_spot()).map(|l| {
                    if l.countered {
                        Item::new(
                            format!("{} has heard your number", l.owner(g)),
                            "One counter per load. The rate stands.",
                        )
                    } else {
                        Item::new(
                            format!("Counter the rate with {}", l.owner(g)),
                            "Five minutes on the phone, once per load. A broker has a ceiling they don't say, and comes closer to it the more they trust you. A no sometimes gives the load away.",
                        )
                    }
                });
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
                                        g.queue_refusal(s, *load, d.id, w).unwrap_or_else(|| {
                                            "Rolling. Enter queues this as their next load.".into()
                                        }),
                                    );
                                }
                                refusal_or_warning(j, d, w)
                            });
                            let note = job
                                .filter(|_| d.status == crate::roster::Status::Idle)
                                .and_then(|j| crate::board::short_note(j, d, w))
                                .unwrap_or("");
                            let asked = if job.is_some_and(|j| d.dislikes(j, w)) {
                                " Asked not to get this kind of run."
                            } else {
                                ""
                            };
                            Item::new(
                                format!("{}{note}{asked}", d.card_at(w, s.now(g.day), g.day)),
                                why.unwrap_or_else(|| "Legal for this load.".into()),
                            )
                        })
                        .chain(counter)
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
                        Item::new(
                            d.card_at(w, now, g.day),
                            format!(
                                "{}{} Enter for loads or home time.",
                                capitalized(&d.wants_text()),
                                d.week_text()
                            ),
                        )
                    })
                    .collect(),
            ),
            Screen::Hiring => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
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
                                a.line(g.day),
                            ),
                            Item::new(
                                format!("Hire {}, {}", a.driver.name, crate::money(a.signing)),
                                a.line(g.day),
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
                                format!(
                                    " From {}, {} empty miles.",
                                    w.spoken_city(&d.location, None),
                                    p.deadhead_mi.round()
                                )
                            } else {
                                " At the shipper's city.".into()
                            };
                            let note = crate::board::short_note(&l.job, d, w).unwrap_or("");
                            // A local driver sleeps at home: say how far from
                            // home each load leaves them.
                            let local = if d.pattern() == crate::roster::HomePattern::Local
                                && l.job.destination != g.terminal
                            {
                                let (m, _) = crate::trip::empty_leg(&l.job.destination, &g.terminal, w);
                                format!(" Ends {} miles from home.", m.round())
                            } else {
                                String::new()
                            };
                            let asked = if d.dislikes(&l.job, w) {
                                " Not the kind of run they asked for."
                            } else {
                                ""
                            };
                            Item::new(
                                format!("{}{empty}{local}{note}{asked}", g.load_text(l, w)),
                                refusal_or_warning(&l.job, d, w)
                                    .unwrap_or_else(|| "Legal for this driver.".into()),
                            )
                        })
                        .collect(),
                )
            }
            Screen::Driver(driver_id) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let Some(d) = g.driver(*driver_id) else {
                    return Menu::new("Roster", vec![]);
                };
                let (miles, _) = crate::trip::empty_leg(&d.location, &g.terminal, w);
                let home = if d.home_given() {
                    "Already has home time.".to_string()
                } else if miles >= 1.0 {
                    format!("{} empty miles from {}.", miles.round(), w.spoken_city(&d.location, None))
                } else {
                    "Already in town.".to_string()
                };
                Menu::new(
                    d.card_at(w, s.now(g.day), g.day),
                    vec![
                        Item::new(format!("Loads for {}", d.name), "The board, with empty miles from where they are."),
                        Item::new(
                            format!("Send {} home to {}", d.name, w.spoken_city(&g.terminal, None)),
                            format!("{home} Home time starts when they get there."),
                        ),
                        Item::new(
                            format!("{}'s record and traits", d.name),
                            "Morale, what you've learned about them, citations, inspections, CDL and medical card.",
                        ),
                    ],
                )
            }
            Screen::Tenders => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let mut items: Vec<Item> = s
                    .tenders
                    .iter()
                    .map(|t| Item::new(tender_line(t, g, w), "Enter to accept or decline."))
                    .collect();
                items.extend(g.coming_tenders().into_iter().map(|i| {
                    let t = &g.accepted[i];
                    let who = match t.planned_driver.and_then(|id| g.driver(id)) {
                        Some(d) => format!("{} planned", d.name),
                        None => "no driver planned".into(),
                    };
                    Item::new(
                        format!(
                            "{} Accepted for {}, {who}.",
                            tender_line(t, g, w),
                            g.tender_day_text(t.for_day)
                        ),
                        "Enter plans its driver tonight, 5 minutes. When the desk opens the plan confirms and the truck rolls with no desk time, or it falls through and the load is on the board.",
                    )
                }));
                let accounts = g
                    .customers
                    .iter()
                    .map(|c| format!("{}, {} a week", c.name, c.weekly_loads))
                    .collect::<Vec<_>>()
                    .join("; ");
                items.push(Item::new(
                    if s.sales_called {
                        "Sales call made today".to_string()
                    } else {
                        "Make a sales call, 30 minutes".to_string()
                    },
                    format!(
                        "Once a day. A new account depends on reputation. Accounts now: {accounts}."
                    ),
                ));
                Menu::new(
                    match s.tenders.len() {
                        0 => "Tenders. None open.".to_string(),
                        1 => "Tenders. 1 open.".to_string(),
                        n => format!("Tenders. {n} open."),
                    },
                    items,
                )
            }
            Screen::Tender(idx) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let title = match s.tenders.get(*idx) {
                    Some(t) => tender_line(t, g, w),
                    None => "That tender is gone.".into(),
                };
                Menu::new(
                    title,
                    vec![
                        Item::new("Accept", "The load leads the board on its day. Not covering it then counts against you."),
                        Item::new("Decline", "Counts against the week's acceptance. Two weeks under 80 percent cuts volume."),
                    ],
                )
            }
            Screen::PrePlan(coming) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let tender = g.coming_tenders().get(*coming).map(|&i| &g.accepted[i]);
                let title = match tender {
                    Some(t) => format!(
                        "Who takes it {} morning? {}",
                        g.tender_day_text(t.for_day),
                        tender_line(t, g, w)
                    ),
                    None => "That load is gone.".into(),
                };
                Menu::new(
                    title,
                    g.drivers
                        .iter()
                        .map(|d| {
                            let planned = tender.is_some_and(|t| t.planned_driver == Some(d.id));
                            let asked = if tender.is_some_and(|t| d.dislikes(&t.job, w)) {
                                " Asked not to get this kind of run."
                            } else {
                                ""
                            };
                            // When the truck they're on lands, so the plan
                            // isn't made blind.
                            let lands = g
                                .trips
                                .iter()
                                .find(|t| t.driver_id == d.id)
                                .map(|t| {
                                    format!(
                                        " Delivers in {} {}.",
                                        w.spoken_city(&t.job.destination, None),
                                        crate::desk::abs_text(t.eta(), g.day)
                                    )
                                })
                                .unwrap_or_default();
                            Item::new(
                                format!(
                                    "{}{}{lands}{asked}",
                                    if planned { "Planned. " } else { "" },
                                    d.card_at(w, s.now(g.day), g.day)
                                ),
                                "Enter plans them on this load. The plan holds if they're in, rested and legal when the desk opens.",
                            )
                        })
                        .collect(),
                )
            }
            Screen::Money => {
                let g = self.game.as_ref().unwrap();
                let mut items: Vec<Item> = g
                    .money_lines()
                    .into_iter()
                    .map(|l| Item::new(l, ""))
                    .collect();
                items.push(Item::new(
                    if g.factoring {
                        "Factoring: on, 3 percent for next-day pay"
                    } else {
                        "Factoring: off"
                    },
                    "Enter switches it. Factoring pays every load the next day for 3 percent; off, customers pay in 30 to 45 days and brokers in 30.",
                ));
                if !g.factoring {
                    items.push(Item::new(
                        if g.quick_pay {
                            "Broker quick pay: on, 3 percent for pay in 2 days"
                        } else {
                            "Broker quick pay: off"
                        },
                        "Enter switches it. Quick pay has brokers pay their loads in 2 days for 3 percent; customers still pay on their own terms.",
                    ));
                }
                items.push(Item::new(
                    format!("Trucks, {}", g.tractors.len()),
                    "Each truck's age, miles and note. Enter on one services it; the last row buys a used tractor.",
                ));
                Menu::new("Money", items)
            }
            Screen::Trucks => {
                let g = self.game.as_ref().unwrap();
                let mut items: Vec<Item> = g
                    .tractors
                    .iter()
                    .map(|t| {
                        Item::new(
                            t.text(g.day, g),
                            format!(
                                "Enter services it: {}, in the shop for the rest of today. Past {} miles since service, breakdowns triple.",
                                crate::money(crate::money::SERVICE_COST),
                                crate::money(crate::money::SERVICE_INTERVAL_MI).trim_start_matches('$')
                            ),
                        )
                    })
                    .collect();
                items.push(Item::new(
                    format!(
                        "Buy a used tractor: {} down, {} a month for 5 years",
                        crate::money(crate::money::TRUCK_DOWN),
                        crate::money(crate::money::TRUCK_NOTE)
                    ),
                    "A spare until a driver is hired for it. Hiring needs a spare truck.",
                ));
                Menu::new("Trucks", items)
            }
            Screen::Company => {
                let g = self.game.as_ref().unwrap();
                let mut items: Vec<Item> = g
                    .company_lines(w)
                    .into_iter()
                    .map(|l| Item::new(l, ""))
                    .collect();
                if g.terminal2.is_none() {
                    items.push(Item::new(
                        "Open a second terminal",
                        "Pick a city at the far end of your lanes. It takes an established reputation, 5 drivers and $10,000, then $4,000 a month in rent.",
                    ));
                }
                Menu::new("Company", items)
            }
            Screen::TerminalPick => {
                let g = self.game.as_ref().unwrap();
                Menu::new(
                    "Second terminal",
                    crate::growth::terminal_choices(g, w)
                        .into_iter()
                        .map(|(city, miles)| {
                            Item::new(
                                format!(
                                    "{}, {} miles from {}",
                                    w.spoken_city(&city, None),
                                    miles.round(),
                                    w.spoken_city(&g.terminal, None)
                                ),
                                "Enter opens the terminal here. It can't be moved later.",
                            )
                        })
                        .collect(),
                )
            }
            Screen::Tips => {
                let g = self.game.as_ref().unwrap();
                let heard = g.tips.heard_tips();
                let left = Tip::ALL.len().saturating_sub(heard.len());
                let title = if left == 0 {
                    format!("Tips. All {} heard.", heard.len())
                } else {
                    format!("Tips. {} heard, {left} still to find.", heard.len())
                };
                let mut items = vec![Item::new(
                    if self.tips_on { "Tips: on" } else { "Tips: off" },
                    "Tips play the first time you reach something.",
                )];
                items.extend(heard.iter().map(|t| Item::new(t.title(), t.text())));
                items.push(Item::new(
                    "Hear all tips again",
                    "Forgets the tips you have heard, so each plays again the next time you reach it.",
                ));
                Menu::new(title, items)
            }
            Screen::Phone(ring) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                Menu::new(
                    ring.title(g, s, w),
                    ring.labels(g, s, w)
                        .into_iter()
                        .zip(ring.helps(g, s, w))
                        .map(|(l, h)| Item::new(l, h))
                        .collect(),
                )
            }
        }
    }

    pub fn key(&mut self, key: Key) -> Vec<Out> {
        self.opened_now = false;
        self.cursor_moved = false;
        let mut out = self.run_fresh();
        out.extend(self.handle(key));
        self.screen_tips();
        self.play_tip(&mut out);
        out.extend(self.autosave());
        out
    }

    /// Save the career and today's desk if anything changed. Not while the
    /// phone is answered (the call already left the queue, and the last
    /// save still has it ringing), nor while a closed day waits for its
    /// next morning.
    fn autosave(&mut self) -> Vec<Out> {
        if self.fresh.is_some() || self.stack.iter().any(|s| matches!(s, Screen::Phone(_))) {
            return vec![];
        }
        match (&self.game, &self.shift) {
            (Some(g), Some(_)) if g.ended.is_none() => {}
            _ => return vec![],
        }
        let saved = self.store(true);
        self.save_said(saved)
    }

    /// Write the career, with today's desk when `with_desk`, unless the
    /// file already holds exactly that.
    fn store(&mut self, with_desk: bool) -> std::io::Result<()> {
        let Some(g) = self.game.as_ref() else {
            return Ok(());
        };
        let desk = self.shift.as_ref().filter(|_| with_desk);
        let bytes = save_bytes(g, desk)?;
        if self.last_saved.as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        write_save(&save_path(g, &self.dirs.careers), &bytes)?;
        self.last_saved = Some(bytes);
        Ok(())
    }

    /// A failed save is said once, until a save works again.
    fn save_said(&mut self, saved: std::io::Result<()>) -> Vec<Out> {
        match saved {
            Ok(()) => {
                self.save_failing = false;
                vec![]
            }
            Err(e) => {
                log::error!("save failed: {e}");
                if std::mem::replace(&mut self.save_failing, true) {
                    vec![]
                } else {
                    vec![Out::Queue(format!("The career could not be saved: {e}"))]
                }
            }
        }
    }

    fn settings(&self) -> Settings {
        Settings {
            online_services: self.online_enabled,
            ff_saves_folder: self.ff_saves_folder.clone(),
            tips: self.tips_on,
        }
    }

    /// The tips this key press reached by opening a screen or landing on
    /// a row.
    fn screen_tips(&mut self) {
        let (Some(g), Some(s)) = (self.game.as_ref(), self.shift.as_ref()) else {
            return;
        };
        let top = self.stack.last();
        if self.opened_now {
            let tip = match top {
                Some(Screen::Board) => Some(Tip::Board),
                Some(Screen::Tenders) if !s.tenders.is_empty() => Some(Tip::Tenders),
                Some(Screen::PrePlan(_)) => Some(Tip::PrePlan),
                Some(Screen::Drivers(_)) => Some(Tip::Who),
                Some(Screen::Roster) => Some(Tip::Roster),
                Some(Screen::Trips) if !g.trips.is_empty() => Some(Tip::Trips),
                Some(Screen::Hiring) => Some(Tip::Hiring),
                Some(Screen::Money) if g.factoring => Some(Tip::Money),
                Some(Screen::Trucks) => Some(Tip::Trucks),
                Some(Screen::Company) => Some(Tip::Company),
                Some(Screen::Phone(ring)) => phone_tip(ring),
                _ => None,
            };
            self.tip_due.extend(tip);
        }
        if !(self.opened_now || self.cursor_moved) {
            return;
        }
        let index = self.menu.index;
        if matches!(top, Some(Screen::Board))
            && s.board.get(index).is_some_and(|l| l.source.is_spot())
        {
            self.tip_due.push(Tip::Spot);
        }
        if matches!(
            top,
            Some(Screen::Drivers(_) | Screen::BoardFor(_) | Screen::PrePlan(_))
        ) {
            let label = self.menu.items.get(index).map_or("", |i| i.label.as_str());
            if label.contains(" Short on hours.") {
                self.tip_due.push(Tip::ShortHours);
            }
            if label.contains(" Asked not to get this kind of run.")
                || label.contains(" Not the kind of run they asked for.")
            {
                self.tip_due.push(Tip::Preference);
            }
        }
    }

    /// The first tip reached and not heard yet, after everything else,
    /// with its sound. The others wait for their next time.
    fn play_tip(&mut self, out: &mut Vec<Out>) {
        let due = std::mem::take(&mut self.tip_due);
        if !self.tips_on {
            return;
        }
        let Some(g) = self.game.as_mut() else {
            return;
        };
        if let Some(tip) = due.into_iter().find(|t| !g.tips.has_heard(*t)) {
            g.tips.hear(tip);
            out.push(Out::Cue(Cue::Notify));
            out.push(Out::Queue(tip.text().into()));
            // The desk's Tips row counts what's heard.
            self.rebuild();
        }
    }

    fn handle(&mut self, key: Key) -> Vec<Out> {
        if self.wants_text() {
            return self.type_key(key);
        }
        let armed = std::mem::take(&mut self.leave_armed);
        let forget_armed = std::mem::take(&mut self.forget_armed);
        let close_armed = self.close_armed;
        let confirm_over = self.confirm_over.take();
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
        if key == Key::Space {
            if let (Some(g), Some(s)) = (self.game.as_ref(), self.shift.as_ref()) {
                if on_phone {
                    return vec![Out::Say("The caller is waiting; pick an answer.".into())];
                }
                let line = g.needs_now(s, self.world);
                let first_week = (self.tips_on && line.starts_with("Nothing needs you"))
                    .then(|| crate::tips::unopened_line(&g.tips, g.day))
                    .flatten();
                let mut out = vec![Out::Say(line)];
                out.extend(first_week.map(Out::Queue));
                return out;
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
        let activate = matches!(&event, MenuEvent::Activate(_));
        let back = matches!(&event, MenuEvent::Back);
        self.cursor_moved = cursor;
        if (activate || back) && !on_phone {
            self.closing = false; // other desk work cancels a pending close
        }
        let mut out = match event {
            MenuEvent::Speak(text) => vec![Out::Say(text)],
            MenuEvent::Back if on_phone && !self.menu.items.is_empty() => {
                vec![Out::Say("Pick an answer; the caller is waiting.".into())]
            }
            MenuEvent::Back if self.stack.len() <= 1 && !armed => {
                self.leave_armed = true;
                vec![Out::Say("Press Escape again to quit.".into())]
            }
            MenuEvent::Back if on_desk => match self.store(true) {
                Err(e) if !armed => {
                    log::error!("save failed: {e}");
                    self.save_failing = true;
                    self.leave_armed = true;
                    vec![Out::Say(format!(
                        "The career could not be saved: {e} Press Escape again to leave anyway."
                    ))]
                }
                _ => {
                    self.save_failing = false;
                    self.shift = None;
                    self.game = None;
                    self.back()
                }
            },
            MenuEvent::Back => self.back(),
            MenuEvent::Activate(i) if matches!(self.stack.last(), Some(Screen::Tips)) => {
                self.activate_tips(i, forget_armed)
            }
            MenuEvent::Activate(i) => self.activate(i, close_armed, confirm_over),
            MenuEvent::None => vec![],
        };
        if cursor {
            out.insert(0, Out::Cue(Cue::Cursor));
        } else if activate {
            out.insert(0, Out::Cue(Cue::Activate));
        } else if back {
            out.insert(0, Out::Cue(Cue::Back));
        }
        out.extend(self.announce_ring());
        out
    }

    /// A key on the saves folder screen: letters type, Backspace deletes,
    /// arrows read back, Enter checks the folder.
    fn type_key(&mut self, key: Key) -> Vec<Out> {
        let typed = |s: &str| {
            if s.is_empty() {
                "Empty.".to_string()
            } else {
                s.to_string()
            }
        };
        match key {
            Key::Char(c) => {
                self.typed.push(c);
                self.rebuild();
                vec![Out::Cue(Cue::Type), Out::Say(spoken_char(c))]
            }
            Key::Backspace => match self.typed.pop() {
                Some(c) => {
                    self.rebuild();
                    vec![Out::Cue(Cue::Back), Out::Say(spoken_char(c))]
                }
                None => vec![Out::Say("Empty.".into())],
            },
            Key::Escape => {
                self.typed.clear();
                self.back()
            }
            Key::F1 => vec![Out::Say(self.menu.items[0].help.clone())],
            Key::Enter => {
                let dir = clean_typed_path(&self.typed);
                if self.typed.trim().is_empty() {
                    return vec![
                        Out::Cue(Cue::Error),
                        Out::Say("Type a folder path first.".into()),
                    ];
                }
                match check_saves_folder(&dir) {
                    Ok((found, n)) => {
                        self.typed.clear();
                        self.ff_saves_folder = Some(dir);
                        self.stack.pop();
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.saves_folder_changed(found_line(&found, n)));
                        out
                    }
                    Err(why) => vec![Out::Cue(Cue::Error), Out::Say(why)],
                }
            }
            _ => vec![Out::Say(typed(&self.typed))],
        }
    }

    /// Save the folder setting, look again, and land on the import list.
    fn saves_folder_changed(&mut self, line: String) -> Vec<Out> {
        self.dirs.ff_saves = profile_dirs(
            env_saves_dir(),
            self.ff_saves_folder.as_deref(),
            self.ff_saves_defaults.clone(),
        );
        self.menu.index = 0;
        self.rebuild();
        let mut out = vec![Out::Say(line)];
        if let Some(env) = env_saves_dir() {
            out.push(Out::Queue(format!(
                "{} is set, so Dispatch uses {} until it's cleared.",
                crate::import::FF_SAVES_ENV,
                env.display()
            )));
        }
        if let Err(error) = save_settings(&self.settings(), &self.dirs.data) {
            log::error!("settings save failed: {error}");
            out.push(Out::Queue(format!(
                "The saves folder could not be saved: {error}"
            )));
        }
        out.push(Out::Queue(self.menu.current_text()));
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
            self.tip_due.push(Tip::Phone);
            return vec![
                Out::Cue(Cue::PhoneRing),
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
                let mut out = vec![Out::Cue(Cue::PhonePickup)];
                out.extend(ring_cue(&ring).map(Out::Cue));
                out.extend(self.open(Screen::Phone(ring)));
                out
            }
            Err(crate::career::ActError::NoSuchItem) => {
                self.announced = 0;
                vec![Out::Say("The call dropped.".into())]
            }
            Err(e) => vec![Out::Say(e.spoken())],
        }
    }

    /// Say lines after an action, then refresh the screen underneath. A
    /// delivery or a goal met along the way gets its sound first.
    fn after(&mut self, lines: Vec<String>) -> Vec<Out> {
        self.rebuild();
        let mut out: Vec<Out> = self.heard_since().into_iter().map(Out::Cue).collect();
        out.extend(lead(lines.into_iter().map(Out::Queue).collect()));
        out
    }

    /// The sounds for what the record shows since the last look: a load
    /// delivered on time or late, and the daily goal met.
    fn heard_since(&mut self) -> Vec<Cue> {
        let (Some(g), Some(s)) = (self.game.as_ref(), self.shift.as_ref()) else {
            return vec![];
        };
        let mut cues = vec![];
        if g.record.delivered > self.heard.delivered {
            if g.record.on_time > self.heard.on_time {
                cues.push(Cue::Delivered);
            } else {
                cues.push(Cue::Late);
                self.tip_due.push(Tip::Late);
            }
        }
        if s.goal_met_said && !self.heard.goal_met {
            cues.push(Cue::Goal);
        }
        self.heard = Heard {
            delivered: g.record.delivered,
            on_time: g.record.on_time,
            goal_met: s.goal_met_said,
        };
        cues
    }

    /// The desk clock, in minutes since midnight, while a career is open.
    pub fn desk_minute(&self) -> Option<u32> {
        self.shift.as_ref().map(|s| s.clock.minute)
    }

    /// `after`, then say where the focus landed.
    fn landed(&mut self, lines: Vec<String>) -> Vec<Out> {
        let mut out = self.after(lines);
        out.push(Out::Queue(self.menu.current_text()));
        out
    }

    fn activate(
        &mut self,
        i: usize,
        close_armed: bool,
        confirm_over: Option<(usize, u32)>,
    ) -> Vec<Out> {
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
                6 => vec![Out::OpenSettings],
                7 => vec![Out::OpenSoundtrack],
                8 => vec![Out::OpenHelp],
                _ => vec![Out::Quit],
            },
            Screen::NewCareer => {
                let seed = self.seed.unwrap_or_else(now_seed);
                let game = Game::new(CARRIERS[i].0, seed, w);
                self.fresh = Some(Fresh::Begin(Box::new(game)));
                vec![Out::Say("Setting up the desk.".into())]
            }
            Screen::Continue => match list_slots(&self.dirs.careers).into_iter().nth(i) {
                Some(Slot::Ok {
                    game,
                    desk: Some(shift),
                    ..
                }) if game.ended.is_none() => self.resume(game, shift),
                Some(Slot::Ok { game, .. }) => self.begin(game, false),
                Some(slot) => vec![Out::Say(slot.label())],
                None => vec![],
            },
            Screen::Import => {
                let saves = list_ff_saves(&self.dirs.ff_saves);
                let setting_at = saves.len().max(1);
                if i == setting_at {
                    self.typed = self
                        .ff_saves_folder
                        .as_ref()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default();
                    return self.open(Screen::SavesFolder);
                }
                if i == setting_at + 1 {
                    self.ff_saves_folder = None;
                    return self.saves_folder_changed(
                        "The saves folder setting is cleared. Dispatch looks in the usual places."
                            .into(),
                    );
                }
                let Some(path) = saves.into_iter().nth(i) else {
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
            // Typing and the Tips screen take Enter before it gets here.
            Screen::SavesFolder | Screen::Tips => vec![],
            Screen::OnlineBoard => vec![Out::Say(self.menu.items[i].label.clone())],
            Screen::Desk => match i {
                0 => self.open(Screen::Board),
                1 => self.open(Screen::Tenders),
                2 => self.open(Screen::Trips),
                3 => self.open(Screen::Roster),
                4 => self.open(Screen::Hiring),
                5 => self.open(Screen::Money),
                6 => self.open(Screen::Company),
                7 => {
                    let lines = self
                        .game
                        .as_ref()
                        .unwrap()
                        .briefing(self.shift.as_ref().unwrap(), w);
                    self.after(lines)
                }
                8 => self.open(Screen::Tips),
                9 => {
                    let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                    if !s.pending.is_empty() {
                        return vec![Out::Say(crate::career::ActError::PhoneRinging.spoken())];
                    }
                    let mut lines = g.advance(s, w);
                    if lines.len() == 1 && s.pending.is_empty() {
                        lines.push(g.needs_now(s, w));
                    }
                    let mut out = vec![Out::Cue(Cue::Clock)];
                    out.extend(self.after(lines));
                    out
                }
                _ => self.close_day(close_armed),
            },
            Screen::Board => self.open(Screen::Drivers(i)),
            Screen::Drivers(load) if i >= self.game.as_ref().unwrap().drivers.len() => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                match g.counter_rate(s, load, w) {
                    Ok(lines) => {
                        let gone = s.board.len() <= load;
                        if gone {
                            self.stack.pop();
                        }
                        let mut out = vec![Out::Cue(Cue::Activate)];
                        out.extend(if gone {
                            self.landed(lines)
                        } else {
                            self.after(lines)
                        });
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::Drivers(load) => {
                let id = self.game.as_ref().unwrap().drivers[i].id;
                if let Some(out) = self.over_hours_check(load, id, confirm_over) {
                    return out;
                }
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                match g.assign(s, load, id, w) {
                    Ok(lines) => {
                        // Back to the board, on the next load; to the desk
                        // once the board is empty.
                        self.stack.pop();
                        let left = self.shift.as_ref().map_or(0, |s| s.board.len());
                        if left == 0 {
                            self.stack.pop();
                            self.menu.index = 0;
                        } else {
                            self.menu.index = load.min(left - 1);
                        }
                        self.tip_due.push(Tip::Assign);
                        let mut out = vec![Out::Cue(Cue::Radio)];
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
                    Ok(line) => {
                        let mut out = vec![Out::Cue(Cue::TruckIdle)];
                        out.extend(self.after(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Say(e.spoken())],
                }
            }
            Screen::Roster => {
                let id = self.game.as_ref().unwrap().drivers[i].id;
                self.open(Screen::Driver(id))
            }
            Screen::Driver(driver_id) => {
                if i == 0 {
                    self.stack.pop();
                    return self.open(Screen::BoardFor(driver_id));
                }
                if i == 2 {
                    let g = self.game.as_ref().unwrap();
                    let Some(d) = g.driver(driver_id) else {
                        return vec![];
                    };
                    let card_days =
                        crate::law::MEDICAL_CARD_DAYS - (g.day - d.record.medical_card_day);
                    let lines = vec![
                        format!(
                            "{}: morale {}, goodwill {}. {}{}",
                            d.name,
                            d.morale(),
                            d.goodwill,
                            capitalized(&d.wants_text()),
                            d.week_text()
                        ),
                        d.traits_text(),
                        d.record.text(g.day),
                        format!("Medical card good for {card_days} more days."),
                    ];
                    return self.after(lines);
                }
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                match g.send_home(s, driver_id, w) {
                    Ok(line) => {
                        self.stack.pop();
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.after(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::Hiring => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let hiring = !i.is_multiple_of(2);
                let r = if hiring {
                    g.hire(s, i / 2)
                } else {
                    g.interview(s, i / 2)
                };
                match r {
                    Ok(line) => {
                        let mut out = vec![Out::Cue(if hiring { Cue::Hire } else { Cue::Success })];
                        out.extend(self.after(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::Tenders => {
                let s = self.shift.as_ref().unwrap();
                if i < s.tenders.len() {
                    return self.open(Screen::Tender(i));
                }
                let coming = i - s.tenders.len();
                if coming < self.game.as_ref().unwrap().coming_tenders().len() {
                    return self.open(Screen::PrePlan(coming));
                }
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                match g.sales_call(s, w) {
                    Ok(line) => {
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.landed(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::Money => {
                let g = self.game.as_mut().unwrap();
                let n = g.money_lines().len();
                if i == n {
                    let line = g.toggle_factoring();
                    return self.landed(vec![line]);
                }
                if i == n + 1 && !g.factoring {
                    let line = g.toggle_quick_pay();
                    return self.landed(vec![line]);
                }
                if i == self.menu.items.len() - 1 {
                    return self.open(Screen::Trucks);
                }
                vec![Out::Say(self.menu.items[i].label.clone())]
            }
            Screen::Company => {
                let g = self.game.as_ref().unwrap();
                if g.terminal2.is_none() && i == self.menu.items.len() - 1 {
                    return self.open(Screen::TerminalPick);
                }
                vec![Out::Say(self.menu.items[i].label.clone())]
            }
            Screen::TerminalPick => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let Some((city, _)) = crate::growth::terminal_choices(g, w).into_iter().nth(i)
                else {
                    return vec![Out::Say(crate::career::ActError::NoSuchItem.spoken())];
                };
                match g.open_terminal(s, &city, w) {
                    Ok(line) => {
                        self.stack.pop();
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.landed(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::Trucks => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let r = match g.tractors.get(i) {
                    // Servicing costs money and a day off the road; ask first.
                    Some(t) if confirm_over != Some((usize::MAX, t.id)) && !t.in_shop(g.day) => {
                        let since = t.since_service();
                        let soon = if since < crate::money::SERVICE_INTERVAL_MI / 4.0 {
                            format!(
                                " It's only {} miles since its last service.",
                                crate::money(since).trim_start_matches('$')
                            )
                        } else {
                            String::new()
                        };
                        self.confirm_over = Some((usize::MAX, t.id));
                        return vec![Out::Say(format!(
                            "Service truck {} for {}? It's in the shop for the rest of today.{soon} Press Enter again to service it.",
                            t.id,
                            crate::money(crate::money::SERVICE_COST)
                        ))];
                    }
                    Some(t) => {
                        let id = t.id;
                        g.service_truck(s, id)
                    }
                    None => g.buy_truck(s),
                };
                match r {
                    Ok(line) => {
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.landed(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::PrePlan(coming) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let Some(id) = g.drivers.get(i).map(|d| d.id) else {
                    return vec![];
                };
                match g.preplan(s, coming, id, w) {
                    Ok(line) => {
                        self.stack.pop();
                        self.menu.index = self
                            .return_index
                            .get(self.stack.len() - 1)
                            .copied()
                            .unwrap_or(0);
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.landed(vec![line]));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
            Screen::Tender(idx) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let r = if i == 0 {
                    g.accept_tender(s, idx, w)
                } else {
                    g.decline_tender(s, idx)
                };
                match r {
                    Ok(line) => {
                        self.stack.pop();
                        self.menu.index = idx.min(self.shift.as_ref().unwrap().tenders.len());
                        let mut out = vec![Out::Cue(Cue::Success)];
                        out.extend(self.landed(vec![line]));
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
                let mut out = vec![Out::Cue(Cue::PhoneHangup)];
                if let (Ring::Call(crate::phone::Call::Empty { driver_id }), 0) = (&ring, i) {
                    self.closing = false;
                    out.extend(self.open(Screen::BoardFor(*driver_id)));
                    return out;
                }
                if let (Ring::Call(crate::phone::Call::Tenders), 1) = (&ring, i) {
                    self.closing = false;
                    out.extend(lead(lines.into_iter().map(Out::Queue).collect()));
                    out.extend(queued(self.open(Screen::Tenders)));
                    return out;
                }
                if self.closing {
                    out.extend(self.after(lines));
                    // Another call already ringing is announced by `handle`.
                    if self.shift.as_ref().unwrap().pending.is_empty() {
                        out.extend(queued(self.close_day(true)));
                    }
                    return out;
                }
                out.extend(self.landed(lines));
                out
            }
            Screen::BoardFor(driver_id) => {
                if let Some(out) = self.over_hours_check(i, driver_id, confirm_over) {
                    return out;
                }
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                match g.assign(s, i, driver_id, w) {
                    Ok(lines) => {
                        self.stack.pop();
                        self.menu.index = self
                            .return_index
                            .get(self.stack.len() - 1)
                            .copied()
                            .unwrap_or(0);
                        self.tip_due.push(Tip::Assign);
                        let mut out = vec![Out::Cue(Cue::Radio)];
                        out.extend(self.landed(lines));
                        out
                    }
                    Err(e) => vec![Out::Cue(Cue::Error), Out::Say(e.spoken())],
                }
            }
        }
    }

    /// Enter on the Tips screen: the switch, a tip heard, or forgetting them.
    fn activate_tips(&mut self, i: usize, forget_armed: bool) -> Vec<Out> {
        let last = self.menu.items.len().saturating_sub(1);
        if i == 0 {
            self.tips_on = !self.tips_on;
            let mut out = vec![Out::Say(
                if self.tips_on {
                    "Tips on."
                } else {
                    "Tips off."
                }
                .into(),
            )];
            if let Err(error) = save_settings(&self.settings(), &self.dirs.data) {
                log::error!("settings save failed: {error}");
                out.push(Out::Queue(format!(
                    "The tips setting could not be saved: {error}"
                )));
            }
            self.rebuild();
            return out;
        }
        if i < last {
            return vec![Out::Say(self.menu.items[i].help.clone())];
        }
        let Some(g) = self.game.as_mut() else {
            return vec![];
        };
        let n = g.tips.heard_tips().len();
        if n == 0 {
            return vec![Out::Say("No tips heard yet.".into())];
        }
        if !forget_armed {
            self.forget_armed = true;
            return vec![Out::Say(match n {
                1 => "Forget the tip you have heard? Enter forgets it, Escape keeps it.".into(),
                n => format!(
                    "Forget the {n} tips you have heard? Enter forgets them, Escape keeps them."
                ),
            })];
        }
        g.tips.forget();
        self.rebuild();
        self.menu.index = self.menu.items.len() - 1;
        vec![
            Out::Cue(Cue::Success),
            Out::Say("Tips forgotten. Each plays again the next time you reach it.".into()),
            Out::Queue(self.menu.current_text()),
        ]
    }

    /// Enter on a driver who can't make the appointment legally asks first:
    /// the warning, then a second Enter sends them. `None` means go ahead.
    fn over_hours_check(
        &mut self,
        load: usize,
        driver_id: u32,
        confirmed: Option<(usize, u32)>,
    ) -> Option<Vec<Out>> {
        let (g, s) = (self.game.as_ref()?, self.shift.as_ref()?);
        let (l, d) = (s.board.get(load)?, g.driver(driver_id)?);
        if crate::board::refusal(&l.job, d, self.world).is_some() {
            return None;
        }
        let warning = crate::board::hours_warning(&l.job, d, self.world)?;
        if confirmed == Some((load, driver_id)) {
            return None;
        }
        self.confirm_over = Some((load, driver_id));
        Some(vec![
            Out::Cue(Cue::Error),
            Out::Say(format!("{warning} Press Enter again to send them anyway.")),
        ])
    }

    /// Start a career day: weather (fresh days only), save, brief, desk.
    fn begin(&mut self, mut game: Game, fresh_day: bool) -> Vec<Out> {
        game.normalize(self.world);
        self.game = Some(game);
        self.last_saved = None;
        self.stack.truncate(1);
        self.stack.push(Screen::Desk);
        self.menu.index = 0;
        let mut out = self.morning(fresh_day);
        if fresh_day && self.game.as_ref().is_some_and(|g| g.day == 1) {
            self.tip_due.push(Tip::Desk);
        }
        self.rebuild();
        out.extend(queued(self.menu.intro()));
        out
    }

    /// Pick a saved day up at the desk where it was left: no morning, no
    /// pre-plans, no briefing. A call still ringing is announced after.
    fn resume(&mut self, mut game: Game, shift: Shift) -> Vec<Out> {
        // Fills in what older saves lack; a save with a desk has it all,
        // so the day in progress is left as it was.
        game.normalize(self.world);
        let line = format!(
            "{}, day {}, {}.",
            game.name,
            game.day,
            crate::roster::WEEKDAYS[crate::roster::weekday(game.day) as usize]
        );
        // Deliveries and a goal met before the save don't sound again.
        self.heard = Heard {
            delivered: game.record.delivered,
            on_time: game.record.on_time,
            goal_met: shift.goal_met_said,
        };
        self.game = Some(game);
        self.shift = Some(shift);
        self.last_saved = None;
        self.announced = 0;
        self.close_armed = false;
        self.closing = false;
        self.confirm_over = None;
        self.leave_armed = false;
        self.forget_armed = false;
        self.phone_return = 0;
        self.stack.truncate(1);
        self.stack.push(Screen::Desk);
        self.menu.index = 0;
        self.rebuild();
        let mut out = vec![Out::Say(line)];
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
        if let Some(ending) = &game.ended {
            let line = format!(
                "{} is finished after {} days: {}. The career stays saved to look back on.",
                game.name,
                game.day - 1,
                ending.text()
            );
            // A finished career keeps no desk.
            let saved = self.store(false);
            out.extend(self.save_said(saved));
            self.game = None;
            self.shift = None;
            self.stack.truncate(1);
            self.menu.index = 0;
            self.rebuild();
            out.push(Out::Say(line));
            out.extend(queued(self.menu.intro()));
            return out;
        }
        let mut shift = game.morning(w);
        self.close_armed = false;
        self.closing = false;
        // Last night's plans roll or fall through before the briefing
        // counts who is free.
        let plans = game.confirm_preplans(&mut shift, w);
        let mut lines = game.briefing(&shift, w);
        if crate::money::is_bill_day(game.day) {
            self.tip_due.push(Tip::Bills);
        }
        match crate::roster::weekday(game.day) {
            4 => self.tip_due.push(Tip::Settlement),
            5 => self.tip_due.push(Tip::Weekend),
            6 => self.tip_due.push(Tip::Scorecard),
            _ => {}
        }
        if game.day > 1 && game.drivers.iter().any(|d| game.due_home(d)) {
            self.tip_due.push(Tip::Home);
        }
        let at = lines.len().min(2);
        lines.splice(at..at, plans);
        self.heard = Heard {
            delivered: game.record.delivered,
            on_time: game.record.on_time,
            goal_met: shift.goal_met_said,
        };
        self.shift = Some(shift);
        self.announced = 0;
        // The new morning saves with its desk, after the pre-plans, so a
        // reload lands here without running them again.
        let saved = self.store(true);
        out.extend(self.save_said(saved));
        out.extend(lines.into_iter().map(Out::Queue));
        lead(out)
    }

    fn close_day(&mut self, warned: bool) -> Vec<Out> {
        let w = self.world;
        let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
        g.drop_dead_rings(s);
        if !s.pending.is_empty() {
            return vec![Out::Say(crate::career::ActError::PhoneRinging.spoken())];
        }
        if !warned && !s.clock.closed() {
            if let Some(line) = g.left_open(s, w) {
                self.close_armed = true;
                return vec![Out::Cue(Cue::Error), Out::Say(line)];
            }
        }
        self.closing = true;
        // Closing runs the rest of the day in order by itself: each event
        // before 6 PM is heard, and it stops only when the phone rings.
        let end = crate::desk::abs_min(g.day, crate::desk::SHIFT_END);
        let mut collected: Vec<String> = vec![];
        while !s.clock.closed() && g.next_event_at(s).is_some_and(|t| t <= end) {
            let mut lines = g.advance(s, w);
            if lines.len() < 2 && s.pending.is_empty() {
                continue;
            }
            if collected.is_empty() {
                lines[0] = format!("Before you close, it's {}", lines[0]);
            }
            collected.extend(lines);
            if !s.pending.is_empty() {
                let mut out = self.after(collected);
                out.extend(self.announce_ring());
                return out;
            }
        }
        let had_news = !collected.is_empty();
        let mut out = if had_news {
            self.after(collected)
        } else {
            vec![]
        };
        let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
        s.clock.minute = crate::desk::SHIFT_END;
        g.ring_due(s);
        if !s.pending.is_empty() {
            self.announced = s.pending.len(); // this line is the announcement
            self.tip_due.push(Tip::Phone);
            self.rebuild();
            let line = "Before you close: the phone is ringing. Press Enter to answer.";
            out.push(if had_news {
                Out::Queue(line.into())
            } else {
                Out::Say(line.into())
            });
            return out;
        }
        let closing_line = "Closing the desk.".to_string();
        out.extend([
            Out::Cue(Cue::DayClose),
            if had_news {
                Out::Queue(closing_line)
            } else {
                Out::Say(closing_line)
            },
        ]);
        let roster = g.drivers.len();
        let at = out.len() - 1;
        out.extend(g.close_day(s, w).into_iter().map(Out::Queue));
        if g.drivers.len() < roster {
            out.insert(at, Out::Cue(Cue::Door));
        }
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
        self.play_tip(&mut out);
        // Only a morning or a tip changes the career between keys.
        if !out.is_empty() {
            out.extend(self.autosave());
        }
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

/// The tip for the first call of its kind.
fn phone_tip(ring: &Ring) -> Option<Tip> {
    use crate::incidents::Kind;
    use crate::phone::Call;
    Some(match ring {
        Ring::Incident(incident) => match incident.kind {
            Kind::HoursShort => Tip::OutOfHours,
            Kind::Breakdown => Tip::Breakdown,
            Kind::Detention => Tip::Detention,
            Kind::Inspection { .. } => Tip::Inspection,
            Kind::Speeding => Tip::Speeding,
            Kind::Weather { .. } => return None,
        },
        Ring::Call(call) => match call {
            Call::Sick { .. } | Call::PersonalDay { .. } => Tip::Sick,
            Call::HotLoad { .. } => Tip::HotLoad,
            Call::Empty { .. } => Tip::Empty,
            Call::QuitWarning { .. } => Tip::Notice,
            Call::Tenders => Tip::TenderCall,
            Call::Dedicated { .. } => Tip::Dedicated,
        },
    })
}

/// The sound of what is on the line, after the handset is picked up: the
/// road for an incident, nothing extra for a driver or a broker calling.
fn ring_cue(ring: &Ring) -> Option<Cue> {
    use crate::incidents::Kind;
    match ring {
        Ring::Incident(incident) => Some(match incident.kind {
            Kind::HoursShort => Cue::Hours,
            Kind::Breakdown => Cue::Breakdown,
            Kind::Detention => Cue::Dock,
            Kind::Weather { .. } => Cue::Weather,
            Kind::Inspection { .. } | Kind::Speeding => Cue::Inspection,
        }),
        Ring::Call(_) => None,
    }
}

/// A sentence that starts mid-thought, as a line of its own.
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
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

/// A typed character as a screen reader would echo it.
fn spoken_char(c: char) -> String {
    match c {
        ' ' => "space".into(),
        '\\' => "backslash".into(),
        '/' => "slash".into(),
        ':' => "colon".into(),
        '.' => "dot".into(),
        '-' => "dash".into(),
        '_' => "underscore".into(),
        '"' => "quote".into(),
        c if c.is_uppercase() => format!("cap {}", c.to_lowercase()),
        c => c.to_string(),
    }
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
        let mut a = App::new(get_world(), dirs, Some(42), false);
        // Most tests hear the desk without tips; the tips tests turn them on.
        a.tips_on = false;
        (a, dir)
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
        assert!(
            out.contains("Close the day again to close anyway."),
            "open tenders warn first: {out}"
        );
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("Closing the desk."), "{out}");
        assert!(
            !out.contains("Day 2"),
            "the morning waits for the next tick"
        );
        let out = said(&a.tick());
        assert!(out.contains("Day 2"), "{out}");
        assert!(out.ends_with("Load board, 8 loads, 1 of 11."), "{out}");
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
            out.contains("| Tenders, ") && out.ends_with(" open, 2 of 11."),
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
    fn sending_a_driver_short_on_hours_asks_first() {
        let (mut a, _d) = on_desk();
        {
            let s = a.shift.as_mut().unwrap();
            s.schedule = Default::default();
            s.pending.clear();
            for l in &mut s.board {
                l.job.deadline_game_h = 0.5;
            }
        }
        a.key(Key::Enter); // Load board
        let picker = said(&a.key(Key::Enter)); // first load
        assert!(picker.contains("Short on hours"), "{picker}");
        let out = said(&a.key(Key::Enter)); // first driver
        assert!(
            out.contains("Press Enter again to send them anyway."),
            "{out}"
        );
        assert!(a.game.as_ref().unwrap().trips.is_empty());
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("past the clock"), "{out}");
        assert_eq!(a.game.as_ref().unwrap().trips.len(), 1);
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
        let out = said(&a.close_day(false));
        assert!(out.contains("The phone is ringing"), "{out}");
        assert_eq!(a.game.as_ref().unwrap().day, 1);
    }

    #[test]
    fn the_driver_picker_names_its_load() {
        let (mut a, _d) = on_desk();
        a.key(Key::Enter); // Load board
        let out = said(&a.key(Key::Enter)); // first load
        let (g, s) = (a.game.as_ref().unwrap(), a.shift.as_ref().unwrap());
        let line = g.load_text(&s.board[0], get_world());
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
        a.close_armed = true; // past the open-tenders warning
        a.key(Key::End); // Close the day
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("phone is ringing"), "{out}");
        assert_eq!(a.game.as_ref().unwrap().day, 1);
        let out = said(&a.key(Key::Enter)); // answer
        assert!(out.contains("not ready to load"), "{out}");
        // Wait it out: the close carries on by itself, no second Close,
        // stopping only for the next call.
        let mut out = said(&a.key(Key::Enter));
        for _ in 0..20 {
            if out.contains("Closing the desk.") {
                break;
            }
            assert!(out.contains("phone is ringing"), "{out}");
            a.key(Key::Enter); // answer
            a.key(Key::End);
            out = said(&a.key(Key::Enter));
        }
        assert!(out.contains("Closing the desk."), "{out}");
        assert!(
            !out.contains("Close the day,"),
            "the desk focus is not read: {out}"
        );
        let out = said(&a.tick());
        assert!(out.contains("Day 2 at"), "{out}");
    }

    #[test]
    fn closing_carries_on_after_each_call_until_the_day_saves() {
        let (mut a, d) = on_desk();
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        let (load, driver) = crate::career::tests::legal_pair(g, s);
        g.assign(s, load, driver, get_world()).unwrap();
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
        a.key(Key::End); // Close the day, the only press on it
        let mut out = said(&a.key(Key::Enter));
        if out.contains("Close the day again") {
            out = said(&a.key(Key::Enter));
        }
        let mut answered = 0;
        let mut seen = out;
        for _ in 0..40 {
            if a.game.as_ref().unwrap().day == 2 {
                break;
            }
            if matches!(a.stack.last(), Some(Screen::Phone(_))) {
                let last = a.menu.items.len() - 1;
                a.key(Key::Home);
                for _ in 0..last {
                    a.key(Key::Down);
                }
                let o = a.key(Key::Enter);
                assert!(
                    matches!(a.stack.last(), Some(Screen::Desk | Screen::Phone(_))),
                    "the last answer opens no screen: {}",
                    said(&o)
                );
                answered += 1;
                seen.push_str(&said(&o));
            } else if !a.shift.as_ref().unwrap().pending.is_empty() {
                a.key(Key::Enter); // answer the ringing phone
            }
            seen.push_str(&said(&a.tick()));
        }
        assert!(answered >= 1, "a call rang along the way: {seen}");
        assert_eq!(a.game.as_ref().unwrap().day, 2, "{seen}");
        let saved = list_slots(&d.path().join("careers"));
        assert_eq!(saved.len(), 1);
        assert!(
            matches!(&saved[0], Slot::Ok { game, .. } if game.day == 2),
            "the new morning saved"
        );
    }

    #[test]
    fn closing_with_only_plain_events_runs_to_the_close_in_one_press() {
        let (mut a, _d) = on_desk();
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        let w = get_world();
        for _ in 0..2 {
            let (load, driver) = crate::career::tests::legal_pair(g, s);
            g.assign(s, load, driver, w).unwrap();
        }
        s.schedule = Default::default();
        s.pending.clear();
        // Both arrive just before 6 PM, so no driver is free to call about
        // an empty truck afterwards.
        let end = crate::desk::abs_min(g.day, crate::desk::SHIFT_END);
        for (n, t) in g.trips.iter_mut().enumerate() {
            t.delay_min = (end - 20 + 10 * n as i64 - t.depart) as f64 - t.plan_min;
        }
        assert_eq!(g.trips.len(), 2);
        a.close_armed = true;
        a.key(Key::End);
        let out = said(&a.key(Key::Enter));
        let close = out.find("Closing the desk.").expect(&out);
        assert!(out.starts_with("Before you close, it's "), "{out}");
        assert!(out[..close].contains("delivered"), "news first: {out}");
        assert!(a.game.as_ref().unwrap().trips.is_empty());
    }

    #[test]
    fn escape_from_the_desk_leaves_at_once_with_the_day_saved() {
        let (mut a, d) = on_desk();
        a.key(Key::Down);
        let out = said(&a.key(Key::Escape));
        assert!(out.starts_with("Freight Fate: Dispatch"), "{out}");
        assert!(a.shift.is_none() && a.game.is_none());
        let slots = list_slots(&d.path().join("careers"));
        assert!(matches!(&slots[..], [Slot::Ok { desk: Some(_), .. }]));
        // The start screen still asks before quitting.
        let out = said(&a.key(Key::Escape));
        assert!(out.contains("Press Escape again to quit."), "{out}");
    }

    /// A second App on the same data folder, at its Start screen.
    fn reopen(d: &tempfile::TempDir) -> App {
        let dirs = Dirs {
            careers: d.path().join("careers"),
            ff_saves: vec![],
            data: d.path().to_path_buf(),
        };
        let mut b = App::new(get_world(), dirs, Some(42), false);
        b.tips_on = false;
        b.start();
        b
    }

    /// Start, Continue, the first career.
    fn continue_first(b: &mut App) -> String {
        b.key(Key::Down);
        b.key(Key::Enter); // Continue
        said(&b.key(Key::Enter))
    }

    fn state(a: &App) -> (String, String) {
        (
            serde_json::to_string(a.game.as_ref().unwrap()).unwrap(),
            serde_json::to_string(a.shift.as_ref().unwrap()).unwrap(),
        )
    }

    /// An assignment through the menus, from the desk's top row.
    fn assign_by_keys(a: &mut App) {
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
        a.key(Key::Enter);
        if a.game.as_ref().unwrap().trips.is_empty() {
            a.key(Key::Enter); // short on hours: send them anyway
        }
    }

    #[test]
    fn an_assignment_survives_escape_and_continue_at_the_same_clock() {
        let (mut a, _d) = on_desk();
        a.shift.as_mut().unwrap().schedule = Default::default();
        a.shift.as_mut().unwrap().pending.clear();
        assign_by_keys(&mut a);
        assert_eq!(a.game.as_ref().unwrap().trips.len(), 1);
        let clock = a.shift.as_ref().unwrap().clock;
        assert!(clock.minute > crate::desk::SHIFT_START);
        let before = state(&a);
        a.key(Key::Escape); // the board
        a.key(Key::Escape); // the desk, then out
        assert!(a.game.is_none());
        a.start();
        let out = continue_first(&mut a);
        let (g, s) = (a.game.as_ref().unwrap(), a.shift.as_ref().unwrap());
        assert_eq!(g.trips.len(), 1, "the assignment is kept");
        assert_eq!(s.clock, clock, "the clock is where it was");
        assert_eq!(state(&a), before);
        let weekday = crate::roster::WEEKDAYS[crate::roster::weekday(g.day) as usize];
        assert!(
            out.starts_with(&format!(
                "{}, day 1, {weekday}. | Desk, {}",
                g.name,
                clock.spoken()
            )),
            "{out}"
        );
        assert!(!out.contains("Day 1 at"), "no briefing: {out}");
        assert!(!out.contains("phone is ringing"), "{out}");
        assert!(matches!(a.stack.last(), Some(Screen::Desk)));
    }

    #[test]
    fn the_continue_list_says_the_time_of_a_day_under_way() {
        let (mut a, d) = on_desk();
        a.shift.as_mut().unwrap().schedule = Default::default();
        a.shift.as_mut().unwrap().pending.clear();
        let name = a.game.as_ref().unwrap().name.clone();
        let mut b = reopen(&d);
        b.key(Key::Down);
        let out = said(&b.key(Key::Enter));
        assert!(out.contains(&format!("{name}, day 1, $25,000, ")), "{out}");
        assign_by_keys(&mut a);
        let clock = a.shift.as_ref().unwrap().clock.spoken();
        let mut b = reopen(&d);
        b.key(Key::Down);
        let out = said(&b.key(Key::Enter));
        assert!(out.contains(&format!("{name}, day 1, {clock}, $")), "{out}");
    }

    #[test]
    fn a_ringing_call_survives_a_reload_and_answering_it_saves() {
        let (mut a, d) = on_desk();
        let id = a.game.as_ref().unwrap().drivers[0].id;
        {
            let s = a.shift.as_mut().unwrap();
            s.schedule = Default::default();
            s.pending.clear();
            s.pending
                .push_back(Ring::Call(crate::phone::Call::PersonalDay {
                    driver_id: id,
                }));
        }
        let out = said(&a.key(Key::Down));
        assert!(out.contains("phone is ringing"), "{out}");
        let ringing = state(&a);
        // Picked up: the call has left the queue, but the save still rings.
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("personal day"), "{out}");
        assert!(a.shift.as_ref().unwrap().pending.is_empty());
        let mut b = reopen(&d);
        let out = continue_first(&mut b);
        assert_eq!(state(&b), ringing);
        assert!(
            out.ends_with("The phone is ringing. Press Enter to answer."),
            "the ring is announced after the desk: {out}"
        );
        let out = said(&b.key(Key::Enter));
        assert!(out.contains("personal day"), "{out}");
        // The answer is given: that saves.
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("Day granted"), "{out}");
        let answered = state(&a);
        let mut c = reopen(&d);
        continue_first(&mut c);
        assert_eq!(state(&c), answered);
        assert!(c.shift.as_ref().unwrap().pending.is_empty());
    }

    /// Back to the desk by keys: answer the phone, or Escape a screen.
    fn to_desk(apps: &mut [&mut App]) {
        for _ in 0..10 {
            let key = match apps[0].stack.last() {
                Some(Screen::Desk) | None => return,
                Some(Screen::Phone(_)) => Key::Enter,
                Some(_) => Key::Escape,
            };
            for a in apps.iter_mut() {
                a.key(key);
            }
        }
    }

    #[test]
    fn a_reloaded_day_plays_on_as_if_never_left() {
        let (mut a, d) = on_desk();
        a.shift.as_mut().unwrap().pending.clear();
        assign_by_keys(&mut a);
        to_desk(&mut [&mut a]);
        a.key(Key::Home);
        assert!(matches!(a.stack.last(), Some(Screen::Desk)));
        let mut b = reopen(&d);
        continue_first(&mut b);
        assert_eq!(state(&a), state(&b));
        let segments: [&[Key]; 5] = [
            // Wait for what's next, twice.
            &[Key::End, Key::Up, Key::Enter, Key::Enter],
            // Tenders: accept the first.
            &[Key::Home, Key::Down, Key::Enter, Key::Enter, Key::Enter],
            // The board: the first load to the first driver.
            &[Key::Home, Key::Enter, Key::Enter, Key::Enter, Key::Enter],
            // Hiring: interview the first applicant.
            &[
                Key::Home,
                Key::Down,
                Key::Down,
                Key::Down,
                Key::Down,
                Key::Enter,
                Key::Enter,
            ],
            // Wait again.
            &[Key::End, Key::Up, Key::Enter, Key::Enter, Key::Enter],
        ];
        for keys in segments {
            for &key in keys {
                a.key(key);
                b.key(key);
                assert_eq!(state(&a), state(&b), "after {key:?}");
            }
            to_desk(&mut [&mut a, &mut b]);
            assert_eq!(state(&a), state(&b));
        }
        // Close the day, answering whatever rings, through the next morning.
        for _ in 0..30 {
            if a.game.as_ref().unwrap().day == 2 {
                break;
            }
            a.key(Key::End);
            b.key(Key::End);
            a.key(Key::Enter);
            b.key(Key::Enter);
            to_desk(&mut [&mut a, &mut b]);
            assert_eq!(a.game.as_ref().unwrap().day, b.game.as_ref().unwrap().day);
        }
        assert_eq!(said(&a.tick()), said(&b.tick()));
        assert_eq!(a.game.as_ref().unwrap().day, 2);
        assert_eq!(state(&a), state(&b));
    }

    #[test]
    fn no_save_pairs_the_next_day_with_the_closed_one() {
        let (mut a, d) = on_desk();
        let s = a.shift.as_mut().unwrap();
        s.schedule = Default::default(); // a quiet day
        s.pending.clear();
        a.close_armed = true;
        let careers = d.path().join("careers");
        let on_disk = || match list_slots(&careers).into_iter().next() {
            Some(Slot::Ok { game, desk, .. }) => (game.day, desk.map(|s| s.clock.minute)),
            _ => panic!("no save"),
        };
        a.key(Key::End);
        let out = said(&a.key(Key::Enter)); // Close the day
        assert!(out.contains("Closing the desk."), "{out}");
        assert_eq!(a.game.as_ref().unwrap().day, 2, "the career is on tomorrow");
        let (day, minute) = on_disk();
        assert_eq!(day, 1, "the save is still today, with today's desk");
        assert!(minute.is_some_and(|m| m < crate::desk::SHIFT_END));
        a.tick();
        let (day, minute) = on_disk();
        assert_eq!(day, 2);
        assert_eq!(minute, Some(a.game.as_ref().unwrap().desk_opens()));
        let mut b = reopen(&d);
        let out = continue_first(&mut b);
        assert!(
            !out.contains("Day 2 at"),
            "the morning isn't run again: {out}"
        );
        assert_eq!(state(&a), state(&b));
        let names: Vec<_> = std::fs::read_dir(&careers)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 1, "no temp files: {names:?}");
        assert!(names[0].ends_with(".json"), "{names:?}");
    }

    #[test]
    fn an_old_save_or_an_unreadable_desk_starts_at_the_morning() {
        for desk in [None, Some(serde_json::json!({"clock": {"minute": "late"}}))] {
            let (a, d) = on_desk();
            let g = a.game.as_ref().unwrap();
            let mut v = serde_json::to_value(g).unwrap();
            if let Some(desk) = desk.clone() {
                v.as_object_mut().unwrap().insert("desk".into(), desk);
            }
            let path = crate::saves::save_path(g, &d.path().join("careers"));
            std::fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
            let mut b = reopen(&d);
            let out = continue_first(&mut b);
            assert!(out.contains("Day 1 at"), "the morning briefing: {out}");
            assert!(!out.contains("could not be read"), "{out}");
            assert!(matches!(b.stack.last(), Some(Screen::Desk)));
            // The morning saved again, now with its desk.
            assert!(matches!(
                &list_slots(&d.path().join("careers"))[..],
                [Slot::Ok { desk: Some(_), .. }]
            ));
        }
    }

    #[test]
    fn moving_the_cursor_writes_nothing() {
        let (mut a, d) = on_desk();
        let s = a.shift.as_mut().unwrap();
        s.schedule = Default::default();
        s.pending.clear();
        a.key(Key::Down); // the cleared phone saves
        let careers = d.path().join("careers");
        let path = std::fs::read_dir(&careers)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::remove_file(&path).unwrap();
        a.key(Key::Up);
        a.key(Key::Down);
        a.key(Key::F1);
        assert!(!path.exists(), "nothing changed, nothing written");
        a.key(Key::Enter); // Tenders
        a.key(Key::End);
        a.key(Key::Enter); // the sales call
        assert!(path.exists());
    }

    #[test]
    fn a_failed_save_is_said_once_until_one_works() {
        let (mut a, d) = on_desk();
        {
            let s = a.shift.as_mut().unwrap();
            s.schedule = Default::default();
            s.pending.clear();
        }
        let careers = a.dirs.careers.clone();
        let blocked = d.path().join("blocked");
        std::fs::write(&blocked, b"a file, not a folder").unwrap();
        a.dirs.careers = blocked.join("careers");
        let tenders = |a: &mut App| {
            [
                Key::Home,
                Key::Down,
                Key::Enter,
                Key::Enter,
                Key::Enter,
                Key::Escape,
            ]
            .into_iter()
            .map(|key| said(&a.key(key)))
            .collect::<Vec<_>>()
            .join(" ")
        };
        let first = tenders(&mut a);
        assert_eq!(
            first.matches("The career could not be saved: ").count(),
            1,
            "{first}"
        );
        let again = tenders(&mut a);
        assert!(!again.contains("could not be saved"), "{again}");
        // Escape on the desk asks before leaving work that isn't saved.
        let out = said(&a.key(Key::Escape));
        assert!(out.contains("Press Escape again to leave anyway."), "{out}");
        assert!(a.game.is_some());
        a.dirs.careers = careers;
        let out = said(&a.key(Key::Escape));
        assert!(out.starts_with("Freight Fate: Dispatch"), "{out}");
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
        assert!(
            matches!(a.stack.last(), Some(Screen::Board)),
            "back on the board for the next load"
        );
        let next = a.menu.current_text();
        assert!(out.ends_with(&next), "the landing is spoken: {out}");
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
        assert!(
            out.contains("Set the Freight Fate saves folder below"),
            "{out}"
        );
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
        let first = a.key(Key::Escape);
        assert!(!first.contains(&Out::Quit), "the first Escape asks");
        assert!(said(&first).contains("Press Escape again to quit."));
        assert!(a.key(Key::Escape).contains(&Out::Quit));
    }

    #[test]
    fn the_tenders_screen_accepts_one_and_lands_back_on_the_list() {
        let (mut a, _d) = on_desk();
        a.key(Key::Down); // Tenders
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("Tenders. "), "{out}");
        let n = a.shift.as_ref().unwrap().tenders.len();
        assert!(n > 0);
        let out = said(&a.key(Key::Enter)); // first tender
        assert!(
            out.contains("deliver by") || out.contains("strict appointment"),
            "{out}"
        );
        assert!(out.ends_with("Accept, 1 of 2."), "{out}");
        let out = said(&a.key(Key::Enter)); // Accept
        assert!(out.starts_with("Accepted. It's on the board:"), "{out}");
        assert!(matches!(a.stack.last(), Some(Screen::Tenders)));
        assert_eq!(a.shift.as_ref().unwrap().tenders.len(), n - 1);
        a.key(Key::End);
        let out = said(&a.key(Key::Enter)); // sales call
        assert!(
            out.contains("will try you") || out.starts_with("No luck today"),
            "{out}"
        );
        assert!(a.shift.as_ref().unwrap().sales_called);
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("You've made today's sales call"), "{out}");
    }

    #[test]
    fn a_spot_load_offers_a_counter_after_the_drivers() {
        let (mut a, _d) = on_desk();
        let load = a
            .shift
            .as_ref()
            .unwrap()
            .board
            .iter()
            .position(|l| l.source.is_spot())
            .unwrap();
        a.key(Key::Enter); // Load board
        for _ in 0..load {
            a.key(Key::Down);
        }
        a.key(Key::Enter);
        let out = said(&a.key(Key::End));
        assert!(out.starts_with("Counter the rate with "), "{out}");
        let out = said(&a.key(Key::Enter));
        let g = a.game.as_ref().unwrap();
        assert!(
            crate::customers::BROKERS.iter().any(|b| out.starts_with(b)),
            "{out}"
        );
        assert_eq!(
            a.shift.as_ref().unwrap().clock.minute,
            crate::desk::SHIFT_START + 5
        );
        let _ = g;
    }

    #[test]
    fn assigning_from_a_drivers_own_board_cues_the_landing() {
        let (mut a, _d) = on_desk();
        let id = a.game.as_ref().unwrap().drivers[0].id;
        a.open(Screen::BoardFor(id));
        let outs = a.key(Key::Enter);
        let out = said(&outs);
        if out.contains("Press Enter again") {
            return; // short on hours: it asks first
        }
        assert!(
            out.ends_with(&a.menu.current_text()),
            "the landing is spoken: {out}"
        );
    }

    fn type_text(a: &mut App, text: &str) {
        for c in text.chars() {
            a.key(Key::Char(c));
        }
    }

    #[test]
    fn a_typed_saves_folder_is_found_spoken_and_kept() {
        let (mut a, d) = app();
        let parent = d.path().join("ff-tester");
        let profiles = parent.join("FreightFate").join("saves").join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        for n in ["Alma.ffsave", "Bo.ffsave"] {
            std::fs::write(profiles.join(n), b"x").unwrap();
        }
        a.start();
        a.menu.index = 2;
        a.key(Key::Enter); // Import
        assert!(said(&a.key(Key::End)).contains("Freight Fate saves folder: not set"));
        assert!(said(&a.key(Key::Enter)).starts_with("Freight Fate saves folder. Type"));
        assert!(a.wants_text());
        type_text(&mut a, &d.path().join("nope").display().to_string());
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("There's no folder at "), "{out}");
        for _ in 0.."nope".len() {
            a.key(Key::Backspace);
        }
        type_text(&mut a, "ff-tester");
        let out = said(&a.key(Key::Enter));
        assert!(
            out.starts_with(&format!(
                "Found 2 Freight Fate saves in {}.",
                profiles.display()
            )),
            "{out}"
        );
        assert!(out.ends_with("Alma, 1 of 4."), "{out}");
        assert_eq!(
            load_settings(d.path()).ff_saves_folder,
            Some(parent.clone())
        );
        let dirs = Dirs {
            careers: d.path().join("careers"),
            ff_saves: vec![],
            data: d.path().to_path_buf(),
        };
        let b = App::new(get_world(), dirs, Some(42), false);
        assert_eq!(b.dirs.ff_saves, vec![profiles]);
    }

    #[test]
    fn clearing_the_saves_folder_goes_back_to_the_usual_places() {
        let (mut a, d) = app();
        a.ff_saves_folder = Some(d.path().join("gone"));
        a.start();
        a.menu.index = 2;
        a.key(Key::Enter);
        a.key(Key::End);
        let out = said(&a.key(Key::Enter));
        assert!(
            out.starts_with("The saves folder setting is cleared."),
            "{out}"
        );
        assert_eq!(load_settings(d.path()).ff_saves_folder, None);
        assert!(a.dirs.ff_saves.is_empty());
    }

    #[test]
    fn escape_cancels_typing_a_saves_folder() {
        let (mut a, d) = app();
        a.start();
        a.menu.index = 2;
        a.key(Key::Enter);
        a.key(Key::End);
        a.key(Key::Enter);
        assert_eq!(said(&a.key(Key::Char('C'))), "cap c");
        assert_eq!(said(&a.key(Key::Char('\\'))), "backslash");
        assert!(said(&a.key(Key::Escape)).starts_with("Import a Freight Fate driver"));
        assert!(!a.wants_text());
        assert_eq!(load_settings(d.path()).ff_saves_folder, None);
    }

    #[test]
    fn the_money_screen_reads_cash_switches_factoring_and_opens_the_trucks() {
        let (mut a, _d) = on_desk();
        for _ in 0..5 {
            a.key(Key::Down);
        }
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("Cash $"), "{out}");
        a.key(Key::End);
        a.key(Key::Up);
        let out = said(&a.key(Key::Enter)); // factoring row
        assert!(out.starts_with("Factoring off:"), "{out}");
        assert!(!a.game.as_ref().unwrap().factoring);
        a.key(Key::End);
        let out = said(&a.key(Key::Enter)); // Trucks
        assert!(out.contains("Truck 1, "), "{out}");
        let trucks = a.game.as_ref().unwrap().tractors.len();
        a.key(Key::End);
        let out = said(&a.key(Key::Enter)); // buy
        assert!(out.starts_with("Truck "), "{out}");
        assert!(
            out.contains("It's a spare until someone's hired for it."),
            "{out}"
        );
        assert_eq!(a.game.as_ref().unwrap().tractors.len(), trucks + 1);
        a.key(Key::Home);
        let out = said(&a.key(Key::Enter)); // service the first truck: it asks
        assert!(out.contains("Press Enter again to service it."), "{out}");
        let out = said(&a.key(Key::Enter));
        assert!(
            out.contains("in the shop for the rest of today") || out.contains("has it on the road"),
            "{out}"
        );
    }

    #[test]
    fn a_finished_career_speaks_its_ending_and_returns_to_the_start() {
        let (mut a, _d) = on_desk();
        a.game.as_mut().unwrap().ended = Some(crate::money::Ending::NoDrivers);
        a.shift.as_mut().unwrap().schedule = Default::default();
        a.close_armed = true;
        a.key(Key::End);
        a.key(Key::Enter); // Close
        let out = said(&a.tick());
        assert!(out.contains("is finished after"), "{out}");
        assert!(out.contains("the last driver left"), "{out}");
        assert!(a.game.is_none());
        assert_eq!(a.stack.len(), 1);
    }

    #[test]
    fn the_company_screen_reads_the_tier_and_offers_the_second_terminal() {
        let (mut a, _d) = on_desk();
        for _ in 0..6 {
            a.key(Key::Down);
        }
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("Company | A new authority at "), "{out}");
        a.key(Key::End);
        let out = said(&a.key(Key::Enter)); // Open a second terminal
        assert!(out.starts_with("Second terminal | "), "{out}");
        assert!(out.contains(" miles from "), "{out}");
        let out = said(&a.key(Key::Enter)); // too new for it
        assert!(out.contains("established reputation"), "{out}");
        assert!(a.game.as_ref().unwrap().terminal2.is_none());
    }

    #[test]
    fn an_accepted_load_for_tomorrow_can_be_planned_from_the_tenders_screen() {
        let (mut a, _d) = on_desk();
        {
            let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
            s.schedule = Default::default();
            s.pending.clear();
            while !s.tenders.is_empty() {
                g.accept_tender(s, 0, a.world).unwrap();
            }
            s.clock.minute = crate::customers::TENDER_MINUTE;
            g.answer_tenders(s, Some(true), a.world);
            assert!(!g.coming_tenders().is_empty());
        }
        a.key(Key::Down); // Tenders
        let out = said(&a.key(Key::Enter));
        assert!(
            out.contains(" Accepted for tomorrow, no driver planned"),
            "{out}"
        );
        let out = said(&a.key(Key::Enter)); // the first coming load
        assert!(out.starts_with("Who takes it tomorrow morning? "), "{out}");
        let out = said(&a.key(Key::Enter)); // the first driver
        assert!(out.contains(" is planned on "), "{out}");
        assert!(matches!(a.stack.last(), Some(Screen::Tenders)));
        let g = a.game.as_ref().unwrap();
        let first = g.drivers[0].name.clone();
        assert_eq!(
            g.accepted[g.coming_tenders()[0]].planned_driver,
            Some(g.drivers[0].id)
        );
        assert!(
            out.contains(&format!(" Accepted for tomorrow, {first} planned")),
            "{out}"
        );
        a.key(Key::End);
        let out = said(&a.key(Key::Enter)); // still the sales call
        assert!(
            out.contains("will try you") || out.starts_with("No luck today"),
            "{out}"
        );
    }

    #[test]
    fn quick_pay_is_offered_on_the_money_screen_once_factoring_is_off() {
        let (mut a, _d) = on_desk();
        for _ in 0..5 {
            a.key(Key::Down);
        }
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("Money | "), "{out}");
        assert!(!a.screen_lines().iter().any(|l| l.contains("quick pay")));
        let rows = a.game.as_ref().unwrap().money_lines().len();
        for _ in 0..rows {
            a.key(Key::Down);
        }
        let out = said(&a.key(Key::Enter)); // factoring off
        assert!(out.starts_with("Factoring off: "), "{out}");
        a.key(Key::Down);
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("Quick pay on: "), "{out}");
        assert!(a.game.as_ref().unwrap().quick_pay);
        a.key(Key::End);
        let out = said(&a.key(Key::Enter));
        assert!(out.starts_with("Trucks | "), "{out}");
    }

    fn tips(out: &[Out]) -> Vec<String> {
        out.iter()
            .filter_map(|o| match o {
                Out::Say(t) | Out::Queue(t) if t.starts_with("Tip: ") => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    fn on_desk_with_tips() -> (App, tempfile::TempDir, Vec<Out>) {
        let (mut a, d) = app();
        a.tips_on = true;
        a.start();
        a.key(Key::Enter);
        a.key(Key::Enter);
        let out = a.tick();
        (a, d, out)
    }

    #[test]
    fn the_first_morning_plays_the_desk_tip_last_with_its_sound() {
        let (a, _d, out) = on_desk_with_tips();
        assert_eq!(tips(&out), vec![Tip::Desk.text().to_string()]);
        let n = out.len();
        assert_eq!(out[n - 2], Out::Cue(Cue::Notify));
        assert_eq!(out[n - 1], Out::Queue(Tip::Desk.text().into()));
        assert!(a.game.as_ref().unwrap().tips.has_heard(Tip::Desk));
        assert!(a.menu.items[8].label.starts_with("Tips, 1 heard"));
    }

    #[test]
    fn a_screen_tip_plays_once() {
        let (mut a, _d, _) = on_desk_with_tips();
        let out = a.key(Key::Enter); // Load board
        assert_eq!(tips(&out), vec![Tip::Board.text().to_string()]);
        a.key(Key::Escape);
        let out = a.key(Key::Enter);
        assert!(
            !tips(&out).contains(&Tip::Board.text().to_string()),
            "{out:?}"
        );
    }

    #[test]
    fn one_tip_per_key_and_the_other_waits_for_its_next_time() {
        let (mut a, _d, _) = on_desk_with_tips();
        a.tip_due = vec![Tip::Roster, Tip::Hiring];
        let mut out = vec![];
        a.play_tip(&mut out);
        assert_eq!(tips(&out), vec![Tip::Roster.text().to_string()]);
        let log = &a.game.as_ref().unwrap().tips;
        assert!(log.has_heard(Tip::Roster) && !log.has_heard(Tip::Hiring));
        assert!(a.tip_due.is_empty());
        // Hiring plays when Hiring opens.
        a.menu.index = 4;
        let out = a.key(Key::Enter);
        assert_eq!(tips(&out), vec![Tip::Hiring.text().to_string()]);
    }

    #[test]
    fn with_tips_off_nothing_is_said_or_marked() {
        let (mut a, _d) = on_desk();
        let out = a.key(Key::Enter);
        assert!(tips(&out).is_empty(), "{out:?}");
        assert!(a.game.as_ref().unwrap().tips.heard.is_empty());
    }

    #[test]
    fn a_save_from_before_tips_loads_with_every_tip_heard() {
        let (a, _d, _) = on_desk_with_tips();
        let mut json = serde_json::to_value(a.game.as_ref().unwrap()).unwrap();
        json.as_object_mut().unwrap().remove("tips");
        let old: Game = serde_json::from_value(json).unwrap();
        assert!(Tip::ALL.iter().all(|t| old.tips.has_heard(*t)));
        assert_eq!(old.tips.first_unopened(), None);
    }

    #[test]
    fn tips_change_no_rules_or_random_numbers() {
        let play = |on: bool| {
            let (mut a, _d) = app();
            a.tips_on = on;
            a.start();
            a.key(Key::Enter);
            a.key(Key::Enter);
            let mut out = a.tick();
            for key in [
                Key::Enter, // board
                Key::Enter, // who takes the first load
                Key::Enter, // the first driver
                Key::Enter, // and again, if short on hours
                Key::Escape,
                Key::Escape,
                Key::Down,
                Key::Down,
                Key::Down,
                Key::Enter, // roster
                Key::Escape,
                Key::End,
                Key::Up,
                Key::Enter, // wait for what's next
                Key::End,
                Key::Enter,
                Key::Enter,
                Key::Enter,
            ] {
                out.extend(a.key(key));
            }
            out.extend(a.tick());
            let mut game = serde_json::to_value(a.game.as_ref().unwrap()).unwrap();
            game.as_object_mut().unwrap().remove("tips");
            // Everything said, less the tips and the desk row counting them.
            let spoken: Vec<String> = out
                .iter()
                .filter_map(|o| match o {
                    Out::Say(t) | Out::Queue(t) if !t.starts_with("Tip: ") => {
                        let mut t = t.clone();
                        for n in (0..=Tip::ALL.len()).rev() {
                            t = t.replace(&format!("Tips, {n} heard"), "Tips");
                        }
                        Some(t)
                    }
                    _ => None,
                })
                .collect();
            (game, spoken)
        };
        let (off, said_off) = play(false);
        let (on, said_on) = play(true);
        assert_eq!(off, on);
        assert_eq!(said_off, said_on);
    }

    fn nothing_needs_you(a: &mut App) {
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        s.board.clear();
        s.tenders.clear();
        s.pending.clear();
        s.schedule = Default::default();
        g.trips.clear();
        for d in &mut g.drivers {
            d.status = crate::roster::Status::OffDuty;
        }
    }

    #[test]
    fn space_names_an_unopened_screen_in_the_first_week_only() {
        let (mut a, _d, _) = on_desk_with_tips();
        nothing_needs_you(&mut a);
        let out = a.key(Key::Space);
        assert!(said(&out).starts_with("Nothing needs you"), "{out:?}");
        assert_eq!(
            out.last(),
            Some(&Out::Queue("You haven't opened Tenders yet.".into()))
        );
        a.menu.index = 1;
        a.key(Key::Enter); // Tenders
        a.key(Key::Escape);
        let out = said(&a.key(Key::Space));
        assert!(out.ends_with("You haven't opened Trips yet."), "{out}");
        a.game.as_mut().unwrap().day = 8;
        let out = said(&a.key(Key::Space));
        assert!(!out.contains("haven't opened"), "{out}");
        a.game.as_mut().unwrap().day = 2;
        a.tips_on = false;
        let out = said(&a.key(Key::Space));
        assert!(!out.contains("haven't opened"), "{out}");
    }

    #[test]
    fn the_tips_screen_replays_switches_and_forgets() {
        let (mut a, d, _) = on_desk_with_tips();
        a.menu.index = 8;
        let out = said(&a.key(Key::Enter));
        assert!(
            out.starts_with(&format!(
                "Tips. 1 heard, {} still to find. | Tips: on, 1 of 3.",
                Tip::ALL.len() - 1
            )),
            "{out}"
        );
        a.key(Key::Down);
        assert_eq!(a.help_line(), Tip::Desk.text());
        let out = said(&a.key(Key::Enter));
        assert!(out.ends_with(Tip::Desk.text()), "{out}");
        a.key(Key::Home);
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("Tips off."), "{out}");
        assert!(!load_settings(d.path()).tips);
        a.key(Key::Enter);
        assert!(load_settings(d.path()).tips);
        a.key(Key::End);
        let out = said(&a.key(Key::Enter));
        assert!(
            out.contains("Forget the tip you have heard? Enter forgets it, Escape keeps it."),
            "{out}"
        );
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("Tips forgotten."), "{out}");
        assert!(a.game.as_ref().unwrap().tips.heard.is_empty());
        assert!(out.ends_with("Hear all tips again, 2 of 2."), "{out}");
    }

    #[test]
    fn the_first_ring_and_the_first_call_of_a_kind_each_get_a_tip() {
        let (mut a, _d, _) = on_desk_with_tips();
        let id = a.game.as_ref().unwrap().drivers[0].id;
        a.shift
            .as_mut()
            .unwrap()
            .pending
            .push_back(crate::career::Ring::Call(crate::phone::Call::Sick {
                driver_id: id,
            }));
        let out = a.key(Key::Down);
        assert_eq!(tips(&out), vec![Tip::Phone.text().to_string()]);
        let out = a.key(Key::Enter);
        assert_eq!(tips(&out), vec![Tip::Sick.text().to_string()]);
    }
}
