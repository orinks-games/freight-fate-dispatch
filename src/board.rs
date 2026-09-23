//! The day's loads, from ff-core's own job board at the terminal city.

use ff_core::data::world::World;
use ff_core::data::world_models::Route;
use ff_core::models::jobs::{required_hours, Job, JobBoard, OfferOptions};
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
    JobBoard::seeded(world, seed).offers(
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
    let route = route_for(world, job);
    let need_h = required_hours(
        job.distance_mi,
        route.as_ref(),
        Some(world),
        Some(&driver.hos),
    );
    if need_h > job.deadline_game_h {
        return Some(format!(
            "{} doesn't have the hours to make this appointment.",
            driver.name
        ));
    }
    None
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
