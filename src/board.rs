//! The day's loads, from ff-core's own job board at the terminal city.

use ff_core::data::world::World;
use ff_core::data::world_models::Route;
use ff_core::models::jobs::{Job, JobBoard, OfferOptions};
use ff_core::models::market::Market;

use crate::roster::{Driver, Status};

pub const BOARD_SIZE: usize = 8;

/// One offer on the morning board, with whether it's flagged hot.
#[derive(Clone, Debug)]
pub struct Load {
    pub job: Job,
    pub hot: bool,
    pub weather: Vec<crate::career::StoredAlert>,
}

/// Lane pay is part fixed (stops, dock time) and part per mile, so the
/// rate per mile tapers on lanes longer than `TAPER_REF_MI`. Shorter lanes
/// keep the board's pay.
pub const TAPER_REF_MI: f64 = 150.0;
pub const TAPER_FIXED_SHARE: f64 = 0.5;

/// The board's pay for a lane once the long-haul taper is applied.
pub fn lane_pay(pay: f64, miles: f64) -> f64 {
    let factor = (1.0 - TAPER_FIXED_SHARE) + TAPER_FIXED_SHARE * TAPER_REF_MI / miles.max(1.0);
    (pay * factor.min(1.0)).round()
}

/// Offers for the whole roster: every credential anyone holds, the top level.
pub fn morning_board(
    world: &World,
    terminal: &str,
    carrier_key: &str,
    market: &Market,
    seed: i64,
    roster: &[Driver],
) -> Vec<Job> {
    let mut endorsements: Vec<String> = roster.iter().flat_map(Driver::endorsements).collect();
    endorsements.sort();
    endorsements.dedup();
    let level = roster.iter().map(Driver::level).max().unwrap_or(1);
    JobBoard::seeded(world, seed)
        .offers(
            terminal,
            &endorsements,
            OfferOptions {
                count: BOARD_SIZE,
                level,
                market: Some(market),
                carrier_key: Some(carrier_key),
                direct_freight: false,
            },
        )
        .into_iter()
        .map(|mut job| {
            job.pay = lane_pay(job.pay, job.distance_mi);
            job
        })
        .collect()
}

pub fn route_for(world: &World, job: &Job) -> Option<Route> {
    world
        .supported_route(&job.origin, &job.destination, None)
        .ok()
        .flatten()
}

/// Freight Fate's gates for its own player, applied to a roster driver.
/// `None` means the driver may take the load.
pub fn refusal(job: &Job, driver: &Driver, world: &World) -> Option<String> {
    if driver.status != Status::Idle {
        return Some(format!("{} is {}.", driver.name, driver.status.text()));
    }
    let reason = job.locked_reason(&driver.endorsements(), driver.level(), None, false);
    if !reason.is_empty() {
        return Some(format!("{} can't take it. {reason}", driver.name));
    }
    if let Some(why) = cycle_refusal(job, driver, world) {
        return Some(why);
    }
    let need_h = crate::trip::plan(job, driver, world).need_h;
    if need_h > job.deadline_game_h {
        return Some(format!(
            "{} doesn't have the hours to make this appointment.",
            driver.name
        ));
    }
    None
}

/// The 70-hour clock: the run's driving plus an hour to load and unload
/// must fit in what's left of the driver's 8-day cycle.
pub fn cycle_refusal(job: &Job, driver: &Driver, world: &World) -> Option<String> {
    let p = crate::trip::plan(job, driver, world);
    let left = driver.hos.cycle_remaining_min("realistic")?;
    (p.deadhead_min + p.drive_min + 60.0 > left).then(|| {
        format!(
            "{} is near the 70-hour limit and needs a 34-hour restart first.",
            driver.name
        )
    })
}

/// "Hot. Steel coils, Chicago to Denver, 1,003 miles, pays $2,450, due in 22 hours."
pub fn load_line(job: &Job, hot: bool, world: &World) -> String {
    format!(
        "{}{}, {} to {}, {} miles, pays {}, due in {}.",
        if hot { "Hot. " } else { "" },
        job.cargo.label,
        world.spoken_city(&job.origin, None),
        world.spoken_city(&job.destination, None),
        crate::money(job.distance_mi).trim_start_matches('$'),
        crate::money(job.pay),
        crate::hours(job.deadline_game_h * 60.0),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ff_core::data::world::get_world;
    use ff_core::models::career::Career;
    use ff_core::models::jobs::required_hours;

    pub(crate) fn chicago() -> String {
        get_world().resolve_city_key("Chicago")
    }

    fn fresh(id: u32) -> Driver {
        Driver::new(id, "Alma Reyes", Career::with_xp(20_000.0))
    }

    #[test]
    fn board_is_deterministic_per_seed() {
        let w = get_world();
        let m = Market::with_seed(1);
        let roster = [fresh(1)];
        let a = morning_board(w, &chicago(), "northstar", &m, 11, &roster);
        assert_eq!(a.len(), BOARD_SIZE);
        assert_eq!(
            a,
            morning_board(w, &chicago(), "northstar", &m, 11, &roster)
        );
        assert_ne!(
            a,
            morning_board(w, &chicago(), "northstar", &m, 12, &roster)
        );
    }

    #[test]
    fn busy_driver_is_refused() {
        let w = get_world();
        let job = morning_board(
            w,
            &chicago(),
            "northstar",
            &Market::with_seed(1),
            5,
            &[fresh(1)],
        )
        .remove(0);
        let mut d = fresh(1);
        d.status = Status::OnLoad;
        assert!(refusal(&job, &d, w).unwrap().contains("on a load"));
    }

    #[test]
    fn under_level_driver_is_refused_with_ff_reason() {
        let w = get_world();
        let senior = Driver::new(1, "Senior", Career::with_xp(1_000_000.0));
        let rookie = Driver::new(2, "Rookie", Career::new());
        let job = (0..40)
            .flat_map(|s| {
                let mut b = JobBoard::seeded(w, s);
                b.offers(
                    &chicago(),
                    &senior.endorsements(),
                    OfferOptions {
                        count: 20,
                        level: senior.level(),
                        ..Default::default()
                    },
                )
            })
            .find(|j| {
                j.cargo.min_level > rookie.level()
                    || !j
                        .cargo
                        .missing_credentials(&rookie.endorsements())
                        .is_empty()
            })
            .expect("some load a rookie cannot take");
        assert!(refusal(&job, &rookie, w)
            .unwrap()
            .starts_with("Rookie can't take it."));
    }

    #[test]
    fn out_of_hours_driver_is_refused_but_fresh_driver_is_not() {
        let w = get_world();
        let mut job = morning_board(
            w,
            &chicago(),
            "northstar",
            &Market::with_seed(1),
            5,
            &[fresh(1)],
        )
        .remove(0);
        let route = route_for(w, &job);
        job.deadline_game_h = required_hours(job.distance_mi, route.as_ref(), Some(w), None) + 0.5;
        let mut tired = fresh(2);
        tired.hos.drive(660.0);
        assert!(refusal(&job, &tired, w).unwrap().contains("hours"));
        assert_eq!(refusal(&job, &fresh(3), w), None);
    }
}
