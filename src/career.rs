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
pub const TRUCK_COST_PER_DAY: f64 = 180.0;
pub const DEADHEAD_RATE: f64 = 0.65;
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub delivered: u32,
    pub on_time: u32,
    pub abandoned: u32,
    pub days_worked: u32,
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
}

impl Shift {
    pub fn now(&self, day: i64) -> i64 {
        abs_min(day, self.clock.minute)
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
        };
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
    /// Rings due after the close never queue, so they never ring.
    pub fn ring_due(&mut self, shift: &mut Shift) {
        let now = shift.now(self.day).min(abs_min(self.day, SHIFT_END));
        for ring in shift.schedule.due(now) {
            if self.ring_live(&ring, now) {
                shift.pending.push_back(ring);
            }
        }
    }

    /// Whether a ring still has something to be about at `now`: its truck
    /// still on the road (and still short of hours, for an hours call), or
    /// its driver still free.
    fn ring_live(&self, ring: &Ring, now: i64) -> bool {
        match ring {
            Ring::Incident(i) => self.trips.iter().any(|t| {
                t.id == i.trip_id
                    && now < t.eta()
                    && (i.kind != crate::incidents::Kind::HoursShort || t.hours_short())
            }),
            Ring::Call(
                crate::phone::Call::Sick { driver_id }
                | crate::phone::Call::PersonalDay { driver_id },
            ) => self
                .driver(*driver_id)
                .is_some_and(|d| d.status == Status::Idle),
            Ring::Call(crate::phone::Call::HotLoad { .. }) => true,
        }
    }

    /// Delay a trip; a delay that breaks the driver's hours rings the phone.
    pub fn add_delay(&mut self, shift: &mut Shift, trip_id: u32, minutes: f64) {
        let at = shift.now(self.day) + 30;
        let Some(trip) = self.trip_mut(trip_id) else {
            return;
        };
        trip.delay_min += minutes;
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
    /// time. Rings whose subject went away while they waited are dropped.
    pub fn answer(&mut self, shift: &mut Shift) -> Result<Ring, ActError> {
        let now = shift.now(self.day);
        while let Some(ring) = shift.pending.pop_front() {
            if self.ring_live(&ring, now) {
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
                Some(&ch) => crate::phone::apply(self, shift, c, ch),
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
        let line = format!(
            "{} is rolling: {} to {}, arriving {}.",
            driver.name,
            trip.job.cargo.label,
            world.spoken_city(&trip.job.destination, None),
            abs_text(trip.eta(), self.day)
        );
        self.driver_mut(driver_id).unwrap().status = Status::OnLoad;
        for (at, inc) in crate::incidents::roll_for_trip(self, &trip, &weather) {
            shift.schedule.push(at, Ring::Incident(inc));
        }
        self.trips.push(trip);
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
        let line = format!(
            "{} signed on for {}. They're available now.",
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
        let mut news = Vec::new();
        let (arrived, rolling): (Vec<Trip>, Vec<Trip>) = std::mem::take(&mut self.trips)
            .into_iter()
            .partition(|t| t.eta() <= next_morning);
        for trip in arrived {
            let hours_taken = (trip.eta() - trip.depart) as f64 / 60.0;
            let gross = trip.job.payout(hours_taken, 0.0, 0.0);
            let driver_pay = gross * pay_share;
            self.funds += gross - driver_pay;
            self.record.delivered += 1;
            self.record.on_time += trip.on_time() as u32;
            if let Some(d) = self.driver_mut(trip.driver_id) {
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
                    "{} delivered {} to {}, {}. {} to the carrier.",
                    d.name,
                    trip.job.cargo.label,
                    world.spoken_city(&trip.job.destination, None),
                    if trip.on_time() { "on time" } else { "late" },
                    crate::money(gross - driver_pay)
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
        news.push(format!("Truck costs today: {}.", crate::money(truck_costs)));
        for d in &mut self.drivers {
            d.hos.sleep(); // everyone gets a 10-hour reset overnight
            if d.status == Status::OffDuty {
                d.status = Status::Idle;
            }
        }
        self.record.days_worked += 1;
        self.day += 1;
        self.market.advance_to(self.day);
        self.news = news.clone();
        shift.clock.minute = SHIFT_END;
        news
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ff_core::data::world::get_world;

    pub(crate) fn game() -> Game {
        Game::new("northstar", 42, get_world())
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
        assert!(lines[0].contains("is rolling"));
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
            assert!(news[0].contains("delivered"));
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
}
