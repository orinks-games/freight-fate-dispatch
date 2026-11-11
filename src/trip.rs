//! A load in progress. Its position is derived from time and the route's
//! miles; nothing moves in real time.

use ff_core::data::world::World;
use ff_core::models::jobs::{required_hours, route_drive_hours, Job};
use serde::{Deserialize, Serialize};

use crate::board::route_for;
use crate::desk::spoken_duration;
use crate::roster::Driver;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delay {
    pub min: f64,
    /// "the 4-and-a-half-hour wait at the shipper": what a late result is blamed on.
    pub cause: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trip {
    pub id: u32,
    pub driver_id: u32,
    #[serde(with = "job_json")]
    pub job: Job,
    /// Absolute game minute of dispatch.
    pub depart: i64,
    /// Wheel time for the whole route.
    pub drive_min: f64,
    /// Wheel time plus the HOS breaks and rests ff-core plans for this driver.
    pub plan_min: f64,
    /// The driver's legal wheel time at dispatch.
    pub legal_min: f64,
    /// Driving and on-duty window left at dispatch; 0 in older saves,
    /// which fall back to `legal_min` for both.
    #[serde(default)]
    pub drive_left_min: f64,
    #[serde(default)]
    pub window_left_min: f64,
    #[serde(default)]
    pub delay_min: f64,
    #[serde(default)]
    pub hours_resolved: bool,
    #[serde(default)]
    pub hot: bool,
    /// Route city keys and the cumulative miles at each.
    #[serde(default)]
    pub cities: Vec<String>,
    #[serde(default)]
    pub city_mi: Vec<f64>,
    /// At the shipper and not loaded yet: a detention call is still open.
    #[serde(default)]
    pub dock_hold: bool,
    /// Minutes of `delay_min` spent at the shipper before the truck rolled.
    #[serde(default)]
    pub dock_min: f64,
    /// Every delay and its cause, so a late result can say why.
    #[serde(default)]
    pub delays: Vec<Delay>,
    /// Empty miles from where the driver sat to the shipper.
    #[serde(default)]
    pub deadhead_mi: f64,
    /// The driver's next load, queued while this one rolls, and when it
    /// was queued (its appointment clock started then).
    #[serde(default, with = "opt_job_json")]
    pub next: Option<Job>,
    #[serde(default)]
    pub next_queued_at: i64,
    /// Wheel minutes already logged to a driver relieved off this trip.
    #[serde(default)]
    pub logged_min: f64,
    /// Whose freight it is; older saves say a broker.
    #[serde(default)]
    pub source: crate::customers::Source,
}

/// How a driver gets a load done: deadhead to the shipper, then the run.
pub struct Plan {
    pub deadhead_mi: f64,
    /// Loaded wheel time, shipper to receiver.
    pub drive_min: f64,
    pub deadhead_min: f64,
    /// Hours from dispatch to delivery, rests included.
    pub need_h: f64,
    pub cities: Vec<String>,
    pub city_mi: Vec<f64>,
}

pub fn plan(job: &Job, driver: &Driver, world: &World) -> Plan {
    let route = route_for(world, job);
    let empty = world
        .supported_route(&driver.location, &job.origin, None)
        .ok()
        .flatten()
        .filter(|_| !driver.location.is_empty() && driver.location != job.origin);
    let (deadhead_mi, deadhead_min) = match &empty {
        Some(r) => (
            r.miles(),
            route_drive_hours(Some(r), 0.0, Some(world)) * 60.0,
        ),
        None => empty_leg(&driver.location, &job.origin, world),
    };
    let mut hos = driver.hos.clone();
    hos.drive(deadhead_min.min(driver.legal_left_min()));
    let mut need_h = deadhead_min / 60.0
        + required_hours(job.distance_mi, route.as_ref(), Some(world), Some(&hos));
    if deadhead_min > driver.legal_left_min() {
        need_h += crate::incidents::RESET_MIN / 60.0;
    }
    let mut cities = Vec::new();
    let mut city_mi = Vec::new();
    for r in empty.iter().chain(route.iter()) {
        let base = city_mi.last().copied().unwrap_or(0.0);
        let skip = usize::from(!cities.is_empty());
        let mut cum = base;
        for (i, c) in r.cities.iter().enumerate().skip(skip) {
            if i > 0 {
                cum += r.legs[i - 1].miles;
            }
            cities.push(c.clone());
            city_mi.push(cum);
        }
    }
    if cities.is_empty() {
        cities = vec![job.origin.clone(), job.destination.clone()];
        city_mi = vec![0.0, job.distance_mi];
    }
    Plan {
        deadhead_mi,
        drive_min: route_drive_hours(route.as_ref(), 0.0, Some(world)) * 60.0,
        deadhead_min,
        need_h,
        cities,
        city_mi,
    }
}

/// Empty miles and wheel minutes between two cities: the road network's
/// route, or straight-line miles times 1.2 at 50 mph when no route is found.
pub fn empty_leg(from: &str, to: &str, world: &World) -> (f64, f64) {
    if from.is_empty() || from == to {
        return (0.0, 0.0);
    }
    if let Some(r) = world.supported_route(from, to, None).ok().flatten() {
        return (
            r.miles(),
            route_drive_hours(Some(&r), 0.0, Some(world)) * 60.0,
        );
    }
    let (Ok(a), Ok(b)) = (world.city(from), world.city(to)) else {
        return (0.0, 0.0);
    };
    let miles = straight_line_mi(a.lat, a.lon, b.lat, b.lon) * 1.2;
    (miles, miles / 50.0 * 60.0)
}

fn straight_line_mi(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 3958.8 * h.sqrt().asin()
}

impl Trip {
    pub fn start(id: u32, driver: &Driver, job: Job, depart: i64, world: &World) -> Trip {
        let route = route_for(world, &job);
        let p = plan(&job, driver, world);
        let drive_min = route_drive_hours(route.as_ref(), 0.0, Some(world)) * 60.0 + p.deadhead_min;
        let plan_min = p.need_h * 60.0;
        let (cities, city_mi) = (p.cities, p.city_mi);
        Trip {
            id,
            driver_id: driver.id,
            job,
            depart,
            drive_min,
            plan_min,
            legal_min: driver.legal_left_min(),
            drive_left_min: driver.drive_left_min(),
            window_left_min: driver.window_left_min(),
            delay_min: 0.0,
            hours_resolved: false,
            hot: false,
            cities,
            city_mi,
            dock_hold: false,
            dock_min: 0.0,
            delays: Vec::new(),
            deadhead_mi: p.deadhead_mi,
            next: None,
            next_queued_at: 0,
            logged_min: 0.0,
            source: crate::customers::Source::default(),
        }
    }

    pub fn deadline(&self) -> i64 {
        self.depart + (self.job.deadline_game_h * 60.0).round() as i64
    }

    pub fn eta(&self) -> i64 {
        self.depart + (self.plan_min + self.delay_min).round() as i64
    }

    pub fn on_time(&self) -> bool {
        self.eta() <= self.deadline()
    }

    pub fn progress(&self, now: i64) -> f64 {
        // Rounded the same way `eta()` rounds its total: `now == eta()` must
        // land on exactly 1.0, or `city_at` undershoots the last city.
        if self.dock_hold {
            return 0.0;
        }
        let rolling = self.depart + self.dock_min.round() as i64;
        let total = (self.plan_min + self.delay_min - self.dock_min)
            .round()
            .max(1.0);
        ((now - rolling) as f64 / total).clamp(0.0, 1.0)
    }

    /// The absolute minute the truck reaches a fraction of the route.
    pub fn at_fraction(&self, f: f64) -> i64 {
        let road = self.plan_min + self.delay_min - self.dock_min;
        self.depart + self.dock_min.round() as i64 + (f.clamp(0.0, 1.0) * road).round() as i64
    }

    /// None when on time; else "late by 2 hours, because of the 4-and-a-half-hour
    /// wait at the shipper", blamed on the biggest delay, or on the plan itself.
    pub fn late_reason(&self) -> Option<String> {
        if self.on_time() {
            return None;
        }
        let late = spoken_duration((self.eta() - self.deadline()) as f64);
        let cause = self
            .delays
            .iter()
            .max_by(|a, b| a.min.total_cmp(&b.min))
            .map(|d| d.cause.clone())
            .unwrap_or_else(|| "an appointment too tight for the run".into());
        Some(format!("late by {late}, because of {cause}"))
    }

    /// "on time", or the late reason.
    pub fn timing_text(&self) -> String {
        self.late_reason().unwrap_or_else(|| "on time".into())
    }

    /// "New arrival 3:10 PM tomorrow, on time." after a delay.
    pub fn eta_line(&self, today: i64) -> String {
        format!(
            "New arrival {}, {}.",
            crate::desk::abs_text(self.eta(), today),
            self.timing_text()
        )
    }

    pub fn fraction_at_city(&self, city: &str) -> f64 {
        let total = self.city_mi.last().copied().unwrap_or(0.0).max(1.0);
        self.cities
            .iter()
            .position(|c| c == city)
            .map(|i| self.city_mi[i] / total)
            .unwrap_or(0.5)
    }

    /// The last route city the truck has passed.
    pub fn city_at(&self, now: i64) -> String {
        let miles = self.progress(now) * self.city_mi.last().copied().unwrap_or(0.0);
        let i = self.city_mi.iter().rposition(|&m| m <= miles).unwrap_or(0);
        self.cities[i].clone()
    }

    /// A run planned inside one shift that the delays have pushed past the
    /// driver's legal hours. Runs whose plan already includes a 10-hour
    /// reset carry their rest in the appointment and never trip this.
    pub fn hours_short(&self) -> bool {
        let rest_planned = self.plan_min >= self.drive_min + 600.0;
        if rest_planned || self.hours_resolved {
            return false;
        }
        // Waits, holds and repairs burn the 14-hour window but not the 11
        // hours of driving; reroute miles are already in `drive_min`.
        let (drive_left, window_left) = if self.window_left_min > 0.0 {
            (self.drive_left_min, self.window_left_min)
        } else {
            (self.legal_min, self.legal_min)
        };
        self.drive_min > drive_left || self.drive_min + self.delay_min > window_left
    }

    /// "Alma Reyes: near Des Moines, 42 percent, due 3:10 PM tomorrow, on time."
    pub fn status_line(&self, driver_name: &str, now: i64, today: i64, world: &World) -> String {
        let dock_called = self.depart + crate::incidents::DOCK_CALL_AFTER_MIN as i64;
        if self.dock_hold && now >= dock_called {
            return format!(
                "{driver_name}: at the shipper in {}, waiting to load {} for {}.",
                world.spoken_city(&self.job.origin, None),
                self.job.cargo.label,
                world.spoken_city(&self.job.destination, None),
            );
        }
        format!(
            "{driver_name}: {} to {}, near {}, {} percent, {} {}, {}.",
            self.job.cargo.label,
            world.spoken_city(&self.job.destination, None),
            world.spoken_city(&self.city_at(now), None),
            (self.progress(now) * 100.0).round(),
            if now >= self.eta() {
                "arrived"
            } else {
                "arriving"
            },
            crate::desk::abs_text(self.eta(), today),
            match self.late_reason() {
                Some(why) => format!("running {why}"),
                None => "on time".into(),
            },
        )
    }
}

mod opt_job_json {
    use ff_core::models::jobs::{job_from_payload, job_payload, Job};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(job: &Option<Job>, s: S) -> Result<S::Ok, S::Error> {
        job.as_ref().map(job_payload).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Job>, D::Error> {
        let map = Option::<serde_json::Map<String, serde_json::Value>>::deserialize(d)?;
        Ok(map.and_then(|m| job_from_payload(&m)))
    }
}

/// Job has no serde; ff-core's own payload form round-trips it.
pub(crate) mod job_json {
    use ff_core::models::jobs::{job_from_payload, job_payload, Job};
    use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(job: &Job, s: S) -> Result<S::Ok, S::Error> {
        job_payload(job).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Job, D::Error> {
        let map = serde_json::Map::deserialize(d)?;
        job_from_payload(&map).ok_or_else(|| D::Error::custom("unreadable job payload"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{morning_board, tests::chicago};
    use ff_core::data::world::get_world;
    use ff_core::models::career::Career;
    use ff_core::models::market::Market;

    fn trip() -> Trip {
        let w = get_world();
        let d = Driver::new(1, "Alma Reyes", Career::with_xp(20_000.0));
        let job = morning_board(
            w,
            &chicago(),
            "northstar",
            &Market::with_seed(1),
            5,
            std::slice::from_ref(&d),
        )
        .remove(0);
        Trip::start(9, &d, job, 1440 + 400, w)
    }

    #[test]
    fn eta_and_on_time_follow_delay() {
        let mut t = trip();
        assert!(t.plan_min >= t.drive_min);
        assert!(t.on_time(), "a fresh legal dispatch starts on time");
        t.delay_min = (t.deadline() - t.eta()) as f64 + 1.0;
        assert!(!t.on_time());
    }

    #[test]
    fn a_late_trip_names_its_biggest_delay() {
        let mut t = trip();
        assert_eq!(t.timing_text(), "on time");
        let slack = (t.deadline() - t.eta()) as f64;
        t.delay_min = slack + 120.0 + 240.0;
        t.delays = vec![
            Delay {
                min: 120.0,
                cause: "the breakdown".into(),
            },
            Delay {
                min: 240.0 + slack,
                cause: "the 4-and-a-half-hour wait at the shipper".into(),
            },
        ];
        assert_eq!(
            t.late_reason().unwrap(),
            "late by 6 hours, because of the 4-and-a-half-hour wait at the shipper"
        );
        assert!(t
            .eta_line(0)
            .ends_with("late by 6 hours, because of the 4-and-a-half-hour wait at the shipper."));
    }

    #[test]
    fn position_walks_the_route() {
        let t = trip();
        assert_eq!(t.city_at(t.depart), t.cities[0]);
        assert_eq!(&t.city_at(t.eta()), t.cities.last().unwrap());
        assert_eq!(t.progress(t.depart - 50), 0.0);
        assert!(t.at_fraction(0.5) > t.depart && t.at_fraction(0.5) < t.eta());
    }

    #[test]
    fn hours_short_only_when_delay_breaks_a_single_shift_plan() {
        let mut t = trip();
        t.plan_min = t.drive_min + 30.0;
        t.legal_min = t.drive_min + 60.0;
        t.drive_left_min = t.legal_min;
        t.window_left_min = t.legal_min;
        assert!(!t.hours_short());
        t.delay_min = 90.0;
        assert!(t.hours_short());
        t.hours_resolved = true;
        assert!(!t.hours_short());
        t.hours_resolved = false;
        t.plan_min = t.drive_min + 700.0;
        assert!(!t.hours_short(), "a planned reset covers delays");
    }

    #[test]
    fn a_dock_wait_burns_the_window_not_the_driving_hours() {
        let mut t = trip();
        t.drive_min = 420.0;
        t.plan_min = 420.0;
        t.legal_min = 660.0;
        t.drive_left_min = 660.0;
        t.window_left_min = 840.0;
        t.delay_min = 270.0;
        assert!(
            !t.hours_short(),
            "7 hours of driving fits 9.5 hours of window"
        );
        t.delay_min = 450.0;
        assert!(t.hours_short(), "the window runs out");
        t.delay_min = 0.0;
        t.drive_min = 700.0;
        assert!(t.hours_short(), "reroute miles count against the 11 hours");
    }

    #[test]
    fn a_truck_check_keeps_the_dock_wait_quiet_until_the_call() {
        let w = get_world();
        let mut t = trip();
        t.dock_hold = true;
        assert!(!t
            .status_line("Alma", t.depart + 10, 1, w)
            .contains("at the shipper"));
        assert!(t
            .status_line("Alma", t.depart + 30, 1, w)
            .contains("at the shipper"));
    }

    #[test]
    fn status_line_tense_follows_arrival() {
        let t = trip();
        let w = get_world();
        assert!(t.status_line("Alma", t.depart, 1, w).contains(" arriving "));
        assert!(t.status_line("Alma", t.eta(), 1, w).contains(" arrived "));
    }

    #[test]
    fn trip_round_trips_through_json() {
        let t = trip();
        let back: Trip = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.job, t.job);
        assert_eq!(back.eta(), t.eta());
    }
}
