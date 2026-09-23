//! Drivers on the roster: an ff-core career and a real HOS clock each.

use ff_core::models::career::Career;
use ff_core::pyrandom::PyRandom;
use ff_core::sim::hos::HosClock;
use serde::{Deserialize, Serialize};

pub const GOODWILL_START: i32 = 60;
const DRIVE_LIMIT_MIN: f64 = 11.0 * 60.0;
const WINDOW_LIMIT_MIN: f64 = 14.0 * 60.0;

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
}

impl Status {
    pub fn text(self) -> &'static str {
        match self {
            Status::Idle => "idle",
            Status::OnLoad => "on a load",
            Status::OffDuty => "off duty today",
            Status::OutOnRun => "out on a multi-day run",
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

    /// Wheel time left before the 11-hour or 14-hour limit, whichever is first.
    pub fn legal_left_min(&self) -> f64 {
        (DRIVE_LIMIT_MIN - self.hos.driving_min)
            .min(WINDOW_LIMIT_MIN - self.hos.duty_min)
            .max(0.0)
    }

    pub fn adjust_goodwill(&mut self, delta: i32) {
        self.goodwill = (self.goodwill + delta).clamp(0, 100);
    }

    pub fn card(&self) -> String {
        format!(
            "{}, level {}, {}, {} left, goodwill {}.",
            self.name,
            self.level(),
            self.status.text(),
            crate::hours(self.legal_left_min()),
            self.goodwill
        )
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
    fn goodwill_clamps() {
        let mut d = Driver::new(1, "Alma Reyes", Career::new());
        d.adjust_goodwill(100);
        assert_eq!(d.goodwill, 100);
        d.adjust_goodwill(-500);
        assert_eq!(d.goodwill, 0);
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
