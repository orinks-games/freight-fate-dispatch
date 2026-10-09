//! The career (persistent, saved at day boundaries) and today's shift
//! (rebuilt from the career's seed every morning).

use std::collections::{BTreeMap, VecDeque};

use ff_core::data::world::World;
use ff_core::data::world_models::Route;
use ff_core::models::market::Market;
use ff_core::models::start_options::{pay_plan_for_key, start_option};
use serde::{Deserialize, Serialize};

use crate::board::{morning_board, refusal, route_for, Load};
use crate::desk::{abs_min, abs_text, Action, DeskClock, Schedule, SHIFT_END, SHIFT_START};
use crate::roster::{seeded_drivers, Driver, Status};
use crate::trip::Trip;

/// The day's dispatcher goal.
#[derive(Clone, Debug, PartialEq)]
pub enum DayGoal {
    OnTime,
    /// Dollars gross per total mile, empty miles included.
    RevenuePerMile(f64),
    /// Empty miles under this percent of all miles.
    Deadhead(f64),
    Home {
        driver_id: u32,
    },
}

pub const GOAL_REVENUE_PER_MILE: f64 = 2.0;
pub const GOAL_DEADHEAD_PCT: f64 = 15.0;
pub const HOME_MISS_GOODWILL: i32 = 8;
pub const QUIT_WARN_GOODWILL: i32 = 30;
pub const QUIT_GOODWILL: i32 = 20;

/// One delivery today, for the goal and the end-of-day story.
#[derive(Clone, Debug, PartialEq)]
pub struct Delivered {
    pub driver_id: u32,
    pub on_time: bool,
    pub reason: Option<String>,
    pub gross: f64,
    pub loaded_mi: f64,
    pub deadhead_mi: f64,
}

pub const STARTING_FUNDS: f64 = 25_000.0;
pub const STARTING_DRIVERS: usize = 3;
/// Terminal, office, and parked-equipment overhead omitted by ATRI's per-mile costs.
pub const TRUCK_COST_PER_DAY: f64 = 180.0;
/// ATRI, "An Analysis of the Operational Costs of Trucking: 2026 Update,"
/// Table 8, 2025 vehicle-based costs per mile: fuel 0.482, truck/trailer
/// payments 0.404, repair and maintenance 0.215, insurance 0.106, permits
/// 0.008, tires 0.050, and tolls 0.043. Driver pay remains the pay-plan share.
pub const OPERATING_COST_PER_MILE: f64 = 1.31;
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub delivered: u32,
    pub on_time: u32,
    pub abandoned: u32,
    pub days_worked: u32,
    pub dispatch_streak: u32,
    pub best_dispatch_streak: u32,
}

impl Record {
    pub fn on_time_pct(&self) -> f64 {
        if self.delivered == 0 {
            100.0
        } else {
            (self.on_time as f64 * 100.0 / self.delivered as f64).round()
        }
    }
}

/// One weather warning on a board load's route, as stored in the day's save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredAlert {
    pub event: String,
    /// Route city (key or name) the alert was found at.
    pub city: String,
    pub delay_min: f64,
    pub avoid: bool,
}

/// Something that makes the phone ring. Filled in by incidents and phone.
#[derive(Clone, Debug, PartialEq)]
pub enum Ring {
    Incident(crate::incidents::Incident),
    Call(crate::phone::Call),
}

impl Ring {
    pub fn title(&self, game: &Game, shift: &Shift, world: &World) -> String {
        match self {
            Ring::Incident(i) => crate::incidents::script(game, i, shift.now(game.day), world),
            Ring::Call(c) => crate::phone::script(game, shift, c, world),
        }
    }

    pub fn labels(&self, game: &Game, shift: &Shift, world: &World) -> Vec<String> {
        match self {
            Ring::Incident(i) => crate::incidents::choices(game, i, shift.now(game.day), world)
                .iter()
                .map(|c| c.label(game))
                .collect(),
            Ring::Call(c) => crate::phone::choices(c)
                .iter()
                .map(|c| c.label().to_string())
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    pub schema_version: u32,
    pub name: String,
    pub seed: i64,
    pub day: i64,
    pub carrier_key: String,
    pub terminal: String,
    pub funds: f64,
    pub reputation: f64,
    #[serde(default)]
    pub record: Record,
    pub drivers: Vec<Driver>,
    #[serde(default)]
    pub trips: Vec<Trip>,
    #[serde(default)]
    pub market: Market,
    #[serde(default)]
    pub shipper_goodwill: BTreeMap<String, i32>,
    /// Today's board weather, by morning board position.
    #[serde(default)]
    pub weather: Vec<Vec<StoredAlert>>,
    #[serde(default)]
    pub weather_live: bool,
    /// Overnight news read in the next morning's briefing.
    #[serde(default)]
    pub news: Vec<String>,
    #[serde(default)]
    pub next_id: u32,
}

pub struct Shift {
    pub clock: DeskClock,
    pub board: Vec<Load>,
    pub schedule: Schedule<Ring>,
    pub pending: VecDeque<Ring>,
    pub hot_offers: Vec<Load>,
    pub applicants: Vec<crate::hiring::Applicant>,
    pub assigned: u32,
    pub goal: DayGoal,
    pub abandoned_at_start: u32,
    /// "Daily goal met" was spoken and still holds; it isn't said again.
    pub goal_met_said: bool,
    pub log: Vec<Delivered>,
}

impl Shift {
    pub fn now(&self, day: i64) -> i64 {
        abs_min(day, self.clock.minute)
    }

    pub fn credited_dispatches(&self, record: &Record) -> u32 {
        self.assigned
            .saturating_sub(record.abandoned - self.abandoned_at_start)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActError {
    PhoneRinging,
    Closed,
    Refused(String),
    NoSuchItem,
    RosterFull,
    NoFunds,
}

impl ActError {
    pub fn spoken(&self) -> String {
        match self {
            ActError::PhoneRinging => "The phone is ringing. Press Enter to answer.".into(),
            ActError::Closed => "The desk is closed for the day. Close the day to go on.".into(),
            ActError::Refused(why) => why.clone(),
            ActError::NoSuchItem => "That is no longer available.".into(),
            ActError::RosterFull => "The roster is full at 10 drivers.".into(),
            ActError::NoFunds => "There isn't enough money for that.".into(),
        }
    }
}

impl Game {
    pub fn new(carrier_key: &str, seed: i64, world: &World) -> Game {
        let opt = start_option(Some(carrier_key));
        let drivers = seeded_drivers(&mut crate::rng(seed, 0, "roster"), 1, STARTING_DRIVERS, &[]);
        let mut game = Game {
            schema_version: SCHEMA_VERSION,
            name: opt.carrier_name.to_string(),
            seed,
            day: 1,
            carrier_key: opt.key.to_string(),
            terminal: world.resolve_city_key(opt.default_city),
            funds: STARTING_FUNDS,
            reputation: opt.starting_reputation,
            record: Record::default(),
            next_id: drivers.len() as u32 + 1,
            drivers,
            trips: Vec::new(),
            market: Market::with_seed(seed),
            shipper_goodwill: BTreeMap::new(),
            weather: Vec::new(),
            weather_live: false,
            news: Vec::new(),
        };
        game.normalize();
        game
    }

    pub fn take_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id - 1
    }

    pub fn driver(&self, id: u32) -> Option<&Driver> {
        self.drivers.iter().find(|d| d.id == id)
    }

    pub fn driver_mut(&mut self, id: u32) -> Option<&mut Driver> {
        self.drivers.iter_mut().find(|d| d.id == id)
    }

    pub fn eligible_idle(&self, shift: &Shift, world: &World) -> usize {
        self.drivers
            .iter()
            .filter(|driver| {
                driver.status == Status::Idle
                    && shift
                        .board
                        .iter()
                        .any(|load| refusal(&load.job, driver, world).is_none())
            })
            .count()
    }

    pub fn trip_mut(&mut self, id: u32) -> Option<&mut Trip> {
        self.trips.iter_mut().find(|t| t.id == id)
    }

    /// Old saves have no truck locations or home days: everyone is home.
    pub fn normalize(&mut self) {
        let (terminal, day) = (self.terminal.clone(), self.day);
        for d in &mut self.drivers {
            if d.location.is_empty() {
                d.location = terminal.clone();
            }
            if d.last_home_day == 0 {
                d.last_home_day = day;
            }
        }
        // Streaks and their bonus are gone; old saves drop them.
        self.record.dispatch_streak = 0;
    }

    /// Where loads are offered from: the terminal, then every city a free
    /// truck sits in.
    fn origins(&self) -> Vec<String> {
        let mut out = vec![self.terminal.clone()];
        for d in &self.drivers {
            let busy = matches!(d.status, Status::OnLoad | Status::OutOnRun);
            if !busy && !d.location.is_empty() && !out.contains(&d.location) {
                out.push(d.location.clone());
            }
        }
        out
    }

    fn city_jobs(
        &self,
        world: &World,
        city: &str,
        count: usize,
    ) -> Vec<ff_core::models::jobs::Job> {
        let purpose = if *city == self.terminal {
            "board".to_string()
        } else {
            format!("board:{city}")
        };
        let mut jobs = morning_board(
            world,
            city,
            &self.carrier_key,
            &self.market,
            crate::seed_for(self.seed, self.day, &purpose),
            &self.drivers,
        );
        jobs.truncate(count);
        jobs
    }

    fn board_jobs(&self, world: &World) -> Vec<ff_core::models::jobs::Job> {
        let origins = self.origins();
        let per = if origins.len() == 1 {
            crate::board::BOARD_SIZE
        } else {
            (crate::board::BOARD_SIZE / origins.len()).max(3)
        };
        origins
            .iter()
            .flat_map(|c| self.city_jobs(world, c, per))
            .collect()
    }

    /// A truck just went free somewhere new: that city's freight joins the board.
    pub fn add_city_offers(&self, shift: &mut Shift, city: &str, world: &World) {
        if city.is_empty() || shift.board.iter().any(|l| l.job.origin == city) {
            return;
        }
        for job in self.city_jobs(world, city, 3) {
            shift.board.push(Load {
                job,
                hot: false,
                weather: Vec::new(),
            });
        }
    }

    /// The next thing that happens on its own: a call, a truck arriving, or
    /// a driver coming off a break.
    pub fn next_event_at(&self, shift: &Shift) -> Option<i64> {
        let arrivals = self.trips.iter().filter(|t| !t.dock_hold).map(Trip::eta);
        let rests = self
            .drivers
            .iter()
            .filter(|d| d.rest_note.is_some())
            .map(|d| d.available_at);
        shift
            .schedule
            .next_at()
            .into_iter()
            .chain(arrivals)
            .chain(rests)
            .min()
    }

    /// Run the desk to the next event and stop there. Nothing moves
    /// otherwise; with nothing left today the desk runs to 6 PM.
    pub fn advance(&mut self, shift: &mut Shift, world: &World) -> Vec<String> {
        let now = shift.now(self.day);
        let end = abs_min(self.day, SHIFT_END);
        let Some(at) = self.next_event_at(shift).filter(|&t| t <= end) else {
            shift.clock.minute = SHIFT_END;
            return vec![
                "Nothing else is due before 6 PM. Close the day when you're ready.".into(),
            ];
        };
        let at = at.max(now);
        shift.clock.minute = (at - abs_min(self.day, 0)) as u32;
        let mut lines = vec![format!("{}.", shift.clock.spoken())];
        lines.extend(self.settle(shift, at, world));
        for id in self.wake(at) {
            let city = self
                .driver(id)
                .map(|d| d.location.clone())
                .unwrap_or_default();
            self.add_city_offers(shift, &city, world);
        }
        self.ring_due(shift);
        lines
    }

    /// Drivers whose break is over by `now` are free again. Returns their ids.
    pub fn wake(&mut self, now: i64) -> Vec<u32> {
        let mut woke = Vec::new();
        for d in &mut self.drivers {
            let Some(note) = d.rest_note.clone() else {
                continue;
            };
            if d.available_at > now {
                continue;
            }
            if note.contains("34-hour") {
                d.hos.restart();
            } else {
                d.hos.sleep();
            }
            d.rest_note = None;
            if d.status == Status::OffDuty {
                d.status = Status::Idle;
            }
            woke.push(d.id);
        }
        woke
    }

    /// Deliver every trip in by `upto`: pay, record, the truck stays where it
    /// delivered, the driver rests if the hours call for it, and a queued
    /// next load starts if it's still legal.
    pub fn settle(&mut self, shift: &mut Shift, upto: i64, world: &World) -> Vec<String> {
        let pay_share = pay_plan_for_key(Some(&self.carrier_key)).pay_share;
        let (arrived, rolling): (Vec<Trip>, Vec<Trip>) = std::mem::take(&mut self.trips)
            .into_iter()
            .partition(|t| !t.dock_hold && t.eta() <= upto);
        self.trips = rolling;
        let mut lines = Vec::new();
        for trip in arrived {
            let end = trip.eta();
            let gross = trip.job.payout((end - trip.depart) as f64 / 60.0, 0.0, 0.0);
            let driver_pay = gross * pay_share;
            let operating_costs =
                (trip.job.distance_mi + trip.deadhead_mi) * OPERATING_COST_PER_MILE;
            let net = gross - driver_pay - operating_costs;
            self.funds += net;
            self.record.delivered += 1;
            self.record.on_time += trip.on_time() as u32;
            shift.log.push(Delivered {
                driver_id: trip.driver_id,
                on_time: trip.on_time(),
                reason: trip.late_reason(),
                gross,
                loaded_mi: trip.job.distance_mi,
                deadhead_mi: trip.deadhead_mi,
            });
            let Some(d) = self.driver_mut(trip.driver_id) else {
                continue;
            };
            d.hos.drive(trip.drive_min.min(d.legal_left_min()));
            d.hos.on_duty(30.0);
            d.career.record_delivery(
                trip.job.distance_mi,
                driver_pay,
                trip.on_time(),
                0.0,
                1.0,
                1.0,
            );
            d.location = trip.job.destination.clone();
            let cycle_low = d
                .hos
                .cycle_remaining_min("realistic")
                .is_some_and(|m| m < 120.0);
            (d.available_at, d.rest_note) = if cycle_low {
                (end + 34 * 60, Some("a 34-hour restart".to_string()))
            } else if d.legal_left_min() < 60.0 {
                (end + 600, Some("a 10-hour break".to_string()))
            } else {
                (end + 30, None)
            };
            d.status = if d.rest_note.is_some() {
                Status::OffDuty
            } else {
                Status::Idle
            };
            let (id, name, avail) = (d.id, d.name.clone(), d.available_at);
            let empty = if trip.deadhead_mi >= 1.0 {
                format!(", {} of them empty", trip.deadhead_mi.round())
            } else {
                String::new()
            };
            let mut line = format!(
                "{name} delivered {} to {}, {}. {} to the carrier after {} in operating costs for {} miles{empty}.",
                trip.job.cargo.label,
                world.spoken_city(&trip.job.destination, None),
                trip.timing_text(),
                crate::money(net),
                crate::money(operating_costs),
                (trip.job.distance_mi + trip.deadhead_mi).round(),
            );
            if let Some(note) = &self.driver(id).unwrap().rest_note {
                line.push_str(&format!(
                    " {name} is in {}, available at {} after {note}.",
                    world.spoken_city(&trip.job.destination, None),
                    abs_text(avail, self.day)
                ));
            }
            lines.push(line);
            if self.driver(id).is_some_and(|d| d.status == Status::Idle) {
                self.add_city_offers(shift, &trip.job.destination, world);
            }
            let started = match trip.next.clone() {
                Some(job) => {
                    let (ok, said) = self.start_queued(shift, id, job, trip.next_queued_at, world);
                    lines.push(said);
                    ok
                }
                None => false,
            };
            if !started && avail <= abs_min(self.day, SHIFT_END) {
                shift.schedule.push(
                    avail.max(end),
                    Ring::Call(crate::phone::Call::Empty { driver_id: id }),
                );
            }
        }
        lines
    }

    /// Start a driver's queued load when they go empty, if the appointment
    /// can still be made legally from where they are.
    fn start_queued(
        &mut self,
        shift: &mut Shift,
        driver_id: u32,
        mut job: ff_core::models::jobs::Job,
        queued_at: i64,
        world: &World,
    ) -> (bool, String) {
        let d = self.driver(driver_id).unwrap().clone();
        let start = d.available_at;
        let left_h = job.deadline_game_h - (start - queued_at) as f64 / 60.0;
        let mut rested = d.clone();
        if rested.rest_note.is_some() {
            rested.hos.sleep();
        }
        let need_h = crate::trip::plan(&job, &rested, world).need_h;
        if need_h > left_h {
            return (
                false,
                format!(
                    "{} can't legally make the queued {} appointment in {}. It goes back to the broker.",
                    d.name,
                    job.cargo.label,
                    world.spoken_city(&job.destination, None)
                ),
            );
        }
        job.deadline_game_h = left_h;
        let id = self.take_id();
        let trip = Trip::start(id, &rested, job, start, world);
        let line = format!(
            "{} rolls on the queued load: {} from {} to {}, arriving {}.",
            d.name,
            trip.job.cargo.label,
            world.spoken_city(&trip.job.origin, None),
            world.spoken_city(&trip.job.destination, None),
            abs_text(trip.eta(), self.day)
        );
        let dd = self.driver_mut(driver_id).unwrap();
        dd.hos = rested.hos;
        dd.rest_note = None;
        dd.status = Status::OnLoad;
        self.push_trip(shift, trip, &[]);
        (true, line)
    }

    fn push_trip(&mut self, shift: &mut Shift, mut trip: Trip, weather: &[StoredAlert]) {
        for (at, inc) in crate::incidents::roll_for_trip(self, &trip, weather) {
            // Detention holds the truck at the dock until the call is resolved.
            trip.dock_hold |= inc.kind == crate::incidents::Kind::Detention;
            shift.schedule.push(at, Ring::Incident(inc));
        }
        self.trips.push(trip);
    }

    /// Send a free driver home to the terminal for home time.
    pub fn send_home(&mut self, shift: &mut Shift, driver_id: u32, world: &World) -> String {
        let now = shift.now(self.day);
        let terminal = self.terminal.clone();
        let Some(d) = self.driver(driver_id) else {
            return ActError::NoSuchItem.spoken();
        };
        let route = world
            .supported_route(&d.location, &terminal, None)
            .ok()
            .flatten()
            .filter(|_| d.location != terminal);
        let miles = route.as_ref().map(|r| r.miles()).unwrap_or(0.0);
        let minutes = route
            .as_ref()
            .map(|r| ff_core::models::jobs::route_drive_hours(Some(r), 0.0, Some(world)) * 60.0)
            .unwrap_or(0.0);
        let rest = if minutes > d.legal_left_min() {
            600.0
        } else {
            0.0
        };
        let home_at = now + (minutes + rest).round() as i64;
        let back = abs_min(
            home_at.div_euclid(crate::desk::MIN_PER_DAY) + 1,
            SHIFT_START,
        );
        self.funds -= miles * OPERATING_COST_PER_MILE;
        let d = self.driver_mut(driver_id).unwrap();
        d.hos.drive(minutes.min(d.legal_left_min()));
        d.location = terminal.clone();
        d.status = Status::OffDuty;
        d.rest_note = Some("home time".into());
        d.available_at = back;
        d.last_home_day = home_at.div_euclid(crate::desk::MIN_PER_DAY);
        d.quit_warned = false;
        let name = d.name.clone();
        if miles < 1.0 {
            return format!(
                "{name} is home in {} tonight.",
                world.spoken_city(&terminal, None)
            );
        }
        format!(
            "{name} is headed home to {}, {} miles empty, home about {}.",
            world.spoken_city(&terminal, None),
            miles.round(),
            abs_text(home_at, self.day)
        )
    }

    pub fn todays_routes(&self, world: &World) -> Vec<Option<Route>> {
        self.board_jobs(world)
            .iter()
            .map(|j| route_for(world, j))
            .collect()
    }

    /// `Some` = live NWS alerts for today's board; `None` = seeded weather.
    pub fn set_weather(&mut self, fetched: Option<Vec<Vec<StoredAlert>>>) {
        self.weather_live = fetched.is_some();
        self.weather = fetched.unwrap_or_default();
    }

    pub fn morning(&self, world: &World) -> Shift {
        let board = self
            .board_jobs(world)
            .into_iter()
            .enumerate()
            .map(|(i, job)| Load {
                job,
                hot: false,
                weather: self.weather.get(i).cloned().unwrap_or_default(),
            })
            .collect();
        let calls = crate::phone::morning_calls(self);
        let hot = calls
            .iter()
            .filter(|(_, c)| matches!(c, crate::phone::Call::HotLoad { .. }))
            .count();
        let hot_offers = crate::phone::hot_offers(self, world, hot);
        let calls = crate::phone::live_calls(calls, hot_offers.len());
        let mut shift = Shift {
            clock: DeskClock::morning(),
            board,
            schedule: Schedule::default(),
            pending: VecDeque::new(),
            hot_offers,
            applicants: crate::hiring::daily_pool(self),
            assigned: 0,
            goal: DayGoal::OnTime,
            abandoned_at_start: self.record.abandoned,
            goal_met_said: false,
            log: Vec::new(),
        };
        let end = abs_min(self.day, SHIFT_END);
        for d in self.drivers.iter().filter(|d| d.rest_note.is_some()) {
            if d.available_at > abs_min(self.day, SHIFT_START) && d.available_at <= end {
                shift.schedule.push(
                    d.available_at,
                    Ring::Call(crate::phone::Call::Empty { driver_id: d.id }),
                );
            }
        }
        let due_home = self.drivers.iter().find(|d| {
            !matches!(d.status, Status::OnLoad | Status::OutOnRun)
                && d.location != self.terminal
                && d.home_due_day() <= self.day
        });
        shift.goal = match (due_home, self.day % 3) {
            (Some(d), _) => DayGoal::Home { driver_id: d.id },
            (None, 0) => DayGoal::RevenuePerMile(GOAL_REVENUE_PER_MILE),
            (None, 1) => DayGoal::OnTime,
            _ => DayGoal::Deadhead(GOAL_DEADHEAD_PCT),
        };
        for d in &self.drivers {
            let overdue = self.day > d.home_due_day() && d.location != self.terminal;
            if overdue && d.goodwill <= QUIT_WARN_GOODWILL && !d.quit_warned {
                shift.schedule.push(
                    abs_min(self.day, SHIFT_START + 15),
                    Ring::Call(crate::phone::Call::QuitWarning { driver_id: d.id }),
                );
            }
        }
        for (at, call) in calls {
            shift.schedule.push(at, Ring::Call(call));
        }
        shift
    }

    pub fn briefing(&self, shift: &Shift, world: &World) -> Vec<String> {
        let mut lines = vec![format!(
            "Day {} at {}, {} terminal. {}.",
            self.day,
            self.name,
            world.spoken_city(&self.terminal, None),
            shift.clock.spoken()
        )];
        lines.push(format!(
            "Funds {}. On time {} percent.",
            crate::money(self.funds),
            self.record.on_time_pct()
        ));
        lines.push(format!(
            "Today's goal: {}.",
            self.goal_text(&shift.goal, world)
        ));
        for d in &self.drivers {
            let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
            if free && d.location != self.terminal && d.home_due_day() <= self.day {
                lines.push(format!(
                    "{} is due home in {} tonight and is in {}.",
                    d.name,
                    world.spoken_city(&self.terminal, None),
                    world.spoken_city(&d.location, None)
                ));
            }
        }
        let idle: Vec<&str> = self
            .drivers
            .iter()
            .filter(|d| d.status == Status::Idle)
            .map(|d| d.name.as_str())
            .collect();
        lines.push(match idle.len() {
            0 => "No drivers available.".into(),
            n => format!("{n} available: {}.", idle.join(", ")),
        });
        for t in &self.trips {
            let name = self
                .driver(t.driver_id)
                .map(|d| d.name.as_str())
                .unwrap_or("A driver");
            lines.push(format!(
                "{name} is still out, arriving {}.",
                abs_text(t.eta(), self.day)
            ));
        }
        lines.extend(self.news.iter().cloned());
        lines.push(format!("{} loads on the board.", shift.board.len()));
        for load in shift.board.iter().filter(|l| !l.weather.is_empty()) {
            let a = &load.weather[0];
            lines.push(format!(
                "Weather: {} near {} on the {} load.",
                a.event,
                world.spoken_city(&a.city, None),
                world.spoken_city(&load.job.destination, None)
            ));
        }
        lines
    }

    pub fn ensure_open(&self, shift: &Shift) -> Result<(), ActError> {
        if !shift.pending.is_empty() {
            return Err(ActError::PhoneRinging);
        }
        if shift.clock.closed() {
            return Err(ActError::Closed);
        }
        Ok(())
    }

    /// Move everything now due onto the ringing phone, dropping stale rings.
    /// Each ring is judged at the time it was due, so closing early still
    /// rings an incident on a truck that arrives before 6 PM. Rings due after
    /// the close never queue, so they never ring.
    pub fn ring_due(&mut self, shift: &mut Shift) {
        let now = shift.now(self.day).min(abs_min(self.day, SHIFT_END));
        let closed = shift.clock.closed();
        for (at, ring) in shift.schedule.due(now) {
            if self.ring_live(&ring, Some(at), closed) {
                shift.pending.push_back(ring);
            }
        }
    }

    /// Whether a ring still has something to be about: its truck still on
    /// the road at `at` (when given) and still short of hours, for an hours
    /// call; or its driver still free. Driver and hot-load calls are dead
    /// once the desk is closed.
    fn ring_live(&self, ring: &Ring, at: Option<i64>, closed: bool) -> bool {
        match ring {
            Ring::Incident(i) if i.kind == crate::incidents::Kind::Detention => {
                self.trips.iter().any(|t| t.id == i.trip_id && t.dock_hold)
            }
            Ring::Incident(i) => self.trips.iter().any(|t| {
                t.id == i.trip_id
                    && at.is_none_or(|at| at < t.eta())
                    && (i.kind != crate::incidents::Kind::HoursShort || t.hours_short())
            }),
            Ring::Call(_) if closed => false,
            Ring::Call(
                crate::phone::Call::Sick { driver_id }
                | crate::phone::Call::PersonalDay { driver_id },
            ) => self
                .driver(*driver_id)
                .is_some_and(|d| d.status == Status::Idle),
            Ring::Call(crate::phone::Call::HotLoad { .. }) => true,
            Ring::Call(crate::phone::Call::Empty { driver_id }) => self
                .driver(*driver_id)
                .is_some_and(|d| d.status == Status::Idle),
            Ring::Call(crate::phone::Call::QuitWarning { driver_id }) => {
                self.driver(*driver_id).is_some()
            }
        }
    }

    /// Drop rings whose subject went away (a canceled pickup, a driver who
    /// took a load), so the phone never rings for nothing.
    pub fn drop_dead_rings(&self, shift: &mut Shift) {
        let closed = shift.clock.closed();
        shift.pending.retain(|r| self.ring_live(r, None, closed));
    }

    /// Delay a trip for `cause`; its later rings move with it, and a delay
    /// that breaks the driver's hours rings the phone.
    pub fn add_delay(&mut self, shift: &mut Shift, trip_id: u32, minutes: f64, cause: &str) {
        let now = shift.now(self.day);
        let at = now + 30;
        let Some(trip) = self.trip_mut(trip_id) else {
            return;
        };
        trip.delay_min += minutes;
        trip.delays.push(crate::trip::Delay {
            min: minutes,
            cause: cause.into(),
        });
        shift.schedule.delay_where(
            now,
            minutes.round() as i64,
            |r| matches!(r, Ring::Incident(i) if i.trip_id == trip_id),
        );
        if trip.hours_short() {
            shift.schedule.push(
                at,
                Ring::Incident(crate::incidents::Incident {
                    trip_id,
                    kind: crate::incidents::Kind::HoursShort,
                }),
            );
        }
    }

    /// Pick up the phone: the oldest ring still live, five minutes of desk
    /// time. Rings whose subject went away while they waited are dropped; a
    /// truck arriving meanwhile doesn't kill a ring that was live when due.
    pub fn answer(&mut self, shift: &mut Shift) -> Result<Ring, ActError> {
        let closed = shift.clock.closed();
        while let Some(ring) = shift.pending.pop_front() {
            if self.ring_live(&ring, None, closed) {
                shift.clock.spend(Action::TakeCall);
                return Ok(ring);
            }
        }
        Err(ActError::NoSuchItem)
    }

    pub fn resolve(
        &mut self,
        shift: &mut Shift,
        ring: &Ring,
        choice: usize,
        world: &World,
    ) -> Vec<String> {
        let lines = match ring {
            Ring::Incident(i) => {
                let now = shift.now(self.day);
                match crate::incidents::choices(self, i, now, world).get(choice) {
                    Some(&c) => crate::incidents::apply(self, shift, i, c, world),
                    None => vec![ActError::NoSuchItem.spoken()],
                }
            }
            Ring::Call(c) => match crate::phone::choices(c).get(choice) {
                Some(&ch) => crate::phone::apply(self, shift, c, ch, world),
                None => vec![ActError::NoSuchItem.spoken()],
            },
        };
        self.ring_due(shift);
        lines
    }

    pub fn assign(
        &mut self,
        shift: &mut Shift,
        load: usize,
        driver_id: u32,
        world: &World,
    ) -> Result<Vec<String>, ActError> {
        self.ensure_open(shift)?;
        let job = &shift.board.get(load).ok_or(ActError::NoSuchItem)?.job;
        let driver = self.driver(driver_id).ok_or(ActError::NoSuchItem)?;
        if matches!(driver.status, Status::OnLoad | Status::OutOnRun) {
            return self.queue_next(shift, load, driver_id, world);
        }
        if let Some(why) = refusal(job, driver, world) {
            return Err(ActError::Refused(why));
        }
        shift.clock.spend(Action::Assign);
        let now = shift.now(self.day);
        let id = self.take_id();
        let loaded = shift.board.remove(load);
        let weather = loaded.weather.clone();
        let driver = self.driver(driver_id).unwrap();
        let mut trip = Trip::start(id, driver, loaded.job, now, world);
        trip.hot = loaded.hot;
        let mut lines = vec![format!(
            "{} is headed to the shipper in {}: {} to {}, arriving {}.",
            driver.name,
            world.spoken_city(&trip.job.origin, None),
            trip.job.cargo.label,
            world.spoken_city(&trip.job.destination, None),
            abs_text(trip.eta(), self.day)
        )];
        self.driver_mut(driver_id).unwrap().status = Status::OnLoad;
        shift.assigned += 1;
        if trip.deadhead_mi >= 1.0 {
            lines[0] = lines[0].replacen(
                " is headed to the shipper in ",
                &format!(
                    " is deadheading {} miles to the shipper in ",
                    trip.deadhead_mi.round()
                ),
                1,
            );
        }
        self.push_trip(shift, trip, &weather);
        self.ring_due(shift);
        Ok(lines)
    }

    /// Pre-plan: a rolling driver's next load, started on delivery if legal.
    fn queue_next(
        &mut self,
        shift: &mut Shift,
        load: usize,
        driver_id: u32,
        world: &World,
    ) -> Result<Vec<String>, ActError> {
        let d = self.driver(driver_id).ok_or(ActError::NoSuchItem)?;
        let job = &shift.board[load].job;
        let locked = job.locked_reason(&d.endorsements(), d.level(), None, false);
        if !locked.is_empty() {
            return Err(ActError::Refused(format!(
                "{} can't take it. {locked}",
                d.name
            )));
        }
        let name = d.name.clone();
        let Some(t) = self.trips.iter().find(|t| t.driver_id == driver_id) else {
            return Err(ActError::Refused(format!("{name} is {}.", d.status.text())));
        };
        if t.next.is_some() {
            return Err(ActError::Refused(format!(
                "{name} already has a next load queued."
            )));
        }
        let arrive = abs_text(t.eta(), self.day);
        shift.clock.spend(Action::Assign);
        let now = shift.now(self.day);
        let loaded = shift.board.remove(load);
        let line = format!(
            "Queued for {name} after the delivery at {arrive}: {} from {} to {}. It starts then if the hours allow.",
            loaded.job.cargo.label,
            world.spoken_city(&loaded.job.origin, None),
            world.spoken_city(&loaded.job.destination, None),
        );
        let t = self
            .trips
            .iter_mut()
            .find(|t| t.driver_id == driver_id)
            .unwrap();
        t.next = Some(loaded.job);
        t.next_queued_at = now;
        self.ring_due(shift);
        Ok(vec![line])
    }

    pub fn check_truck(
        &mut self,
        shift: &mut Shift,
        trip_id: u32,
        world: &World,
    ) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let trip = self
            .trips
            .iter()
            .find(|t| t.id == trip_id)
            .ok_or(ActError::NoSuchItem)?;
        shift.clock.spend(Action::CheckTruck);
        let name = self
            .driver(trip.driver_id)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let line = trip.status_line(&name, shift.now(self.day), self.day, world);
        self.ring_due(shift);
        Ok(line)
    }

    pub fn interview(&mut self, shift: &mut Shift, idx: usize) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let a = shift.applicants.get_mut(idx).ok_or(ActError::NoSuchItem)?;
        if a.interviewed {
            return Ok(format!("Interview already complete. {}", a.line()));
        }
        shift.clock.spend(Action::Interview);
        a.interviewed = true;
        let line = a.line();
        self.ring_due(shift);
        Ok(line)
    }

    pub fn hire(&mut self, shift: &mut Shift, idx: usize) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let a = shift.applicants.get(idx).ok_or(ActError::NoSuchItem)?;
        if self.drivers.len() >= crate::hiring::ROSTER_CAP {
            return Err(ActError::RosterFull);
        }
        if self.funds < a.signing {
            return Err(ActError::NoFunds);
        }
        let mut a = shift.applicants.remove(idx);
        self.funds -= a.signing;
        a.driver.id = self.take_id();
        a.driver.status = Status::OffDuty;
        let line = format!(
            "{} signed on for {}. The drug test and background checks clear overnight; they start tomorrow morning.",
            a.driver.name,
            crate::money(a.signing)
        );
        self.drivers.push(a.driver);
        Ok(line)
    }

    /// Close the desk, settle what arrives overnight, rest everyone, and
    /// roll to the next morning. Returns what was said.
    pub fn close_day(&mut self, shift: &mut Shift, world: &World) -> Vec<String> {
        shift.clock.minute = shift.clock.minute.max(SHIFT_START);
        let next_morning = abs_min(self.day + 1, SHIFT_START);
        let mut news = Vec::new();
        // A detention call due after the close never rang: the driver waited.
        for t in self.trips.iter_mut().filter(|t| t.dock_hold) {
            t.dock_hold = false;
            let waited = crate::incidents::DOCK_CALL_AFTER_MIN + crate::incidents::DETENTION_MIN;
            t.dock_min += waited;
            t.delay_min += waited;
            t.delays.push(crate::trip::Delay {
                min: waited,
                cause: crate::incidents::DETENTION_CAUSE.into(),
            });
        }
        // Overnight: free drivers rest; trucks still out keep rolling.
        for d in &mut self.drivers {
            let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
            if free && d.rest_note.is_none() {
                d.hos.sleep();
            }
        }
        news.extend(self.settle(shift, next_morning, world));
        // In by 8 PM is 10 hours off by 6 AM; later than that, the break runs late.
        let rest_by = abs_min(self.day, 20 * 60);
        for d in &mut self.drivers {
            let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
            if free && d.rest_note.is_none() && d.available_at > rest_by {
                d.available_at += 570;
                d.rest_note = Some("a 10-hour break".into());
                d.status = Status::OffDuty;
            } else if free && d.rest_note.is_none() {
                d.hos.sleep();
            }
        }
        self.wake(next_morning);
        let rolling: Vec<u32> = self.trips.iter().map(|t| t.driver_id).collect();
        for id in rolling {
            if let Some(d) = self.driver_mut(id) {
                d.status = Status::OutOnRun;
            }
        }
        let truck_costs = TRUCK_COST_PER_DAY * self.drivers.len() as f64;
        self.funds -= truck_costs;
        news.push(format!(
            "Fleet overhead today: {}.",
            crate::money(truck_costs)
        ));
        let home = self.home_time_check(shift, &mut news);
        let (met, why) = self.goal_result(&shift.goal, shift, world);
        if met {
            self.reputation += 1.0;
        }
        let goal_result = format!(" Goal {}.", if met { "met" } else { "missed" });
        let story = self.day_story(shift, world, &home, met, &why);
        for d in &mut self.drivers {
            if d.status == Status::OffDuty && d.rest_note.is_none() {
                d.status = Status::Idle;
            }
        }
        self.record.days_worked += 1;
        self.day += 1;
        self.market.advance_to(self.day);
        self.news = news;
        shift.clock.minute = SHIFT_END;
        // Loads that delivered earlier in the day count too, not just the close.
        let delivered = shift.log.len() as u32;
        let on_time = shift.log.iter().filter(|l| l.on_time).count();
        let mut out = story;
        out.push(format!(
            "Day closed. {}, {} on time. Funds now {}.{}",
            deliveries_text(delivered),
            on_time,
            crate::money(self.funds),
            goal_result
        ));
        out
    }

    pub fn goal_text(&self, goal: &DayGoal, world: &World) -> String {
        match goal {
            DayGoal::OnTime => "every load that delivers today is on time".into(),
            DayGoal::RevenuePerMile(t) => {
                format!("{} or more a mile, empty miles included", money_cents(*t))
            }
            DayGoal::Deadhead(p) => format!("keep empty miles under {p} percent of all miles"),
            DayGoal::Home { driver_id } => format!(
                "get {} home to {} tonight, as promised",
                self.driver(*driver_id)
                    .map(|d| d.name.as_str())
                    .unwrap_or("your driver"),
                world.spoken_city(&self.terminal, None)
            ),
        }
    }

    /// Whether today's goal held, and the plain reason.
    pub fn goal_result(&self, goal: &DayGoal, shift: &Shift, world: &World) -> (bool, String) {
        let n = shift.log.len();
        let on_time = shift.log.iter().filter(|l| l.on_time).count();
        let loaded: f64 = shift.log.iter().map(|l| l.loaded_mi).sum();
        let empty: f64 = shift.log.iter().map(|l| l.deadhead_mi).sum();
        let gross: f64 = shift.log.iter().map(|l| l.gross).sum();
        let total = loaded + empty;
        match goal {
            _ if n == 0 && !matches!(goal, DayGoal::Home { .. }) => {
                (false, "nothing was delivered today".into())
            }
            DayGoal::OnTime => (on_time == n, format!("{on_time} of {n} deliveries on time")),
            DayGoal::RevenuePerMile(t) => {
                let rpm = if total > 0.0 { gross / total } else { 0.0 };
                (
                    rpm >= *t,
                    format!("{} a mile against {}", money_cents(rpm), money_cents(*t)),
                )
            }
            DayGoal::Deadhead(p) => {
                let pct = if total > 0.0 {
                    empty / total * 100.0
                } else {
                    0.0
                };
                (
                    pct < *p,
                    format!("empty miles were {} percent", pct.round()),
                )
            }
            DayGoal::Home { driver_id } => match self.driver(*driver_id) {
                Some(d) if d.location == self.terminal => {
                    (true, format!("{} made it home", d.name))
                }
                Some(d) => (
                    false,
                    format!(
                        "{} is still in {}",
                        d.name,
                        world.spoken_city(&d.location, None)
                    ),
                ),
                None => (false, "that driver is gone".into()),
            },
        }
    }

    /// Home time at the close: who got home, who missed it (goodwill), and
    /// who quit after a warning.
    fn home_time_check(&mut self, _shift: &Shift, news: &mut Vec<String>) -> HomeNight {
        let mut night = HomeNight::default();
        let (day, terminal) = (self.day, self.terminal.clone());
        for d in &mut self.drivers {
            let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
            let due = d.home_due_day() <= day;
            if free && d.location == terminal {
                if due {
                    night.kept.push(d.name.clone());
                }
                d.last_home_day = day;
                continue;
            }
            if !due {
                continue;
            }
            d.adjust_goodwill(-HOME_MISS_GOODWILL);
            night.missed.push(d.name.clone());
            news.push(format!(
                "{} missed promised home time. Goodwill is down to {}.",
                d.name, d.goodwill
            ));
            if free && d.quit_warned && d.goodwill <= QUIT_GOODWILL {
                night.quit.push(d.id);
            }
        }
        for id in &night.quit {
            if let Some(d) = self.driver(*id) {
                news.push(format!(
                    "{} quit. They were promised home and kept out on the road.",
                    d.name
                ));
                night.quit_names.push(d.name.clone());
            }
        }
        let quit = night.quit.clone();
        self.drivers.retain(|d| !quit.contains(&d.id));
        night
    }

    /// Two or three sentences on how the day went and why.
    fn day_story(
        &self,
        shift: &Shift,
        world: &World,
        home: &HomeNight,
        met: bool,
        why: &str,
    ) -> Vec<String> {
        let _ = world;
        let name = |id: u32| {
            self.driver(id)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| "A driver".into())
        };
        let log = &shift.log;
        let n = log.len();
        let late: Vec<&Delivered> = log.iter().filter(|l| !l.on_time).collect();
        let mut out = vec![match (n, late.first()) {
            (0, _) => "No loads delivered today, so the trucks earned nothing on the road.".into(),
            (_, None) => format!("All {} came in on time.", deliveries_text(n as u32)),
            (_, Some(l)) => format!(
                "{} of {} were on time; {}'s load ran late because of {}.",
                n - late.len(),
                deliveries_text(n as u32),
                name(l.driver_id),
                l.reason
                    .clone()
                    .unwrap_or_else(|| "an appointment too tight for the run".into())
            ),
        }];
        let loaded: f64 = log.iter().map(|l| l.loaded_mi).sum();
        let empty: f64 = log.iter().map(|l| l.deadhead_mi).sum();
        let gross: f64 = log.iter().map(|l| l.gross).sum();
        if loaded + empty > 0.0 {
            let worst = log
                .iter()
                .filter(|l| l.deadhead_mi >= 50.0)
                .max_by(|a, b| a.deadhead_mi.total_cmp(&b.deadhead_mi));
            let mostly = worst
                .map(|l| {
                    format!(
                        ", mostly {}'s {} empty miles",
                        name(l.driver_id),
                        l.deadhead_mi.round()
                    )
                })
                .unwrap_or_default();
            out.push(format!(
                "Empty miles were {} percent of {}{mostly}, for {} a mile overall.",
                (empty / (loaded + empty) * 100.0).round(),
                (loaded + empty).round(),
                money_cents(gross / (loaded + empty))
            ));
        }
        out.push(if !home.quit_names.is_empty() {
            format!(
                "{} quit after missing home time again.",
                home.quit_names.join(" and ")
            )
        } else if !home.missed.is_empty() {
            format!(
                "{} missed promised home time, and it cost goodwill.",
                home.missed.join(" and ")
            )
        } else if !home.kept.is_empty() {
            format!("{} got home as promised.", home.kept.join(" and "))
        } else {
            format!(
                "The goal was {} because {why}.",
                if met { "met" } else { "missed" }
            )
        });
        out
    }

    /// The one thing that most needs the dispatcher now.
    pub fn needs_now(&self, shift: &Shift, world: &World) -> String {
        if !shift.pending.is_empty() {
            return "The phone is ringing. Press Enter to answer.".into();
        }
        let now = shift.now(self.day);
        if let Some(t) = self.trips.iter().find(|t| !t.on_time()) {
            return format!(
                "{}'s load to {} is at risk: {}.",
                self.driver(t.driver_id)
                    .map(|d| d.name.as_str())
                    .unwrap_or("A driver"),
                world.spoken_city(&t.job.destination, None),
                t.timing_text()
            );
        }
        if let Some(d) = self.drivers.iter().find(|d| {
            d.status == Status::Idle && d.location != self.terminal && d.home_due_day() <= self.day
        }) {
            return format!(
                "{} is due home in {} tonight and is in {}.",
                d.name,
                world.spoken_city(&self.terminal, None),
                world.spoken_city(&d.location, None)
            );
        }
        if let Some(d) = self
            .drivers
            .iter()
            .find(|d| d.status == Status::Idle && d.available_at <= now)
        {
            return format!(
                "{} is empty in {}. Find them a load.",
                d.name,
                world.spoken_city(&d.location, None)
            );
        }
        match self.next_event_at(shift) {
            Some(t) if t <= abs_min(self.day, SHIFT_END) => format!(
                "Nothing needs you right now. Next thing happens at {}.",
                abs_text(t, self.day)
            ),
            _ => "Nothing needs you. Close the day when you're ready.".into(),
        }
    }
}

#[derive(Default)]
struct HomeNight {
    kept: Vec<String>,
    missed: Vec<String>,
    quit: Vec<u32>,
    quit_names: Vec<String>,
}

/// "$2.14".
fn money_cents(x: f64) -> String {
    format!("${:.2}", x)
}

fn deliveries_text(count: u32) -> String {
    match count {
        1 => "1 delivery".into(),
        n => format!("{n} deliveries"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ff_core::data::world::get_world;

    pub(crate) fn game() -> Game {
        Game::new("northstar", 42, get_world())
    }

    #[test]
    fn a_driver_call_due_before_the_close_is_dead_at_the_close() {
        let mut g = game();
        let mut s = g.morning(get_world());
        s.schedule = Default::default();
        let id = g.drivers[0].id;
        assert_eq!(g.driver(id).unwrap().status, Status::Idle);
        s.schedule.push(
            abs_min(g.day, SHIFT_END - 60),
            Ring::Call(crate::phone::Call::Sick { driver_id: id }),
        );
        s.clock.minute = SHIFT_END;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty());
        s.pending
            .push_back(Ring::Call(crate::phone::Call::Sick { driver_id: id }));
        assert_eq!(g.answer(&mut s), Err(ActError::NoSuchItem));
    }

    /// First (load, driver) pair the gates allow.
    pub(crate) fn legal_pair(g: &Game, s: &Shift) -> (usize, u32) {
        let w = get_world();
        for (i, l) in s.board.iter().enumerate() {
            for d in &g.drivers {
                if refusal(&l.job, d, w).is_none() {
                    return (i, d.id);
                }
            }
        }
        panic!("seed 42 has no legal pair; pick another seed");
    }

    #[test]
    fn new_career_matches_the_carrier() {
        let g = game();
        assert_eq!(g.drivers.len(), 3);
        assert_eq!(g.funds, STARTING_FUNDS);
        assert_eq!(
            g.terminal,
            get_world().resolve_city_key(start_option(Some("northstar")).default_city)
        );
    }

    #[test]
    fn briefing_names_drivers_loads_and_funds() {
        let g = game();
        let s = g.morning(get_world());
        let text = g.briefing(&s, get_world()).join(" ");
        assert!(text.contains("Day 1"));
        assert!(text.contains("Funds $25,000"));
        assert!(text.contains(&g.drivers[0].name));
        assert!(text.contains("8 loads on the board"));
    }

    #[test]
    fn assign_costs_ten_minutes_and_puts_a_driver_on_the_road() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        let lines = g.assign(&mut s, load, driver, w).unwrap();
        assert!(lines[0].contains("is headed to the shipper"));
        assert_eq!(lines.len(), 1, "no goal chatter after a dispatch");
        assert_eq!(s.clock.minute, SHIFT_START + 10);
        assert_eq!(s.board.len(), 7);
        assert_eq!(g.driver(driver).unwrap().status, Status::OnLoad);
        assert_eq!(g.trips.len(), 1);
    }

    #[test]
    fn refused_assignment_spends_no_time() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        for d in &mut g.drivers {
            d.status = Status::OffDuty;
        }
        let err = g.assign(&mut s, 0, g.drivers[0].id, w).unwrap_err();
        assert!(matches!(err, ActError::Refused(_)));
        assert_eq!(s.clock.minute, SHIFT_START);
        assert_eq!(s.board.len(), 8);
    }

    #[test]
    fn closed_desk_refuses_actions_but_the_day_still_closes() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.clock.minute = SHIFT_END;
        let (load, driver) = (0, g.drivers[0].id);
        assert_eq!(g.assign(&mut s, load, driver, w), Err(ActError::Closed));
        g.close_day(&mut s, w);
        assert_eq!(g.day, 2);
    }

    /// Dispatch a legal pair, then move the trip so it arrives at
    /// `minute_of_day` today. Returns the driver's id.
    fn arriving_at(g: &mut Game, s: &mut Shift, minute_of_day: u32) -> u32 {
        let (load, driver) = legal_pair(g, s);
        g.assign(s, load, driver, get_world()).unwrap();
        let t = g.trips.last_mut().unwrap();
        t.depart = abs_min(1, minute_of_day) - t.plan_min.round() as i64;
        assert_eq!(t.eta(), abs_min(1, minute_of_day));
        driver
    }

    #[test]
    fn a_trip_in_by_eight_pm_leaves_its_driver_rested() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let d = arriving_at(&mut g, &mut s, 19 * 60);
        g.close_day(&mut s, w);
        assert_eq!(g.driver(d).unwrap().hos.driving_min, 0.0);
        assert_eq!(g.driver(d).unwrap().status, Status::Idle);
    }

    #[test]
    fn a_trip_in_after_eight_pm_keeps_its_wheel_time_overnight() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let d = arriving_at(&mut g, &mut s, 22 * 60);
        g.close_day(&mut s, w);
        let hos = &g.driver(d).unwrap().hos;
        assert!(hos.driving_min > 0.0, "no 10 hours off before 6 AM");
        assert!(
            g.drivers
                .iter()
                .filter(|x| x.id != d)
                .all(|x| x.hos.driving_min == 0.0),
            "everyone else rests"
        );
    }

    #[test]
    fn overnight_settles_arrivals_and_carries_long_runs() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        let eta = g.trips[0].eta();
        let funds = g.funds;
        let news = g.close_day(&mut s, w);
        if eta <= abs_min(2, SHIFT_START) {
            assert_eq!(g.record.delivered, 1);
            assert!(g.trips.is_empty());
            assert!(news[0].contains("1 delivery"));
            assert!(g.news[0].contains("delivered"));
            assert!(g.funds > funds - TRUCK_COST_PER_DAY * 3.0);
        } else {
            assert_eq!(g.driver(driver).unwrap().status, Status::OutOnRun);
            assert_eq!(g.trips.len(), 1);
        }
        assert_eq!(g.day, 2);
        assert!(
            g.drivers.iter().all(|d| d.hos.driving_min == 0.0),
            "overnight reset"
        );
    }

    #[test]
    fn arrived_trip_pays_operating_costs_before_fleet_overhead() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let driver = arriving_at(&mut g, &mut s, 19 * 60);
        let trip = g.trips[0].clone();
        let hours_taken = (trip.eta() - trip.depart) as f64 / 60.0;
        let gross = trip.job.payout(hours_taken, 0.0, 0.0);
        let driver_pay = gross * pay_plan_for_key(Some(&g.carrier_key)).pay_share;
        let operating_costs = trip.job.distance_mi * OPERATING_COST_PER_MILE;
        let funds_before = g.funds;
        let expected_funds = funds_before + gross
            - driver_pay
            - operating_costs
            - TRUCK_COST_PER_DAY * g.drivers.len() as f64;

        g.close_day(&mut s, w);

        assert!((g.funds - expected_funds).abs() < 0.01);
        let news = g
            .news
            .iter()
            .find(|line| line.contains(" delivered "))
            .unwrap();
        assert!(news.contains("to the carrier after"));
        assert!(news.contains(&format!(
            "{} in operating costs",
            crate::money(operating_costs)
        )));
        assert!(g
            .news
            .iter()
            .any(|line| line.starts_with("Fleet overhead today:")));
        assert_eq!(g.driver(driver).unwrap().status, Status::Idle);
    }

    #[test]
    fn repeating_an_interview_reveals_the_card_without_spending_more_time() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        g.interview(&mut s, 0).unwrap();
        let after_first = s.clock.minute;
        let line = g.interview(&mut s, 0).unwrap();
        assert_eq!(s.clock.minute, after_first);
        assert!(line.starts_with("Interview already complete."));
    }

    fn one_delivery(driver_id: u32, on_time: bool, deadhead_mi: f64) -> Delivered {
        Delivered {
            driver_id,
            on_time,
            reason: (!on_time).then(|| crate::incidents::DETENTION_CAUSE.into()),
            gross: 1000.0,
            loaded_mi: 400.0,
            deadhead_mi,
        }
    }

    #[test]
    fn the_day_goal_is_a_dispatcher_goal_without_a_streak_bonus() {
        let w = get_world();
        let mut g = game();
        g.record.dispatch_streak = 4;
        g.normalize();
        assert_eq!(g.record.dispatch_streak, 0, "old streaks migrate away");
        let mut s = g.morning(w);
        s.schedule = Default::default();
        s.goal = DayGoal::OnTime;
        let id = g.drivers[0].id;
        s.log.push(one_delivery(id, true, 0.0));
        let funds = g.funds;
        let out = g.close_day(&mut s, w).join(" ");
        assert!(out.contains("Goal met."), "{out}");
        assert_eq!(g.record.dispatch_streak, 0);
        assert!(g.funds <= funds, "no cash bonus");
        let (met, why) = g.goal_result(&DayGoal::Deadhead(15.0), &s, w);
        assert!(met, "{why}");
        s.log.push(one_delivery(id, false, 300.0));
        let (met, why) = g.goal_result(&DayGoal::Deadhead(15.0), &s, w);
        assert!(!met && why.contains("percent"), "{why}");
    }

    #[test]
    fn the_close_tells_the_day_in_two_or_three_sentences_with_the_late_cause() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = g.drivers[0].id;
        s.log.push(one_delivery(id, true, 0.0));
        s.log.push(one_delivery(id, false, 120.0));
        let out = g.close_day(&mut s, w);
        let story = &out[..out.len() - 1];
        assert!((2..=3).contains(&story.len()), "{out:?}");
        assert!(
            story[0].contains("load ran late because of the 4-and-a-half-hour wait at the shipper"),
            "{out:?}"
        );
        assert!(
            out.last()
                .unwrap()
                .starts_with("Day closed. 2 deliveries, 1 on time."),
            "{out:?}"
        );
    }

    #[test]
    fn missing_home_time_costs_goodwill_and_a_warned_driver_quits() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let away = g.drivers[1].clone();
        let d = g.driver_mut(away.id).unwrap();
        d.location = "denver".into();
        d.last_home_day = -30;
        let before = d.goodwill;
        g.close_day(&mut s, w);
        let d = g.driver(away.id).unwrap();
        assert_eq!(d.goodwill, before - HOME_MISS_GOODWILL);
        assert!(g
            .news
            .iter()
            .any(|l| l.contains("missed promised home time")));
        let d = g.driver_mut(away.id).unwrap();
        d.goodwill = QUIT_WARN_GOODWILL;
        let s = g.morning(w);
        assert!(s
            .schedule
            .clone()
            .due(i64::MAX)
            .iter()
            .any(|(_, r)| matches!(
                r,
                Ring::Call(crate::phone::Call::QuitWarning { driver_id }) if *driver_id == away.id
            )));
        assert!(matches!(s.goal, DayGoal::Home { driver_id } if driver_id == away.id));
        let mut s = s;
        s.schedule = Default::default();
        let d = g.driver_mut(away.id).unwrap();
        d.quit_warned = true;
        d.goodwill = QUIT_GOODWILL;
        g.close_day(&mut s, w);
        assert!(g.driver(away.id).is_none(), "quit after the warning");
    }

    #[test]
    fn a_mid_day_arrival_settles_leaves_the_truck_there_and_rings() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let driver = arriving_at(&mut g, &mut s, 12 * 60);
        g.trips[0].dock_hold = false;
        s.schedule = Default::default();
        let dest = g.trips[0].job.destination.clone();
        let lines = g.advance(&mut s, w);
        assert_eq!(s.clock.minute, 12 * 60);
        assert!(lines.iter().any(|l| l.contains(" delivered ")), "{lines:?}");
        assert!(g.trips.is_empty());
        let d = g.driver(driver).unwrap();
        assert_eq!(d.location, dest);
        if d.rest_note.is_none() {
            g.advance(&mut s, w);
            assert!(matches!(
                s.pending.front(),
                Some(Ring::Call(crate::phone::Call::Empty { .. }))
            ));
            assert!(s.board.iter().any(|l| l.job.origin == dest));
        }
    }

    #[test]
    fn wait_with_nothing_due_runs_to_six_and_needs_now_says_so() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        assert!(g.needs_now(&s, w).contains("is empty in"));
        let lines = g.advance(&mut s, w);
        assert!(lines[0].starts_with("Nothing else is due"), "{lines:?}");
        assert!(s.clock.closed());
    }

    #[test]
    fn a_rolling_driver_gets_a_queued_next_load() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let (load, driver) = legal_pair(&g, &s);
        let d = g.driver(driver).unwrap().clone();
        let can = |l: &Load| {
            l.job
                .locked_reason(&d.endorsements(), d.level(), None, false)
                .is_empty()
        };
        let next = (0..s.board.len())
            .find(|&i| i != load && can(&s.board[i]))
            .unwrap();
        g.assign(&mut s, load, driver, w).unwrap();
        let next = if next > load { next - 1 } else { next };
        let lines = g.assign(&mut s, next, driver, w).unwrap();
        assert!(lines[0].starts_with("Queued for"), "{lines:?}");
        assert!(g.trips[0].next.is_some());
        let other = (0..s.board.len()).find(|&i| can(&s.board[i])).unwrap();
        assert!(
            g.assign(&mut s, other, driver, w).is_err(),
            "one queued load"
        );
    }

    #[test]
    fn old_saves_put_every_driver_at_the_terminal() {
        let mut g = game();
        let json = serde_json::to_value(&g.drivers[0]).unwrap();
        let mut map = json.as_object().unwrap().clone();
        for k in [
            "location",
            "available_at",
            "rest_note",
            "home",
            "last_home_day",
            "quit_warned",
        ] {
            map.remove(k);
        }
        g.drivers[0] = serde_json::from_value(serde_json::Value::Object(map)).unwrap();
        g.normalize();
        assert_eq!(g.drivers[0].location, g.terminal);
        assert_eq!(g.drivers[0].last_home_day, g.day);
    }
}
