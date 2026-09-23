//! Calls that aren't about a truck on the road: drivers and brokers.

use ff_core::data::world::World;
use ff_core::models::jobs::{JobBoard, OfferOptions};

use crate::board::{load_line, Load};
use crate::career::{Game, Shift};
use crate::desk::abs_min;
use crate::roster::{Driver, Status};

pub const SICK_ODDS: f64 = 0.05;
pub const PERSONAL_ODDS: f64 = 0.04;
pub const HOT_PAY: f64 = 1.35;
pub const HOT_DEADLINE: f64 = 0.85;

#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Sick { driver_id: u32 },
    PersonalDay { driver_id: u32 },
    HotLoad { offer: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallChoice {
    LetRest,
    AskIn,
    Grant,
    Deny,
    Accept,
    Decline,
}

impl CallChoice {
    pub fn label(self) -> &'static str {
        match self {
            CallChoice::LetRest => "Tell them to rest up",
            CallChoice::AskIn => "Ask them to come in anyway",
            CallChoice::Grant => "Give them the day",
            CallChoice::Deny => "Ask them to work",
            CallChoice::Accept => "Take the load",
            CallChoice::Decline => "Pass on it",
        }
    }
}

/// Today's calls: drivers early in the shift, brokers any time.
pub fn morning_calls(game: &Game) -> Vec<(i64, Call)> {
    let mut rng = crate::rng(game.seed, game.day, "calls");
    let mut out = Vec::new();
    for d in game.drivers.iter().filter(|d| d.status == Status::Idle) {
        let roll = rng.random();
        let at = abs_min(game.day, rng.randint(370, 480) as u32);
        if roll < SICK_ODDS {
            out.push((at, Call::Sick { driver_id: d.id }));
        } else if roll < SICK_ODDS + PERSONAL_ODDS {
            out.push((at, Call::PersonalDay { driver_id: d.id }));
        }
    }
    for offer in 0..rng.randint(0, 2) as usize {
        out.push((
            abs_min(game.day, rng.randint(420, 960) as u32),
            Call::HotLoad { offer },
        ));
    }
    out
}

/// Broker loads: the terminal's freight, paid up, on a tighter clock.
///
/// `map_while` (not `filter_map`) so a miss stops the run rather than
/// leaving a gap: offers always come back as a contiguous `0..len`, which
/// keeps a `Call::HotLoad { offer }` index meaningful.
pub fn hot_offers(game: &Game, world: &World, count: usize) -> Vec<Load> {
    (0..count)
        .map_while(|n| {
            let seed = crate::seed_for(game.seed, game.day, &format!("hot:{n}"));
            let endorsements: Vec<String> =
                game.drivers.iter().flat_map(Driver::endorsements).collect();
            let level = game.drivers.iter().map(Driver::level).max().unwrap_or(1);
            let mut job = JobBoard::seeded(world, seed)
                .offers(
                    &game.terminal,
                    &endorsements,
                    OfferOptions {
                        count: 1,
                        level,
                        market: Some(&game.market),
                        carrier_key: Some(&game.carrier_key),
                        direct_freight: false,
                    },
                )
                .into_iter()
                .next()?;
            job.pay = (job.pay * HOT_PAY).round();
            job.deadline_game_h *= HOT_DEADLINE;
            Some(Load {
                job,
                hot: true,
                weather: Vec::new(),
            })
        })
        .collect()
}

/// Drop hot-load calls whose offer never made it onto the board (a shorter
/// `hot_offers` than the day's calls expected), so no `Call::HotLoad { offer
/// }` schedules past the end of `Shift.hot_offers`.
pub(crate) fn live_calls(calls: Vec<(i64, Call)>, offers_len: usize) -> Vec<(i64, Call)> {
    calls
        .into_iter()
        .filter(|(_, c)| !matches!(c, Call::HotLoad { offer } if *offer >= offers_len))
        .collect()
}

pub fn script(game: &Game, shift: &Shift, call: &Call, world: &World) -> String {
    let name = |id: u32| game.driver(id).map(|d| d.name.clone()).unwrap_or_default();
    match call {
        Call::Sick { driver_id } => format!("{} is calling in sick today.", name(*driver_id)),
        Call::PersonalDay { driver_id } => {
            format!("{} is asking for a personal day.", name(*driver_id))
        }
        Call::HotLoad { offer } => match shift.hot_offers.get(*offer) {
            Some(l) => format!(
                "A broker has a hot load. {}",
                load_line(&l.job, true, world)
            ),
            None => String::new(),
        },
    }
}

pub fn choices(call: &Call) -> Vec<CallChoice> {
    match call {
        Call::Sick { .. } => vec![CallChoice::LetRest, CallChoice::AskIn],
        Call::PersonalDay { .. } => vec![CallChoice::Grant, CallChoice::Deny],
        Call::HotLoad { .. } => vec![CallChoice::Accept, CallChoice::Decline],
    }
}

pub fn apply(game: &mut Game, shift: &mut Shift, call: &Call, choice: CallChoice) -> Vec<String> {
    let mut rng = crate::rng(game.seed, game.day, &format!("call:{}", shift.clock.minute));
    let off = |game: &mut Game, id: u32, goodwill: i32, line: &str| {
        if let Some(d) = game.driver_mut(id) {
            d.status = Status::OffDuty;
            d.adjust_goodwill(goodwill);
        }
        vec![line.to_string()]
    };
    match (call, choice) {
        (Call::Sick { driver_id }, CallChoice::LetRest) => {
            off(game, *driver_id, 3, "They'll be back tomorrow.")
        }
        (Call::Sick { driver_id }, CallChoice::AskIn) => {
            if rng.random() < 0.5 {
                if let Some(d) = game.driver_mut(*driver_id) {
                    d.adjust_goodwill(-10);
                }
                vec!["They'll come in. They aren't happy about it.".into()]
            } else {
                off(game, *driver_id, -5, "They can't. They're staying home.")
            }
        }
        (Call::PersonalDay { driver_id }, CallChoice::Grant) => {
            off(game, *driver_id, 5, "Day granted.")
        }
        (Call::PersonalDay { driver_id }, CallChoice::Deny) => {
            if let Some(d) = game.driver_mut(*driver_id) {
                d.adjust_goodwill(-8);
            }
            vec!["They'll work. You'll hear about it.".into()]
        }
        (Call::HotLoad { offer }, CallChoice::Accept) => {
            match shift.hot_offers.get(*offer).cloned() {
                Some(load) => {
                    shift.board.push(load);
                    vec!["The hot load is on the board, at the end.".into()]
                }
                None => Vec::new(),
            }
        }
        _ => vec!["Passed.".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::{tests::game, Ring};
    use ff_core::data::world::get_world;

    #[test]
    fn calls_replay_for_the_same_morning() {
        let g = game();
        assert_eq!(morning_calls(&g), morning_calls(&g));
        assert!(morning_calls(&g)
            .iter()
            .all(|(at, _)| *at > abs_min(g.day, crate::desk::SHIFT_START)));
    }

    #[test]
    fn hot_load_pays_more_on_a_tighter_clock_and_lands_on_the_board() {
        let w = get_world();
        let g = game();
        let offer = hot_offers(&g, w, 1).remove(0);
        assert!(offer.hot);
        let mut g = g;
        let mut s = g.morning(w);
        s.hot_offers = vec![offer];
        apply(
            &mut g,
            &mut s,
            &Call::HotLoad { offer: 0 },
            CallChoice::Accept,
        );
        assert!(s.board.last().unwrap().hot);
        assert!(
            !s.board[0].hot,
            "board positions under an open picker stay put"
        );
    }

    #[test]
    fn granting_a_day_takes_the_driver_off_and_raises_goodwill() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let id = g.drivers[0].id;
        apply(
            &mut g,
            &mut s,
            &Call::PersonalDay { driver_id: id },
            CallChoice::Grant,
        );
        assert_eq!(g.driver(id).unwrap().status, Status::OffDuty);
        assert_eq!(
            g.driver(id).unwrap().goodwill,
            crate::roster::GOODWILL_START + 5
        );
    }

    #[test]
    fn a_hot_load_call_with_no_matching_offer_is_dropped() {
        let calls = vec![
            (100, Call::HotLoad { offer: 0 }),
            (200, Call::PersonalDay { driver_id: 1 }),
        ];
        let live = live_calls(calls, 0);
        assert!(!live.iter().any(|(_, c)| matches!(c, Call::HotLoad { .. })));
        assert_eq!(live.len(), 1);
    }

    #[test]
    fn a_call_about_a_driver_now_on_a_load_is_dropped() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let id = g.drivers[0].id;
        g.driver_mut(id).unwrap().status = Status::OnLoad;
        s.schedule.push(
            abs_min(g.day, 361),
            Ring::Call(Call::Sick { driver_id: id }),
        );
        s.clock.minute = 400;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty());
    }
}
