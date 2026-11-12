//! Where a career goes: reputation tiers that open doors, dedicated
//! accounts, a second terminal, milestones, and a score for the weeks to
//! add up to.

use serde::{Deserialize, Serialize};

use crate::career::Game;

/// Reputation at which each tier starts. A career starts at 50.
pub const TIER_FLOORS: [f64; 4] = [0.0, 60.0, 75.0, 90.0];
/// A customer this deep into a good run asks about a dedicated account.
pub const DEDICATED_AFTER_LOADS: u32 = 30;
/// Weeks between asking again after a no.
pub const DEDICATED_ASK_AGAIN_WEEKS: i64 = 4;
pub const DEDICATED_WEEKLY_LOADS: u32 = 10;
pub const DEDICATED_RATE_BONUS: f64 = 0.05;
/// Drivers on the roster before a dedicated account can be taken.
pub const DEDICATED_MIN_DRIVERS: usize = 4;
/// A dedicated account drops back to ordinary after this many weeks under.
pub const DEDICATED_LOSE_WEEKS: u32 = 2;
/// Opening a second terminal: the cost, and what it takes.
pub const TERMINAL_COST: f64 = 10_000.0;
pub const TERMINAL_MIN_DRIVERS: usize = 5;
pub const TERMINAL_MIN_MILES: f64 = 250.0;
pub const TERMINAL_MAX_MILES: f64 = 900.0;
pub const TERMINAL_CHOICES: usize = 4;
/// A safety score over the audit line costs this much reputation a week.
pub const SAFETY_REPUTATION: f64 = 1.0;
pub const MILESTONE_SCORE: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    New,
    Known,
    Established,
    Preferred,
}

impl Tier {
    pub fn of(reputation: f64) -> Tier {
        if reputation >= TIER_FLOORS[3] {
            Tier::Preferred
        } else if reputation >= TIER_FLOORS[2] {
            Tier::Established
        } else if reputation >= TIER_FLOORS[1] {
            Tier::Known
        } else {
            Tier::New
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tier::New => "a new authority",
            Tier::Known => "a known carrier",
            Tier::Established => "an established carrier",
            Tier::Preferred => "a preferred carrier",
        }
    }

    pub fn next(self) -> Option<(Tier, f64)> {
        match self {
            Tier::New => Some((Tier::Known, TIER_FLOORS[1])),
            Tier::Known => Some((Tier::Established, TIER_FLOORS[2])),
            Tier::Established => Some((Tier::Preferred, TIER_FLOORS[3])),
            Tier::Preferred => None,
        }
    }

    /// Spot loads the brokers add to the board each day.
    pub fn extra_loads(self) -> usize {
        match self {
            Tier::New => 0,
            Tier::Known => 1,
            Tier::Established => 2,
            Tier::Preferred => 3,
        }
    }

    /// Accounts the desk can serve.
    pub fn max_customers(self) -> usize {
        crate::customers::MAX_CUSTOMERS + self.extra_loads()
    }

    /// Loads a week a customer won on a sales call ships.
    pub fn new_customer_loads(self) -> u32 {
        2 + self.extra_loads() as u32
    }

    /// What this tier opens, for the Company screen.
    pub fn opens(self) -> &'static str {
        match self {
            Tier::New => "Brokers take your calls. Three accounts and the spot board.",
            Tier::Known => "One more spot load a day, a seventh account, and bigger shippers answer the sales call.",
            Tier::Established => "Two more spot loads a day, dedicated accounts, and a second terminal.",
            Tier::Preferred => "Three more spot loads a day, nine accounts, and the biggest shippers.",
        }
    }
}

/// "An established carrier at 78 reputation, 12 points from preferred."
pub fn tier_line(reputation: f64) -> String {
    let tier = Tier::of(reputation);
    let mut s = format!(
        "{} at {} reputation",
        capitalize(tier.name()),
        reputation.round()
    );
    match tier.next() {
        Some((next, floor)) => s.push_str(&format!(
            ", {} points from {}.",
            (floor - reputation).ceil().max(1.0),
            next.name()
                .trim_start_matches("a ")
                .trim_start_matches("an ")
        )),
        None => s.push_str(", the top of the trade."),
    }
    s
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Milestone {
    pub day: i64,
    pub text: String,
}

/// Milestones not yet earned that are, now.
pub fn new_milestones(game: &Game) -> Vec<String> {
    let have = |key: &str| game.milestones.iter().any(|m| m.text.starts_with(key));
    let mut out = Vec::new();
    let mut add = |key: &str, text: String| {
        if !have(key) {
            out.push(text);
        }
    };
    for n in [100u32, 500, 1000] {
        if game.record.delivered >= n {
            add(
                &format!("{n} loads delivered"),
                format!("{n} loads delivered since the authority opened"),
            );
        }
    }
    if let Some(c) = game.customers.iter().find(|c| c.delivered_total >= 100) {
        add(
            "100 loads with one customer",
            format!("100 loads with one customer: {}", c.name),
        );
    }
    if let Some(c) = game.customers.iter().find(|c| c.dedicated) {
        add(
            "First dedicated account",
            format!("First dedicated account: {}", c.name),
        );
    }
    if let Some(d) = game
        .drivers
        .iter()
        .find(|d| game.day - d.hired_day.max(1) >= 365)
    {
        add(
            "A driver with a year in",
            format!("A driver with a year in: {}", d.name),
        );
    }
    if game.tractors.iter().any(|t| t.paid_off) {
        add("First truck paid off", "First truck paid off".into());
    }
    if game.terminal2.is_some() {
        add("Second terminal", "Second terminal opened".into());
    }
    if Tier::of(game.reputation) == Tier::Preferred {
        add("Preferred carrier", "Preferred carrier".into());
    }
    if game.customers.len() >= 6 {
        add("Six accounts", "Six accounts on the books".into());
    }
    if game.drivers.len() >= crate::hiring::ROSTER_CAP {
        add("A full roster", "A full roster of ten drivers".into());
    }
    out
}

/// The career score: every delivery, on-time deliveries again, accounts,
/// milestones and reputation.
pub fn score(game: &Game) -> u32 {
    game.record.delivered
        + game.record.on_time
        + game.customers.len() as u32 * 10
        + game.milestones.len() as u32 * MILESTONE_SCORE
        + (game.reputation.max(0.0) * 5.0).round() as u32
}

/// Whether a customer is ready to ask about a dedicated account this week.
pub fn dedicated_offer(game: &Game) -> Option<u32> {
    if Tier::of(game.reputation) < Tier::Established || game.drivers.len() < DEDICATED_MIN_DRIVERS {
        return None;
    }
    let week = (game.day - 1) / 7;
    game.customers
        .iter()
        .find(|c| {
            !c.dedicated
                && c.delivered_total >= DEDICATED_AFTER_LOADS
                && c.weeks_under == 0
                && (c.dedicated_asked_week == 0
                    || week - c.dedicated_asked_week >= DEDICATED_ASK_AGAIN_WEEKS)
        })
        .map(|c| c.id)
}

/// The terminal a driver goes home to: the nearest by road.
pub fn nearest_terminal(
    location: &str,
    terminals: &[String],
    world: &ff_core::data::world::World,
) -> String {
    if location.is_empty() || terminals.len() == 1 {
        return terminals[0].clone();
    }
    terminals
        .iter()
        .min_by(|a, b| {
            let (ma, _) = crate::trip::empty_leg(location, a, world);
            let (mb, _) = crate::trip::empty_leg(location, b, world);
            ma.total_cmp(&mb)
        })
        .cloned()
        .unwrap_or_default()
}

/// Cities a second terminal could open in: the busiest cities 250 to 900
/// air miles from the first, nearest first.
pub fn terminal_choices(game: &Game, world: &ff_core::data::world::World) -> Vec<(String, f64)> {
    let Ok(home) = world.city(&game.terminal) else {
        return Vec::new();
    };
    let mut all: Vec<(String, f64, usize)> = world
        .cities
        .iter()
        .filter(|(key, _)| **key != game.terminal)
        .map(|(key, c)| {
            (
                key.clone(),
                ff_core::data::world::air_miles(home.lat, home.lon, c.lat, c.lon).round(),
                c.locations.len(),
            )
        })
        .filter(|(_, miles, _)| (TERMINAL_MIN_MILES..=TERMINAL_MAX_MILES).contains(miles))
        .collect();
    all.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.total_cmp(&b.1)));
    all.truncate(TERMINAL_CHOICES);
    all.sort_by(|a, b| a.1.total_cmp(&b.1));
    all.into_iter().map(|(c, m, _)| (c, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_follow_reputation_and_say_what_is_next() {
        assert_eq!(Tier::of(50.0), Tier::New);
        assert_eq!(Tier::of(60.0), Tier::Known);
        assert_eq!(Tier::of(89.9), Tier::Established);
        assert_eq!(Tier::of(90.0), Tier::Preferred);
        assert_eq!(
            tier_line(50.0),
            "A new authority at 50 reputation, 10 points from known carrier."
        );
        assert_eq!(
            tier_line(95.0),
            "A preferred carrier at 95 reputation, the top of the trade."
        );
        assert_eq!(Tier::Preferred.max_customers(), 9);
        assert_eq!(Tier::Known.new_customer_loads(), 3);
    }
}
