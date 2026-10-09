//! Drivers on the roster: an ff-core career and a real HOS clock each.

use ff_core::models::career::endorsement_label_spoken;
use ff_core::models::career::Career;
use ff_core::models::credentials::{credential, credential_keys};
use ff_core::pyrandom::PyRandom;
use ff_core::sim::hos::{HosClock, REALISTIC_LIMITS};
use serde::{Deserialize, Serialize};

pub const GOODWILL_START: i32 = 60;

pub const NAMES: &[&str] = &[
    "Alma Reyes",
    "Bo Whitaker",
    "Casey Lin",
    "Dee Okafor",
    "Eli Brandt",
    "Faye Monroe",
    "Gus Tran",
    "Hattie Cole",
    "Ike Serrano",
    "Jo Petrov",
    "Kit Mercer",
    "Lou Abara",
    "Mack Dunn",
    "Nell Ibarra",
    "Otis Grant",
    "Pru Salas",
    "Quinn Harlow",
    "Rae Wojcik",
    "Sal Mendez",
    "Tess Oduya",
    "Ugo Farris",
    "Val Kincaid",
    "Wes Amato",
    "Xena Pruitt",
    "Yuri Bell",
    "Zed Carver",
    "Ada Nwosu",
    "Bram Keller",
    "Cora Vance",
    "Dom Rinaldi",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    #[default]
    Idle,
    OnLoad,
    OffDuty,
    OutOnRun,
    /// At the receiver for the 30-minute unload; can be pre-planned.
    Unloading,
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::OnLoad => "on a load",
            Status::OffDuty => "not driving today",
            Status::OutOnRun => "out on a multi-day run",
            Status::Unloading => "unloading",
        }
    }
}

/// How often a driver is promised home time at the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HomePattern {
    Local,
    Regional,
    OverTheRoad,
}

/// Career day 1 is a Monday; 0 = Monday .. 6 = Sunday.
pub fn weekday(day: i64) -> i64 {
    (day - 1).rem_euclid(7)
}

pub const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

impl HomePattern {
    /// The career day home time is due for a driver who left home on
    /// `left`: local the same night, regional the next Friday, over the
    /// road two to three weeks out.
    pub fn due_day(self, left: i64, id: u32) -> i64 {
        match self {
            HomePattern::Local => left,
            HomePattern::Regional => left + 1 + (4 - weekday(left + 1)).rem_euclid(7),
            HomePattern::OverTheRoad => left + 14 + (id % 8) as i64,
        }
    }

    /// Minutes of home time owed on arriving home after `days_out` days:
    /// a night for local, a 34-hour weekend for regional, about a day per
    /// week out (two to four days) over the road.
    pub fn home_min(self, days_out: i64) -> i64 {
        match self {
            HomePattern::Local => 600,
            HomePattern::Regional => 34 * 60,
            HomePattern::OverTheRoad => ((days_out + 6) / 7).clamp(2, 4) * 24 * 60,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            HomePattern::Local => "local, home every night",
            HomePattern::Regional => "regional, home on weekends",
            HomePattern::OverTheRoad => "over the road, out two to three weeks",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Driver {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub career: Career,
    #[serde(with = "hos_json", default = "HosClock::new")]
    pub hos: HosClock,
    #[serde(default)]
    pub status: Status,
    #[serde(default = "goodwill_start")]
    pub goodwill: i32,
    /// City key where the truck sits; empty in old saves (the terminal).
    #[serde(default)]
    pub location: String,
    /// Absolute minute the driver can take a load; 0 means now.
    #[serde(default)]
    pub available_at: i64,
    /// Why the driver is off until `available_at`: "a 10-hour break".
    #[serde(default)]
    pub rest_note: Option<String>,
    #[serde(default)]
    pub home: Option<HomePattern>,
    /// Career day the driver was last home; 0 in old saves (today).
    #[serde(default)]
    pub last_home_day: i64,
    /// The driver has already called to warn they'll quit.
    #[serde(default)]
    pub quit_warned: bool,
    /// Heading home or at home: the minute they reach the terminal; 0 otherwise.
    #[serde(default)]
    pub home_at: i64,
    /// Promised home: head there when the current load delivers.
    #[serde(default)]
    pub home_next: bool,
}

fn goodwill_start() -> i32 {
    GOODWILL_START
}

impl Driver {
    pub fn new(id: u32, name: &str, career: Career) -> Self {
        Driver {
            id,
            name: name.to_string(),
            career,
            hos: HosClock::new(),
            status: Status::Idle,
            goodwill: GOODWILL_START,
            location: String::new(),
            available_at: 0,
            rest_note: None,
            home: None,
            last_home_day: 0,
            quit_warned: false,
            home_at: 0,
            home_next: false,
        }
    }

    pub fn pattern(&self) -> HomePattern {
        self.home.unwrap_or(HomePattern::Regional)
    }

    /// The career day home time is due.
    pub fn home_due_day(&self) -> i64 {
        self.pattern().due_day(self.last_home_day, self.id)
    }

    /// Given home time: headed there or there now.
    pub fn home_given(&self) -> bool {
        self.home_at > 0
    }

    /// "home due today", "home due in 3 days", "home time overdue".
    pub fn home_text(&self, today: i64) -> String {
        match self.home_due_day() - today {
            n if n < 0 => "home time overdue".into(),
            0 => "home due tonight".into(),
            1 => "home due tomorrow night".into(),
            n => format!("home due in {n} days"),
        }
    }

    pub fn level(&self) -> i64 {
        self.career.level()
    }

    pub fn endorsements(&self) -> Vec<String> {
        self.career
            .endorsements()
            .into_iter()
            .map(String::from)
            .collect()
    }

    pub fn credential_lines(&self) -> Vec<String> {
        let held = self.career.endorsements();
        let cdl_keys = ["hazmat", "tank", "doubles_triples"];
        let endorsements: Vec<&str> = cdl_keys
            .iter()
            .filter(|key| held.contains(**key))
            .filter_map(|key| endorsement_label_spoken(key))
            .collect();
        let other: Vec<&str> = credential_keys()
            .filter(|key| !cdl_keys.contains(key) && held.contains(key))
            .filter_map(|key| {
                if key == "lcv" {
                    Some("long combination vehicle certificate")
                } else {
                    credential(key).map(|credential| credential.gate_label)
                }
            })
            .collect();
        let mut lines = Vec::new();
        if !endorsements.is_empty() {
            lines.push(format!("CDL endorsements: {}.", endorsements.join(", ")));
        }
        if !other.is_empty() {
            lines.push(format!("Other credentials: {}.", other.join(", ")));
        }
        if lines.is_empty() {
            lines.push("No endorsements or other credentials.".into());
        }
        lines
    }

    /// Wheel time left before the 11-hour drive or 14-hour window limit,
    /// whichever binds first, before a 10-hour break is due. This is not
    /// trip time: the 30-minute break rule is deliberately left out here,
    /// since `ff_core`'s `plan_hos` already plans that break into a trip.
    pub fn legal_left_min(&self) -> f64 {
        let (drive_limit, window_limit, _break_after) = REALISTIC_LIMITS;
        (drive_limit - self.hos.driving_min)
            .min(window_limit - self.hos.duty_min)
            .max(0.0)
    }

    pub fn drive_left_min(&self) -> f64 {
        (REALISTIC_LIMITS.0 - self.hos.driving_min).max(0.0)
    }

    pub fn window_left_min(&self) -> f64 {
        (REALISTIC_LIMITS.1 - self.hos.duty_min).max(0.0)
    }

    /// Log `minutes` of driving, taking a 10-hour break whenever the 11 or
    /// 14 runs out, the way a multi-day run is really logged.
    pub fn drive_with_breaks(&mut self, mut minutes: f64) {
        while minutes > 0.0 {
            let allow = self.legal_left_min();
            if allow <= 0.0 {
                self.hos.sleep();
                continue;
            }
            let m = minutes.min(allow);
            self.hos.drive(m);
            minutes -= m;
        }
    }

    pub fn adjust_goodwill(&mut self, delta: i32) {
        self.goodwill = (self.goodwill + delta).clamp(0, 100);
    }

    pub fn card(&self) -> String {
        let hours = match self.status {
            Status::OnLoad | Status::OutOnRun => String::new(),
            _ => format!(", {} left", crate::hours(self.legal_left_min())),
        };
        format!(
            "{}, level {}, {}{hours}, goodwill {}.",
            self.name,
            self.level(),
            self.status.text(),
            self.goodwill
        )
    }

    /// The card with where the truck is, when the driver is free, and home
    /// time: "Wes Amato, level 2, in Gary, available at 3:40 PM after a
    /// 10-hour break, goodwill 60, home due in 3 days."
    pub fn card_at(&self, world: &ff_core::data::world::World, now: i64, today: i64) -> String {
        let place = if self.location.is_empty() || self.status == Status::OnLoad {
            String::new()
        } else {
            format!(" in {}", world.spoken_city(&self.location, None))
        };
        let state = match (self.status, &self.rest_note) {
            (Status::OnLoad | Status::OutOnRun, _) => self.status.text().to_string(),
            (_, Some(_)) if self.home_at > now => format!(
                "headed home, home about {}",
                crate::desk::abs_text(self.home_at, today)
            ),
            (Status::Unloading, _) => format!(
                "unloading until {}",
                crate::desk::abs_text(self.available_at, today)
            ),
            (_, Some(note)) if self.available_at > now => format!(
                "available at {} after {note}",
                crate::desk::abs_text(self.available_at, today)
            ),
            (Status::Idle, _) => format!("idle, {} left", crate::hours(self.legal_left_min())),
            (status, _) => status.text().to_string(),
        };
        format!(
            "{},{place}, level {}, {state}, goodwill {}, {}.",
            self.name,
            self.level(),
            self.goodwill,
            self.home_text(today)
        )
        .replace(",,", ",")
    }
}

/// `count` new drivers with names not in `taken`, ids from `first_id`.
pub fn seeded_drivers(
    rng: &mut PyRandom,
    first_id: u32,
    count: usize,
    taken: &[String],
) -> Vec<Driver> {
    let mut pool: Vec<&str> = NAMES
        .iter()
        .copied()
        .filter(|n| !taken.iter().any(|t| t == n))
        .collect();
    rng.shuffle(&mut pool);
    pool.into_iter()
        .take(count)
        .enumerate()
        .map(|(i, name)| {
            Driver::new(
                first_id + i as u32,
                name,
                Career::with_xp(rng.randint(0, 2500) as f64),
            )
        })
        .collect()
}

/// HosClock has no serde; it persists through its own tolerant dict form.
mod hos_json {
    use ff_core::sim::hos::HosClock;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(hos: &HosClock, s: S) -> Result<S::Ok, S::Error> {
        hos.to_dict().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<HosClock, D::Error> {
        Ok(HosClock::from_dict(&serde_json::Value::deserialize(d)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legal_hours_shrink_with_driving_and_window() {
        let mut d = Driver::new(1, "Alma Reyes", Career::new());
        assert_eq!(d.legal_left_min(), 660.0);
        d.hos.drive(300.0);
        assert_eq!(d.legal_left_min(), 360.0);
    }

    #[test]
    fn a_working_drivers_card_leaves_out_hours() {
        let mut d = Driver::new(1, "Alma Reyes", Career::new());
        assert!(d.card().contains("11 hours left"), "{}", d.card());
        for status in [Status::OnLoad, Status::OutOnRun] {
            d.status = status;
            assert!(!d.card().contains("left"), "{}", d.card());
        }
    }

    #[test]
    fn goodwill_clamps() {
        let mut d = Driver::new(1, "Alma Reyes", Career::new());
        d.adjust_goodwill(100);
        assert_eq!(d.goodwill, 100);
        d.adjust_goodwill(-500);
        assert_eq!(d.goodwill, 0);
    }

    #[test]
    fn credential_lines_group_endorsements_and_keep_other_catalog_order() {
        let mut career = Career::new();
        career.purchased_endorsements = vec![
            "hazmat".into(),
            "tank".into(),
            "doubles_triples".into(),
            "twic".into(),
            "refrigerated".into(),
            "lcv".into(),
        ];
        let d = Driver::new(1, "Alma Reyes", career);
        assert_eq!(
            d.credential_lines(),
            vec![
                "CDL endorsements: hazmat, tank vehicle, doubles.",
                "Other credentials: refrigerated certificate, TWIC port card, long combination vehicle certificate."
            ]
        );
    }

    #[test]
    fn credential_lines_explain_when_no_credentials_are_held() {
        let d = Driver::new(1, "Alma Reyes", Career::new());
        assert_eq!(
            d.credential_lines(),
            vec!["No endorsements or other credentials."]
        );
    }

    #[test]
    fn driver_round_trips_with_hos_and_tolerates_missing_fields() {
        let mut d = Driver::new(4, "Bo Whitaker", Career::with_xp(900.0));
        d.hos.drive(125.0);
        let back: Driver = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back.hos.driving_min, 125.0);
        assert_eq!(back.career, d.career);
        let bare: Driver =
            serde_json::from_str(r#"{"id":2,"name":"Casey Lin","extra":true}"#).unwrap();
        assert_eq!(bare.goodwill, GOODWILL_START);
        assert_eq!(bare.status, Status::Idle);
    }

    #[test]
    fn seeded_drivers_are_deterministic_and_skip_taken_names() {
        let taken = vec!["Alma Reyes".to_string()];
        let a = seeded_drivers(&mut crate::rng(3, 0, "roster"), 1, 3, &taken);
        let b = seeded_drivers(&mut crate::rng(3, 0, "roster"), 1, 3, &taken);
        let names: Vec<_> = a.iter().map(|d| d.name.clone()).collect();
        assert_eq!(names, b.iter().map(|d| d.name.clone()).collect::<Vec<_>>());
        assert!(!names.contains(&"Alma Reyes".to_string()));
        assert_eq!(a.iter().map(|d| d.id).collect::<Vec<_>>(), vec![1, 2, 3]);
    }
}
