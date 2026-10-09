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

/// The revenue goal is this share of the day's board rate per loaded mile,
/// so it stays a stretch when empty miles are counted.
pub const GOAL_REVENUE_SHARE: f64 = 0.85;
/// Shippers expect 95 percent on time once there's a record to judge.
pub const ON_TIME_FLOOR: f64 = 95.0;
pub const ON_TIME_FLOOR_AFTER: u32 = 20;
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
        for (i, d) in game.drivers.iter_mut().enumerate() {
            d.home = Some(if i == 0 {
                crate::roster::HomePattern::Local
            } else {
                crate::roster::HomePattern::Regional
            });
        }
        game.normalize();
        game
    }

    /// Owed home time and not given it: the close will count a miss.
    pub fn due_home(&self, d: &Driver) -> bool {
        let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
        let local_home =
            d.pattern() == crate::roster::HomePattern::Local && d.location == self.terminal;
        free && !d.home_given() && !local_home && d.home_due_day() <= self.day
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
            if d.hos_at == 0 {
                d.hos_at = abs_min(day, SHIFT_START);
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
            let busy = matches!(d.status, Status::OnLoad | Status::OutOnRun) || d.home_given();
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
            .filter(|d| d.rest_note.is_some() || d.status == Status::Unloading)
            .map(|d| d.available_at);
        let homes = self
            .drivers
            .iter()
            .filter(|d| d.home_given() && d.location != self.terminal)
            .map(|d| d.home_at);
        shift
            .schedule
            .next_at()
            .into_iter()
            .chain(arrivals)
            .chain(rests)
            .chain(homes)
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
        lines.extend(self.arrive_home(at, world));
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
        let terminal = self.terminal.clone();
        for d in &mut self.drivers {
            if d.status == Status::Unloading && d.available_at <= now {
                d.status = Status::Idle;
                woke.push(d.id);
                continue;
            }
            let Some(note) = d.rest_note.clone() else {
                continue;
            };
            if d.available_at > now {
                continue;
            }
            if d.home_given() {
                // Home time of 34 hours or more is a restart of the 70-hour cycle.
                if d.available_at - d.home_at >= 34 * 60 {
                    d.hos.restart();
                } else {
                    d.hos.sleep();
                }
                d.location = terminal.clone();
                d.last_home_day = d.available_at.div_euclid(crate::desk::MIN_PER_DAY);
                d.quit_warned = false;
                d.home_at = 0;
            } else if note.contains("34-hour") {
                d.hos.restart();
            } else {
                d.hos.sleep();
            }
            d.rest_note = None;
            d.hos_at = d.hos_at.max(d.available_at);
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
            log_trip(d, &trip);
            let free_at = end + 30;
            d.hos_at = free_at;
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
                (free_at + 34 * 60, Some("a 34-hour restart".to_string()))
            } else if d.legal_left_min() < 60.0 {
                (free_at + 600, Some("a 10-hour break".to_string()))
            } else {
                (free_at, None)
            };
            d.status = if d.rest_note.is_some() {
                Status::OffDuty
            } else {
                Status::Unloading
            };
            let home_next = std::mem::take(&mut d.home_next);
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
            if self
                .driver(id)
                .is_some_and(|d| d.status == Status::Unloading)
            {
                self.add_city_offers(shift, &trip.job.destination, world);
            }
            if home_next {
                lines.push(self.home_trip(id, free_at, world));
                continue;
            }
            // A warned driver still kept out walks off once the load is in.
            if let Some(d) = self
                .driver(id)
                .filter(|d| d.quit_warned && d.goodwill <= QUIT_GOODWILL)
            {
                lines.push(format!(
                    "{} delivered and quit. They were promised home and kept out on the road.",
                    d.name
                ));
                self.drivers.retain(|d| d.id != id);
                continue;
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
        match &rested.rest_note {
            Some(note) if note.contains("34-hour") => rested.hos.restart(),
            Some(_) => rested.hos.sleep(),
            None => {}
        }
        let need_h = crate::trip::plan(&job, &rested, world).need_h;
        if let Some(why) = crate::board::cycle_refusal(&job, &rested, world) {
            return (
                false,
                format!("{why} The queued load goes back to the broker."),
            );
        }
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

    /// Send a driver who isn't rolling home to the terminal for home time:
    /// now, or once their unload or break is over.
    pub fn send_home(
        &mut self,
        shift: &mut Shift,
        driver_id: u32,
        world: &World,
    ) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let now = shift.now(self.day);
        let d = self.driver_mut(driver_id).ok_or(ActError::NoSuchItem)?;
        if d.home_given() {
            return Err(ActError::Refused(format!(
                "{} already has home time.",
                d.name
            )));
        }
        if matches!(d.status, Status::OnLoad | Status::OutOnRun) {
            return Err(ActError::Refused(format!(
                "{} is {}. Send them home once they deliver.",
                d.name,
                d.status.text()
            )));
        }
        d.clock_to(now);
        match d.rest_note.take() {
            Some(note) if note.contains("34-hour") => d.hos.restart(),
            Some(_) => d.hos.sleep(),
            None => {}
        }
        let start = d.available_at.max(now);
        let line = self.home_trip(driver_id, start, world);
        self.ring_due(shift);
        Ok(line)
    }

    /// The driver deadheads home from `start`; home time runs from arrival:
    /// a night for local, a weekend for regional, days for over the road.
    pub fn home_trip(&mut self, driver_id: u32, start: i64, world: &World) -> String {
        let terminal = self.terminal.clone();
        let Some(d) = self.driver(driver_id) else {
            return ActError::NoSuchItem.spoken();
        };
        let (miles, minutes) = crate::trip::empty_leg(&d.location, &terminal, world);
        let rest = if minutes > d.legal_left_min() {
            crate::incidents::RESET_MIN
        } else {
            0.0
        };
        let home_at = start + (minutes + rest).round() as i64;
        let today = self.day;
        self.funds -= miles * OPERATING_COST_PER_MILE;
        let d = self.driver_mut(driver_id).unwrap();
        d.drive_with_breaks(minutes);
        let days_out = (home_at.div_euclid(crate::desk::MIN_PER_DAY) - d.last_home_day).max(0);
        let back = next_shift_start(home_at + d.pattern().home_min(days_out));
        d.status = Status::OffDuty;
        d.rest_note = Some("home time".into());
        d.home_at = home_at.max(1);
        d.home_next = false;
        d.available_at = back;
        let name = d.name.clone();
        let back = abs_text(back, today);
        if miles < 1.0 {
            d.location = terminal.clone();
            return format!(
                "{name} is home in {}, back at {back}.",
                world.spoken_city(&terminal, None)
            );
        }
        format!(
            "{name} is headed home to {}, {} miles empty, home about {}, back at {back}.",
            world.spoken_city(&terminal, None),
            miles.round(),
            abs_text(home_at, today)
        )
    }

    /// Drivers headed home who got there by `now`.
    pub fn arrive_home(&mut self, now: i64, world: &World) -> Vec<String> {
        let terminal = self.terminal.clone();
        let mut lines = Vec::new();
        for d in &mut self.drivers {
            if d.home_given() && d.home_at <= now && d.location != terminal {
                d.location = terminal.clone();
                lines.push(format!(
                    "{} is home in {}.",
                    d.name,
                    world.spoken_city(&terminal, None)
                ));
            }
        }
        lines
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
        let due_home = self.drivers.iter().find(|d| self.due_home(d));
        shift.goal = match (due_home, self.day % 3) {
            (Some(d), _) => DayGoal::Home { driver_id: d.id },
            (None, 0) => DayGoal::RevenuePerMile(revenue_goal(&shift.board)),
            (None, 1) => DayGoal::OnTime,
            _ => DayGoal::Deadhead(GOAL_DEADHEAD_PCT),
        };
        for d in &self.drivers {
            let overdue = self.day > d.home_due_day() && self.due_home(d);
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
            "Day {} at {}, {} terminal. {}, {}.",
            self.day,
            self.name,
            world.spoken_city(&self.terminal, None),
            crate::roster::WEEKDAYS[crate::roster::weekday(self.day) as usize],
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
            if self.due_home(d) {
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
        for d in &mut self.drivers {
            d.clock_to(now);
        }
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
            Ring::Call(crate::phone::Call::QuitWarning { driver_id }) => self
                .driver(*driver_id)
                .is_some_and(|d| !d.home_given() && !d.quit_warned),
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
        let now = shift.now(self.day);
        if let Some(d) = self.driver_mut(driver_id) {
            d.clock_to(now);
        }
        let job = &shift.board.get(load).ok_or(ActError::NoSuchItem)?.job;
        let driver = self.driver(driver_id).ok_or(ActError::NoSuchItem)?;
        if matches!(driver.status, Status::OnLoad | Status::OutOnRun) {
            return self.queue_next(shift, load, driver_id, world);
        }
        // Unloading drivers can be planned; they roll when the unload is done.
        let mut free = driver.clone();
        let unloading = free.status == Status::Unloading;
        if unloading {
            free.status = Status::Idle;
        }
        if let Some(why) = refusal(job, &free, world) {
            return Err(ActError::Refused(why));
        }
        shift.clock.spend(Action::Assign);
        let now = shift.now(self.day);
        let id = self.take_id();
        let mut loaded = shift.board.remove(load);
        let weather = loaded.weather.clone();
        let depart = if unloading {
            free.available_at.max(now)
        } else {
            now
        };
        loaded.job.deadline_game_h -= (depart - now) as f64 / 60.0;
        let driver = self.driver(driver_id).unwrap();
        let mut trip = Trip::start(id, &free, loaded.job, depart, world);
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
        let now = shift.now(self.day);
        let (after, start) = after_trip(t, d);
        let left_h = job.deadline_game_h - (start - now) as f64 / 60.0;
        if let Some(why) = crate::board::cycle_refusal(job, &after, world) {
            return Err(ActError::Refused(why));
        }
        if crate::trip::plan(job, &after, world).need_h > left_h {
            return Err(ActError::Refused(format!(
                "{name} can't make that appointment after delivering in {} at {arrive}, free at {}.",
                world.spoken_city(&t.job.destination, None),
                abs_text(start, self.day)
            )));
        }
        shift.clock.spend(Action::Assign);
        let loaded = shift.board.remove(load);
        let line = format!(
            "Queued for {name} after the delivery at {arrive}: {} from {} to {}, rolling at {}.",
            loaded.job.cargo.label,
            world.spoken_city(&loaded.job.origin, None),
            world.spoken_city(&loaded.job.destination, None),
            abs_text(start, self.day)
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
        // Overnight: free drivers are off duty until morning; trucks still
        // out keep rolling. Off-duty days add up toward a 34-hour restart.
        let closed_at = shift.now(self.day);
        for d in &mut self.drivers {
            d.clock_to(closed_at);
        }
        let mut rested = Vec::new();
        for d in &mut self.drivers {
            let free = matches!(d.status, Status::Idle | Status::OffDuty);
            if free && d.rest_note.is_none() {
                d.hos
                    .off_duty((next_morning - d.available_at.max(closed_at)) as f64);
                rested.push(d.id);
            }
        }
        news.extend(self.settle(shift, next_morning, world));
        news.extend(self.arrive_home(next_morning, world));
        // In by 8 PM is 10 hours off by 6 AM; later than that, the break runs late.
        let rest_by = abs_min(self.day, 20 * 60);
        for d in &mut self.drivers {
            let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
            if free && d.rest_note.is_none() && d.available_at > rest_by {
                d.available_at += 570;
                d.rest_note = Some("a 10-hour break".into());
                d.status = Status::OffDuty;
            } else if free && d.rest_note.is_none() && !rested.contains(&d.id) {
                d.hos
                    .off_duty((next_morning - d.available_at.min(next_morning)) as f64);
            }
        }
        self.wake(next_morning);
        for d in &mut self.drivers {
            if d.rest_note.is_none() && !matches!(d.status, Status::OnLoad | Status::OutOnRun) {
                d.hos_at = d.hos_at.max(next_morning);
            }
        }
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
            DayGoal::OnTime if self.record.delivered >= ON_TIME_FLOOR_AFTER => {
                format!("keep the on-time record at {ON_TIME_FLOOR} percent or better")
            }
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
            DayGoal::OnTime if self.record.delivered >= ON_TIME_FLOOR_AFTER => (
                self.record.on_time_pct() >= ON_TIME_FLOOR,
                format!(
                    "{on_time} of {n} were on time today, and the record is {} percent",
                    self.record.on_time_pct()
                ),
            ),
            DayGoal::OnTime => (
                on_time == n,
                format!("{on_time} of {n} deliveries were on time"),
            ),
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
                Some(d) if !self.due_home(d) && d.home_due_day() <= self.day => {
                    (true, format!("{} got home time", d.name))
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
            // Local drivers live at the terminal; anyone else is home only
            // when they were sent there.
            let local_home = free
                && d.location == terminal
                && d.pattern() == crate::roster::HomePattern::Local
                && !d.home_given();
            if d.home_given() || local_home {
                if due {
                    night.kept.push(d.name.clone());
                }
                if local_home {
                    d.last_home_day = day + 1;
                }
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
                "{} of {} were on time; {}'s load ran {}.",
                n - late.len(),
                deliveries_text(n as u32),
                name(l.driver_id),
                l.reason.clone().unwrap_or_else(|| {
                    "late because of an appointment too tight for the run".into()
                })
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
        if let Some(d) = self
            .drivers
            .iter()
            .find(|d| d.status == Status::Idle && self.due_home(d))
        {
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
/// Dollars a mile, empty miles included, to beat today: a share of what the
/// board pays per loaded mile, to the cent.
/// Log a delivered trip's hours: loading and any dock wait on duty, holds
/// off duty (the 14-hour window still runs), the run with the 10-hour
/// breaks it needed, then the 30-minute unload. A relieved trip logs only
/// the relief driver's part.
fn log_trip(d: &mut Driver, t: &Trip) {
    if t.logged_min <= 0.0 {
        d.hos.on_duty(30.0 + t.dock_min);
        d.hos.off_duty((t.delay_min - t.dock_min).max(0.0));
    }
    d.drive_with_breaks((t.drive_min - t.logged_min).max(0.0));
    d.hos.on_duty(30.0);
}

/// A rolling driver as they'll be after this trip: at the delivery city with
/// the trip logged and any break it calls for taken, and when they're free.
fn after_trip(t: &Trip, d: &Driver) -> (Driver, i64) {
    let mut after = d.clone();
    log_trip(&mut after, t);
    after.location = t.job.destination.clone();
    after.status = Status::Idle;
    let mut start = t.eta() + 30;
    let cycle_low = after
        .hos
        .cycle_remaining_min("realistic")
        .is_some_and(|m| m < 120.0);
    if cycle_low {
        after.hos.restart();
        start += 34 * 60;
    } else if after.legal_left_min() < 60.0 {
        after.hos.sleep();
        start += 600;
    }
    (after, start)
}

pub fn revenue_goal(board: &[Load]) -> f64 {
    let (pay, miles) = board.iter().fold((0.0, 0.0), |(p, m), l| {
        (p + l.job.pay, m + l.job.distance_mi)
    });
    if miles <= 0.0 {
        return 0.0;
    }
    (pay / miles * GOAL_REVENUE_SHARE * 100.0).round() / 100.0
}

/// The first 6 AM at or after `t`.
fn next_shift_start(t: i64) -> i64 {
    let day = t.div_euclid(crate::desk::MIN_PER_DAY);
    let start = abs_min(day, SHIFT_START);
    if t <= start {
        start
    } else {
        abs_min(day + 1, SHIFT_START)
    }
}

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
            reason: (!on_time).then(|| {
                format!(
                    "late by 2 hours, because of {}",
                    crate::incidents::DETENTION_CAUSE
                )
            }),
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
            story[0].contains(
                "load ran late by 2 hours, because of the 4-and-a-half-hour wait at the shipper"
            ),
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

    fn answer_warning(g: &mut Game, s: &mut Shift, id: u32, want: crate::phone::CallChoice) {
        let call = crate::phone::Call::QuitWarning { driver_id: id };
        let c = crate::phone::choices(&call)
            .iter()
            .position(|&c| c == want)
            .unwrap();
        g.resolve(s, &Ring::Call(call), c, get_world());
    }

    #[test]
    fn home_time_drives_home_then_a_weekend_restarts_the_cycle() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = g.drivers[1].id;
        g.driver_mut(id).unwrap().location = "saginaw_mi_us".into();
        let line = g.send_home(&mut s, id, w).unwrap();
        let d = g.driver(id).unwrap().clone();
        assert!(
            line.contains("headed home") && line.contains("miles empty"),
            "{line}"
        );
        assert_eq!(d.location, "saginaw_mi_us", "no teleport");
        assert!(d.home_at > s.now(g.day) + 120);
        assert!(d.hos.cycle_used_min() > 0.0, "the drive home is logged");
        assert!(g.arrive_home(d.home_at - 1, w).is_empty());
        assert!(g.arrive_home(d.home_at, w)[0].contains("is home"));
        assert_eq!(g.driver(id).unwrap().location, g.terminal);
        assert!(d.available_at >= d.home_at + 34 * 60, "a regional weekend");
        assert!(g.wake(d.available_at - 1).is_empty(), "not back early");
        g.wake(d.available_at);
        let d = g.driver(id).unwrap();
        assert_eq!(d.hos.cycle_used_min(), 0.0, "34 hours home is a restart");
        assert!(!d.home_given() && d.rest_note.is_none());
        assert_eq!(
            d.last_home_day,
            d.available_at.div_euclid(crate::desk::MIN_PER_DAY)
        );
    }

    #[test]
    fn a_free_otr_driver_at_the_terminal_is_not_home_until_sent() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = g.drivers[2].id;
        let d = g.driver_mut(id).unwrap();
        d.home = Some(crate::roster::HomePattern::OverTheRoad);
        d.last_home_day = -30;
        g.close_day(&mut s, w);
        let d = g.driver(id).unwrap();
        assert_eq!(d.last_home_day, -30);
        assert!(g
            .news
            .iter()
            .any(|l| l.contains("missed promised home time")));
    }

    #[test]
    fn otr_home_time_is_days_and_local_is_a_night() {
        use crate::roster::HomePattern::*;
        assert_eq!(OverTheRoad.home_min(14), 2 * 24 * 60);
        assert_eq!(OverTheRoad.home_min(21), 3 * 24 * 60);
        assert_eq!(Local.home_min(1), 600);
        // Day 1 is a Monday: regional drivers are due home on Friday.
        assert_eq!(Regional.due_day(1, 0), 5);
        assert_eq!(Regional.due_day(7, 0), 12);
    }

    #[test]
    fn the_quit_warning_rings_once_per_overdue_stretch() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = g.drivers[1].id;
        let d = g.driver_mut(id).unwrap();
        d.location = "saginaw_mi_us".into();
        d.last_home_day = -30;
        d.goodwill = QUIT_WARN_GOODWILL;
        answer_warning(&mut g, &mut s, id, crate::phone::CallChoice::AskStay);
        assert!(g.driver(id).unwrap().quit_warned);
        g.driver_mut(id).unwrap().goodwill = QUIT_WARN_GOODWILL;
        g.close_day(&mut s, w);
        let s = g.morning(w);
        assert!(!s
            .schedule
            .clone()
            .due(i64::MAX)
            .iter()
            .any(|(_, r)| matches!(r, Ring::Call(crate::phone::Call::QuitWarning { .. }))));
    }

    #[test]
    fn a_promise_home_on_a_load_heads_home_on_delivery() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = arriving_at(&mut g, &mut s, 12 * 60);
        g.trips[0].dock_hold = false;
        s.schedule = Default::default();
        answer_warning(&mut g, &mut s, id, crate::phone::CallChoice::PromiseHome);
        assert!(g.driver(id).unwrap().home_next);
        let lines = g.advance(&mut s, w);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("headed home") || l.contains("is home")),
            "{lines:?}"
        );
        assert!(g.driver(id).unwrap().home_given());
    }

    #[test]
    fn a_driver_unloads_before_the_next_load_or_the_break() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = arriving_at(&mut g, &mut s, 12 * 60);
        g.trips[0].dock_hold = false;
        let eta = g.trips[0].eta();
        s.schedule = Default::default();
        g.advance(&mut s, w);
        let d = g.driver(id).unwrap();
        if d.rest_note.is_some() {
            assert!(d.available_at >= eta + 30 + 600);
        } else {
            assert_eq!(d.status, Status::Unloading);
            assert_eq!(d.available_at, eta + 30);
            assert!(d.card_at(w, s.now(g.day), g.day).contains("unloading"));
        }
    }

    #[test]
    fn the_70_hour_clock_refuses_a_run_that_does_not_fit() {
        let w = get_world();
        let mut g = game();
        let s = g.morning(w);
        let (load, id) = legal_pair(&g, &s);
        let d = g.driver_mut(id).unwrap();
        for _ in 0..5 {
            d.hos.on_duty(13.0 * 60.0);
            d.hos.sleep();
        }
        d.hos.on_duty(4.0 * 60.0);
        let why = refusal(&s.board[load].job, g.driver(id).unwrap(), w).unwrap();
        assert!(why.contains("70-hour"), "{why}");
    }

    #[test]
    fn a_multi_day_run_logs_all_its_driving() {
        let mut d = game().drivers[0].clone();
        d.drive_with_breaks(1000.0);
        assert!((d.hos.cycle_used_min() - 1000.0).abs() < 1.0);
        assert!(d.hos.driving_min > 0.0 && d.hos.driving_min < 660.0);
    }

    #[test]
    fn the_revenue_goal_follows_the_board_and_on_time_gets_a_floor() {
        let w = get_world();
        let mut g = game();
        let s = g.morning(w);
        let (pay, mi) = s.board.iter().fold((0.0, 0.0), |(p, m), l| {
            (p + l.job.pay, m + l.job.distance_mi)
        });
        let goal = revenue_goal(&s.board);
        assert!(goal > 2.0 && goal < pay / mi, "{goal}");
        g.record.delivered = 25;
        g.record.on_time = 23;
        let (met, _) = g.goal_result(&DayGoal::OnTime, &s, w);
        assert!(!met, "92 percent misses the 95 percent floor");
        assert!(g.goal_text(&DayGoal::OnTime, w).contains("95 percent"));
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
        g.assign(&mut s, load, driver, w).unwrap();
        let mut queued = None;
        let candidates: Vec<usize> = (0..s.board.len()).filter(|&i| can(&s.board[i])).collect();
        for i in candidates {
            let before = s.clock.minute;
            match g.assign(&mut s, i, driver, w) {
                Ok(lines) => {
                    queued = Some(lines);
                    break;
                }
                Err(ActError::Refused(why)) => {
                    assert!(
                        why.contains("after delivering in") || why.contains("70-hour"),
                        "{why}"
                    );
                    assert_eq!(s.clock.minute, before, "a refused queue spends no time");
                }
                Err(e) => panic!("{e:?}"),
            }
        }
        let lines = queued.expect("some load fits after the delivery");
        assert!(
            lines[0].starts_with("Queued for") && lines[0].contains("rolling at"),
            "{lines:?}"
        );
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
