//! The career (persistent, saved at day boundaries) and today's shift
//! (rebuilt from the career's seed every morning).

use std::collections::{BTreeMap, VecDeque};

use ff_core::data::world::World;
use ff_core::data::world_models::Route;
use ff_core::models::market::Market;
use ff_core::models::start_options::{pay_plan_for_key, start_option};
use serde::{Deserialize, Serialize};

use crate::board::{morning_board, refusal, refusal_or_warning, route_for, Load};
use crate::customers::{Customer, Source, Tender, WeekTotals};
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
/// A local driver kept out overnight costs a motel and layover pay.
pub const LAYOVER_COST: f64 = 150.0;
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

fn default_true() -> bool {
    true
}
pub const STARTING_DRIVERS: usize = 3;
/// Parking, ELD and permits per truck per day; the terminal, notes and
/// insurance come monthly.
pub const TRUCK_OVERHEAD_PER_DAY: f64 = crate::money::TRUCK_OVERHEAD_PER_DAY;
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
    /// Shippers that have kept a truck waiting at the dock, by facility.
    /// The board says so on their loads.
    #[serde(default)]
    pub slow_docks: Vec<String>,
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
    /// Contract customers; an older save gets its three on load.
    #[serde(default)]
    pub customers: Vec<Customer>,
    /// Accepted tenders waiting for their day (today's are on the board).
    #[serde(default)]
    pub accepted: Vec<Tender>,
    /// How far each broker trusts the carrier: on-time loads up, late
    /// loads and canceled pickups down.
    #[serde(default)]
    pub broker_trust: BTreeMap<String, i32>,
    /// The week so far, for Sunday's scorecard.
    #[serde(default)]
    pub week: WeekTotals,
    /// Points against the carrier, by category and day.
    #[serde(default)]
    pub safety: crate::law::SafetyScore,
    /// Friends solid drivers brought in, with who vouched for them.
    #[serde(default)]
    pub referrals: Vec<(String, Driver)>,
    /// An audit starts tomorrow: the desk opens at 10 AM.
    #[serde(default)]
    pub audit_pending: bool,
    /// Money owed to the carrier, by the day it comes.
    #[serde(default)]
    pub receivables: Vec<crate::money::Receivable>,
    /// Driver pay owed, settled on Friday.
    #[serde(default)]
    pub settlement_owed: f64,
    /// Everything pays the next day, for 3 percent. A new authority
    /// starts with it on; dropping it is the first cash-flow decision.
    #[serde(default = "default_true")]
    pub factoring: bool,
    #[serde(default)]
    pub tractors: Vec<crate::money::Tractor>,
    /// Days running with cash under the line of credit.
    #[serde(default)]
    pub days_over_line: i64,
    /// Days running with the safety score at shutdown level.
    #[serde(default)]
    pub shutdown_days: i64,
    #[serde(default)]
    pub ended: Option<crate::money::Ending>,
    /// What the career has reached, in the order it got there.
    #[serde(default)]
    pub milestones: Vec<crate::growth::Milestone>,
    /// The second terminal, once one is open.
    #[serde(default)]
    pub terminal2: Option<String>,
    /// The career score at the last scorecard.
    #[serde(default)]
    pub score_last: u32,
}

pub struct Shift {
    pub clock: DeskClock,
    pub board: Vec<Load>,
    /// Tenders waiting for an answer. Unanswered at the close is declined.
    pub tenders: Vec<Tender>,
    /// Tomorrow's tenders, held until the 5 PM call.
    pub tomorrow: Vec<Tender>,
    pub sales_called: bool,
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
            slow_docks: Vec::new(),
            weather: Vec::new(),
            weather_live: false,
            news: Vec::new(),
            customers: Vec::new(),
            accepted: Vec::new(),
            broker_trust: BTreeMap::new(),
            week: WeekTotals::default(),
            safety: crate::law::SafetyScore::default(),
            referrals: Vec::new(),
            audit_pending: false,
            receivables: Vec::new(),
            settlement_owed: 0.0,
            factoring: true,
            milestones: Vec::new(),
            terminal2: None,
            score_last: 0,
            tractors: Vec::new(),
            days_over_line: 0,
            shutdown_days: 0,
            ended: None,
        };
        for (i, d) in game.drivers.iter_mut().enumerate() {
            d.home = Some(if i == 0 {
                crate::roster::HomePattern::Local
            } else {
                crate::roster::HomePattern::Regional
            });
        }
        game.normalize(world);
        game
    }

    /// The truck a driver is assigned.
    pub fn tractor_of(&self, driver_id: u32) -> Option<&crate::money::Tractor> {
        self.tractors
            .iter()
            .find(|t| t.driver_id == Some(driver_id))
    }

    /// Trucks nobody on the roster drives.
    pub fn spare_tractors(&self) -> Vec<u32> {
        self.tractors
            .iter()
            .filter(|t| t.driver_id.is_none_or(|id| self.driver(id).is_none()))
            .map(|t| t.id)
            .collect()
    }

    fn add_miles(&mut self, driver_id: u32, miles: f64) {
        if let Some(t) = self
            .tractors
            .iter_mut()
            .find(|t| t.driver_id == Some(driver_id))
        {
            t.miles += miles;
        }
    }

    pub fn fuel_price(&self) -> f64 {
        crate::money::fuel_price(self.seed, self.day)
    }

    /// Cash plus what's owed to the carrier and the drivers, as the desk
    /// says it.
    pub fn money_lines(&self) -> Vec<String> {
        let owed: f64 = self.receivables.iter().map(|r| r.amount).sum();
        let next = self.receivables.iter().map(|r| r.due_day).min();
        let mut lines = vec![match self.funds {
            f if f < -crate::money::CREDIT_LINE => format!(
                "Cash {}, over the {} line of credit for {} days. A month over ends the career.",
                crate::money(f),
                crate::money(crate::money::CREDIT_LINE),
                self.days_over_line
            ),
            f if f < 0.0 => format!(
                "Cash {}, on the line of credit at {} percent a month.",
                crate::money(f),
                crate::money::CREDIT_INTEREST * 100.0
            ),
            f => format!("Cash {}.", crate::money(f)),
        }];
        lines.push(match next {
            Some(day) => format!(
                "{} owed to the carrier, the next payment {}.",
                crate::money(owed),
                match day - self.day {
                    n if n <= 0 => "tomorrow morning".to_string(),
                    1 => "tomorrow".to_string(),
                    n => format!("in {n} days"),
                }
            ),
            None => "Nothing owed to the carrier.".into(),
        });
        let to_friday = (4 - crate::roster::weekday(self.day)).rem_euclid(7);
        lines.push(format!(
            "{} in driver pay, settled {}.",
            crate::money(self.settlement_owed),
            match to_friday {
                0 => "tonight".to_string(),
                n => format!("Friday, in {n} days"),
            }
        ));
        let notes: f64 = self.tractors.iter().map(|t| t.note).sum();
        let insurance = crate::money::insurance(self.tractors.len(), self.safety.total(self.day));
        let to_bill =
            crate::money::MONTH_DAYS - (self.day - 1).rem_euclid(crate::money::MONTH_DAYS);
        lines.push(format!(
            "Bills on the first, in {to_bill} days: {} in truck notes, {} insurance, {} terminal.",
            crate::money(notes),
            crate::money(insurance),
            crate::money(crate::money::TERMINAL_RENT * self.terminals().len() as f64)
        ));
        lines.push(format!(
            "Diesel ${:.2} a gallon this week.",
            self.fuel_price()
        ));
        lines
    }

    pub fn customer(&self, id: u32) -> Option<&Customer> {
        self.customers.iter().find(|c| c.id == id)
    }

    pub fn customer_mut(&mut self, id: u32) -> Option<&mut Customer> {
        self.customers.iter_mut().find(|c| c.id == id)
    }

    pub fn customer_name(&self, id: u32) -> String {
        self.customer(id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "A former customer".into())
    }

    /// A load as spoken on the board.
    pub fn load_text(&self, load: &Load, world: &World) -> String {
        crate::board::load_text(load, self, world)
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
                        .any(|load| refusal_or_warning(&load.job, driver, world).is_none())
            })
            .count()
    }

    pub fn trip_mut(&mut self, id: u32) -> Option<&mut Trip> {
        self.trips.iter_mut().find(|t| t.id == id)
    }

    /// Old saves have no truck locations, home days or customers: everyone
    /// is home, and the terminal's three accounts are seeded.
    pub fn normalize(&mut self, world: &World) {
        let (terminal, day) = (self.terminal.clone(), self.day);
        if self.customers.is_empty() {
            let first_id = self.next_id;
            self.customers = crate::customers::seeded_customers(
                world,
                &terminal,
                &self.carrier_key,
                self.seed,
                &self.drivers,
                first_id,
            );
            self.next_id += self.customers.len() as u32;
        }
        if self.tractors.is_empty() {
            let ids: Vec<u32> = self.drivers.iter().map(|d| d.id).collect();
            let mut rng = crate::rng(self.seed, 0, "fleet");
            self.tractors = crate::money::starting_fleet(&mut rng, 1, &ids);
        }
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
            d.ensure_traits(self.seed);
            if d.record.medical_card_day == 0 {
                d.record.medical_card_day = day;
            }
        }
        // Streaks and their bonus are gone; old saves drop them.
        self.record.dispatch_streak = 0;
    }

    /// Where loads are offered from: the terminal, then every city a free
    /// truck sits in.
    fn origins(&self) -> Vec<String> {
        let mut out = self.terminals();
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
        let size = crate::board::board_size(self.day)
            + crate::growth::Tier::of(self.reputation).extra_loads();
        let per = if origins.len() == 1 {
            size
        } else {
            (size / origins.len()).max(3)
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
        // Three offers, one of them headed back toward the terminal, so a
        // driver out on the road can work their way home.
        let mut jobs = self.city_jobs(world, city, crate::board::BOARD_SIZE);
        let home = |j: &ff_core::models::jobs::Job| {
            crate::trip::empty_leg(&j.destination, &self.terminal, world).0
        };
        if let Some(best) =
            (0..jobs.len()).min_by(|&a, &b| home(&jobs[a]).total_cmp(&home(&jobs[b])))
        {
            if best >= 3 {
                jobs.swap(2, best);
            }
        }
        jobs.truncate(3);
        for (i, job) in jobs.into_iter().enumerate() {
            let broker = crate::customers::spot_broker(self.seed, self.day, 200 + i);
            shift.board.push(Load::spot(job, broker));
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
        let (seed, today) = (self.seed, self.day);
        let (arrived, rolling): (Vec<Trip>, Vec<Trip>) = std::mem::take(&mut self.trips)
            .into_iter()
            .partition(|t| !t.dock_hold && t.eta() <= upto);
        self.trips = rolling;
        let mut lines = Vec::new();
        let price = self.fuel_price();
        for trip in arrived {
            let end = trip.eta();
            let mut gross = trip.job.payout((end - trip.depart) as f64 / 60.0, 0.0, 0.0);
            if !trip.source.is_spot() {
                gross += crate::money::surcharge(trip.job.distance_mi, price);
            }
            let driver_pay = gross * pay_share;
            let miles = trip.job.distance_mi + trip.deadhead_mi;
            let fuel = crate::money::fuel_cost(miles, price);
            let road = (miles * crate::money::ROAD_COST_PER_MILE).round();
            let operating_costs = fuel + road;
            self.funds -= operating_costs;
            self.settlement_owed += driver_pay;
            self.add_miles(trip.driver_id, miles);
            let due_day = crate::money::pay_day(self, &trip.source, today);
            let billed = if self.factoring {
                (gross * (1.0 - crate::money::FACTORING_FEE)).round()
            } else {
                gross
            };
            let owner = match &trip.source {
                Source::Spot { broker } => broker.clone(),
                Source::Contract { customer_id } => self.customer_name(*customer_id),
            };
            self.receivables.push(crate::money::Receivable {
                amount: billed,
                due_day,
                from: owner.clone(),
            });
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
            self.week.delivered += 1;
            self.week.on_time += trip.on_time() as u32;
            self.week.gross += gross;
            self.remember_delivery(&trip.source, trip.on_time());
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
            let suspended = d.record.suspended(today);
            (d.available_at, d.rest_note) = if suspended {
                (
                    abs_min(d.record.suspended_until, SHIFT_START),
                    Some("a CDL suspension".to_string()),
                )
            } else if cycle_low {
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
            let reveal_odds = if d.on_probation(today) {
                crate::roster::REVEAL_ODDS_PROBATION
            } else {
                crate::roster::REVEAL_ODDS
            };
            let learned = d.reveal_random(
                &mut crate::rng(seed, today, &format!("reveal:{}", trip.id)),
                reveal_odds,
            );
            let empty = if trip.deadhead_mi >= 1.0 {
                format!(", {} of them empty", trip.deadhead_mi.round())
            } else {
                String::new()
            };
            let due = match due_day - today {
                1 => "tomorrow".to_string(),
                n => format!("in {n} days"),
            };
            let mut line = format!(
                "{name} delivered {} to {}, {}. {} billed to {owner}, due {due}; {} fuel and {} road costs for {} miles{empty}, {} to {name} on Friday.",
                trip.job.cargo.label,
                world.spoken_city(&trip.job.destination, None),
                trip.timing_text(),
                crate::money(billed),
                crate::money(fuel),
                crate::money(road),
                miles.round(),
                crate::money(driver_pay),
            );
            if let Some(note) = &self.driver(id).unwrap().rest_note {
                line.push_str(&format!(
                    " {name} is in {}, available at {} after {note}.",
                    world.spoken_city(&trip.job.destination, None),
                    abs_text(avail, self.day)
                ));
            }
            lines.push(line);
            lines.extend(learned);
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
                    let (ok, said) = self.start_queued(
                        shift,
                        id,
                        job,
                        (trip.next_queued_at, trip.next_source.clone()),
                        world,
                    );
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
        (queued_at, source): (i64, Source),
        world: &World,
    ) -> (bool, String) {
        let d = self.driver(driver_id).unwrap().clone();
        let owner = match &source {
            Source::Spot { broker } => broker.clone(),
            Source::Contract { customer_id } => self.customer_name(*customer_id),
        };
        let start = d.available_at;
        let left_h = job.deadline_game_h - (start - queued_at) as f64 / 60.0;
        let mut rested = d.clone();
        match &rested.rest_note {
            Some(note) if note.contains("34-hour") => rested.hos.restart(),
            Some(_) => rested.hos.sleep(),
            None => {}
        }
        let need_h = crate::trip::plan(&job, &rested, world).need_h;
        let refused = crate::board::cycle_refusal(&job, &rested, world);
        if refused.is_some() || need_h > left_h {
            self.load_dropped(&source);
        }
        if let Some(why) = refused {
            return (
                false,
                format!("{why} The queued load goes back to {owner}."),
            );
        }
        if need_h > left_h {
            return (
                false,
                format!(
                    "{} can't legally make the queued {} appointment in {}. It goes back to {owner}.",
                    d.name,
                    job.cargo.label,
                    world.spoken_city(&job.destination, None)
                ),
            );
        }
        job.deadline_game_h = left_h;
        let id = self.take_id();
        let mut trip = Trip::start(id, &rested, job, start, world);
        trip.source = source;
        let line = format!(
            "{} rolls on the queued load: {} from {} to {}, delivering {}.",
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

    /// The owner's record of a delivery: a customer's week, or a broker's trust.
    fn remember_delivery(&mut self, source: &Source, on_time: bool) {
        match source {
            Source::Spot { broker } => {
                *self.broker_trust.entry(broker.clone()).or_insert(0) +=
                    if on_time { 1 } else { -1 };
            }
            Source::Contract { customer_id } => {
                if let Some(c) = self.customer_mut(*customer_id) {
                    c.week.delivered += 1;
                    c.week.on_time += on_time as u32;
                    c.delivered_total += 1;
                    c.on_time_total += on_time as u32;
                }
            }
        }
    }

    /// A load the carrier walked away from: a canceled pickup, or a
    /// contract load never covered.
    pub fn load_dropped(&mut self, source: &Source) {
        match source {
            Source::Spot { broker } => {
                *self.broker_trust.entry(broker.clone()).or_insert(0) -= 2;
            }
            Source::Contract { customer_id } => {
                if let Some(c) = self.customer_mut(*customer_id) {
                    c.week.failed += 1;
                }
            }
        }
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
        let Some(d) = self.driver(driver_id) else {
            return ActError::NoSuchItem.spoken();
        };
        let terminal = crate::growth::nearest_terminal(&d.location, &self.terminals(), world);
        let (miles, minutes) = crate::trip::empty_leg(&d.location, &terminal, world);
        let rest = if minutes > d.legal_left_min() {
            crate::incidents::RESET_MIN
        } else {
            0.0
        };
        let home_at = start + (minutes + rest).round() as i64;
        let today = self.day;
        self.funds -= crate::money::fuel_cost(miles, self.fuel_price())
            + (miles * crate::money::ROAD_COST_PER_MILE).round();
        self.add_miles(driver_id, miles);
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
        let terminals = self.terminals();
        let mut lines = Vec::new();
        for d in &mut self.drivers {
            if !d.home_given() || d.home_at > now || terminals.contains(&d.location) {
                continue;
            }
            let terminal = crate::growth::nearest_terminal(&d.location, &terminals, world);
            d.location = terminal.clone();
            lines.push(format!(
                "{} is home in {}.",
                d.name,
                world.spoken_city(&terminal, None)
            ));
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
        let morning = abs_min(self.day, SHIFT_START);
        let mut board: Vec<Load> = self
            .accepted
            .iter()
            .filter(|t| t.for_day <= self.day)
            .map(|t| {
                let mut load = Load::contract(t);
                load.refresh(morning);
                load
            })
            .collect();
        board.extend(
            self.board_jobs(world)
                .into_iter()
                .enumerate()
                .map(|(i, job)| {
                    let mut load =
                        Load::spot(job, crate::customers::spot_broker(self.seed, self.day, i));
                    load.weather = self.weather.get(i).cloned().unwrap_or_default();
                    load
                }),
        );
        let tenders = if self.day == 1 {
            crate::customers::tenders_for(self, world, self.day)
        } else {
            Vec::new()
        };
        let tomorrow = crate::customers::next_tender_day(self.day)
            .map(|d| crate::customers::tenders_for(self, world, d))
            .unwrap_or_default();
        let calls = crate::phone::morning_calls(self);
        let hot = calls
            .iter()
            .filter(|(_, c)| matches!(c, crate::phone::Call::HotLoad { .. }))
            .count();
        let hot_offers = crate::phone::hot_offers(self, world, hot);
        let calls = crate::phone::live_calls(calls, hot_offers.len());
        let mut shift = Shift {
            clock: if self.audit_pending {
                DeskClock { minute: 10 * 60 }
            } else {
                DeskClock::morning()
            },
            board,
            tenders,
            tomorrow,
            sales_called: false,
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
        if crate::roster::weekday(self.day) == 0 {
            if let Some(customer_id) = crate::growth::dedicated_offer(self) {
                shift.schedule.push(
                    abs_min(self.day, SHIFT_START + 30),
                    Ring::Call(crate::phone::Call::Dedicated { customer_id }),
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
        if !shift.tomorrow.is_empty() {
            shift.schedule.push(
                abs_min(self.day, crate::customers::TENDER_MINUTE),
                Ring::Call(crate::phone::Call::Tenders),
            );
        }
        shift
    }

    /// Tenders for `day`, as the phone describes them: "Tomorrow's tenders
    /// are in: 3 loads, from Lakeshore Foods and Prairie Steel."
    pub fn tenders_text(&self, tenders: &[Tender]) -> String {
        let Some(first) = tenders.first() else {
            return String::new();
        };
        let when = match first.for_day - self.day {
            1 => "Tomorrow's".to_string(),
            _ => format!(
                "{}'s",
                crate::roster::WEEKDAYS[crate::roster::weekday(first.for_day) as usize]
            ),
        };
        let mut names: Vec<String> = Vec::new();
        for t in tenders {
            let name = self.customer_name(t.customer_id);
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let from = match names.len() {
            1 => names[0].clone(),
            n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
        };
        format!(
            "{when} tenders are in: {}, from {from}.",
            match tenders.len() {
                1 => "1 load".to_string(),
                n => format!("{n} loads"),
            }
        )
    }

    /// Move tomorrow's tenders onto the desk, answering them all one way
    /// when `all` is given.
    pub fn answer_tenders(
        &mut self,
        shift: &mut Shift,
        all: Option<bool>,
        world: &World,
    ) -> Vec<String> {
        let tenders = std::mem::take(&mut shift.tomorrow);
        let n = tenders.len();
        let first = shift.tenders.len();
        shift.tenders.extend(tenders);
        match all {
            None => vec!["They're on the Tenders screen, waiting for an answer.".into()],
            Some(accept) => {
                // Only the new batch; tenders already open keep waiting.
                while shift.tenders.len() > first {
                    let _ = if accept {
                        self.accept_tender(shift, first, world)
                    } else {
                        self.decline_tender(shift, first)
                    };
                }
                vec![match (accept, n) {
                    (true, 1) => "Accepted. It leads the board on its day.".to_string(),
                    (true, n) => format!("All {n} accepted. They lead the board on their day."),
                    (false, 1) => "Declined. The customer will count it.".to_string(),
                    (false, n) => format!("All {n} declined. The customers will count it."),
                }]
            }
        }
    }

    /// Accept an open tender: it leads the board on its day. Today's goes
    /// on the board now.
    pub fn accept_tender(
        &mut self,
        shift: &mut Shift,
        idx: usize,
        world: &World,
    ) -> Result<String, ActError> {
        if idx >= shift.tenders.len() {
            return Err(ActError::NoSuchItem);
        }
        let t = shift.tenders.remove(idx);
        if let Some(c) = self.customer_mut(t.customer_id) {
            c.week.tendered += 1;
            c.week.accepted += 1;
        }
        let when = if t.for_day <= self.day {
            let mut load = Load::contract(&t);
            load.refresh(shift.now(self.day));
            let at = shift
                .board
                .iter()
                .position(|l| l.source.is_spot())
                .unwrap_or(shift.board.len());
            shift.board.insert(at, load);
            "It's on the board".to_string()
        } else {
            format!(
                "It leads {}'s board",
                crate::roster::WEEKDAYS[crate::roster::weekday(t.for_day) as usize]
            )
        };
        let line = format!(
            "Accepted. {when}: {}, {} to {}, {}.",
            self.customer_name(t.customer_id),
            t.job.cargo.label,
            world.spoken_city(&t.job.destination, None),
            crate::money(t.job.pay)
        );
        self.accepted.push(t);
        Ok(line)
    }

    pub fn decline_tender(&mut self, shift: &mut Shift, idx: usize) -> Result<String, ActError> {
        if idx >= shift.tenders.len() {
            return Err(ActError::NoSuchItem);
        }
        let t = shift.tenders.remove(idx);
        if let Some(c) = self.customer_mut(t.customer_id) {
            c.week.tendered += 1;
        }
        Ok(format!(
            "Declined. {} will count it against the week.",
            self.customer_name(t.customer_id)
        ))
    }

    /// Ask a broker for 10 percent more on a spot load. Five minutes; one
    /// try per load. A refusal sometimes gives the load away.
    pub fn counter_rate(
        &mut self,
        shift: &mut Shift,
        load: usize,
        world: &World,
    ) -> Result<Vec<String>, ActError> {
        self.ensure_open(shift)?;
        let l = shift.board.get(load).ok_or(ActError::NoSuchItem)?;
        let Source::Spot { broker } = &l.source else {
            return Err(ActError::Refused(
                "That's contract freight. The rate is the contract rate.".into(),
            ));
        };
        if l.countered {
            return Err(ActError::Refused(format!(
                "{broker} already heard your number. The rate stands."
            )));
        }
        let broker = broker.clone();
        shift.clock.spend(Action::TakeCall);
        let mut rng = crate::rng(
            self.seed,
            self.day,
            &format!("counter:{}:{}", shift.clock.minute, load),
        );
        let trust = self.broker_trust.get(&broker).copied().unwrap_or(0);
        let odds = crate::customers::counter_odds(self.reputation, trust);
        let l = &mut shift.board[load];
        l.countered = true;
        let ask = (l.job.pay * crate::customers::COUNTER_ASK).round();
        let lines = if rng.random() < odds {
            l.job.pay = ask;
            vec![format!(
                "{broker} takes {} for the {} to {}.",
                crate::money(ask),
                l.job.cargo.label,
                world.spoken_city(&l.job.destination, None)
            )]
        } else if rng.random() < crate::customers::COUNTER_PULL_ODDS {
            let gone = shift.board.remove(load);
            vec![format!(
                "{broker} won't go to {}, and gave the {} to {} to another carrier.",
                crate::money(ask),
                gone.job.cargo.label,
                world.spoken_city(&gone.job.destination, None)
            )]
        } else {
            vec![format!(
                "{broker} holds at {}. Take it or leave it.",
                crate::money(l.job.pay)
            )]
        };
        self.ring_due(shift);
        Ok(lines)
    }

    /// Thirty minutes on the phone looking for a new account. Once a day.
    pub fn sales_call(&mut self, shift: &mut Shift, world: &World) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        if shift.sales_called {
            return Err(ActError::Refused(
                "You've made today's sales call. Try again tomorrow.".into(),
            ));
        }
        let cap = crate::growth::Tier::of(self.reputation).max_customers();
        if self.customers.len() >= cap {
            return Err(ActError::Refused(format!(
                "{cap} accounts is all {} can serve. Keep the ones you have happy; a better reputation opens more.",
                crate::growth::Tier::of(self.reputation).name()
            )));
        }
        shift.clock.spend(Action::SalesCall);
        shift.sales_called = true;
        let mut rng = crate::rng(self.seed, self.day, "sales:roll");
        let won = rng.random() < crate::customers::sales_odds(self.reputation);
        let line = match crate::customers::new_customer(self, world, self.next_id) {
            Some(c) if won => {
                self.next_id += 1;
                let line = format!(
                    "{} will try you: {} loads a week, {} to {}. The first tenders come with the next batch.",
                    c.name,
                    c.weekly_loads,
                    world.spoken_city(&c.lanes[0].from, None),
                    world.spoken_city(&c.lanes[0].to, None)
                );
                self.customers.push(c);
                line
            }
            Some(_) => {
                "No luck today. Shippers ask about the on-time record first. Call again tomorrow."
                    .into()
            }
            None => "Nobody new is shipping out of here right now.".into(),
        };
        self.ring_due(shift);
        Ok(line)
    }

    pub fn briefing(&self, shift: &Shift, world: &World) -> Vec<String> {
        let mut lines = vec![format!(
            "Day {} at {}, {} terminal. {}, {}. Funds {}, on time {} percent.",
            self.day,
            self.name,
            world.spoken_city(&self.terminal, None),
            crate::roster::WEEKDAYS[crate::roster::weekday(self.day) as usize],
            shift.clock.spoken(),
            crate::money(self.funds),
            self.record.on_time_pct()
        )];
        lines.push(format!(
            "Today's goal: {}.",
            self.goal_text(&shift.goal, world)
        ));
        if self.audit_pending {
            lines.push("The auditor took the morning. The desk opens at 10 AM.".into());
        }
        let idle: Vec<String> = self
            .drivers
            .iter()
            .filter(|d| d.status == Status::Idle)
            .map(|d| d.name.clone())
            .collect();
        lines.push(match idle.len() {
            0 => "No drivers available.".into(),
            n => format!("{n} available: {}.", crate::and_list(&idle)),
        });
        let due: Vec<String> = self
            .drivers
            .iter()
            .filter(|d| self.due_home(d))
            .map(|d| {
                if d.location == self.terminal {
                    format!("{}, already in town", d.name)
                } else {
                    format!("{} in {}", d.name, world.spoken_city(&d.location, None))
                }
            })
            .collect();
        if !due.is_empty() {
            lines.push(format!(
                "Due home in {} tonight: {}.",
                world.spoken_city(&self.terminal, None),
                crate::and_list(&due)
            ));
        }
        let contract = shift.board.iter().filter(|l| !l.source.is_spot()).count();
        let mut board = match contract {
            0 => format!("{} loads on the board.", shift.board.len()),
            n => format!(
                "{} loads on the board, {n} of them contract freight with appointments.",
                shift.board.len()
            ),
        };
        match shift.tenders.len() {
            0 => {}
            1 => board.push_str(" 1 tender is waiting for an answer."),
            n => board.push_str(&format!(" {n} tenders are waiting for an answer.")),
        }
        match crate::roster::weekday(self.day) {
            5 => board.push_str(" Saturday: a light board and no tenders."),
            6 => board.push_str(
                " Sunday: a light board, no tenders, and the week's scorecard at the close.",
            ),
            _ => {}
        }
        lines.push(board);
        lines.extend(self.check_calls(shift, world));
        lines.extend(self.news.iter().cloned());
        if self.safety.total(self.day) >= crate::law::AUDIT_THRESHOLD / 2.0 {
            lines.push(self.safety.text(self.day));
        }
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

    /// 6 AM check calls from the trucks still out: where they are and when
    /// they land. A driver who goes quiet leaves the desk to check.
    pub fn check_calls(&self, shift: &Shift, world: &World) -> Vec<String> {
        let now = shift.now(self.day);
        self.trips
            .iter()
            .map(|t| {
                let (name, talks) = self
                    .driver(t.driver_id)
                    .map(|d| (d.name.as_str(), d.traits().communication >= 3))
                    .unwrap_or(("A driver", true));
                if t.dock_hold {
                    format!(
                        "{name} is at the shipper in {}, waiting to load for {}.",
                        world.spoken_city(&t.job.origin, None),
                        world.spoken_city(&t.job.destination, None)
                    )
                } else if talks {
                    format!(
                        "{name} checked in near {}, {} percent of the way to {}, delivering {}, {}.",
                        world.spoken_city(&t.city_at(now), None),
                        (t.progress(now) * 100.0).round(),
                        world.spoken_city(&t.job.destination, None),
                        abs_text(t.eta(), self.day),
                        t.timing_text()
                    )
                } else {
                    format!(
                        "{name} hasn't checked in. Last plan had them arriving in {} {}.",
                        world.spoken_city(&t.job.destination, None),
                        abs_text(t.eta(), self.day)
                    )
                }
            })
            .collect()
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
        // A break that ended while the desk was busy (a call, an interview)
        // ends now, so the driver's own call still rings.
        let world = ff_core::data::world::get_world();
        for id in self.wake(now) {
            let city = self
                .driver(id)
                .map(|d| d.location.clone())
                .unwrap_or_default();
            self.add_city_offers(shift, &city, world);
        }
        for l in &mut shift.board {
            l.refresh(now);
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
            Ring::Call(
                crate::phone::Call::HotLoad { .. }
                | crate::phone::Call::Tenders
                | crate::phone::Call::Dedicated { .. },
            ) => true,
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
        if self
            .tractor_of(driver_id)
            .is_some_and(|t| t.in_shop(self.day))
        {
            return Err(ActError::Refused(format!(
                "{}'s truck is in the shop today.",
                free.name
            )));
        }
        let over_hours = crate::board::hours_warning(job, &free, world).is_some();
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
        trip.source = loaded.source;
        if over_hours {
            trip.push_past_hours();
        }
        let mut lines = vec![format!(
            "{} is headed to the shipper in {}: {} to {}, delivering {}.",
            driver.name,
            world.spoken_city(&trip.job.origin, None),
            trip.job.cargo.label,
            world.spoken_city(&trip.job.destination, None),
            abs_text(trip.eta(), self.day)
        )];
        self.driver_mut(driver_id).unwrap().status = Status::OnLoad;
        shift.assigned += 1;
        if over_hours {
            lines.push(format!(
                "{} is running it past the clock. A scale will see the log.",
                self.driver(driver_id)
                    .map(|d| d.name.as_str())
                    .unwrap_or("The driver")
            ));
        }
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
        t.next_source = loaded.source;
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
            return Ok(format!("Interview already complete. {}", a.line(self.day)));
        }
        shift.clock.spend(Action::Interview);
        a.interviewed = true;
        // An interview shows two of the five traits.
        let mut rng = crate::rng(self.seed, self.day, &format!("interview:{}", a.driver.name));
        for _ in 0..2 {
            a.driver.reveal_random(&mut rng, 1.0);
        }
        let line = a.line(self.day);
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
        let Some(truck) = self.spare_tractors().first().copied() else {
            return Err(ActError::Refused(
                "There's no truck for them. Buy one on the Money screen first.".into(),
            ));
        };
        let mut a = shift.applicants.remove(idx);
        self.funds -= a.signing;
        a.driver.id = self.take_id();
        if let Some(t) = self.tractors.iter_mut().find(|t| t.id == truck) {
            t.driver_id = Some(a.driver.id);
        }
        a.driver.status = Status::OffDuty;
        a.driver.hired_day = self.day + 1;
        // New hires report to the terminal, home since today.
        a.driver.location = self.terminal.clone();
        a.driver.last_home_day = self.day + 1;
        a.driver.hos_at = abs_min(self.day + 1, SHIFT_START);
        a.driver.ensure_traits(self.seed);
        if a.driver.record.medical_card_day == 0 {
            a.driver.record.medical_card_day = self.day + 1;
        }
        let name = a.driver.name.clone();
        self.referrals.retain(|(_, d)| d.name != name);
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
        self.audit_pending = false;
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
        // Contract loads nobody covered are service failures; open tenders
        let mut dropped = Vec::new();
        // are declined.
        let uncovered: Vec<Load> = shift
            .board
            .iter()
            .filter(|l| !l.source.is_spot())
            .cloned()
            .collect();
        for l in &uncovered {
            self.load_dropped(&l.source);
            let dedicated = match &l.source {
                Source::Contract { customer_id } => {
                    self.customer(*customer_id).is_some_and(|c| c.dedicated)
                }
                _ => false,
            };
            self.reputation -=
                crate::customers::UNCOVERED_REPUTATION * if dedicated { 2.0 } else { 1.0 };
            dropped.push(format!(
                "{} {} to {} was never covered. They'll remember it.",
                crate::possessive(&l.owner(self)),
                l.job.cargo.label,
                world.spoken_city(&l.job.destination, None)
            ));
        }
        shift.board.retain(|l| l.source.is_spot());
        let unanswered = shift.tenders.len() + shift.tomorrow.len();
        while !shift.tenders.is_empty() {
            let _ = self.decline_tender(shift, 0);
        }
        for t in std::mem::take(&mut shift.tomorrow) {
            if let Some(c) = self.customer_mut(t.customer_id) {
                c.week.tendered += 1;
            }
        }
        if unanswered > 0 {
            dropped.push(if unanswered == 1 {
                "1 tender went unanswered and counts as declined.".to_string()
            } else {
                format!("{unanswered} tenders went unanswered and count as declined.")
            });
        }
        let today = self.day;
        self.accepted.retain(|t| t.for_day > today);
        news.extend(dropped);
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
        // Home time first: a driver who quits over it tonight doesn't also
        // give notice tonight.
        let home = self.home_time_check(shift, &mut news);
        news.extend(self.law_overnight(world));
        news.extend(self.morale_overnight());
        let truck_costs = TRUCK_OVERHEAD_PER_DAY * self.tractors.len() as f64;
        self.funds -= truck_costs;
        news.extend(self.money_overnight());
        let (met, why) = self.goal_result(&shift.goal, shift, world);
        self.week.goals += 1;
        if met {
            self.reputation += 1.0;
            self.week.goals_met += 1;
        }
        for text in crate::growth::new_milestones(self) {
            news.push(format!("Milestone: {text}."));
            self.milestones.push(crate::growth::Milestone {
                day: self.day,
                text,
            });
        }
        let goal_result = format!(" Goal {}.", if met { "met" } else { "missed" });
        let story = self.day_story(shift, world, &home, met, &why);
        self.reputation = self.reputation.clamp(0.0, 100.0);
        let scorecard = if crate::roster::weekday(self.day) == 6 {
            crate::customers::scorecard(self)
        } else {
            Vec::new()
        };
        self.reputation = self.reputation.clamp(0.0, 100.0);
        for d in &mut self.drivers {
            if d.status == Status::OffDuty && d.rest_note.is_none() {
                d.status = Status::Idle;
            }
        }
        self.record.days_worked += 1;
        self.day += 1;
        self.market.advance_to(self.day);
        // The news is written at the close and heard in the morning: a day on.
        self.news = news
            .iter()
            .map(|l| crate::desk::told_next_morning(l))
            .collect();
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
        out.extend(scorecard);
        out
    }

    /// What the law does overnight: medical cards that lapse, the
    /// quarter's random drug test, the safety score aging, and an audit
    /// when the score is over the line.
    fn law_overnight(&mut self, _world: &World) -> Vec<String> {
        let mut news = Vec::new();
        let tomorrow = self.day + 1;
        let next_morning = abs_min(tomorrow, SHIFT_START);
        let day_after = abs_min(tomorrow + 1, SHIFT_START);
        let mut rng = crate::rng(self.seed, self.day, "law");
        let parked_for = |d: &mut Driver, note: &str| {
            let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
            if free && d.rest_note.is_none() {
                d.status = Status::OffDuty;
                d.rest_note = Some(note.to_string());
                d.available_at = d.available_at.max(next_morning).max(day_after);
                true
            } else {
                false
            }
        };
        for d in &mut self.drivers {
            if d.record.medical_card_expired(tomorrow) && parked_for(d, "a medical card renewal") {
                d.record.medical_card_day = tomorrow;
                news.push(format!(
                    "{}'s medical card has expired. They're at the clinic tomorrow and back the day after.",
                    d.name
                ));
            }
        }
        if tomorrow % crate::law::DRUG_TEST_EVERY_DAYS == 30 && !self.drivers.is_empty() {
            let pick = rng.randint(0, self.drivers.len() as i64 - 1) as usize;
            let d = &mut self.drivers[pick];
            if parked_for(d, "a random drug test") {
                news.push(format!(
                    "{} drew the quarter's random drug test. They lose tomorrow to it.",
                    d.name
                ));
            }
        }
        self.safety.prune(tomorrow);
        if self.safety.over_threshold(tomorrow)
            && (self.safety.audited_day == 0 || tomorrow - self.safety.audited_day >= 90)
            && !self.audit_pending
        {
            self.audit_pending = true;
            self.safety.audited_day = tomorrow;
            let hours_trouble = self.drivers.iter().any(|d| {
                d.record
                    .active(tomorrow)
                    .any(|c| c.violation == crate::law::Violation::Hours)
            });
            let mut line = format!(
                "The safety score is {}, over the line. An auditor is at the desk in the morning; it opens at 10 AM.",
                self.safety.total(tomorrow).round()
            );
            if hours_trouble {
                self.funds -= crate::law::AUDIT_FINE;
                line.push_str(&format!(
                    " The hours records are bad: {} in fines.",
                    crate::money(crate::law::AUDIT_FINE)
                ));
            }
            news.push(line);
        }
        news
    }

    /// Money overnight: payments in, Friday's settlement, the first's
    /// bills, interest on the line, and the clocks that end a career.
    fn money_overnight(&mut self) -> Vec<String> {
        use crate::money::*;
        let mut news = Vec::new();
        let tomorrow = self.day + 1;
        let (paid, waiting): (Vec<Receivable>, Vec<Receivable>) =
            std::mem::take(&mut self.receivables)
                .into_iter()
                .partition(|r| r.due_day <= tomorrow);
        self.receivables = waiting;
        if !paid.is_empty() {
            let total: f64 = paid.iter().map(|r| r.amount).sum();
            self.funds += total;
            let mut from: Vec<String> = Vec::new();
            for r in &paid {
                if !from.contains(&r.from) {
                    from.push(r.from.clone());
                }
            }
            news.push(format!(
                "Payments in: {} from {}.",
                crate::money(total),
                from.join(", ")
            ));
        }
        if is_settlement_day(self.day) && self.settlement_owed > 0.0 {
            let owed = self.settlement_owed;
            self.settlement_owed = 0.0;
            self.funds -= owed;
            news.push(format!(
                "Friday settlement: {} to the drivers.",
                crate::money(owed)
            ));
        }
        if is_bill_day(tomorrow) {
            let mut notes = 0.0;
            for t in &mut self.tractors {
                if t.note > 0.0 {
                    notes += t.note;
                    t.note_months_left = t.note_months_left.saturating_sub(1);
                    if t.note_months_left == 0 {
                        news.push(format!("Truck {} is paid off.", t.id));
                        t.note = 0.0;
                        t.paid_off = true;
                    }
                }
            }
            let insurance = insurance(self.tractors.len(), self.safety.total(tomorrow));
            let interest = if self.funds < 0.0 {
                (-self.funds * CREDIT_INTEREST).round()
            } else {
                0.0
            };
            let rent = TERMINAL_RENT * self.terminals().len() as f64;
            self.funds -= notes + insurance + rent + interest;
            let mut line = format!(
                "The first of the month: {} in truck notes, {} insurance, {} terminal rent.",
                crate::money(notes),
                crate::money(insurance),
                crate::money(rent)
            );
            if interest > 0.0 {
                line.push_str(&format!(
                    " {} interest on the line of credit.",
                    crate::money(interest)
                ));
            }
            news.push(line);
        }
        if self.funds < -CREDIT_LINE {
            self.days_over_line += 1;
            let left = OVER_LINE_DAYS - self.days_over_line;
            if left == WARNING_DAYS {
                news.push(format!(
                    "The bank called: cash has been under the {} line for {} days. A week more and they call the loan.",
                    crate::money(CREDIT_LINE),
                    self.days_over_line
                ));
            } else if left <= 0 {
                self.ended = Some(Ending::OverTheLine);
            }
        } else {
            self.days_over_line = 0;
        }
        if self.safety.total(tomorrow) >= SHUTDOWN_SCORE {
            self.shutdown_days += 1;
            if self.shutdown_days == 1 {
                news.push(format!(
                    "The safety score is {}. The agency gives a week to bring it under {SHUTDOWN_SCORE} before a shutdown order.",
                    self.safety.total(tomorrow).round()
                ));
            } else if self.shutdown_days > WARNING_DAYS {
                self.ended = Some(Ending::Shutdown);
            }
        } else {
            self.shutdown_days = 0;
        }
        if self.drivers.is_empty() && self.ended.is_none() {
            self.ended = Some(Ending::NoDrivers);
        }
        if let Some(e) = &self.ended {
            news.push(format!("The career is over: {}.", e.text()));
        }
        news
    }

    /// Service a truck: $600, and it's in the shop tomorrow.
    pub fn service_truck(
        &mut self,
        shift: &mut Shift,
        tractor_id: u32,
    ) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let today = self.day;
        let t = self
            .tractors
            .iter_mut()
            .find(|t| t.id == tractor_id)
            .ok_or(ActError::NoSuchItem)?;
        if t.in_shop(today) {
            return Err(ActError::Refused(format!(
                "Truck {} is in the shop already.",
                t.id
            )));
        }
        if let Some(d) = t
            .driver_id
            .and_then(|id| self.drivers.iter().find(|d| d.id == id))
        {
            if matches!(d.status, Status::OnLoad | Status::OutOnRun) {
                return Err(ActError::Refused(format!(
                    "{} has it on the road. Service it when they're back.",
                    d.name
                )));
            }
        }
        t.miles_at_service = t.miles;
        t.in_shop_until = today + 1;
        let id = t.id;
        self.funds -= crate::money::SERVICE_COST;
        shift.clock.spend(Action::CheckTruck);
        self.ring_due(shift);
        Ok(format!(
            "Truck {id} is in the shop for the rest of today: {}. It's back tomorrow morning.",
            crate::money(crate::money::SERVICE_COST)
        ))
    }

    /// Buy a used tractor on a note.
    pub fn buy_truck(&mut self, shift: &mut Shift) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        if self.funds < crate::money::TRUCK_DOWN {
            return Err(ActError::NoFunds);
        }
        self.funds -= crate::money::TRUCK_DOWN;
        // Trucks are numbered in the fleet, 1 up, not from the shared ids.
        let id = self.tractors.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        let mut rng = crate::rng(self.seed, self.day, &format!("truck:{id}"));
        let miles = rng.randint(250_000, 450_000) as f64;
        self.tractors.push(crate::money::Tractor {
            id,
            age: rng.randint(4, 7) as u32,
            miles,
            miles_at_service: miles,
            note: crate::money::TRUCK_NOTE,
            note_months_left: crate::money::TRUCK_NOTE_MONTHS,
            in_shop_until: 0,
            driver_id: None,
            paid_off: false,
        });
        shift.clock.spend(Action::CheckTruck);
        self.ring_due(shift);
        Ok(format!(
            "Truck {id} is yours: {} down and {} a month for 5 years. It's a spare until someone's hired for it.",
            crate::money(crate::money::TRUCK_DOWN),
            crate::money(crate::money::TRUCK_NOTE)
        ))
    }

    /// Every terminal the carrier runs from.
    pub fn terminals(&self) -> Vec<String> {
        let mut out = vec![self.terminal.clone()];
        out.extend(self.terminal2.clone());
        out
    }

    /// "Lakeshore Foods here. We've liked the service. Would you take us on
    /// dedicated: 10 loads a week, 5 percent over the contract rate, paid in
    /// 30 days? We'd expect 90 percent on both counts, every week."
    pub fn dedicated_text(&self, customer_id: u32) -> String {
        format!(
            "{} here. We've liked the service. Would you take us on dedicated: {} loads a week, {} percent over the contract rate, paid in 30 days? We'd expect 90 percent acceptance and on time every week, and we'd pull it after two weeks under.",
            self.customer_name(customer_id),
            crate::growth::DEDICATED_WEEKLY_LOADS,
            (crate::growth::DEDICATED_RATE_BONUS * 100.0).round()
        )
    }

    pub fn answer_dedicated(&mut self, customer_id: u32, take: bool) -> String {
        let week = (self.day - 1) / 7;
        let Some(c) = self.customer_mut(customer_id) else {
            return "They've moved on.".into();
        };
        c.dedicated_asked_week = week.max(1);
        if take {
            c.dedicated = true;
            c.dedicated_weeks_under = 0;
            c.weekly_loads = c.weekly_loads.max(crate::growth::DEDICATED_WEEKLY_LOADS);
            c.rate_mult =
                ((c.rate_mult + crate::growth::DEDICATED_RATE_BONUS) * 100.0).round() / 100.0;
            c.pay_days = crate::money::PAY_DAYS[0];
            format!(
                "{} is a dedicated account. Their tenders come every working day from the next batch.",
                c.name
            )
        } else {
            format!("{} understands. They'll ask again in a month.", c.name)
        }
    }

    /// Open the second terminal in `city`: the opening cost, rent from the
    /// first, two accounts shipping out of it, and loads offered there
    /// every morning.
    pub fn open_terminal(
        &mut self,
        shift: &mut Shift,
        city: &str,
        world: &World,
    ) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        if self.terminal2.is_some() {
            return Err(ActError::Refused("You have two terminals already.".into()));
        }
        if crate::growth::Tier::of(self.reputation) < crate::growth::Tier::Established {
            return Err(ActError::Refused(
                "A second terminal takes an established reputation, 75 or better.".into(),
            ));
        }
        if self.drivers.len() < crate::growth::TERMINAL_MIN_DRIVERS {
            return Err(ActError::Refused(format!(
                "A second terminal needs {} drivers to run it.",
                crate::growth::TERMINAL_MIN_DRIVERS
            )));
        }
        if self.funds < crate::growth::TERMINAL_COST {
            return Err(ActError::NoFunds);
        }
        self.funds -= crate::growth::TERMINAL_COST;
        self.terminal2 = Some(city.to_string());
        let taken: Vec<String> = self.customers.iter().map(|c| c.name.clone()).collect();
        let new = crate::customers::customers_at(
            world,
            city,
            &self.carrier_key,
            self.seed,
            self.day,
            &self.drivers,
            self.next_id,
            2,
            &taken,
        );
        self.next_id += new.len() as u32;
        let names: Vec<String> = new.iter().map(|c| c.name.clone()).collect();
        self.customers.extend(new);
        shift.clock.spend(Action::SalesCall);
        self.ring_due(shift);
        Ok(format!(
            "The {} terminal is open: {} down and {} a month. {} ship out of it from the next tenders, and its board fills in the morning. Drivers go home to whichever terminal is nearer.",
            world.spoken_city(city, None),
            crate::money(crate::growth::TERMINAL_COST),
            crate::money(crate::money::TERMINAL_RENT),
            match names.len() {
                0 => "No accounts yet".to_string(),
                1 => names[0].clone(),
                _ => format!("{} and {}", names[0], names[1]),
            }
        ))
    }

    /// The Company screen: the tier, the score, the accounts, the terminals
    /// and the milestones.
    pub fn company_lines(&self, world: &World) -> Vec<String> {
        let tier = crate::growth::Tier::of(self.reputation);
        let mut lines = vec![
            crate::growth::tier_line(self.reputation),
            tier.opens().to_string(),
            format!(
                "Career score {}: a point a delivery, another on time, 10 an account, 50 a milestone, 5 a point of reputation.",
                crate::growth::score(self)
            ),
        ];
        let dedicated: Vec<&str> = self
            .customers
            .iter()
            .filter(|c| c.dedicated)
            .map(|c| c.name.as_str())
            .collect();
        lines.push(format!(
            "{} accounts of {}{}.",
            self.customers.len(),
            tier.max_customers(),
            if dedicated.is_empty() {
                String::new()
            } else {
                format!(", dedicated: {}", dedicated.join(", "))
            }
        ));
        lines.push(match &self.terminal2 {
            Some(c) => format!(
                "Terminals: {} and {}.",
                world.spoken_city(&self.terminal, None),
                world.spoken_city(c, None)
            ),
            None => format!(
                "One terminal, {}. A second takes an established reputation, {} drivers and {}.",
                world.spoken_city(&self.terminal, None),
                crate::growth::TERMINAL_MIN_DRIVERS,
                crate::money(crate::growth::TERMINAL_COST)
            ),
        });
        if self.milestones.is_empty() {
            lines.push("No milestones yet. The first is 100 loads.".into());
        }
        for m in &self.milestones {
            lines.push(format!("Day {}: {}.", m.day, m.text));
        }
        lines
    }

    pub fn toggle_factoring(&mut self) -> String {
        self.factoring = !self.factoring;
        if self.factoring {
            "Factoring on: every load pays the next day, less 3 percent.".into()
        } else {
            "Factoring off: customers pay in 30 to 45 days, brokers in 30.".into()
        }
    }

    /// Morale overnight: a driver who is done gives two weeks' notice and
    /// leaves when it runs out; on Sundays a solid driver may bring in a
    /// friend.
    fn morale_overnight(&mut self) -> Vec<String> {
        let mut news = Vec::new();
        let tomorrow = self.day + 1;
        let mut gone = Vec::new();
        for d in &mut self.drivers {
            let free = !matches!(d.status, Status::OnLoad | Status::OutOnRun);
            if d.leaving_day > 0 && tomorrow >= d.leaving_day && free {
                news.push(format!("{}'s notice is up. They've moved on.", d.name));
                gone.push(d.id);
            } else if d.morale() == "done" && d.leaving_day == 0 {
                d.leaving_day = tomorrow + crate::roster::NOTICE_DAYS;
                news.push(format!(
                    "{} gave two weeks' notice. Home time and a fair week might change their mind.",
                    d.name
                ));
            } else if d.leaving_day > 0 && d.morale() != "done" && d.morale() != "grumbling" {
                d.leaving_day = 0;
                news.push(format!("{} is staying after all.", d.name));
            }
        }
        self.drivers.retain(|d| !gone.contains(&d.id));
        if crate::roster::weekday(self.day) == 6 {
            let mut rng = crate::rng(self.seed, self.day, "referrals");
            let solid: Vec<Driver> = self
                .drivers
                .iter()
                .filter(|d| d.goodwill >= 80)
                .cloned()
                .collect();
            for by in solid {
                if rng.random() < 0.3 && self.referrals.len() < 2 {
                    if let Some(friend) = crate::hiring::referral(self, &by) {
                        news.push(format!(
                            "{} has a friend looking for a seat: {}. They're in tomorrow's applicants.",
                            by.name, friend.name
                        ));
                        self.referrals.push((by.name.clone(), friend));
                    }
                }
            }
        }
        news
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
                Some(d)
                    if d.home_given()
                        || d.location == self.terminal
                        || (!self.due_home(d) && d.home_due_day() <= self.day) =>
                {
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
        let mut missed = Vec::new();
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
            missed.push(format!("{}, goodwill {}", d.name, d.goodwill));
            if free && d.quit_warned && d.goodwill <= QUIT_GOODWILL {
                night.quit.push(d.id);
            }
        }
        match missed.len() {
            0 => {}
            1 => news.push(format!("Missed promised home time: {}.", missed[0])),
            _ => news.push(format!("Missed promised home time: {}.", missed.join("; "))),
        }
        let layovers = self
            .drivers
            .iter()
            .filter(|d| {
                night.missed.contains(&d.name) && d.pattern() == crate::roster::HomePattern::Local
            })
            .count();
        if layovers > 0 {
            let cost = layovers as f64 * LAYOVER_COST;
            self.funds -= cost;
            news.push(format!(
                "Local drivers laid over away from home: ${cost:.0} for motels and layover pay."
            ));
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
            (1, None) => "The one delivery came in on time.".into(),
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
                .map(|l| match self.driver(l.driver_id) {
                    Some(d) => format!(
                        ", mostly {} {} empty miles",
                        crate::possessive(&d.name),
                        l.deadhead_mi.round()
                    ),
                    None => format!(", mostly one {}-mile empty run", l.deadhead_mi.round()),
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

    /// What closing now would leave behind: contract loads nobody has, and
    /// tenders that would count as declined. `None` when there's nothing.
    pub fn left_open(&self, shift: &Shift, world: &World) -> Option<String> {
        let uncovered: Vec<String> = shift
            .board
            .iter()
            .filter(|l| !l.source.is_spot())
            .map(|l| {
                format!(
                    "{} load to {}",
                    crate::possessive(&l.owner(self)),
                    world.spoken_city(&l.job.destination, None)
                )
            })
            .collect();
        let mut parts = Vec::new();
        match uncovered.len() {
            0 => {}
            1 => parts.push(format!(
                "Nobody has {}. Uncovered, it costs reputation.",
                uncovered[0]
            )),
            n => parts.push(format!(
                "{n} contract loads have no driver: {}. Each one uncovered costs reputation.",
                crate::and_list(&uncovered)
            )),
        }
        match shift.tenders.len() {
            0 => {}
            1 => parts.push("1 tender is still open; closing declines it.".into()),
            n => parts.push(format!(
                "{n} tenders are still open; closing declines them."
            )),
        }
        if parts.is_empty() {
            return None;
        }
        parts.push("Close the day again to close anyway.".into());
        Some(parts.join(" "))
    }

    /// The one thing that most needs the dispatcher now. Things the desk
    /// can still act on come first: an empty driver, a contract load nobody
    /// has, then a load running late.
    pub fn needs_now(&self, shift: &Shift, world: &World) -> String {
        if !shift.pending.is_empty() {
            return "The phone is ringing. Press Enter to answer.".into();
        }
        let now = shift.now(self.day);
        let open = !shift.clock.closed();
        let idle: Vec<&Driver> = self
            .drivers
            .iter()
            .filter(|d| d.status == Status::Idle && d.available_at <= now)
            .collect();
        let others = |n: usize| match n {
            0 => String::new(),
            1 => " 1 more driver is empty too.".into(),
            n => format!(" {n} more drivers are empty too."),
        };
        if let Some(d) = idle.iter().find(|d| self.due_home(d)).filter(|_| open) {
            return format!(
                "{} is due home in {} tonight and is in {}.{}",
                d.name,
                world.spoken_city(&self.terminal, None),
                world.spoken_city(&d.location, None),
                others(idle.len() - 1)
            );
        }
        if let Some(d) = idle.first().filter(|_| open) {
            return format!(
                "{} is empty in {}. Find them a load.{}",
                d.name,
                world.spoken_city(&d.location, None),
                others(idle.len() - 1)
            );
        }
        let uncovered: Vec<&Load> = shift.board.iter().filter(|l| !l.source.is_spot()).collect();
        if let Some(l) = uncovered.first().filter(|_| open) {
            let more = match uncovered.len() - 1 {
                0 => String::new(),
                1 => " 1 more contract load is waiting too.".into(),
                n => format!(" {n} more contract loads are waiting too."),
            };
            return format!(
                "Nobody has {} {} to {}. Uncovered at the close, it counts against you.{more}",
                crate::possessive(&l.owner(self)),
                l.job.cargo.label,
                world.spoken_city(&l.job.destination, None)
            );
        }
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
        match shift.tenders.len() {
            0 => {}
            1 => return "1 tender is waiting for an answer. Open at the close is declined.".into(),
            n => {
                return format!(
                    "{n} tenders are waiting for an answer. Open at the close is declined."
                )
            }
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
    #[test]
    fn space_names_an_empty_driver_before_a_late_load() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        s.pending.clear();
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        s.pending.clear();
        // Make the rolling trip hopelessly late.
        g.trips[0].delay_min += 3000.0;
        let line = g.needs_now(&s, w);
        assert!(line.contains("is empty in"), "{line}");
        assert!(
            line.contains("more drivers are empty too") || line.contains("1 more driver"),
            "{line}"
        );
        for d in &mut g.drivers {
            if d.status == Status::Idle {
                d.status = Status::OffDuty;
            }
        }
        s.board.retain(|l| l.source.is_spot());
        assert!(g.needs_now(&s, w).contains("is at risk"));
    }

    #[test]
    fn a_break_that_ends_during_a_desk_action_still_rings_the_driver() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        s.pending.clear();
        let id = g.drivers[0].id;
        let wake_at = s.now(g.day) + 5;
        {
            let d = g.driver_mut(id).unwrap();
            d.status = Status::OffDuty;
            d.rest_note = Some("a 10-hour break".into());
            d.available_at = wake_at;
        }
        s.schedule.push(
            wake_at,
            Ring::Call(crate::phone::Call::Empty { driver_id: id }),
        );
        // Checking a truck or an interview runs the clock past the wake-up.
        g.interview(&mut s, 0).unwrap();
        assert_eq!(g.driver(id).unwrap().status, Status::Idle);
        assert!(
            s.pending
                .iter()
                .any(|r| matches!(r, Ring::Call(crate::phone::Call::Empty { driver_id }) if *driver_id == id)),
            "the driver's call rings"
        );
    }

    #[test]
    fn a_new_hire_starts_at_the_terminal() {
        let w = get_world();
        let mut g = game();
        g.funds = 100_000.0;
        let mut s = g.morning(w);
        s.schedule = Default::default();
        s.pending.clear();
        let line = g.hire(&mut s, 0).unwrap();
        assert!(line.contains("signed on"), "{line}");
        let d = g.drivers.last().unwrap();
        assert_eq!(d.location, g.terminal);
        assert!(!d.card_at(w, s.now(g.day), g.day).contains(" in ,"));
    }

    #[test]
    fn closing_warns_about_uncovered_contract_loads_and_open_tenders() {
        let w = get_world();
        let g = game();
        let s = g.morning(w);
        let warning = g.left_open(&s, w).expect("day 1 has open tenders");
        assert!(warning.contains("still open"), "{warning}");
        assert!(warning.ends_with("Close the day again to close anyway."));
    }

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
        g.factoring = false;
        // No local layovers in this money check.
        for d in &mut g.drivers {
            d.home = Some(crate::roster::HomePattern::Regional);
        }
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        let eta = g.trips[0].eta();
        let funds = g.funds;
        let news = g.close_day(&mut s, w);
        if eta <= abs_min(2, SHIFT_START) {
            assert_eq!(g.record.delivered, 1);
            assert!(g.trips.is_empty());
            assert!(news[0].contains("delivery"), "{}", news[0]);
            assert!(g.news[0].contains("delivered"));
            assert!(g.funds > funds - 2_000.0);
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
        g.factoring = false;
        // No local layovers in this money check.
        for d in &mut g.drivers {
            d.home = Some(crate::roster::HomePattern::Regional);
        }
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let driver = arriving_at(&mut g, &mut s, 19 * 60);
        let trip = g.trips[0].clone();
        let hours_taken = (trip.eta() - trip.depart) as f64 / 60.0;
        let gross = trip.job.payout(hours_taken, 0.0, 0.0);
        let driver_pay = gross * pay_plan_for_key(Some(&g.carrier_key)).pay_share;
        let fuel = crate::money::fuel_cost(trip.job.distance_mi, g.fuel_price());
        let road = (trip.job.distance_mi * crate::money::ROAD_COST_PER_MILE).round();
        let funds_before = g.funds;
        let expected_funds =
            funds_before - fuel - road - TRUCK_OVERHEAD_PER_DAY * g.tractors.len() as f64;

        g.close_day(&mut s, w);

        assert!(
            (g.funds - expected_funds).abs() < 0.01,
            "{} vs {expected_funds}",
            g.funds
        );
        assert_eq!(g.settlement_owed, driver_pay, "driver pay waits for Friday");
        assert_eq!(g.receivables.len(), 1);
        assert_eq!(g.receivables[0].amount, gross);
        assert_eq!(g.receivables[0].due_day, 1 + crate::money::BROKER_PAY_DAYS);
        let news = g
            .news
            .iter()
            .find(|line| line.contains(" delivered "))
            .unwrap();
        assert!(news.contains("billed to"), "{news}");
        assert!(
            news.contains(&format!("{} fuel", crate::money(fuel))),
            "{news}"
        );
        assert!(news.contains("on Friday"), "{news}");
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
        g.normalize(w);
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
            .any(|l| l.starts_with("Missed promised home time")));
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
            .any(|l| l.starts_with("Missed promised home time")));
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
    fn day_one_tenders_are_open_and_an_accepted_one_leads_the_board() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        assert!(!s.tenders.is_empty(), "Monday's tenders are on the desk");
        let n = s.tenders.len();
        let cid = s.tenders[0].customer_id;
        let line = g.accept_tender(&mut s, 0, w).unwrap();
        assert!(line.starts_with("Accepted. It's on the board:"), "{line}");
        assert_eq!(s.tenders.len(), n - 1);
        let top = &s.board[0];
        assert!(!top.source.is_spot());
        assert!(top.appointment.is_some());
        let text = g.load_text(top, w);
        assert!(text.starts_with(&g.customer_name(cid)), "{text}");
        assert!(text.contains("deliver by"), "{text}");
        assert_eq!(g.customer(cid).unwrap().week.accepted, 1);
        assert_eq!(g.customer(cid).unwrap().week.tendered, 1);
        if s.tenders.is_empty() {
            s.tenders = crate::customers::tenders_for(&g, w, g.day);
        }
        let cid2 = s.tenders[0].customer_id;
        let accepted = g.customer(cid2).unwrap().week.accepted;
        let line = g.decline_tender(&mut s, 0).unwrap();
        assert!(line.starts_with("Declined."), "{line}");
        assert_eq!(g.customer(cid2).unwrap().week.accepted, accepted);
    }

    #[test]
    fn tomorrows_tenders_ring_at_five_and_accepted_loads_lead_the_next_board() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        assert!(!s.tomorrow.is_empty());
        let due: Vec<i64> = s
            .schedule
            .iter()
            .filter(|(_, r)| matches!(r, Ring::Call(crate::phone::Call::Tenders)))
            .map(|(at, _)| *at)
            .collect();
        assert_eq!(due, vec![abs_min(1, crate::customers::TENDER_MINUTE)]);
        let title = Ring::Call(crate::phone::Call::Tenders).title(&g, &s, w);
        assert!(title.starts_with("Tomorrow's tenders are in:"), "{title}");
        let n = s.tomorrow.len();
        let lines = g.resolve(&mut s, &Ring::Call(crate::phone::Call::Tenders), 0, w);
        assert!(
            lines[0].starts_with(&format!("All {n} accepted."))
                || (n == 1 && lines[0].starts_with("Accepted.")),
            "{lines:?}"
        );
        assert_eq!(g.accepted.iter().filter(|t| t.for_day == 2).count(), n);
        while !s.tenders.is_empty() {
            g.decline_tender(&mut s, 0).unwrap();
        }
        g.close_day(&mut s, w);
        let s = g.morning(w);
        assert!(s.board[..n].iter().all(|l| !l.source.is_spot()));
        assert!(s.board[n..].iter().all(|l| l.source.is_spot()));
        assert!(s.tenders.is_empty(), "day 2's tenders came last night");
    }

    #[test]
    fn open_tenders_and_uncovered_contract_loads_count_at_the_close() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let open = s.tenders.len() + s.tomorrow.len();
        let cid = s.tenders[0].customer_id;
        g.accept_tender(&mut s, 0, w).unwrap();
        let rep = g.reputation;
        let out = g.close_day(&mut s, w);
        let c = g.customer(cid).unwrap();
        assert_eq!(c.week.failed, 1);
        assert_eq!(c.week.tendered, 1 + s_tendered_for(&g, cid, open));
        assert!(
            g.news.iter().any(|l| l.contains("was never covered")),
            "{:?}",
            g.news
        );
        assert!(
            g.news.iter().any(|l| l.contains("unanswered and count")),
            "{:?}",
            g.news
        );
        assert!(g.reputation < rep);
        assert!(s.board.iter().all(|l| l.source.is_spot()));
        assert!(out.last().unwrap().starts_with("Day closed."), "{out:?}");
    }

    /// How many of `open` unanswered tenders were this customer's, read
    /// back from the week's record.
    fn s_tendered_for(g: &Game, cid: u32, open: usize) -> u32 {
        let total: u32 = g.customers.iter().map(|c| c.week.tendered).sum();
        assert_eq!(total as usize, open, "every open tender was counted");
        g.customer(cid).unwrap().week.tendered - 1
    }

    #[test]
    fn a_counter_takes_five_minutes_and_a_broker_hears_it_once() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let load = s.board.iter().position(|l| l.source.is_spot()).unwrap();
        let (pay, broker) = (s.board[load].job.pay, s.board[load].owner(&g));
        let before = s.clock.minute;
        let lines = g.counter_rate(&mut s, load, w).unwrap();
        assert_eq!(s.clock.minute, before + 5);
        assert!(lines[0].starts_with(&broker), "{lines:?}");
        match s.board.get(load) {
            Some(l) if l.owner(&g) == broker => {
                assert!(l.countered);
                assert!(l.job.pay == pay || l.job.pay == (pay * 1.1).round());
                let again = g.counter_rate(&mut s, load, w).unwrap_err();
                assert!(matches!(again, ActError::Refused(why) if why.contains("already heard")));
            }
            _ => assert!(lines[0].contains("another carrier"), "{lines:?}"),
        }
        let contract = s.board.iter().position(|l| !l.source.is_spot());
        if let Some(i) = contract {
            assert!(g.counter_rate(&mut s, i, w).is_err());
        }
    }

    #[test]
    fn weekends_are_light_and_sunday_closes_with_the_scorecard() {
        let w = get_world();
        let mut g = game();
        g.day = 6;
        let s = g.morning(w);
        assert_eq!(s.board.len(), crate::board::SATURDAY_BOARD);
        assert!(s.tenders.is_empty() && s.tomorrow.is_empty());
        assert!(g
            .briefing(&s, w)
            .iter()
            .any(|l| l.contains("Saturday: a light board")));
        g.day = 7;
        let mut s = g.morning(w);
        s.schedule = Default::default();
        assert_eq!(s.board.len(), crate::board::SUNDAY_BOARD);
        let out = g.close_day(&mut s, w);
        assert!(
            out.iter().any(|l| l.starts_with("Week 1 scorecard.")),
            "{out:?}"
        );
        assert_eq!(g.day, 8);
        let s = g.morning(w);
        assert!(s.tenders.is_empty(), "Monday's tenders came on Friday");
    }

    #[test]
    fn an_older_save_gets_its_customers_on_load() {
        let w = get_world();
        let mut v = serde_json::to_value(game()).unwrap();
        let obj = v.as_object_mut().unwrap();
        for field in ["customers", "accepted", "broker_trust", "week"] {
            obj.remove(field);
        }
        let mut g: Game = serde_json::from_value(v).unwrap();
        assert!(g.customers.is_empty());
        g.normalize(w);
        assert_eq!(g.customers.len(), crate::customers::START_CUSTOMERS);
        assert!(g.customers.iter().all(|c| c.id >= 4));
        assert!(g.next_id > g.customers.last().unwrap().id);
    }

    #[test]
    fn a_scale_stop_goes_on_the_record_and_the_score() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let driver = arriving_at(&mut g, &mut s, 15 * 60);
        g.trips[0].dock_hold = false;
        s.schedule = Default::default();
        let trip_id = g.trips[0].id;
        let eta = g.trips[0].eta();
        let inc = crate::incidents::Incident {
            trip_id,
            kind: crate::incidents::Kind::Inspection { level: 2 },
        };
        let title = Ring::Incident(inc.clone()).title(&g, &s, w);
        assert!(title.contains("Level 2 inspection"), "{title}");
        let lines = g.resolve(&mut s, &Ring::Incident(inc), 0, w);
        let d = g.driver(driver).unwrap();
        assert_eq!(d.record.inspections, 1);
        assert!(g.trips[0].eta() >= eta + 30, "{lines:?}");
        let text = lines.join(" ");
        if d.record.citations.is_empty() {
            assert!(text.starts_with("Clean inspection."), "{text}");
            assert_eq!(d.record.clean_inspections, 1);
        } else {
            assert!(text.contains("They found"), "{text}");
            assert!(g.safety.total(g.day) > 0.0);
            assert!(text.contains("Carrier safety score now"), "{text}");
        }
        assert!(text.contains("New arrival"), "{text}");
    }

    #[test]
    fn a_driver_can_be_sent_past_the_clock_and_a_scale_finds_it() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let (load, id) = legal_pair(&g, &s);
        s.board[load].job.deadline_game_h = 0.2;
        let d = g.driver(id).unwrap();
        assert!(crate::board::hours_warning(&s.board[load].job, d, w).is_some());
        let lines = g.assign(&mut s, load, id, w).unwrap();
        assert!(
            lines.iter().any(|l| l.contains("past the clock")),
            "{lines:?}"
        );
        let t = g.trips[0].clone();
        assert!(t.over_hours && t.hours_resolved);
        assert!(t.plan_min <= t.drive_min + 30.0);
        s.schedule = Default::default();
        let inc = crate::incidents::Incident {
            trip_id: t.id,
            kind: crate::incidents::Kind::Inspection { level: 3 },
        };
        let mut found = 0;
        for minute in 0..40 {
            let mut g2 = g.clone();
            let mut s2 = g2.morning(w);
            g2.trips = vec![t.clone()];
            s2.clock.minute = crate::desk::SHIFT_START + 60 + minute;
            let lines = g2.resolve(&mut s2, &Ring::Incident(inc.clone()), 0, w);
            if lines
                .iter()
                .any(|l| l.contains("out of service for 10 hours"))
            {
                found += 1;
                assert!(g2.trips[0].delay_min >= crate::law::HOURS_OOS_MIN);
                let d = g2.driver(id).unwrap();
                assert!(d
                    .record
                    .citations
                    .iter()
                    .any(|c| c.violation == crate::law::Violation::Hours && c.serious));
                assert!(g2.safety.category(crate::law::Category::Hours, g2.day) >= 7.0);
            }
        }
        assert!(
            found > 20,
            "an over-hours log is found most of the time: {found} of 40"
        );
    }

    #[test]
    fn a_driver_who_is_done_gives_notice_and_leaves_when_it_runs_out() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = g.drivers[1].id;
        g.driver_mut(id).unwrap().goodwill = 5;
        assert_eq!(g.driver(id).unwrap().morale(), "done");
        g.close_day(&mut s, w);
        assert!(
            g.news.iter().any(|l| l.contains("gave two weeks' notice")),
            "{:?}",
            g.news
        );
        assert_eq!(
            g.driver(id).unwrap().leaving_day,
            2 + crate::roster::NOTICE_DAYS
        );
        g.driver_mut(id).unwrap().leaving_day = 3;
        let mut s = g.morning(w);
        s.schedule = Default::default();
        g.close_day(&mut s, w);
        assert!(g.driver(id).is_none());
        assert!(
            g.news.iter().any(|l| l.contains("moved on")),
            "{:?}",
            g.news
        );
    }

    #[test]
    fn medical_cards_and_drug_tests_park_a_driver_for_a_day() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = g.drivers[0].id;
        g.driver_mut(id).unwrap().record.medical_card_day = 2 - crate::law::MEDICAL_CARD_DAYS;
        g.close_day(&mut s, w);
        let d = g.driver(id).unwrap();
        assert_eq!(d.rest_note.as_deref(), Some("a medical card renewal"));
        assert_eq!(d.record.medical_card_day, 2);
        assert!(
            g.news
                .iter()
                .any(|l| l.contains("medical card has expired")),
            "{:?}",
            g.news
        );
        let mut g = game();
        g.day = 29;
        let mut s = g.morning(w);
        s.schedule = Default::default();
        g.close_day(&mut s, w);
        assert!(
            g.news.iter().any(|l| l.contains("random drug test")),
            "{:?}",
            g.news
        );
        assert_eq!(
            g.drivers
                .iter()
                .filter(|d| d.rest_note.as_deref() == Some("a random drug test"))
                .count(),
            1
        );
    }

    #[test]
    fn a_score_over_the_line_brings_an_auditor_in_the_morning() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        g.safety.add(1, crate::law::Category::Hours, 35.0);
        let id = g.drivers[0].id;
        g.driver_mut(id)
            .unwrap()
            .record
            .citations
            .push(crate::law::Citation {
                day: 1,
                violation: crate::law::Violation::Hours,
                serious: true,
            });
        let funds = g.funds;
        g.close_day(&mut s, w);
        assert!(g.audit_pending);
        assert!(g.news.iter().any(|l| l.contains("auditor")), "{:?}", g.news);
        assert!(g.funds < funds - crate::law::AUDIT_FINE);
        let mut s = g.morning(w);
        assert_eq!(s.clock.minute, 10 * 60);
        assert!(g.briefing(&s, w).iter().any(|l| l.contains("auditor")));
        assert!(
            crate::customers::tenders_for(&g, w, 3).len()
                <= crate::customers::tenders_for(&game(), w, 3).len()
        );
        s.schedule = Default::default();
        g.close_day(&mut s, w);
        assert!(!g.audit_pending);
        let s = g.morning(w);
        assert_eq!(s.clock.minute, SHIFT_START);
    }

    #[test]
    fn a_suspended_driver_parks_when_the_load_delivers() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = arriving_at(&mut g, &mut s, 12 * 60);
        g.trips[0].dock_hold = false;
        s.schedule = Default::default();
        g.driver_mut(id).unwrap().record.suspended_until = g.day + 60;
        g.advance(&mut s, w);
        let d = g.driver(id).unwrap();
        assert_eq!(d.rest_note.as_deref(), Some("a CDL suspension"));
        assert_eq!(d.available_at, abs_min(g.day + 60, SHIFT_START));
        assert_eq!(d.status, Status::OffDuty);
    }

    #[test]
    fn an_older_save_gets_traits_and_a_medical_card_on_load() {
        let w = get_world();
        let mut g = game();
        let json = serde_json::to_value(&g.drivers[0]).unwrap();
        let mut map = json.as_object().unwrap().clone();
        for k in ["traits", "known", "record", "hired_day", "leaving_day"] {
            map.remove(k);
        }
        g.drivers[0] = serde_json::from_value(serde_json::Value::Object(map)).unwrap();
        assert!(g.drivers[0].traits.is_none());
        g.normalize(w);
        let d = &g.drivers[0];
        assert!(d.traits.is_some());
        assert_eq!(d.record.medical_card_day, g.day);
        assert!(d.traits_text().starts_with("Nothing learned yet"));
        let mut d = d.clone();
        let line = d.reveal("pace").unwrap();
        assert!(
            line.starts_with(&format!("You've learned that {} ", d.name)),
            "{line}"
        );
        assert_eq!(d.reveal("pace"), None);
        assert!(
            d.traits_text().starts_with(&format!("{} runs", d.name))
                || d.traits_text().starts_with(&format!("{} is", d.name))
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
        g.normalize(get_world());
        assert_eq!(g.drivers[0].location, g.terminal);
        assert_eq!(g.drivers[0].last_home_day, g.day);
    }

    #[test]
    fn receivables_pay_on_their_day_drivers_on_friday_and_bills_on_the_first() {
        use crate::money::*;
        let mut g = game();
        g.factoring = false;
        g.receivables.push(Receivable {
            amount: 1000.0,
            due_day: 2,
            from: "Acme".into(),
        });
        g.receivables.push(Receivable {
            amount: 500.0,
            due_day: 40,
            from: "Acme".into(),
        });
        let funds = g.funds;
        let news = g.money_overnight();
        assert_eq!(g.funds, funds + 1000.0);
        assert_eq!(g.receivables.len(), 1);
        assert!(
            news[0].starts_with("Payments in: $1,000 from Acme."),
            "{news:?}"
        );

        g.day = 5; // Friday
        g.settlement_owed = 700.0;
        let funds = g.funds;
        let news = g.money_overnight();
        assert_eq!(g.funds, funds - 700.0);
        assert_eq!(g.settlement_owed, 0.0);
        assert!(news[0].starts_with("Friday settlement: $700"), "{news:?}");

        g.day = 30; // tomorrow is the first
        let notes: f64 = g.tractors.iter().map(|t| t.note).sum();
        assert!(notes > 0.0);
        let months_before: Vec<u32> = g.tractors.iter().map(|t| t.note_months_left).collect();
        let funds = g.funds;
        let news = g.money_overnight();
        let insurance = insurance(g.tractors.len(), 0.0);
        assert_eq!(g.funds, funds - notes - insurance - TERMINAL_RENT);
        assert!(news[0].starts_with("The first of the month:"), "{news:?}");
        for (t, before) in g.tractors.iter().zip(months_before) {
            assert_eq!(t.note_months_left, before.saturating_sub(1));
        }

        g.funds = -1000.0;
        g.day = 60;
        g.receivables.clear();
        let news = g.money_overnight();
        assert!(
            news[0].contains("$15 interest on the line of credit"),
            "{news:?}"
        );
    }

    #[test]
    fn factoring_bills_the_load_the_next_day_less_the_fee() {
        let w = get_world();
        let mut g = game();
        assert!(g.factoring, "a new authority factors");
        for d in &mut g.drivers {
            d.home = Some(crate::roster::HomePattern::Regional);
        }
        let mut s = g.morning(w);
        s.schedule = Default::default();
        arriving_at(&mut g, &mut s, 19 * 60);
        let gross = {
            let trip = &g.trips[0];
            trip.job
                .payout((trip.eta() - trip.depart) as f64 / 60.0, 0.0, 0.0)
        };
        let billed = (gross * (1.0 - crate::money::FACTORING_FEE)).round();
        let funds = g.funds;
        g.close_day(&mut s, w);
        assert!(
            g.receivables.is_empty(),
            "paid overnight: {:?}",
            g.receivables
        );
        assert!(
            g.news.iter().any(|l| l.contains("due tomorrow")),
            "{:?}",
            g.news
        );
        let paid = g
            .news
            .iter()
            .find(|l| l.starts_with("Payments in: "))
            .unwrap();
        assert!(
            paid.starts_with(&format!("Payments in: {} from ", crate::money(billed))),
            "{paid}"
        );
        let costs = funds - g.funds + billed;
        assert!(costs > 0.0, "fuel, road and overhead came out: {costs}");
        assert!(g.toggle_factoring().starts_with("Factoring off"));
        assert!(!g.factoring);
    }

    #[test]
    fn trucks_go_to_the_shop_get_bought_and_hiring_needs_a_spare() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        assert_eq!(
            g.tractors.len(),
            g.drivers.len() + 1,
            "one each and a spare"
        );
        let spare = g.spare_tractors()[0];
        let funds = g.funds;
        let line = g.service_truck(&mut s, spare).unwrap();
        assert!(
            line.starts_with(&format!(
                "Truck {spare} is in the shop for the rest of today"
            )),
            "{line}"
        );
        assert_eq!(g.funds, funds - crate::money::SERVICE_COST);
        assert_eq!(s.clock.minute, SHIFT_START + Action::CheckTruck.cost_min());
        assert_eq!(
            g.service_truck(&mut s, spare),
            Err(ActError::Refused(format!(
                "Truck {spare} is in the shop already."
            )))
        );
        let truck = g.tractors.iter().find(|t| t.id == spare).unwrap();
        assert!(truck.in_shop(g.day) && !truck.in_shop(g.day + 1));

        // A driver whose truck is in the shop can't take a load.
        let (load, driver) = legal_pair(&g, &s);
        let own = g.tractor_of(driver).unwrap().id;
        g.tractors
            .iter_mut()
            .find(|t| t.id == own)
            .unwrap()
            .in_shop_until = g.day + 1;
        let name = g.driver(driver).unwrap().name.clone();
        assert_eq!(
            g.assign(&mut s, load, driver, w),
            Err(ActError::Refused(format!(
                "{name}'s truck is in the shop today."
            )))
        );

        // Hiring takes the spare; the next hire needs a new truck.
        g.hire(&mut s, 0).unwrap();
        assert!(g.spare_tractors().is_empty());
        assert_eq!(
            g.hire(&mut s, 0),
            Err(ActError::Refused(
                "There's no truck for them. Buy one on the Money screen first.".into()
            ))
        );
        let funds = g.funds;
        let line = g.buy_truck(&mut s).unwrap();
        assert!(line.contains("$8,000 down and $1,400 a month"), "{line}");
        assert_eq!(g.funds, funds - crate::money::TRUCK_DOWN);
        assert_eq!(g.spare_tractors().len(), 1);
        let t = g.tractors.last().unwrap();
        assert_eq!(t.note_months_left, crate::money::TRUCK_NOTE_MONTHS);
        assert!(
            t.text(g.day, &g).contains("60 payments left"),
            "{}",
            t.text(g.day, &g)
        );
        g.funds = 100.0;
        assert_eq!(g.buy_truck(&mut s), Err(ActError::NoFunds));
    }

    #[test]
    fn a_month_over_the_line_or_a_week_at_shutdown_ends_the_career() {
        use crate::money::*;
        let mut g = game();
        g.funds = -CREDIT_LINE - 1.0;
        g.days_over_line = OVER_LINE_DAYS - WARNING_DAYS - 1;
        let news = g.money_overnight();
        assert!(
            news.iter().any(|l| l.starts_with("The bank called")),
            "{news:?}"
        );
        assert!(g.ended.is_none());
        g.days_over_line = OVER_LINE_DAYS - 1;
        let news = g.money_overnight();
        assert_eq!(g.ended, Some(Ending::OverTheLine));
        assert!(
            news.last()
                .unwrap()
                .starts_with("The career is over: the bank called"),
            "{news:?}"
        );
        g.funds = 0.0;
        g.ended = None;
        g.money_overnight();
        assert_eq!(g.days_over_line, 0);

        let mut g = game();
        g.safety
            .add(g.day, crate::law::Category::UnsafeDriving, SHUTDOWN_SCORE);
        let news = g.money_overnight();
        assert!(news.iter().any(|l| l.contains("gives a week")), "{news:?}");
        g.shutdown_days = WARNING_DAYS;
        g.money_overnight();
        assert_eq!(g.ended, Some(Ending::Shutdown));

        let mut g = game();
        g.drivers.clear();
        g.money_overnight();
        assert_eq!(g.ended, Some(Ending::NoDrivers));
    }

    #[test]
    fn a_good_customer_asks_about_dedicated_on_monday_once_the_carrier_is_established() {
        use crate::growth::*;
        let w = get_world();
        let mut g = game();
        assert_eq!(dedicated_offer(&g), None, "a new authority gets no offer");
        g.reputation = TIER_FLOORS[2];
        g.customers[1].delivered_total = DEDICATED_AFTER_LOADS;
        g.customers[1].on_time_total = DEDICATED_AFTER_LOADS;
        assert_eq!(dedicated_offer(&g), None, "four drivers first");
        let mut extra = g.drivers[0].clone();
        extra.id = g.take_id();
        g.drivers.push(extra);
        let cid = g.customers[1].id;
        assert_eq!(dedicated_offer(&g), Some(cid));
        let s = g.morning(w);
        let call = s
            .schedule
            .iter()
            .find(|(_, r)| matches!(r, Ring::Call(crate::phone::Call::Dedicated { .. })))
            .map(|(at, _)| *at);
        assert_eq!(call, Some(abs_min(1, SHIFT_START + 30)), "Monday morning");
        let text = g.dedicated_text(cid);
        assert!(
            text.starts_with(&format!("{} here.", g.customer_name(cid))),
            "{text}"
        );
        let line = g.answer_dedicated(cid, false);
        assert!(line.ends_with("They'll ask again in a month."), "{line}");
        assert_eq!(dedicated_offer(&g), None, "not again this month");
        let rate = g.customers[1].rate_mult;
        let line = g.answer_dedicated(cid, true);
        assert!(line.contains("is a dedicated account"), "{line}");
        let c = &g.customers[1];
        assert!(c.dedicated);
        assert_eq!(c.weekly_loads, DEDICATED_WEEKLY_LOADS);
        assert_eq!(c.rate_mult, rate + DEDICATED_RATE_BONUS);
        let mut s = g.morning(w);
        let before = g.reputation;
        s.board.retain(
            |l| !matches!(&l.source, Source::Contract { customer_id } if *customer_id == cid),
        );
        let load = Load::contract(
            &crate::customers::tenders_for(&g, w, g.day + 1)
                .into_iter()
                .find(|t| t.customer_id == cid)
                .expect("a dedicated tender every working day"),
        );
        s.board.insert(0, load);
        s.schedule = Default::default();
        g.close_day(&mut s, w);
        let dropped = g.customers.iter().filter(|c| c.dedicated).count();
        assert_eq!(dropped, 1);
        assert!(
            (before - g.reputation - 2.0 * crate::customers::UNCOVERED_REPUTATION).abs() < 1.5,
            "an uncovered dedicated load costs double: {before} to {}",
            g.reputation
        );
    }

    #[test]
    fn a_dedicated_account_pulls_its_freight_after_two_weeks_under() {
        let mut g = game();
        let cid = g.customers[0].id;
        g.answer_dedicated(cid, true);
        for week in 1..=2 {
            g.day = 7 * week;
            let c = g.customer_mut(cid).unwrap();
            c.week.tendered = 10;
            c.week.accepted = 10;
            c.week.delivered = 10;
            c.week.on_time = 6;
            let lines = crate::customers::scorecard(&mut g);
            let line = lines.iter().find(|l| l.contains(", dedicated:")).unwrap();
            if week == 1 {
                assert!(
                    line.contains("another week like it and they pull it"),
                    "{line}"
                );
            } else {
                assert!(line.contains("they pulled the dedicated freight"), "{line}");
            }
        }
        let c = g.customer(cid).unwrap();
        assert!(!c.dedicated);
        assert_eq!(c.weekly_loads, 4);
        assert!(crate::customers::scorecard(&mut g)
            .last()
            .unwrap()
            .starts_with("Career score "));
    }

    #[test]
    fn the_second_terminal_takes_standing_and_opens_lanes_and_a_home() {
        use crate::growth::*;
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let choices = terminal_choices(&g, w);
        assert!(
            !choices.is_empty() && choices.len() <= TERMINAL_CHOICES,
            "{choices:?}"
        );
        assert!(choices
            .iter()
            .all(|(c, m)| *c != g.terminal && *m >= TERMINAL_MIN_MILES));
        let city = choices[0].0.clone();
        assert!(matches!(
            g.open_terminal(&mut s, &city, w),
            Err(ActError::Refused(_))
        ));
        g.reputation = TIER_FLOORS[2];
        assert!(matches!(
            g.open_terminal(&mut s, &city, w),
            Err(ActError::Refused(_))
        ));
        while g.drivers.len() < TERMINAL_MIN_DRIVERS {
            let mut extra = g.drivers[0].clone();
            extra.id = g.take_id();
            g.drivers.push(extra);
        }
        g.funds = TERMINAL_COST - 1.0;
        assert_eq!(g.open_terminal(&mut s, &city, w), Err(ActError::NoFunds));
        g.funds = 30_000.0;
        let customers = g.customers.len();
        let line = g.open_terminal(&mut s, &city, w).unwrap();
        assert!(line.contains("terminal is open: $10,000 down"), "{line}");
        assert_eq!(g.terminal2.as_deref(), Some(city.as_str()));
        assert_eq!(g.customers.len(), customers + 2);
        assert!(g.customers[customers..]
            .iter()
            .all(|c| c.lanes[0].from == city));
        assert_eq!(g.funds, 30_000.0 - TERMINAL_COST);
        assert!(matches!(
            g.open_terminal(&mut s, &city, w),
            Err(ActError::Refused(_))
        ));
        assert_eq!(g.terminals(), vec![g.terminal.clone(), city.clone()]);
        let s2 = g.morning(w);
        assert!(
            s2.board.iter().any(|l| l.job.origin == city),
            "the new terminal's board"
        );
        assert_eq!(nearest_terminal(&city, &g.terminals(), w), city);
        assert_eq!(nearest_terminal("", &g.terminals(), w), g.terminal);
        g.day = 30;
        g.funds = 0.0;
        let news = g.money_overnight();
        assert!(news[0].contains("$8,000 terminal rent"), "{news:?}");
        assert!(g
            .company_lines(w)
            .iter()
            .any(|l| l.starts_with("Terminals: ")));
    }

    #[test]
    fn milestones_are_news_at_the_close_and_count_once() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        g.record.delivered = 100;
        g.close_day(&mut s, w);
        let m: Vec<&String> = g
            .news
            .iter()
            .filter(|l| l.starts_with("Milestone: "))
            .collect();
        assert_eq!(
            m,
            vec!["Milestone: 100 loads delivered since the authority opened."]
        );
        assert_eq!(g.milestones.len(), 1);
        let mut s = g.morning(w);
        s.schedule = Default::default();
        g.close_day(&mut s, w);
        assert!(
            !g.news.iter().any(|l| l.starts_with("Milestone: ")),
            "{:?}",
            g.news
        );
        assert_eq!(g.milestones.len(), 1);
        let score = crate::growth::score(&g);
        assert_eq!(
            score,
            g.record.delivered + g.record.on_time + 30 + 50 + (g.reputation * 5.0).round() as u32
        );
        let lines = g.company_lines(w);
        assert!(lines[0].starts_with("A new authority at "), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("100 loads delivered")),
            "{lines:?}"
        );
    }
}
