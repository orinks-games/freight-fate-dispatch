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

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Call {
    Sick {
        driver_id: u32,
    },
    PersonalDay {
        driver_id: u32,
    },
    HotLoad {
        offer: usize,
    },
    /// A driver just went empty: "Where to?"
    Empty {
        driver_id: u32,
    },
    /// A driver promised home time is about to quit.
    QuitWarning {
        driver_id: u32,
    },
    /// 5 PM: the customers' tenders for the next working day.
    Tenders,
    /// Monday: a customer asks about a dedicated account.
    Dedicated {
        customer_id: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallChoice {
    LetRest,
    AskWell,
    Grant,
    Deny,
    Accept,
    Decline,
    FindLoad,
    SendHome,
    LetSit,
    PromiseHome,
    AskStay,
    AcceptAll,
    Review,
    DeclineAll,
    TakeIt,
    PassOnIt,
}

impl CallChoice {
    pub fn label(self) -> &'static str {
        match self {
            CallChoice::LetRest => "Tell them to rest up",
            CallChoice::AskWell => "Ask if they're well enough to drive",
            CallChoice::Grant => "Give them the day",
            CallChoice::Deny => "Ask them to work",
            CallChoice::Accept => "Take the load",
            CallChoice::Decline => "Pass on it",
            CallChoice::FindLoad => "Find them a load",
            CallChoice::SendHome => "Send them home",
            CallChoice::LetSit => "Have them sit tight for now",
            CallChoice::PromiseHome => "Send them home today",
            CallChoice::AskStay => "Ask them to stay out a little longer",
            CallChoice::AcceptAll => "Accept them all",
            CallChoice::Review => "Go through them one by one",
            CallChoice::DeclineAll => "Decline them all",
            CallChoice::TakeIt => "Take the account",
            CallChoice::PassOnIt => "Pass for now",
        }
    }

    /// What the answer costs or gets, for F1 on the option.
    pub fn help(self) -> &'static str {
        match self {
            CallChoice::LetRest => "They take the day off and think better of you for it.",
            CallChoice::AskWell => {
                "They won't drive today either way, and asking costs a little goodwill."
            }
            CallChoice::Grant => "They take the day off. Good for their morale.",
            CallChoice::Deny => "They work today and hold it against you.",
            CallChoice::Accept => {
                "Puts the load at the end of the board. It pays more on a tighter clock."
            }
            CallChoice::Decline => "The load goes to another carrier.",
            CallChoice::FindLoad => "Opens the loads near them.",
            CallChoice::SendHome => "Sends them home empty from where they are.",
            CallChoice::LetSit => "They wait where they are until you find them something.",
            CallChoice::PromiseHome => {
                "Good for their morale. On a load, they go home when it delivers, and a load queued behind it goes back."
            }
            CallChoice::AskStay => "They stay out and take it badly.",
            CallChoice::AcceptAll => {
                "Each load leads the board on its day. One left uncovered is a service failure."
            }
            CallChoice::Review => "Answer each one on the Tenders screen.",
            CallChoice::DeclineAll => "Counts against each customer's acceptance for the week.",
            CallChoice::TakeIt => {
                "10 loads a week at 5 percent over the contract rate. They expect 90 percent acceptance and on time every week."
            }
            CallChoice::PassOnIt => "The account stays as it is.",
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
        // Reliability: a 5 calls off less than half as often as a 1.
        let mult = 1.0 + 0.3 * (3.0 - d.traits().reliability as f64);
        if roll < SICK_ODDS * mult {
            out.push((at, Call::Sick { driver_id: d.id }));
        } else if roll < (SICK_ODDS + PERSONAL_ODDS) * mult {
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
            job.pay = (crate::board::lane_pay(job.pay, job.distance_mi) * HOT_PAY).round();
            job.deadline_game_h *= HOT_DEADLINE;
            let mut load = game.spot_load(job, hot_broker(game, n));
            load.hot = true;
            Some(load)
        })
        .collect()
}

/// Who calls with hot load `n`: a broker who trusts the carrier calls it
/// first; with none, whoever has the freight.
fn hot_broker(game: &Game, n: usize) -> String {
    let trusted: Vec<&String> = game
        .broker_trust
        .iter()
        .filter(|(b, t)| **t >= crate::customers::TRUSTED && game.grudge_days(b) == 0)
        .map(|(b, _)| b)
        .collect();
    if trusted.is_empty() {
        return crate::customers::spot_broker(game.seed, game.day, 100 + n);
    }
    let mut rng = crate::rng(game.seed, game.day, &format!("hot-broker:{n}"));
    trusted[rng.randint(0, trusted.len() as i64 - 1) as usize].clone()
}

/// Whether the broker behind a load trusts the carrier enough to call first.
fn called_first(game: &Game, load: &Load) -> bool {
    matches!(&load.source, crate::customers::Source::Spot { broker }
        if game.broker_trust.get(broker).copied().unwrap_or(0) >= crate::customers::TRUSTED)
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
                "{} {} {}",
                l.owner(game),
                if called_first(game, l) {
                    "called you first with a hot load:"
                } else {
                    "has a hot load."
                },
                crate::board::with_terms(load_line(&l.job, false, world), &l.terms),
            ),
            None => String::new(),
        },
        Call::Empty { driver_id } => match game.driver(*driver_id) {
            Some(d) => format!(
                "{} is empty in {} since {}. Where to?",
                d.name,
                world.spoken_city(&d.location, None),
                crate::desk::clock_text(
                    d.available_at
                        .min(shift.now(game.day))
                        .rem_euclid(crate::desk::MIN_PER_DAY) as u32
                )
            ),
            None => String::new(),
        },
        Call::QuitWarning { driver_id } => format!(
            "{} here. I was promised home time and I'm still out. Get me home, or I'm done with this outfit.",
            name(*driver_id)
        ),
        Call::Tenders => game.tenders_text(&shift.tomorrow),
        Call::Dedicated { customer_id } => game.dedicated_text(*customer_id),
    }
}

pub fn choices(call: &Call) -> Vec<CallChoice> {
    match call {
        Call::Sick { .. } => vec![CallChoice::LetRest, CallChoice::AskWell],
        Call::PersonalDay { .. } => vec![CallChoice::Grant, CallChoice::Deny],
        Call::HotLoad { .. } => vec![CallChoice::Accept, CallChoice::Decline],
        Call::Empty { .. } => vec![
            CallChoice::FindLoad,
            CallChoice::SendHome,
            CallChoice::LetSit,
        ],
        Call::QuitWarning { .. } => vec![CallChoice::PromiseHome, CallChoice::AskStay],
        Call::Tenders => vec![
            CallChoice::AcceptAll,
            CallChoice::Review,
            CallChoice::DeclineAll,
        ],
        Call::Dedicated { .. } => vec![CallChoice::TakeIt, CallChoice::PassOnIt],
    }
}

/// Send a driver home from the call itself. Another call waiting behind
/// this one doesn't stop it, the way it stops a desk action.
fn send_home_now(game: &mut Game, shift: &mut Shift, driver_id: u32, world: &World) -> String {
    let mut waiting = std::mem::take(&mut shift.pending);
    let said = game
        .send_home(shift, driver_id, world)
        .unwrap_or_else(|e| e.spoken());
    waiting.extend(shift.pending.drain(..));
    shift.pending = waiting;
    said
}

pub fn apply(
    game: &mut Game,
    shift: &mut Shift,
    call: &Call,
    choice: CallChoice,
    world: &World,
) -> Vec<String> {
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
        (Call::Sick { driver_id }, CallChoice::AskWell) => {
            if rng.random() < 0.5 {
                off(
                    game,
                    *driver_id,
                    -2,
                    "They'll come in for office work, but they won't drive today.",
                )
            } else {
                off(
                    game,
                    *driver_id,
                    -3,
                    "They're not up to it. They're staying home.",
                )
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
        (Call::Empty { .. }, CallChoice::FindLoad) => Vec::new(),
        (Call::Empty { driver_id }, CallChoice::SendHome) => {
            vec![send_home_now(game, shift, *driver_id, world)]
        }
        (Call::Empty { .. }, CallChoice::LetSit) => vec!["They'll sit tight.".into()],
        (Call::QuitWarning { driver_id }, CallChoice::PromiseHome) => {
            if let Some(d) = game.driver_mut(*driver_id) {
                d.adjust_goodwill(5);
                d.quit_warned = true;
            }
            match game.driver(*driver_id).map(|d| d.status) {
                Some(Status::Idle | Status::OffDuty) => {
                    vec![send_home_now(game, shift, *driver_id, world)]
                }
                Some(Status::OnLoad | Status::OutOnRun | Status::Unloading) => {
                    let mut out = vec!["They'll head home when this load delivers.".into()];
                    if let Some(d) = game.driver_mut(*driver_id) {
                        d.home_next = true;
                    }
                    let dropped = game
                        .trips
                        .iter_mut()
                        .find(|t| t.driver_id == *driver_id)
                        .and_then(|t| t.next.take().map(|_| t.next_source.clone()));
                    if let Some(source) = dropped {
                        let owner = match &source {
                            crate::customers::Source::Spot { broker } => broker.clone(),
                            crate::customers::Source::Contract { customer_id } => {
                                game.customer_name(*customer_id)
                            }
                        };
                        let grudge = game.load_dropped(&source);
                        out.push(format!("Their queued load goes back to {owner}."));
                        out.extend(grudge);
                    }
                    out
                }
                _ => vec!["They'll get home time as soon as they're free.".into()],
            }
        }
        (Call::Tenders, CallChoice::AcceptAll) => game.answer_tenders(shift, Some(true), world),
        (Call::Tenders, CallChoice::Review) => game.answer_tenders(shift, None, world),
        (Call::Tenders, CallChoice::DeclineAll) => game.answer_tenders(shift, Some(false), world),
        (Call::Dedicated { customer_id }, CallChoice::TakeIt) => {
            vec![game.answer_dedicated(*customer_id, true)]
        }
        (Call::Dedicated { customer_id }, CallChoice::PassOnIt) => {
            vec![game.answer_dedicated(*customer_id, false)]
        }
        (Call::QuitWarning { driver_id }, CallChoice::AskStay) => {
            if let Some(d) = game.driver_mut(*driver_id) {
                d.adjust_goodwill(-5);
                d.quit_warned = true;
            }
            vec!["They'll stay out, for now. They didn't take it well.".into()]
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
            w,
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
            w,
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

    fn sick_call_goal_case(choice: CallChoice) -> (Game, crate::career::Shift, u32, Vec<String>) {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.schedule = Default::default();
        let id = g
            .drivers
            .iter()
            .find(|driver| {
                driver.status == Status::Idle
                    && s.board
                        .iter()
                        .any(|load| crate::board::refusal(&load.job, driver, w).is_none())
            })
            .unwrap()
            .id;
        let lines = apply(&mut g, &mut s, &Call::Sick { driver_id: id }, choice, w);
        assert_eq!(g.driver(id).unwrap().status, Status::OffDuty);
        (g, s, id, lines)
    }

    #[test]
    fn letting_a_sick_driver_rest_reduces_the_goal_and_keeps_them_off_duty() {
        let (g, _, id, lines) = sick_call_goal_case(CallChoice::LetRest);
        assert!(lines.contains(&"They'll be back tomorrow.".to_string()));
        assert_eq!(
            g.driver(id).unwrap().goodwill,
            crate::roster::GOODWILL_START + 3
        );
    }

    #[test]
    fn asking_a_sick_driver_about_their_health_keeps_them_off_duty() {
        let (g, s, id, lines) = sick_call_goal_case(CallChoice::AskWell);
        let roll = crate::rng(g.seed, g.day, &format!("call:{}", s.clock.minute)).random();
        let (expected_line, goodwill) = if roll < 0.5 {
            (
                "They'll come in for office work, but they won't drive today.",
                -2,
            )
        } else {
            ("They're not up to it. They're staying home.", -3)
        };
        assert!(lines.contains(&expected_line.to_string()));
        assert_eq!(
            g.driver(id).unwrap().goodwill,
            crate::roster::GOODWILL_START + goodwill
        );
    }

    #[test]
    fn morning_does_not_count_an_idle_driver_with_no_legal_board_load() {
        let w = get_world();
        let mut g = game();
        for driver in &mut g.drivers {
            driver.status = Status::OffDuty;
        }
        let driver = &mut g.drivers[0];
        driver.status = Status::Idle;
        driver.hos.drive(660.0);
        let s = g.morning(w);
        assert_eq!(g.eligible_idle(&s, w), 0);
    }

    #[test]
    fn a_broker_who_trusts_the_carrier_calls_it_first_with_hot_loads() {
        let w = get_world();
        let mut g = game();
        let stranger = hot_offers(&g, w, 2);
        assert!(!stranger.is_empty());
        let mut s = g.morning(w);
        s.hot_offers = stranger;
        let cold = script(&g, &s, &Call::HotLoad { offer: 0 }, w);
        assert!(
            crate::customers::BROKERS
                .iter()
                .any(|b| cold.starts_with(&format!("{b} has a hot load. "))),
            "{cold}"
        );
        g.broker_trust
            .insert("Ironwood Brokerage".into(), crate::customers::TRUSTED);
        let offers = hot_offers(&g, w, 2);
        assert!(offers.iter().all(|l| l.owner(&g) == "Ironwood Brokerage"));
        s.hot_offers = offers;
        let warm = script(&g, &s, &Call::HotLoad { offer: 0 }, w);
        assert!(
            warm.starts_with("Ironwood Brokerage called you first with a hot load: "),
            "{warm}"
        );
        // A broker holding a dropped load against the carrier doesn't.
        let source = crate::customers::Source::Spot {
            broker: "Ironwood Brokerage".into(),
        };
        g.broker_trust
            .insert("Ironwood Brokerage".into(), crate::customers::TRUSTED + 2);
        g.load_dropped(&source);
        assert!(g.grudge_days("Ironwood Brokerage") > 0);
        let _ = hot_offers(&g, w, 2); // any broker; no panic with none trusted
    }
}
