//! Money the way a carrier feels it: revenue weeks after the work, drivers
//! paid on Friday, bills on the first, fuel at the pump, trucks with notes
//! and miles, a line of credit, and the ways a career ends.

use serde::{Deserialize, Serialize};

use crate::career::Game;
use crate::customers::Source;
use crate::roster::weekday;

/// Days until a customer pays; a few accounts take longer.
pub const PAY_DAYS: [i64; 2] = [30, 45];
/// Brokers pay in 30 days too, unless the carrier factors.
pub const BROKER_PAY_DAYS: i64 = 30;
/// Factoring: everything pays the next day for this share.
pub const FACTORING_FEE: f64 = 0.03;
/// Friday's settlement pays the week's driver pay.
pub const SETTLEMENT_WEEKDAY: i64 = 4;
/// Bills come on the first of each 30-day month.
pub const MONTH_DAYS: i64 = 30;
pub const TERMINAL_RENT: f64 = 4_000.0;
/// Insurance per tractor per month, before the safety score moves it.
pub const INSURANCE_BASE: f64 = 600.0;
/// Each point on the safety score adds this share to insurance.
pub const INSURANCE_PER_POINT: f64 = 0.02;
/// The per-mile costs that aren't fuel, truck notes or insurance: repair
/// and maintenance, permits, tires, tolls (ATRI 2026, 2025 costs).
pub const ROAD_COST_PER_MILE: f64 = 0.316;
/// Parking, ELD and permits per truck per day.
pub const TRUCK_OVERHEAD_PER_DAY: f64 = 40.0;
pub const MPG: f64 = 6.5;
/// The fuel surcharge on contract freight kicks in above this price.
pub const SURCHARGE_BASE_PRICE: f64 = 3.50;
/// Cash can run this far below zero before the month-long clock to the
/// end starts.
pub const CREDIT_LINE: f64 = 40_000.0;
/// Interest on a negative balance, per month.
pub const CREDIT_INTEREST: f64 = 0.015;
/// Days over the line before the career ends, and when the warning comes.
pub const OVER_LINE_DAYS: i64 = 30;
pub const WARNING_DAYS: i64 = 7;
/// A safety score this high for a week is a shutdown order.
pub const SHUTDOWN_SCORE: f64 = 60.0;
pub const SERVICE_INTERVAL_MI: f64 = 15_000.0;
pub const SERVICE_COST: f64 = 600.0;
/// Breakdown odds multiply by this once a truck is past its service.
pub const OVERDUE_BREAKDOWN: f64 = 3.0;
/// A used tractor: the down payment and the note.
pub const TRUCK_DOWN: f64 = 8_000.0;
pub const TRUCK_NOTE: f64 = 1_400.0;
pub const TRUCK_NOTE_MONTHS: u32 = 60;

/// Money owed to the carrier for a delivered load.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Receivable {
    pub amount: f64,
    pub due_day: i64,
    pub from: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tractor {
    pub id: u32,
    /// Model year age, in years, at the career's start.
    pub age: u32,
    pub miles: f64,
    pub miles_at_service: f64,
    /// Monthly note; 0 when owned outright.
    pub note: f64,
    pub note_months_left: u32,
    /// Career day the truck is in the shop until; 0 when it isn't.
    pub in_shop_until: i64,
    /// Who drives it; None for a spare.
    pub driver_id: Option<u32>,
}

impl Default for Tractor {
    fn default() -> Self {
        Tractor {
            id: 0,
            age: 3,
            miles: 0.0,
            miles_at_service: 0.0,
            note: 0.0,
            note_months_left: 0,
            in_shop_until: 0,
            driver_id: None,
        }
    }
}

impl Tractor {
    pub fn since_service(&self) -> f64 {
        self.miles - self.miles_at_service
    }

    pub fn overdue(&self) -> bool {
        self.since_service() > SERVICE_INTERVAL_MI
    }

    pub fn in_shop(&self, today: i64) -> bool {
        self.in_shop_until > today
    }

    /// "Truck 2, 6 years old, 412,000 miles, service due in 3,400 miles,
    /// $1,400 a month with 48 payments left."
    pub fn text(&self, today: i64, game: &Game) -> String {
        let due = SERVICE_INTERVAL_MI - self.since_service();
        let service = if self.in_shop(today) {
            "in the shop today".to_string()
        } else if due <= 0.0 {
            format!(
                "service overdue by {} miles",
                crate::money(-due).trim_start_matches('$')
            )
        } else {
            format!(
                "service due in {} miles",
                crate::money(due).trim_start_matches('$')
            )
        };
        let note = if self.note > 0.0 {
            format!(
                ", {} a month with {} payments left",
                crate::money(self.note),
                self.note_months_left
            )
        } else {
            ", owned".to_string()
        };
        let who = match self.driver_id.and_then(|id| game.driver(id)) {
            Some(d) => format!(", {}'s", d.name),
            None => ", spare".to_string(),
        };
        format!(
            "Truck {}, {} years old, {} miles, {service}{note}{who}.",
            self.id,
            self.age,
            crate::money(self.miles).trim_start_matches('$')
        )
    }
}

/// The starting fleet: one truck per driver plus a spare, one of them old
/// and paid off, the rest on notes.
pub fn starting_fleet(
    rng: &mut ff_core::pyrandom::PyRandom,
    first_id: u32,
    drivers: &[u32],
) -> Vec<Tractor> {
    let mut out = Vec::new();
    for i in 0..=drivers.len() {
        let old = i == 0;
        let miles = if old {
            rng.randint(550_000, 700_000) as f64
        } else {
            rng.randint(60_000, 300_000) as f64
        };
        out.push(Tractor {
            id: first_id + i as u32,
            age: if old { 8 } else { rng.randint(1, 4) as u32 },
            miles,
            miles_at_service: miles - rng.randint(0, 12_000) as f64,
            note: if old { 0.0 } else { TRUCK_NOTE },
            note_months_left: if old { 0 } else { rng.randint(24, 54) as u32 },
            in_shop_until: 0,
            driver_id: drivers.get(i).copied(),
        });
    }
    out
}

/// Diesel this week, by career day: a slow drift around $3.90.
pub fn fuel_price(seed: i64, day: i64) -> f64 {
    let week = (day - 1).div_euclid(7);
    let mut rng = crate::rng(seed, week, "fuel");
    let mut price = 3.90;
    for _ in 0..=week.min(12) {
        price += rng.uniform(-0.12, 0.12);
    }
    (price.clamp(3.20, 4.80) * 100.0).round() / 100.0
}

/// Fuel for a run at this week's price, through the driver's fuel card.
pub fn fuel_cost(miles: f64, price: f64) -> f64 {
    (miles / MPG * price * 100.0).round() / 100.0
}

/// The fuel surcharge a customer pays on a contract load.
pub fn surcharge(miles: f64, price: f64) -> f64 {
    ((price - SURCHARGE_BASE_PRICE).max(0.0) / MPG * miles).round()
}

/// When a delivered load pays.
pub fn pay_day(game: &Game, source: &Source, today: i64) -> i64 {
    if game.factoring {
        return today + 1;
    }
    match source {
        Source::Spot { .. } => today + BROKER_PAY_DAYS,
        Source::Contract { customer_id } => {
            today
                + game
                    .customer(*customer_id)
                    .map(|c| c.pay_days)
                    .unwrap_or(PAY_DAYS[0])
        }
    }
}

/// Why a career ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Ending {
    OverTheLine,
    Shutdown,
    NoDrivers,
}

impl Ending {
    pub fn text(&self) -> &'static str {
        match self {
            Ending::OverTheLine => "the bank called the line of credit after a month over it",
            Ending::Shutdown => "the safety score brought a shutdown order",
            Ending::NoDrivers => "the last driver left",
        }
    }
}

/// Insurance for the month: the base per truck, up with the safety score.
pub fn insurance(trucks: usize, safety_points: f64) -> f64 {
    (INSURANCE_BASE * trucks as f64 * (1.0 + INSURANCE_PER_POINT * safety_points)).round()
}

pub fn is_settlement_day(day: i64) -> bool {
    weekday(day) == SETTLEMENT_WEEKDAY
}

pub fn is_bill_day(day: i64) -> bool {
    day > 1 && (day - 1) % MONTH_DAYS == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuel_drifts_by_week_and_the_surcharge_follows_it() {
        assert_eq!(fuel_price(7, 1), fuel_price(7, 7));
        assert_eq!(fuel_cost(650.0, 4.0), 400.0);
        assert_eq!(surcharge(650.0, 3.50), 0.0);
        assert_eq!(surcharge(650.0, 4.15), 65.0);
        let prices: Vec<f64> = (0..20).map(|w| fuel_price(3, 1 + 7 * w)).collect();
        assert!(prices.iter().all(|p| (3.2..=4.8).contains(p)), "{prices:?}");
    }

    #[test]
    fn bills_come_on_the_first_and_settlements_on_friday() {
        assert!(!is_bill_day(1));
        assert!(is_bill_day(31) && is_bill_day(61) && !is_bill_day(45));
        assert!(is_settlement_day(5) && !is_settlement_day(6));
        assert_eq!(insurance(3, 0.0), 1800.0);
        assert_eq!(insurance(3, 25.0), 2700.0);
    }

    #[test]
    fn a_truck_knows_when_it_is_due() {
        let mut t = Tractor {
            miles: 20_000.0,
            miles_at_service: 10_000.0,
            ..Default::default()
        };
        assert!(!t.overdue());
        t.miles += 5_001.0;
        assert!(t.overdue());
    }
}
