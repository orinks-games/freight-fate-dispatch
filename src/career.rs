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

pub const STARTING_FUNDS: f64 = 25_000.0;
pub const STARTING_DRIVERS: usize = 3;
/// Terminal, office, and parked-equipment overhead omitted by ATRI's per-mile costs.
pub const TRUCK_COST_PER_DAY: f64 = 180.0;
/// ATRI, "An Analysis of the Operational Costs of Trucking: 2026 Update,"
/// Table 8, 2025 vehicle-based costs per mile: fuel 0.482, truck/trailer
/// payments 0.404, repair and maintenance 0.215, insurance 0.106, permits
/// 0.008, tires 0.050, and tolls 0.043. Driver pay remains the pay-plan share.
pub const OPERATING_COST_PER_MILE: f64 = 1.31;
pub const DISPATCH_GOAL_BONUS: f64 = 25.0;
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
    pub goal: u32,
    pub abandoned_at_start: u32,
    /// "Daily goal met" was spoken and still holds; it isn't said again.
    pub goal_met_said: bool,
}

impl Shift {
    pub fn now(&self, day: i64) -> i64 {
        abs_min(day, self.clock.minute)
    }

    pub fn credited_dispatches(&self, record: &Record) -> u32 {
        self.assigned
            .saturating_sub(record.abandoned - self.abandoned_at_start)
    }

    /// The goal line after a dispatch: "met" once when it is reached, then
    /// quiet while it holds.
    pub fn goal_after_assign(&mut self, record: &Record) -> Option<String> {
        let credited = self.credited_dispatches(record);
        if self.goal == 0 || (credited >= self.goal && self.goal_met_said) {
            return None;
        }
        self.goal_met_said = credited >= self.goal;
        self.goal_progress(record)
    }

    /// The goal line after a canceled pickup: the count drops by one.
    pub fn goal_after_cancel(&mut self, record: &Record) -> Option<String> {
        (self.goal > 0).then(|| {
            let credited = self.credited_dispatches(record);
            self.goal_met_said &= credited >= self.goal;
            format!(
                "Daily goal: {credited} of {} after the canceled pickup.",
                self.goal
            )
        })
    }

    pub fn goal_progress(&self, record: &Record) -> Option<String> {
        (self.goal > 0).then(|| {
            let credited = self.credited_dispatches(record);
            if credited >= self.goal {
                format!("Daily goal met: {credited} of {}.", self.goal)
            } else {
                format!("Daily goal progress: {credited} of {}.", self.goal)
            }
        })
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
        Game {
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
        }
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

    fn board_jobs(&self, world: &World) -> Vec<ff_core::models::jobs::Job> {
        let seed = crate::seed_for(self.seed, self.day, "board");
        morning_board(
            world,
            &self.terminal,
            &self.carrier_key,
            &self.market,
            seed,
            &self.drivers,
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
            goal: 0,
            abandoned_at_start: self.record.abandoned,
            goal_met_said: false,
        };
        shift.goal = (2 + self.record.days_worked / 5)
            .min(4)
            .min(self.eligible_idle(&shift, world) as u32);
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
        lines.push(if shift.goal == 0 {
            "No dispatch goal today; every driver is already committed.".into()
        } else {
            format!(
                "Today's goal: dispatch {}. Current streak: {}.",
                loads_text(shift.goal),
                days_text(self.record.dispatch_streak)
            )
        });
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
        if let Some(progress) = shift.goal_after_assign(&self.record) {
            lines.push(progress);
        }
        for (at, inc) in crate::incidents::roll_for_trip(self, &trip, &weather) {
            // Detention holds the truck at the dock until the call is resolved.
            trip.dock_hold |= inc.kind == crate::incidents::Kind::Detention;
            shift.schedule.push(at, Ring::Incident(inc));
        }
        self.trips.push(trip);
        self.ring_due(shift);
        Ok(lines)
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
        let pay_share = pay_plan_for_key(Some(&self.carrier_key)).pay_share;
        let delivered_before = self.record.delivered;
        let on_time_before = self.record.on_time;
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
        let (arrived, rolling): (Vec<Trip>, Vec<Trip>) = std::mem::take(&mut self.trips)
            .into_iter()
            .partition(|t| t.eta() <= next_morning);
        // In after 8 PM means less than 10 hours off by 6 AM: no reset.
        let rest_by = abs_min(self.day, 20 * 60);
        let mut in_late = Vec::new();
        for trip in arrived {
            if trip.eta() > rest_by {
                in_late.push(trip.driver_id);
            }
            let hours_taken = (trip.eta() - trip.depart) as f64 / 60.0;
            let gross = trip.job.payout(hours_taken, 0.0, 0.0);
            let driver_pay = gross * pay_share;
            let operating_costs = trip.job.distance_mi * OPERATING_COST_PER_MILE;
            let net = gross - driver_pay - operating_costs;
            self.funds += net;
            self.record.delivered += 1;
            self.record.on_time += trip.on_time() as u32;
            if let Some(d) = self.driver_mut(trip.driver_id) {
                d.hos.drive(trip.drive_min.min(d.legal_left_min()));
                d.career.record_delivery(
                    trip.job.distance_mi,
                    driver_pay,
                    trip.on_time(),
                    0.0,
                    1.0,
                    1.0,
                );
                d.status = Status::Idle;
                news.push(format!(
                    "{} delivered {} to {}, {}. {} to the carrier after {} in operating costs.",
                    d.name,
                    trip.job.cargo.label,
                    world.spoken_city(&trip.job.destination, None),
                    trip.timing_text(),
                    crate::money(net),
                    crate::money(operating_costs)
                ));
            }
        }
        for trip in &rolling {
            if let Some(d) = self.driver_mut(trip.driver_id) {
                d.status = Status::OutOnRun;
            }
        }
        self.trips = rolling;
        let truck_costs = TRUCK_COST_PER_DAY * self.drivers.len() as f64;
        self.funds -= truck_costs;
        news.push(format!(
            "Fleet overhead today: {}.",
            crate::money(truck_costs)
        ));
        let credited = shift.credited_dispatches(&self.record);
        let goal_result = if shift.goal == 0 {
            news.push("No dispatch goal was set; the current streak is protected.".into());
            format!(
                " No goal today, {} streak protected.",
                days_text(self.record.dispatch_streak)
            )
        } else if credited >= shift.goal {
            self.record.dispatch_streak += 1;
            self.record.best_dispatch_streak = self
                .record
                .best_dispatch_streak
                .max(self.record.dispatch_streak);
            let bonus = DISPATCH_GOAL_BONUS
                + 5.0 * self.record.dispatch_streak.saturating_sub(1).min(5) as f64;
            self.funds += bonus;
            self.reputation += 1.0;
            news.push(format!(
                "Dispatch goal met. Streak: {}. Streak bonus: {}.",
                days_text(self.record.dispatch_streak),
                crate::money(bonus)
            ));
            format!(
                " Goal met, {} streak.",
                days_text(self.record.dispatch_streak)
            )
        } else {
            self.record.dispatch_streak = 0;
            news.push(format!(
                "Dispatch goal missed: {} credited out of {}.",
                credited, shift.goal
            ));
            format!(" Goal missed, {} credited out of {}.", credited, shift.goal)
        };
        for d in &mut self.drivers {
            // Everyone but a late arrival resets; a run still out plans its own rest.
            if !in_late.contains(&d.id) {
                d.hos.sleep();
            }
            if d.status == Status::OffDuty {
                d.status = Status::Idle;
            }
        }
        self.record.days_worked += 1;
        self.day += 1;
        self.market.advance_to(self.day);
        self.news = news;
        shift.clock.minute = SHIFT_END;
        let delivered = self.record.delivered - delivered_before;
        let on_time = self.record.on_time - on_time_before;
        vec![format!(
            "Day closed. {}, {} on time. Funds now {}.{}",
            deliveries_text(delivered),
            on_time,
            crate::money(self.funds),
            goal_result
        )]
    }
}

fn loads_text(count: u32) -> String {
    match count {
        1 => "1 load".into(),
        n => format!("{n} loads"),
    }
}

fn deliveries_text(count: u32) -> String {
    match count {
        1 => "1 delivery".into(),
        n => format!("{n} deliveries"),
    }
}

fn days_text(count: u32) -> String {
    match count {
        1 => "1 day".into(),
        n => format!("{n} days"),
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
        assert_eq!(lines[1], "Daily goal progress: 1 of 2.");
        s.goal = 1;
        assert_eq!(
            s.goal_progress(&g.record).unwrap(),
            "Daily goal met: 1 of 1."
        );
        assert_eq!(s.clock.minute, SHIFT_START + 10);
        assert_eq!(s.board.len(), 7);
        assert_eq!(g.driver(driver).unwrap().status, Status::OnLoad);
        assert_eq!(g.trips.len(), 1);
    }

    #[test]
    fn the_goal_is_met_once_and_drops_after_a_cancel() {
        let mut g = game();
        let mut s = g.morning(get_world());
        s.goal = 2;
        s.assigned = 1;
        assert_eq!(
            s.goal_after_assign(&g.record).as_deref(),
            Some("Daily goal progress: 1 of 2.")
        );
        s.assigned = 2;
        assert_eq!(
            s.goal_after_assign(&g.record).as_deref(),
            Some("Daily goal met: 2 of 2.")
        );
        s.assigned = 3;
        assert_eq!(s.goal_after_assign(&g.record), None, "not re-announced");
        g.record.abandoned += 1;
        assert_eq!(
            s.goal_after_cancel(&g.record).as_deref(),
            Some("Daily goal: 2 of 2 after the canceled pickup.")
        );
        g.record.abandoned += 1;
        s.goal_after_cancel(&g.record);
        s.assigned = 4;
        assert_eq!(
            s.goal_after_assign(&g.record).as_deref(),
            Some("Daily goal met: 2 of 2."),
            "met again after dropping below"
        );
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
        s.goal = 0;
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
        assert!(news.ends_with(&format!(
            "{} in operating costs.",
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

    #[test]
    fn meeting_the_daily_goal_builds_a_streak_and_pays_a_bonus() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        s.goal = 1;
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        let funds_before_close = g.funds;
        let summary = g.close_day(&mut s, w).join(" ");
        assert_eq!(g.record.dispatch_streak, 1);
        assert_eq!(g.record.best_dispatch_streak, 1);
        assert!(g.funds >= funds_before_close - TRUCK_COST_PER_DAY * 3.0);
        assert!(summary.contains("Goal met"));
        assert!(g.news.iter().any(|line| line.contains("Streak bonus")));
    }

    #[test]
    fn an_abandoned_load_does_not_count_toward_the_daily_goal() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        s.goal = 1;
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        g.record.abandoned += 1;
        let summary = g.close_day(&mut s, w).join(" ");
        assert_eq!(g.record.dispatch_streak, 0);
        assert!(summary.contains("Goal missed"));
    }
}
