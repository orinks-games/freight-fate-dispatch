//! Seeded bots that play whole days through the same calls a player gets,
//! so balance claims ("chasing miles loses") are measured, not assumed.

use crate::board::refusal;
use crate::career::{Game, Ring, Shift, OPERATING_COST_PER_MILE};
use crate::incidents::{Choice, Kind};
use crate::phone::{Call, CallChoice};
use crate::roster::Status;
use ff_core::data::world::World;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bot {
    /// The longest legal load to each idle driver, every time.
    Greedy,
    /// Net per hour, on time, home when due, detention billed.
    Planner,
}

#[derive(Debug, Default)]
pub struct Metrics {
    pub funds: f64,
    pub deliveries: u32,
    pub on_time: u32,
    pub loaded_mi: f64,
    pub deadhead_mi: f64,
    pub home_missed: u32,
    pub quits: u32,
    /// Per day: driver decisions with two or more sensible loads.
    pub close_calls: Vec<u32>,
    pub call_lines: Vec<String>,
    pub late_lines: Vec<String>,
}

impl Metrics {
    fn scan(&mut self, lines: &[String]) {
        for l in lines {
            if l.contains(" late by ") || l.contains(", late") || l.starts_with("Late") {
                self.late_lines.push(l.clone());
            }
            if l.contains("missed promised home time") {
                self.home_missed += 1;
            }
        }
    }

    pub fn on_time_pct(&self) -> f64 {
        100.0 * self.on_time as f64 / self.deliveries.max(1) as f64
    }

    pub fn deadhead_pct(&self) -> f64 {
        100.0 * self.deadhead_mi / (self.loaded_mi + self.deadhead_mi).max(1.0)
    }

    /// Call lines heard word for word more than once.
    pub fn repeated_calls(&self) -> usize {
        let mut seen = std::collections::HashMap::<&str, u32>::new();
        for l in &self.call_lines {
            *seen.entry(l.as_str()).or_default() += 1;
        }
        seen.values().filter(|&&n| n > 1).count()
    }
}

/// Net dollars per hour of a load for this driver, and whether it can
/// make its appointment; None when the driver can't legally take it.
fn value(
    game: &Game,
    driver_id: u32,
    load: &crate::board::Load,
    world: &World,
) -> Option<(f64, bool)> {
    let d = game.driver(driver_id)?;
    if refusal(&load.job, d, world).is_some() {
        return None;
    }
    let p = crate::trip::plan(&load.job, d, world);
    let miles = load.job.distance_mi + p.deadhead_mi;
    let net = load.job.pay - OPERATING_COST_PER_MILE * miles;
    Some((
        net / p.need_h.max(1.0),
        p.need_h + 1.0 <= load.job.deadline_game_h,
    ))
}

/// Whether a local driver can run this load and still drive home on
/// today's hours.
fn home_tonight(game: &Game, driver_id: u32, load: &crate::board::Load, world: &World) -> bool {
    let Some(d) = game.driver(driver_id) else {
        return false;
    };
    let p = crate::trip::plan(&load.job, d, world);
    let (_, home_min) = crate::trip::empty_leg(&load.job.destination, &game.terminal, world);
    p.need_h * 60.0 + home_min + 60.0 <= d.legal_left_min()
}

fn choose(bot: Bot, game: &Game, shift: &Shift, ring: &Ring, world: &World) -> usize {
    match ring {
        Ring::Call(call) => {
            let want = match (bot, call) {
                (_, Call::QuitWarning { .. }) => CallChoice::PromiseHome,
                (_, Call::Empty { .. }) => CallChoice::FindLoad,
                (_, Call::HotLoad { .. }) => CallChoice::Accept,
                _ => return 0,
            };
            crate::phone::choices(call)
                .iter()
                .position(|&c| c == want)
                .unwrap_or(0)
        }
        Ring::Incident(inc) => {
            let now = shift.now(game.day);
            let choices = crate::incidents::choices(game, inc, now, world);
            // Both bots answer calls the same way; only dispatching differs.
            let want = match &inc.kind {
                Kind::Detention => Some(Choice::BillDetention),
                _ => None,
            };
            want.and_then(|w| choices.iter().position(|&c| c == w))
                .unwrap_or(0)
        }
    }
}

fn dispatch(bot: Bot, game: &mut Game, shift: &mut Shift, world: &World) -> u32 {
    let mut close_calls = 0;
    let idle: Vec<u32> = game
        .drivers
        .iter()
        .filter(|d| d.status == Status::Idle)
        .map(|d| d.id)
        .collect();
    for id in idle {
        let pick = match bot {
            Bot::Greedy => shift
                .board
                .iter()
                .enumerate()
                .filter(|(_, l)| value(game, id, l, world).is_some())
                .max_by(|a, b| a.1.job.distance_mi.total_cmp(&b.1.job.distance_mi))
                .map(|(i, _)| i),
            Bot::Planner => {
                let d = game.driver(id).unwrap();
                let away = d.location != game.terminal;
                // Local drivers work the day and head home in the afternoon.
                let late = shift.clock.minute >= 13 * 60;
                let local = d.pattern() == crate::roster::HomePattern::Local;
                let due = if local {
                    late && away
                } else {
                    game.due_home(d)
                };
                if due && !away {
                    let _ = game.send_home(shift, id, world);
                    continue;
                }
                let scored: Vec<(usize, f64)> = shift
                    .board
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| !due || l.job.destination == game.terminal)
                    .filter(|(_, l)| !local || home_tonight(game, id, l, world))
                    .filter_map(|(i, l)| match value(game, id, l, world) {
                        Some((v, true)) if v > 0.0 => Some((i, v)),
                        _ => None,
                    })
                    .collect();
                let best = scored.iter().map(|s| s.1).fold(0.0, f64::max);
                if scored.iter().filter(|s| s.1 >= best * 0.85).count() >= 2 {
                    close_calls += 1;
                }
                let pick = scored
                    .iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|s| s.0);
                if pick.is_none() && due && away {
                    let _ = game.send_home(shift, id, world);
                    continue;
                }
                pick
            }
        };
        if let Some(i) = pick {
            let _ = game.assign(shift, i, id, world);
        }
    }
    close_calls
}

/// Play `days` days of a new seeded career as `bot`.
pub fn play(bot: Bot, seed: i64, days: u32, world: &World) -> Metrics {
    play_from(bot, Game::new("northstar", seed, world), days, world)
}

/// A starting fleet part-way through its time out: a regional driver who
/// missed last weekend and an over-the-road driver due home today, both
/// already short on goodwill.
pub fn mixed_fleet(seed: i64, world: &World) -> Game {
    let mut game = Game::new("northstar", seed, world);
    let day = game.day;
    if let Some(d) = game.drivers.get_mut(1) {
        d.last_home_day = day - 7;
        d.goodwill = 40;
    }
    if let Some(d) = game.drivers.get_mut(2) {
        d.home = Some(crate::roster::HomePattern::OverTheRoad);
        d.last_home_day = day - crate::roster::HomePattern::OverTheRoad.due_day(0, d.id);
        d.goodwill = 40;
    }
    game
}

/// Play `days` days of `game` as `bot`.
pub fn play_from(bot: Bot, mut game: Game, days: u32, world: &World) -> Metrics {
    let mut m = Metrics::default();
    let fleet = game.drivers.len();
    for _ in 0..days {
        let mut shift = game.morning(world);
        let mut close_calls = 0;
        let mut last = None;
        for _ in 0..400 {
            while let Ok(ring) = game.answer(&mut shift) {
                m.call_lines.push(ring.title(&game, &shift, world));
                let c = choose(bot, &game, &shift, &ring, world);
                let out = game.resolve(&mut shift, &ring, c, world);
                m.scan(&out);
            }
            // Only re-plan when the idle drivers or the board changed.
            let idle: Vec<u32> = game
                .drivers
                .iter()
                .filter(|d| d.status == Status::Idle)
                .map(|d| d.id)
                .collect();
            let now = Some((idle, shift.board.len()));
            if now != last {
                close_calls += dispatch(bot, &mut game, &mut shift, world);
                last = now;
            }
            if shift.clock.closed() {
                break;
            }
            let out = game.advance(&mut shift, world);
            m.scan(&out);
        }
        m.close_calls.push(close_calls);
        let out = game.close_day(&mut shift, world);
        for d in &shift.log {
            m.deliveries += 1;
            m.on_time += d.on_time as u32;
            m.loaded_mi += d.loaded_mi;
            m.deadhead_mi += d.deadhead_mi;
        }
        m.scan(&out);
    }
    m.funds = game.funds;
    m.quits = (fleet - game.drivers.len()) as u32;
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff_core::data::world::get_world;

    fn report(name: &str, m: &Metrics) {
        eprintln!(
            "{name}: funds {:.0}, {} deliveries, {:.0}% on time, {:.0}% deadhead, \
             {} home misses, {} quits, close calls per day {:?}, {} repeated call lines, \
             {} late lines",
            m.funds,
            m.deliveries,
            m.on_time_pct(),
            m.deadhead_pct(),
            m.home_missed,
            m.quits,
            m.close_calls,
            m.repeated_calls(),
            m.late_lines.len()
        );
    }

    #[test]
    fn chasing_miles_loses_to_a_planner_over_twelve_fresh_days() {
        let w = get_world();
        let greedy = play(Bot::Greedy, 7, 12, w);
        let planner = play(Bot::Planner, 7, 12, w);
        report("greedy", &greedy);
        report("planner", &planner);
        for l in greedy.late_lines.iter().chain(&planner.late_lines) {
            assert!(l.contains("because of"), "late without a reason: {l}");
        }
        assert!(
            greedy.home_missed > planner.home_missed,
            "planning home time keeps promises"
        );
        assert!(
            planner.funds > greedy.funds,
            "chasing the longest load loses on a fresh career"
        );
    }

    #[test]
    #[ignore]
    fn seeds_report() {
        let w = get_world();
        let days: u32 = std::env::var("DAYS")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(6);
        for seed in [7, 1, 2, 3, 11, 42] {
            report(
                &format!("seed {seed} greedy"),
                &play(Bot::Greedy, seed, days, w),
            );
            report(
                &format!("seed {seed} planner"),
                &play(Bot::Planner, seed, days, w),
            );
        }
    }
}
