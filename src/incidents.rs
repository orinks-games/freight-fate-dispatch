//! The board talks back: the launch incidents, their triggers, and what
//! each choice does to the trip, funds, goodwill and service record.

use ff_core::data::world::World;
use ff_core::models::jobs::relay::RELAY_RANGE_MI;
use ff_core::models::jobs::DEADLINE_AVG_MPH;

use crate::career::{Game, Ring, Shift, StoredAlert};
use crate::roster::Status;
use crate::trip::Trip;

pub const BREAKDOWN_PER_LEG: f64 = 0.03;
pub const DETENTION_BASE: f64 = 0.12;
pub const DETENTION_SLOW_SHIPPER: f64 = 0.35;
pub const DETENTION_SOURED: f64 = 0.10;
pub const REPAIR_COST: f64 = 450.0;
pub const REPAIR_MIN: f64 = 120.0;
pub const RESET_MIN: f64 = 600.0;
pub const DETENTION_MIN: f64 = 240.0;
pub const DETENTION_BILL: f64 = 150.0;
/// The driver calls from the dock this long after dispatch.
pub const DOCK_CALL_AFTER_MIN: f64 = 30.0;
/// Spoken for the whole hold charged: the 30 minutes before the call plus
/// `DETENTION_MIN`.
pub const DETENTION_CAUSE: &str = "the 4-and-a-half-hour wait at the shipper";
pub const SEEDED_STORM: f64 = 0.06;
pub const HOLD_MIN: f64 = 240.0;
pub const WEATHER_FINE: f64 = 300.0;
const SEEDED_EVENTS: &[&str] = &[
    "Winter Storm Warning",
    "Severe Thunderstorm Warning",
    "High Wind Warning",
];

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    HoursShort,
    Breakdown,
    Detention,
    Weather {
        event: String,
        city: String,
        delay_min: f64,
        avoid: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Incident {
    pub trip_id: u32,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Relay(u32),
    ParkLate,
    CallReceiver,
    Repair,
    SendTruck(u32),
    Wait,
    BillDetention,
    PullDriver,
    Hold,
    Reroute,
    PushThrough,
}

impl Choice {
    pub fn label(&self, game: &Game) -> String {
        let name = |id: u32| game.driver(id).map(|d| d.name.clone()).unwrap_or_default();
        match *self {
            Choice::Relay(id) => format!("Relay the load to {}", name(id)),
            Choice::ParkLate => "Park for a 10-hour break and deliver late".into(),
            Choice::CallReceiver => "Call the receiver for a new appointment".into(),
            Choice::Repair => format!(
                "Roadside repair, {} and about 2 hours",
                crate::money(REPAIR_COST)
            ),
            Choice::SendTruck(id) => format!("Send {} out with another truck", name(id)),
            Choice::Wait => "Wait and waive detention".into(),
            Choice::BillDetention => {
                format!(
                    "Wait and bill {} detention after the two free hours",
                    crate::money(DETENTION_BILL)
                )
            }
            Choice::PullDriver => "Cancel the pickup and pull the driver".into(),
            Choice::Hold => "Hold until it clears, about 4 hours".into(),
            Choice::Reroute => "Reroute around it".into(),
            Choice::PushThrough => "Push through".into(),
        }
    }
}

/// Some shippers are slow to load, and it's the same ones every day.
fn slow_shipper(facility: &str) -> bool {
    facility.bytes().map(u32::from).sum::<u32>() % 4 == 0
}

/// Incidents seeded for a trip at dispatch, keyed by absolute game minute.
pub fn roll_for_trip(game: &Game, trip: &Trip, alerts: &[StoredAlert]) -> Vec<(i64, Incident)> {
    let mut rng = crate::rng(game.seed, game.day, &format!("incidents:{}", trip.id));
    let mut out = Vec::new();
    let soured = game
        .shipper_goodwill
        .get(&trip.job.origin_location)
        .copied()
        .unwrap_or(0)
        < 0;
    let detention = if slow_shipper(&trip.job.origin_location) {
        DETENTION_SLOW_SHIPPER
    } else {
        DETENTION_BASE
    } + if soured { DETENTION_SOURED } else { 0.0 };
    if rng.random() < detention {
        out.push((
            trip.depart + DOCK_CALL_AFTER_MIN as i64,
            Incident {
                trip_id: trip.id,
                kind: Kind::Detention,
            },
        ));
    }
    let total = trip.city_mi.last().copied().unwrap_or(0.0).max(1.0);
    for leg in 0..trip.city_mi.len().saturating_sub(1) {
        if rng.random() < BREAKDOWN_PER_LEG {
            let mid = (trip.city_mi[leg] + trip.city_mi[leg + 1]) / 2.0 / total;
            out.push((
                trip.at_fraction(mid),
                Incident {
                    trip_id: trip.id,
                    kind: Kind::Breakdown,
                },
            ));
            break;
        }
    }
    if game.weather_live {
        if let Some(a) = alerts.first() {
            let at = trip.at_fraction(trip.fraction_at_city(&a.city));
            out.push((
                at,
                Incident {
                    trip_id: trip.id,
                    kind: Kind::Weather {
                        event: a.event.clone(),
                        city: a.city.clone(),
                        delay_min: a.delay_min,
                        avoid: a.avoid,
                    },
                },
            ));
        }
    } else if rng.random() < SEEDED_STORM {
        let f = 0.2 + rng.random() * 0.6;
        let event = rng.choice(SEEDED_EVENTS).to_string();
        let miles = f * trip.city_mi.last().copied().unwrap_or(0.0);
        let city = trip.cities[trip.city_mi.iter().rposition(|&m| m <= miles).unwrap_or(0)].clone();
        out.push((
            trip.at_fraction(f),
            Incident {
                trip_id: trip.id,
                kind: Kind::Weather {
                    event,
                    city,
                    delay_min: 90.0,
                    avoid: false,
                },
            },
        ));
    }
    out
}

fn trip<'a>(game: &'a Game, inc: &Incident) -> Option<&'a Trip> {
    game.trips.iter().find(|t| t.id == inc.trip_id)
}

fn driver_name(game: &Game, inc: &Incident) -> String {
    trip(game, inc)
        .and_then(|t| game.driver(t.driver_id))
        .map(|d| d.name.clone())
        .unwrap_or_default()
}

pub fn script(game: &Game, inc: &Incident, now: i64, world: &World) -> String {
    let who = driver_name(game, inc);
    let Some(t) = trip(game, inc) else {
        return String::new();
    };
    let near = world.spoken_city(&t.city_at(now), None);
    match &inc.kind {
        Kind::HoursShort => format!(
            "{who} here, near {near}. With the delays I'm out of hours before the {} appointment. What do you want me to do?",
            world.spoken_city(&t.job.destination, None)
        ),
        Kind::Breakdown => format!("{who} here. I've broken down near {near}. The truck isn't going anywhere without a mechanic."),
        Kind::Detention => format!(
            "{who} here, at the shipper in {}. They're not ready to load. They say about four hours.",
            world.spoken_city(&t.job.origin, None)
        ),
        Kind::Weather { event, city, avoid, .. } => format!(
            "{who} here. There's a {event} near {} on my route.{}",
            world.spoken_city(city, None),
            if *avoid { " They're saying stay off the road." } else { "" }
        ),
    }
}

/// The nearest free driver cleared for this load who is in range and has
/// the hours to drive out to the truck and the cycle hours to finish it.
fn relief(game: &Game, t: &Trip, now: i64, world: &World) -> Option<u32> {
    let truck_at = t.city_at(now);
    let left = t.drive_min * (1.0 - t.progress(now));
    game.drivers
        .iter()
        .filter(|d| {
            d.id != t.driver_id
                && d.status == Status::Idle
                && d.rest_note.is_none()
                && t.job
                    .locked_reason(&d.endorsements(), d.level(), None, false)
                    .is_empty()
        })
        .filter_map(|d| {
            let (miles, minutes) = crate::trip::empty_leg(&d.location, &truck_at, world);
            let mut now_d = d.clone();
            now_d.clock_to(now);
            let cycle_ok = now_d
                .hos
                .cycle_remaining_min("realistic")
                .is_none_or(|c| minutes + left + 60.0 <= c);
            let fits = miles <= RELAY_RANGE_MI && minutes + 60.0 <= now_d.legal_left_min();
            (fits && cycle_ok).then_some((d.id, miles))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

pub fn choices(game: &Game, inc: &Incident, now: i64, world: &World) -> Vec<Choice> {
    let Some(t) = trip(game, inc) else {
        return Vec::new();
    };
    match inc.kind {
        Kind::HoursShort => {
            let mut c = Vec::new();
            if let Some(id) = relief(game, t, now, world) {
                c.push(Choice::Relay(id));
            }
            c.extend([Choice::ParkLate, Choice::CallReceiver]);
            c
        }
        Kind::Breakdown => {
            let mut c = vec![Choice::Repair];
            if let Some(id) = relief(game, t, now, world) {
                c.push(Choice::SendTruck(id));
            }
            c
        }
        Kind::Detention => vec![Choice::Wait, Choice::BillDetention, Choice::PullDriver],
        Kind::Weather { avoid, .. } => {
            let mut choices = vec![Choice::Hold, Choice::Reroute];
            if !avoid {
                choices.push(Choice::PushThrough);
            }
            choices
        }
    }
}

/// Put `new_driver` on the load where it stands: they deadhead out from
/// where they are, the truck waits for them, and the first driver logs
/// their part and takes a 10-hour break there.
fn swap_driver(
    game: &mut Game,
    shift: &mut Shift,
    trip_id: u32,
    new_driver: u32,
    world: &World,
) -> String {
    let now = shift.now(game.day);
    let t = game.trips.iter().find(|t| t.id == trip_id).unwrap().clone();
    let truck_at = t.city_at(now);
    let from = game.driver(new_driver).unwrap().location.clone();
    let (miles, minutes) = crate::trip::empty_leg(&from, &truck_at, world);
    let done = t.drive_min * t.progress(now);
    if let Some(d) = game.driver_mut(t.driver_id) {
        if t.logged_min <= 0.0 {
            d.hos.on_duty(30.0 + t.dock_min);
        }
        d.drive_with_breaks((done - t.logged_min).max(0.0));
        d.location = truck_at;
        d.status = Status::OffDuty;
        d.rest_note = Some("a 10-hour break".into());
        d.available_at = now + 600;
        d.hos_at = now;
    }
    let d = game.driver_mut(new_driver).unwrap();
    d.clock_to(now);
    d.status = Status::OnLoad;
    d.hos.drive(minutes);
    d.hos_at = now + minutes.round() as i64;
    let (legal, name) = (d.legal_left_min(), d.name.clone());
    let t = game.trip_mut(trip_id).unwrap();
    t.driver_id = new_driver;
    t.deadhead_mi += miles;
    t.logged_min = done;
    t.legal_min = legal + t.drive_min * t.progress(now);
    t.drive_left_min = 0.0;
    t.window_left_min = 0.0;
    t.hours_resolved = true;
    game.add_delay(shift, trip_id, minutes, "the wait for a relief driver");
    format!(
        "{name} is heading out from {}, {} empty miles.",
        world.spoken_city(&from, None),
        miles.round()
    )
}

/// The dock wait is over: the truck loads and rolls after the time to the
/// dock call plus the wait. It never moved before this call was resolved.
fn load_after_wait(game: &mut Game, shift: &mut Shift, id: u32) {
    let Some(t) = game.trip_mut(id) else {
        return;
    };
    let waited = DOCK_CALL_AFTER_MIN + DETENTION_MIN;
    t.dock_hold = false;
    t.dock_min += waited;
    game.add_delay(shift, id, waited, DETENTION_CAUSE);
}

pub fn apply(
    game: &mut Game,
    shift: &mut Shift,
    inc: &Incident,
    choice: Choice,
    world: &World,
) -> Vec<String> {
    let id = inc.trip_id;
    let Some(t) = game.trips.iter().find(|t| t.id == id) else {
        return Vec::new();
    };
    let origin = t.job.origin_location.clone();
    let mut out = Vec::new();
    match choice {
        Choice::Relay(driver) | Choice::SendTruck(driver) => {
            if choice == Choice::SendTruck(driver) {
                game.funds -= REPAIR_COST; // the broken truck still gets fixed
            }
            out.push(swap_driver(game, shift, id, driver, world));
        }
        Choice::ParkLate => {
            game.trip_mut(id).unwrap().hours_resolved = true;
            game.add_delay(shift, id, RESET_MIN, "the 10-hour break");
            out.push("Parking for a 10-hour break. The load will be late.".into());
        }
        Choice::CallReceiver => {
            game.reputation -= 1.0;
            let odds = (0.5 + (game.reputation - 50.0) / 200.0).clamp(0.2, 0.8);
            let mut rng = crate::rng(
                game.seed,
                game.day,
                &format!("receiver:{id}:{}", shift.clock.minute),
            );
            let t = game.trip_mut(id).unwrap();
            t.hours_resolved = true;
            if rng.random() < odds {
                t.job.deadline_game_h += RESET_MIN / 60.0;
                out.push("The receiver moved the appointment. Parking for a reset.".into());
            } else {
                out.push(
                    "The receiver won't move it. Parking for a reset; the load will be late."
                        .into(),
                );
            }
            game.add_delay(shift, id, RESET_MIN, "the 10-hour break");
        }
        Choice::Repair => {
            game.funds -= REPAIR_COST;
            game.add_delay(shift, id, REPAIR_MIN, "the breakdown");
            out.push("A mechanic is on the way.".into());
        }
        Choice::Wait => {
            load_after_wait(game, shift, id);
            out.push("Waiting on the dock. Detention waived.".into());
        }
        Choice::BillDetention => {
            game.funds += DETENTION_BILL;
            *game.shipper_goodwill.entry(origin).or_insert(0) -= 1;
            load_after_wait(game, shift, id);
            out.push("Detention billed. The shipper won't forget it.".into());
        }
        Choice::PullDriver => {
            let t = game.trips.iter().position(|t| t.id == id).unwrap();
            let t = game.trips.remove(t);
            if let Some(d) = game.driver_mut(t.driver_id) {
                d.status = Status::Idle;
            }
            *game.shipper_goodwill.entry(origin).or_insert(0) -= 1;
            game.record.abandoned += 1;
            game.reputation -= 3.0;
            out.push("Pickup canceled. The shipper will remember it.".into());
        }
        Choice::Hold => {
            let cause = match &inc.kind {
                Kind::Weather { event, .. } => format!("the hold for the {}", event.to_lowercase()),
                _ => "the hold for weather".into(),
            };
            game.add_delay(shift, id, HOLD_MIN, &cause);
            out.push("Holding until it clears.".into());
        }
        Choice::Reroute => {
            let t = game.trips.iter().find(|t| t.id == id).unwrap();
            let routes = world
                .route_options(&t.job.origin, &t.job.destination, 2, false)
                .unwrap_or_default();
            let extra_mi = match routes.as_slice() {
                [a, b, ..] => (b.miles() - a.miles()).max(0.0),
                _ => 0.0,
            }
            .max(t.job.distance_mi * 0.10);
            let extra_min = extra_mi / DEADLINE_AVG_MPH * 60.0;
            let at = shift.now(game.day) + 30;
            let t = game.trip_mut(id).unwrap();
            t.drive_min += extra_min;
            t.plan_min += extra_min;
            t.delays.push(crate::trip::Delay {
                min: extra_min,
                cause: "the reroute".into(),
            });
            if t.hours_short() {
                shift.schedule.push(
                    at,
                    Ring::Incident(Incident {
                        trip_id: id,
                        kind: Kind::HoursShort,
                    }),
                );
            }
            out.push(format!(
                "Rerouting, about {} extra miles.",
                extra_mi.round()
            ));
        }
        Choice::PushThrough => {
            let Kind::Weather { delay_min, .. } = &inc.kind else {
                return out;
            };
            let (delay_min, risk) = (*delay_min, 0.2);
            let cause = match &inc.kind {
                Kind::Weather { event, .. } => format!("the {}", event.to_lowercase()),
                _ => "the weather".into(),
            };
            game.add_delay(shift, id, delay_min, &cause);
            let mut rng = crate::rng(
                game.seed,
                game.day,
                &format!("push:{id}:{}", shift.clock.minute),
            );
            if rng.random() < risk {
                if rng.random() < 0.5 {
                    game.funds -= WEATHER_FINE;
                    out.push(format!(
                        "Pushed through and got a {} citation.",
                        crate::money(WEATHER_FINE)
                    ));
                } else {
                    let at = shift.now(game.day) + 60;
                    shift.schedule.push(
                        at,
                        Ring::Incident(Incident {
                            trip_id: id,
                            kind: Kind::Breakdown,
                        }),
                    );
                    out.push("Pushing through. The truck is taking a beating.".into());
                }
            } else {
                out.push("Pushing through, slow and steady.".into());
            }
        }
    }
    if let Some(t) = game.trips.iter().find(|t| t.id == id) {
        out.push(t.eta_line(game.day));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::tests::{game, legal_pair};
    use crate::career::{ActError, Ring};
    use crate::desk::{abs_min, SHIFT_START};
    use ff_core::data::world::get_world;

    /// A game with one trip rolling, and that trip's id.
    fn rolling() -> (Game, Shift, u32) {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        s.schedule = Default::default();
        s.pending.clear();
        let id = g.trips[0].id;
        (g, s, id)
    }

    fn storm(id: u32, avoid: bool) -> Incident {
        Incident {
            trip_id: id,
            kind: Kind::Weather {
                event: "Winter Storm Warning".into(),
                city: String::new(),
                delay_min: 45.0,
                avoid,
            },
        }
    }

    #[test]
    fn live_alerts_on_the_route_schedule_a_weather_call_at_that_city() {
        let (mut g, _, _) = rolling();
        g.weather_live = true;
        let t = g.trips[0].clone();
        let city = t.cities[t.cities.len() / 2].clone();
        let alert = StoredAlert {
            event: "Winter Storm Warning".into(),
            city: city.clone(),
            delay_min: 45.0,
            avoid: false,
        };
        let rolls = roll_for_trip(&g, &t, &[alert]);
        let (at, inc) = rolls
            .iter()
            .find(|(_, i)| matches!(i.kind, Kind::Weather { .. }))
            .unwrap();
        assert_eq!(*at, t.at_fraction(t.fraction_at_city(&city)));
        assert!(
            matches!(&inc.kind, Kind::Weather { event, .. } if event == "Winter Storm Warning")
        );
    }

    #[test]
    fn live_day_without_alerts_rolls_no_weather() {
        let (mut g, _, _) = rolling();
        g.weather_live = true;
        let t = g.trips[0].clone();
        assert!(!roll_for_trip(&g, &t, &[])
            .iter()
            .any(|(_, i)| matches!(i.kind, Kind::Weather { .. })));
    }

    #[test]
    fn hold_waits_and_push_through_takes_the_alert_delay() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        apply(&mut g, &mut s, &storm(id, false), Choice::Hold, w);
        assert_eq!(g.trips[0].delay_min, HOLD_MIN);
        apply(&mut g, &mut s, &storm(id, false), Choice::PushThrough, w);
        assert!(g.trips[0].delay_min >= HOLD_MIN + 45.0);
    }

    #[test]
    fn avoid_weather_warnings_do_not_offer_push_through() {
        let w = get_world();
        let (g, s, id) = rolling();
        assert_eq!(
            choices(&g, &storm(id, true), s.now(g.day), w),
            vec![Choice::Hold, Choice::Reroute]
        );
        assert_eq!(
            choices(&g, &storm(id, false), s.now(g.day), w),
            vec![Choice::Hold, Choice::Reroute, Choice::PushThrough]
        );
    }

    #[test]
    fn reroute_adds_wheel_time_not_just_waiting() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let (drive, plan) = (g.trips[0].drive_min, g.trips[0].plan_min);
        apply(&mut g, &mut s, &storm(id, true), Choice::Reroute, w);
        assert!(g.trips[0].drive_min > drive);
        assert_eq!(g.trips[0].plan_min - plan, g.trips[0].drive_min - drive);
    }

    #[test]
    fn rolls_are_deterministic_for_a_reloaded_morning() {
        let (g, _, _) = rolling();
        let t = &g.trips[0];
        assert_eq!(roll_for_trip(&g, t, &[]), roll_for_trip(&g, t, &[]));
    }

    #[test]
    fn detention_wait_delays_and_bill_pays_and_sours_the_shipper() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let inc = Incident {
            trip_id: id,
            kind: Kind::Detention,
        };
        assert_eq!(Choice::Wait.label(&g), "Wait and waive detention");
        assert_eq!(
            Choice::BillDetention.label(&g),
            "Wait and bill $150 detention after the two free hours"
        );
        assert_eq!(
            Choice::PullDriver.label(&g),
            "Cancel the pickup and pull the driver"
        );
        g.trips[0].dock_hold = true;
        assert_eq!(
            g.trips[0].progress(s.now(g.day) + 60),
            0.0,
            "held at the dock"
        );
        let result = apply(&mut g, &mut s, &inc, Choice::Wait, w);
        assert_eq!(result[0], "Waiting on the dock. Detention waived.");
        assert!(result[1].starts_with("New arrival"), "{result:?}");
        assert!(!g.trips[0].dock_hold);
        assert!(g.trips[0].delay_min >= DETENTION_MIN);
        assert_eq!(g.trips[0].dock_min, g.trips[0].delay_min);
        assert_eq!(g.trips[0].delays[0].cause, DETENTION_CAUSE);
        assert_eq!(
            g.trips[0].delays[0].min, 270.0,
            "the cause says 4 and a half hours"
        );
        assert_eq!(DETENTION_MIN, 240.0);
        assert!(script(&g, &inc, s.now(g.day), w).ends_with("They say about four hours."));
        let funds = g.funds;
        g.trips[0].dock_hold = true;
        apply(&mut g, &mut s, &inc, Choice::BillDetention, w);
        assert_eq!(g.funds, funds + DETENTION_BILL);
        assert_eq!(g.shipper_goodwill[&g.trips[0].job.origin_location], -1);
    }

    #[test]
    fn pulling_the_driver_abandons_the_load_and_drops_its_later_rings() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let driver = g.trips[0].driver_id;
        let origin = g.trips[0].job.origin_location.clone();
        let later = g.trips[0].depart + 30;
        s.schedule.push(
            later,
            Ring::Incident(Incident {
                trip_id: id,
                kind: Kind::Breakdown,
            }),
        );
        let result = apply(
            &mut g,
            &mut s,
            &Incident {
                trip_id: id,
                kind: Kind::Detention,
            },
            Choice::PullDriver,
            w,
        );
        assert_eq!(
            result,
            vec!["Pickup canceled. The shipper will remember it."]
        );
        assert_eq!(g.shipper_goodwill[&origin], -1);
        assert!(g.trips.is_empty());
        assert_eq!(g.record.abandoned, 1);
        assert_eq!(g.reputation, 47.0);
        assert_eq!(g.driver(driver).unwrap().status, Status::Idle);
        s.clock.minute = SHIFT_START + 60;
        g.ring_due(&mut s);
        assert!(
            s.pending.is_empty(),
            "a ring about a pulled load never rings"
        );
    }

    #[test]
    fn repair_costs_money_and_time() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let funds = g.funds;
        apply(
            &mut g,
            &mut s,
            &Incident {
                trip_id: id,
                kind: Kind::Breakdown,
            },
            Choice::Repair,
            w,
        );
        assert_eq!(g.funds, funds - REPAIR_COST);
        assert_eq!(g.trips[0].delay_min, REPAIR_MIN);
    }

    #[test]
    fn send_truck_swaps_drivers_and_is_offered_only_with_an_idle_driver() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let inc = Incident {
            trip_id: id,
            kind: Kind::Breakdown,
        };
        let now = s.now(g.day);
        let funds = g.funds;
        let deadhead = g.trips[0].deadhead_mi;
        let spare = g
            .drivers
            .iter()
            .find(|d| d.status == Status::Idle)
            .unwrap()
            .id;
        assert!(choices(&g, &inc, now, w).contains(&Choice::SendTruck(spare)));
        let first = g.trips[0].driver_id;
        apply(&mut g, &mut s, &inc, Choice::SendTruck(spare), w);
        assert_eq!(g.funds, funds - REPAIR_COST, "empty miles bill on delivery");
        assert!(
            g.trips[0].deadhead_mi >= deadhead,
            "the relief's empty miles are the trip's"
        );
        assert_eq!(g.trips[0].driver_id, spare);
        let old = g.driver(first).unwrap();
        assert_eq!(old.status, Status::OffDuty);
        assert!(old.rest_note.is_some(), "the first driver takes a break");
        for d in &mut g.drivers {
            if d.status == Status::Idle {
                d.status = Status::OffDuty;
            }
        }
        assert!(!choices(&g, &inc, now, w)
            .iter()
            .any(|c| matches!(c, Choice::SendTruck(_))));
        assert!(
            choices(&g, &inc, now, w).contains(&Choice::Repair),
            "still answerable"
        );
    }

    #[test]
    fn relief_drivers_must_be_cleared_for_the_load_and_in_range() {
        let w = get_world();
        let (mut g, s, id) = rolling();
        let now = s.now(g.day);
        let helped = |g: &Game, kind: Kind| {
            choices(g, &Incident { trip_id: id, kind }, now, w)
                .iter()
                .any(|c| matches!(c, Choice::SendTruck(_) | Choice::Relay(_)))
        };
        assert!(helped(&g, Kind::Breakdown));
        let mut cargo = g.trips[0].job.cargo.clone();
        cargo.min_level = 99;
        g.trips[0].job.cargo = Box::leak(Box::new(cargo));
        assert!(!helped(&g, Kind::Breakdown), "no idle driver is level 99");
        assert!(!helped(&g, Kind::HoursShort));
        let (mut g, _, _) = rolling();
        for d in g.drivers.iter_mut().filter(|d| d.status == Status::Idle) {
            d.location = w.resolve_city_key("Los Angeles");
        }
        assert!(
            !helped(&g, Kind::Breakdown),
            "beyond relay range from where they are"
        );
        let (mut g, _, _) = rolling();
        for d in g.drivers.iter_mut().filter(|d| d.status == Status::Idle) {
            d.hos.drive(650.0);
        }
        assert!(!helped(&g, Kind::Breakdown), "out of hours");
    }

    #[test]
    fn a_delay_that_breaks_the_hours_rings_hours_short() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let t = g.trip_mut(id).unwrap();
        t.plan_min = t.drive_min + 30.0;
        t.legal_min = t.drive_min + 60.0;
        t.drive_left_min = t.legal_min;
        t.window_left_min = t.legal_min;
        g.add_delay(&mut s, id, 90.0, "the breakdown");
        assert!(s.schedule.iter().any(|(_, r)| matches!(
            r,
            Ring::Incident(Incident {
                kind: Kind::HoursShort,
                ..
            })
        )));
        let inc = Incident {
            trip_id: id,
            kind: Kind::HoursShort,
        };
        apply(&mut g, &mut s, &inc, Choice::ParkLate, w);
        assert!(g.trips[0].hours_resolved);
        assert_eq!(g.trips[0].delay_min, 90.0 + RESET_MIN);
    }

    #[test]
    fn call_receiver_always_parks_and_success_moves_the_appointment() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let deadline = g.trips[0].job.deadline_game_h;
        let rep = g.reputation;
        let lines = apply(
            &mut g,
            &mut s,
            &Incident {
                trip_id: id,
                kind: Kind::HoursShort,
            },
            Choice::CallReceiver,
            w,
        );
        assert_eq!(g.reputation, rep - 1.0);
        assert_eq!(g.trips[0].delay_min, RESET_MIN);
        let moved = g.trips[0].job.deadline_game_h == deadline + RESET_MIN / 60.0;
        assert_eq!(moved, lines.join(" ").contains("moved the appointment"));
    }

    #[test]
    fn a_ringing_phone_blocks_the_desk_until_answered() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        g.trips[0].dock_hold = true;
        s.pending.push_back(Ring::Incident(Incident {
            trip_id: id,
            kind: Kind::Detention,
        }));
        assert_eq!(g.check_truck(&mut s, id, w), Err(ActError::PhoneRinging));
        let minute = s.clock.minute;
        let ring = g.answer(&mut s).unwrap();
        assert_eq!(s.clock.minute, minute + 5);
        let labels = ring.labels(&g, &s, w);
        assert_eq!(labels.len(), 3);
        g.resolve(&mut s, &ring, 0, w);
        assert!(g.check_truck(&mut s, id, w).is_ok());
    }

    #[test]
    fn answering_skips_a_ring_whose_trip_was_pulled_after_it_rang() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let spare = g
            .drivers
            .iter()
            .find(|d| d.status == Status::Idle)
            .unwrap()
            .id;
        for kind in [Kind::Detention, Kind::Breakdown] {
            s.pending
                .push_back(Ring::Incident(Incident { trip_id: id, kind }));
        }
        s.pending
            .push_back(Ring::Call(crate::phone::Call::PersonalDay {
                driver_id: spare,
            }));
        let ring = g.answer(&mut s).unwrap();
        let pull = choices(
            &g,
            &Incident {
                trip_id: id,
                kind: Kind::Detention,
            },
            s.now(g.day),
            w,
        )
        .iter()
        .position(|c| *c == Choice::PullDriver)
        .unwrap();
        g.resolve(&mut s, &ring, pull, w);
        let minute = s.clock.minute;
        assert!(matches!(
            g.answer(&mut s),
            Ok(Ring::Call(crate::phone::Call::PersonalDay { .. }))
        ));
        assert_eq!(s.clock.minute, minute + 5);
        assert_eq!(g.answer(&mut s), Err(ActError::NoSuchItem));
    }

    #[test]
    fn a_ring_for_an_arrived_truck_or_resolved_hours_is_dead() {
        let (mut g, mut s, id) = rolling();
        let eta = abs_min(g.day, 600);
        let t = g.trip_mut(id).unwrap();
        t.plan_min = t.drive_min + 30.0;
        t.legal_min = t.drive_min + 60.0;
        t.drive_left_min = t.legal_min;
        t.window_left_min = t.legal_min;
        t.delay_min = 90.0;
        t.depart = eta - (t.plan_min + t.delay_min).round() as i64;
        assert!(t.hours_short());
        let short = |id| {
            Ring::Incident(Incident {
                trip_id: id,
                kind: Kind::HoursShort,
            })
        };
        s.pending.push_back(short(id));
        g.trip_mut(id).unwrap().hours_resolved = true;
        assert_eq!(g.answer(&mut s), Err(ActError::NoSuchItem), "resolved");
        g.trip_mut(id).unwrap().hours_resolved = false;
        s.schedule.push(eta, short(id));
        s.clock.minute = 600;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty(), "due as the truck arrives");
    }

    #[test]
    fn an_early_close_still_rings_a_same_day_trips_incident() {
        let (mut g, mut s, id) = rolling();
        let day = g.day;
        let t = g.trip_mut(id).unwrap();
        t.delay_min = 0.0;
        t.depart = abs_min(day, 480);
        t.plan_min = 300.0; // arrives 1 PM, before the close
        t.dock_hold = true;
        let ring = Ring::Incident(Incident {
            trip_id: id,
            kind: Kind::Detention,
        });
        s.schedule.push(abs_min(g.day, 540), ring.clone());
        s.clock.minute = 510;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty(), "not due yet at 8:30");
        s.clock.minute = crate::desk::SHIFT_END;
        g.ring_due(&mut s);
        assert_eq!(s.pending.len(), 1, "judged at 9 AM, not at 6 PM");
        assert_eq!(g.answer(&mut s), Ok(ring));
    }

    #[test]
    fn rings_after_the_close_never_ring() {
        let (mut g, mut s, id) = rolling();
        s.schedule.push(
            crate::desk::abs_min(g.day, crate::desk::SHIFT_END + 30),
            Ring::Incident(Incident {
                trip_id: id,
                kind: Kind::Breakdown,
            }),
        );
        s.clock.minute = crate::desk::SHIFT_END;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty());
    }
}
