//! Who owns the freight: contract customers with tenders and a weekly
//! scorecard, and the brokers behind the spot board.

use ff_core::data::world::World;
use ff_core::models::jobs::{required_hours, Job, JobBoard, OfferOptions};
use serde::{Deserialize, Serialize};

use crate::board::{lane_pay, route_for};
use crate::career::Game;
use crate::desk::{abs_min, abs_text, SHIFT_START};
use crate::roster::{weekday, Driver};

pub const START_CUSTOMERS: usize = 3;
pub const MAX_CUSTOMERS: usize = 6;
pub const MAX_WEEKLY_LOADS: u32 = 10;
pub const MAX_RATE_MULT: f64 = 1.2;
/// Acceptance and on-time at or over this hold the account.
pub const HOLD_PCT: f64 = 90.0;
/// Acceptance under this is a bad week.
pub const CUT_PCT: f64 = 80.0;
pub const CUT_AFTER_WEEKS: u32 = 2;
pub const LOSE_AFTER_WEEKS: u32 = 4;
pub const GOOD_WEEKS_FOR_GROWTH: u32 = 2;
pub const RATE_STEP: f64 = 0.02;
/// A counter asks this much over the posted rate.
pub const COUNTER_ASK: f64 = 1.10;
/// A refused counter gives the load to another carrier this often.
pub const COUNTER_PULL_ODDS: f64 = 0.25;
/// Tomorrow's tenders ring at 5 PM.
pub const TENDER_MINUTE: u32 = 17 * 60;
/// An uncovered contract load costs this much reputation.
pub const UNCOVERED_REPUTATION: f64 = 1.0;

pub const CUSTOMER_NAMES: &[&str] = &[
    "Lakeshore Foods",
    "Prairie Steel",
    "Northline Paper",
    "Summit Beverage",
    "Redwood Building Supply",
    "Great Plains Grain",
    "Harbor Packaging",
    "Tri-State Auto Parts",
    "Meridian Plastics",
    "Blue Ridge Furniture",
];

pub const BROKERS: &[&str] = &[
    "Apex Logistics",
    "Coastline Freight",
    "Ironwood Brokerage",
    "Keystone Freight Partners",
    "Silverline Logistics",
    "Westgate Transport",
];

/// Where a load came from, and so who remembers how it went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Source {
    Spot { broker: String },
    Contract { customer_id: u32 },
}

impl Default for Source {
    fn default() -> Self {
        Source::Spot {
            broker: "a broker".into(),
        }
    }
}

impl Source {
    pub fn is_spot(&self) -> bool {
        matches!(self, Source::Spot { .. })
    }
}

/// One customer's week, reset on the Sunday scorecard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeekRecord {
    pub tendered: u32,
    pub accepted: u32,
    pub delivered: u32,
    pub on_time: u32,
    /// Accepted and never covered, or the pickup was canceled.
    pub failed: u32,
}

impl WeekRecord {
    pub fn accept_pct(&self) -> f64 {
        if self.tendered == 0 {
            100.0
        } else {
            (self.accepted as f64 * 100.0 / self.tendered as f64).round()
        }
    }

    pub fn on_time_pct(&self) -> f64 {
        let n = self.delivered + self.failed;
        if n == 0 {
            100.0
        } else {
            (self.on_time as f64 * 100.0 / n as f64).round()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lane {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Customer {
    pub id: u32,
    pub name: String,
    pub lanes: Vec<Lane>,
    pub weekly_loads: u32,
    /// The contract rate against the board's lane pay.
    pub rate_mult: f64,
    pub week: WeekRecord,
    pub weeks_under: u32,
    pub good_weeks: u32,
}

impl Default for Customer {
    fn default() -> Self {
        Customer {
            id: 0,
            name: String::new(),
            lanes: Vec::new(),
            weekly_loads: 2,
            rate_mult: 1.0,
            week: WeekRecord::default(),
            weeks_under: 0,
            good_weeks: 0,
        }
    }
}

/// A customer's load offered for a day, with its delivery appointment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tender {
    pub customer_id: u32,
    #[serde(with = "crate::trip::job_json")]
    pub job: Job,
    pub for_day: i64,
    /// Absolute game minute the receiver expects the truck.
    pub appointment: i64,
}

/// The carrier's week, reset on the Sunday scorecard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WeekTotals {
    pub delivered: u32,
    pub on_time: u32,
    pub gross: f64,
}

fn roster_credentials(drivers: &[Driver]) -> (Vec<String>, i64) {
    let mut endorsements: Vec<String> = drivers.iter().flat_map(Driver::endorsements).collect();
    endorsements.sort();
    endorsements.dedup();
    let level = drivers.iter().map(Driver::level).max().unwrap_or(1);
    (endorsements, level)
}

/// Cities the terminal's board reaches, in a seeded order, with no repeats.
fn lane_cities(
    world: &World,
    terminal: &str,
    carrier_key: &str,
    seed: i64,
    drivers: &[Driver],
) -> Vec<String> {
    let (endorsements, level) = roster_credentials(drivers);
    let mut out: Vec<String> = Vec::new();
    for job in JobBoard::seeded(world, seed).offers(
        terminal,
        &endorsements,
        OfferOptions {
            count: 16,
            level,
            market: None,
            carrier_key: Some(carrier_key),
            direct_freight: false,
        },
    ) {
        if !out.contains(&job.destination) {
            out.push(job.destination);
        }
    }
    out
}

/// The three accounts a new career opens with, or an older save gets.
pub fn seeded_customers(
    world: &World,
    terminal: &str,
    carrier_key: &str,
    seed: i64,
    drivers: &[Driver],
    first_id: u32,
) -> Vec<Customer> {
    let mut rng = crate::rng(seed, 0, "customers");
    let cities = lane_cities(
        world,
        terminal,
        carrier_key,
        crate::seed_for(seed, 0, "customers"),
        drivers,
    );
    if cities.is_empty() {
        return Vec::new();
    }
    let mut names: Vec<&str> = CUSTOMER_NAMES.to_vec();
    (0..START_CUSTOMERS)
        .map(|i| {
            let name = names.remove(rng.randint(0, names.len() as i64 - 1) as usize);
            let out = cities[(2 * i) % cities.len()].clone();
            let back = cities[(2 * i + 1) % cities.len()].clone();
            Customer {
                id: first_id + i as u32,
                name: name.to_string(),
                lanes: vec![
                    Lane {
                        from: terminal.to_string(),
                        to: out,
                    },
                    Lane {
                        from: back,
                        to: terminal.to_string(),
                    },
                ],
                weekly_loads: rng.randint(2, 4) as u32,
                rate_mult: (rng.uniform(0.95, 1.05) * 100.0).round() / 100.0,
                ..Customer::default()
            }
        })
        .collect()
}

/// A customer won on a sales call: one lane, two loads a week.
pub fn new_customer(game: &Game, world: &World, id: u32) -> Option<Customer> {
    let mut rng = crate::rng(game.seed, game.day, "sales");
    let taken: Vec<&str> = game.customers.iter().map(|c| c.name.as_str()).collect();
    let names: Vec<&str> = CUSTOMER_NAMES
        .iter()
        .copied()
        .filter(|n| !taken.contains(n))
        .collect();
    if names.is_empty() {
        return None;
    }
    let name = names[rng.randint(0, names.len() as i64 - 1) as usize];
    let cities = lane_cities(
        world,
        &game.terminal,
        &game.carrier_key,
        crate::seed_for(game.seed, game.day, "sales"),
        &game.drivers,
    );
    let used: Vec<&str> = game
        .customers
        .iter()
        .flat_map(|c| c.lanes.iter())
        .map(|l| l.to.as_str())
        .collect();
    let to = cities
        .iter()
        .find(|c| !used.contains(&c.as_str()))
        .or(cities.first())?
        .clone();
    Some(Customer {
        id,
        name: name.to_string(),
        lanes: vec![Lane {
            from: game.terminal.clone(),
            to,
        }],
        weekly_loads: 2,
        rate_mult: 1.0,
        ..Customer::default()
    })
}

/// The day tenders arrive for after `day`: tomorrow on Monday to Thursday,
/// Monday after Friday, and none over the weekend.
pub fn next_tender_day(day: i64) -> Option<i64> {
    match weekday(day) {
        0..=3 => Some(day + 1),
        4 => Some(day + 3),
        _ => None,
    }
}

/// Every customer's loads for `day`: a share of the weekly volume, on the
/// customer's lanes in turn, each with a delivery appointment. Replays for
/// the same career day.
pub fn tenders_for(game: &Game, world: &World, day: i64) -> Vec<Tender> {
    if weekday(day) >= 5 {
        return Vec::new();
    }
    let mut rng = crate::rng(game.seed, day, "tenders");
    let (endorsements, level) = roster_credentials(&game.drivers);
    let morning = abs_min(day, SHIFT_START);
    // A carrier over the audit line makes customers nervous: a quarter of
    // the freight goes elsewhere.
    let nervous = game.safety.over_threshold(game.day);
    let mut out = Vec::new();
    for c in &game.customers {
        if c.lanes.is_empty() {
            continue;
        }
        let extra = rng.random() < (c.weekly_loads % 5) as f64 / 5.0;
        let count = c.weekly_loads / 5 + u32::from(extra);
        for k in 0..count {
            if nervous && rng.random() < 0.25 {
                continue;
            }
            let lane = &c.lanes[(k as i64 + day).rem_euclid(c.lanes.len() as i64) as usize];
            let seed = crate::seed_for(game.seed, day, &format!("tender:{}:{k}", c.id));
            let Some(mut job) = JobBoard::seeded(world, seed).offer_to(
                &lane.from,
                &lane.to,
                &endorsements,
                OfferOptions {
                    count: 1,
                    level,
                    market: Some(&game.market),
                    carrier_key: Some(&game.carrier_key),
                    direct_freight: false,
                },
            ) else {
                continue;
            };
            job.pay = (lane_pay(job.pay, job.distance_mi) * c.rate_mult).round();
            let need_h = required_hours(
                job.distance_mi,
                route_for(world, &job).as_ref(),
                Some(world),
                None,
            );
            let slack_h = rng.uniform(3.0, 9.0);
            let hours = (need_h + slack_h).ceil();
            let appointment = morning + (hours * 60.0) as i64;
            job.deadline_game_h = hours;
            out.push(Tender {
                customer_id: c.id,
                job,
                for_day: day,
                appointment,
            });
        }
    }
    out
}

/// "Lakeshore Foods: Steel coils, Chicago to Denver, 1,003 miles, pays
/// $2,450, deliver by 2 PM tomorrow."
pub fn tender_line(t: &Tender, game: &Game, world: &World) -> String {
    format!(
        "{}: {}, {} to {}, {} miles, pays {}, deliver by {}.",
        game.customer_name(t.customer_id),
        t.job.cargo.label,
        world.spoken_city(&t.job.origin, None),
        world.spoken_city(&t.job.destination, None),
        crate::money(t.job.distance_mi).trim_start_matches('$'),
        crate::money(t.job.pay),
        abs_text(t.appointment, game.day),
    )
}

/// The broker behind a spot load, the same one every time the day replays.
pub fn spot_broker(seed: i64, day: i64, index: usize) -> String {
    let mut rng = crate::rng(seed, day, &format!("broker:{index}"));
    BROKERS[rng.randint(0, BROKERS.len() as i64 - 1) as usize].to_string()
}

/// Odds a broker takes a 10 percent counter: reputation and their trust.
pub fn counter_odds(reputation: f64, trust: i32) -> f64 {
    (0.35 + (reputation - 50.0) / 200.0 + trust as f64 * 0.05).clamp(0.1, 0.9)
}

/// Odds a sales call lands a new customer.
pub fn sales_odds(reputation: f64) -> f64 {
    (0.1 + (reputation - 40.0) / 100.0).clamp(0.05, 0.6)
}

/// The week's scorecard, spoken at Sunday's close: the carrier's week,
/// then each customer's acceptance and on-time record and what it does to
/// the account. Resets the week.
pub fn scorecard(game: &mut Game) -> Vec<String> {
    let week = (game.day - 1) / 7 + 1;
    let totals = std::mem::take(&mut game.week);
    let on_time = if totals.delivered == 0 {
        100.0
    } else {
        (totals.on_time as f64 * 100.0 / totals.delivered as f64).round()
    };
    let mut lines = vec![format!(
        "Week {week} scorecard. {} delivered, {on_time} percent on time, {} gross. Reputation {}.",
        totals.delivered,
        crate::money(totals.gross),
        game.reputation.round()
    )];
    let mut lost = Vec::new();
    for c in &mut game.customers {
        let w = std::mem::take(&mut c.week);
        if w.tendered == 0 && w.delivered == 0 && w.failed == 0 {
            lines.push(format!("{}: no freight this week.", c.name));
            continue;
        }
        let mut line = format!(
            "{}: took {} of {} tenders, {} of {} on time.",
            c.name,
            w.accepted,
            w.tendered,
            w.on_time,
            w.delivered + w.failed
        );
        if w.accept_pct() < CUT_PCT {
            c.weeks_under += 1;
            c.good_weeks = 0;
            if c.weeks_under >= LOSE_AFTER_WEEKS {
                line.push_str(&format!(
                    " {} weeks under {CUT_PCT} percent acceptance: they pulled their freight.",
                    c.weeks_under
                ));
                lost.push(c.id);
                game.reputation -= 2.0;
            } else if c.weeks_under >= CUT_AFTER_WEEKS && c.weekly_loads > 1 {
                c.weekly_loads -= 1;
                game.reputation -= 1.0;
                line.push_str(&format!(
                    " Week {} under {CUT_PCT} percent acceptance: volume cut to {}.",
                    c.weeks_under,
                    loads_a_week(c.weekly_loads)
                ));
            } else {
                line.push_str(" Under 80 percent acceptance. Another week like it cuts volume.");
            }
        } else if w.accept_pct() >= HOLD_PCT && w.on_time_pct() >= HOLD_PCT {
            c.weeks_under = 0;
            c.good_weeks += 1;
            game.reputation += 1.0;
            if c.good_weeks >= GOOD_WEEKS_FOR_GROWTH
                && (c.weekly_loads < MAX_WEEKLY_LOADS || c.rate_mult < MAX_RATE_MULT)
            {
                c.good_weeks = 0;
                c.weekly_loads = (c.weekly_loads + 1).min(MAX_WEEKLY_LOADS);
                c.rate_mult = ((c.rate_mult + RATE_STEP) * 100.0).round() / 100.0;
                c.rate_mult = c.rate_mult.min(MAX_RATE_MULT);
                line.push_str(&format!(
                    " Two good weeks: volume up to {} and 2 percent on the rate.",
                    loads_a_week(c.weekly_loads)
                ));
            } else {
                line.push_str(" A good week. The account holds.");
            }
        } else {
            c.weeks_under = 0;
            c.good_weeks = 0;
            line.push_str(" The account holds, but on time needs work.");
        }
        lines.push(line);
    }
    game.customers.retain(|c| !lost.contains(&c.id));
    lines
}

fn loads_a_week(n: u32) -> String {
    match n {
        1 => "1 load a week".into(),
        n => format!("{n} loads a week"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::tests::game;
    use ff_core::data::world::get_world;

    #[test]
    fn a_new_career_opens_with_three_customers_on_lanes_from_the_terminal() {
        let g = game();
        assert_eq!(g.customers.len(), START_CUSTOMERS);
        let names: Vec<&str> = g.customers.iter().map(|c| c.name.as_str()).collect();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names.len(), unique.len(), "{names:?}");
        for c in &g.customers {
            assert_eq!(c.lanes.len(), 2);
            assert_eq!(c.lanes[0].from, g.terminal);
            assert_eq!(c.lanes[1].to, g.terminal);
            assert!((2..=4).contains(&c.weekly_loads));
        }
        let again = game();
        assert_eq!(g.customers, again.customers, "seeded");
    }

    #[test]
    fn tenders_replay_for_a_day_and_skip_the_weekend() {
        let w = get_world();
        let g = game();
        let monday = tenders_for(&g, w, 1);
        assert!(!monday.is_empty());
        assert_eq!(
            monday.iter().map(|t| t.job.pay).collect::<Vec<_>>(),
            tenders_for(&g, w, 1)
                .iter()
                .map(|t| t.job.pay)
                .collect::<Vec<_>>()
        );
        for t in &monday {
            assert!(t.appointment > abs_min(1, SHIFT_START) + 3 * 60);
            let line = tender_line(t, &g, w);
            assert!(line.contains("deliver by"), "{line}");
            assert!(line.starts_with(&g.customer_name(t.customer_id)), "{line}");
        }
        assert!(tenders_for(&g, w, 6).is_empty(), "Saturday");
        assert!(tenders_for(&g, w, 7).is_empty(), "Sunday");
        assert_eq!(next_tender_day(5), Some(8), "Friday tenders for Monday");
        assert_eq!(next_tender_day(6), None);
    }

    #[test]
    fn a_week_of_tenders_adds_up_to_about_the_weekly_volume() {
        let w = get_world();
        let g = game();
        let volume: u32 = g.customers.iter().map(|c| c.weekly_loads).sum();
        let offered: usize = (1..=7).map(|d| tenders_for(&g, w, d).len()).sum();
        assert!(
            (offered as i64 - volume as i64).abs() <= 3,
            "{offered} offered against {volume} contracted"
        );
    }

    #[test]
    fn the_scorecard_holds_grows_cuts_and_loses_accounts() {
        let mut g = game();
        g.day = 7;
        g.week.delivered = 4;
        g.week.on_time = 4;
        g.week.gross = 8000.0;
        let ids: Vec<u32> = g.customers.iter().map(|c| c.id).collect();
        let good = &mut g.customers[0];
        good.week = WeekRecord {
            tendered: 4,
            accepted: 4,
            delivered: 4,
            on_time: 4,
            failed: 0,
        };
        good.good_weeks = 1;
        let before = good.weekly_loads;
        let bad = &mut g.customers[1];
        bad.week = WeekRecord {
            tendered: 4,
            accepted: 2,
            ..WeekRecord::default()
        };
        bad.weeks_under = 1;
        let gone = &mut g.customers[2];
        gone.week = WeekRecord {
            tendered: 3,
            accepted: 0,
            ..WeekRecord::default()
        };
        gone.weeks_under = 3;
        let rep = g.reputation;
        let lines = scorecard(&mut g);
        assert!(
            lines[0]
                .starts_with("Week 1 scorecard. 4 delivered, 100 percent on time, $8,000 gross."),
            "{lines:?}"
        );
        assert!(
            lines[1].contains("Two good weeks: volume up to"),
            "{lines:?}"
        );
        assert_eq!(g.customers[0].weekly_loads, before + 1);
        assert!(lines[2].contains("volume cut to"), "{lines:?}");
        assert!(lines[3].contains("pulled their freight"), "{lines:?}");
        assert_eq!(g.customers.len(), 2);
        assert!(!g.customers.iter().any(|c| c.id == ids[2]));
        assert!(g.customers.iter().all(|c| c.week == WeekRecord::default()));
        assert_eq!(g.week, WeekTotals::default());
        assert_eq!(g.reputation, rep + 1.0 - 1.0 - 2.0);
    }

    #[test]
    fn counters_land_more_often_with_reputation_and_trust() {
        assert!(counter_odds(50.0, 0) < counter_odds(70.0, 0));
        assert!(counter_odds(50.0, 0) < counter_odds(50.0, 3));
        assert!(counter_odds(0.0, -20) >= 0.1 && counter_odds(100.0, 20) <= 0.9);
        assert_eq!(spot_broker(42, 1, 0), spot_broker(42, 1, 0));
    }
}
