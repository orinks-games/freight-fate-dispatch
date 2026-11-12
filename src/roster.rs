//! Drivers on the roster: an ff-core career and a real HOS clock each.

use ff_core::models::career::endorsement_label_spoken;
use ff_core::models::career::Career;
use ff_core::models::credentials::{credential, credential_keys};
use ff_core::pyrandom::PyRandom;
use ff_core::sim::hos::{HosClock, REALISTIC_LIMITS};
use serde::{Deserialize, Serialize};

pub const GOODWILL_START: i32 = 60;
/// Morale states, by goodwill: solid, fine, grumbling, done.
pub const MORALE_SOLID: i32 = 75;
pub const MORALE_FINE: i32 = 45;
pub const MORALE_GRUMBLING: i32 = 25;
/// A driver who is done gives this much notice, in days.
pub const NOTICE_DAYS: i64 = 14;
/// Traits reveal faster for this many days after hiring.
pub const PROBATION_DAYS: i64 = 90;
/// Odds a delivery reveals one unknown trait, on and off probation.
pub const REVEAL_ODDS: f64 = 0.2;
pub const REVEAL_ODDS_PROBATION: f64 = 0.4;

/// What a driver is like, 1 to 5, learned by working with them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traits {
    /// Call-offs and showing up.
    pub reliability: u8,
    /// Speed against the plan: 5 is fast, and gets stopped for it.
    pub pace: u8,
    /// Inspection results and claims.
    pub care: u8,
    /// Calls in with problems early, or doesn't.
    pub communication: u8,
    /// How much they take before morale drops.
    pub patience: u8,
}

impl Default for Traits {
    fn default() -> Self {
        Traits {
            reliability: 3,
            pace: 3,
            care: 3,
            communication: 3,
            patience: 3,
        }
    }
}

pub const TRAIT_NAMES: [&str; 5] = ["reliability", "pace", "care", "communication", "patience"];

impl Traits {
    pub fn seeded(rng: &mut PyRandom) -> Traits {
        let mut roll = || rng.randint(1, 5) as u8;
        Traits {
            reliability: roll(),
            pace: roll(),
            care: roll(),
            communication: roll(),
            patience: roll(),
        }
    }

    pub fn get(&self, name: &str) -> u8 {
        match name {
            "reliability" => self.reliability,
            "pace" => self.pace,
            "care" => self.care,
            "communication" => self.communication,
            _ => self.patience,
        }
    }

    /// What the dispatcher learns, as a predicate after the driver's name:
    /// "is careful; inspections come back clean".
    pub fn describe(name: &str, value: u8) -> &'static str {
        match (name, value) {
            ("reliability", 1..=2) => "calls off more than most",
            ("reliability", 3) => "shows up about as often as anyone",
            ("reliability", _) => "shows up every time",
            ("pace", 1..=2) => "runs slow against the plan",
            ("pace", 3) => "runs right on the plan",
            ("pace", _) => "is fast on the road, and it gets noticed at the scales",
            ("care", 1..=2) => "is rough on the equipment",
            ("care", 3) => "is average on the equipment",
            ("care", _) => "is careful; inspections come back clean",
            ("communication", 1..=2) => "goes quiet when there's a problem",
            ("communication", 3) => "calls when asked",
            ("communication", _) => "calls early when something's wrong",
            ("patience", 1..=2) => "sours quickly on a bad week",
            ("patience", 3) => "takes a bad week like most drivers",
            (_, _) => "takes a lot before it shows",
        }
    }
}

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

/// The freight a driver asked not to get, said up front at hiring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Preference {
    NoNortheast,
    NoReefer,
    LongRunsOnly,
}

/// Odds a driver has a preference at all.
pub const PREFERENCE_ODDS: f64 = 0.4;
/// A run shorter than this is not a long run.
pub const LONG_RUN_MI: f64 = 300.0;
/// Goodwill a load against a driver's preference costs.
pub const PREFERENCE_GOODWILL: i32 = 2;
/// Loaded miles in a week under which a driver calls it thin, and over
/// which they call it too much.
pub const THIN_WEEK_MI: f64 = 1_000.0;
pub const HEAVY_WEEK_MI: f64 = 3_000.0;

impl Preference {
    /// Who the driver is doesn't change: the same name asks for the same thing.
    pub fn seeded(seed: i64, name: &str) -> Option<Preference> {
        let mut rng = crate::rng(seed, 0, &format!("preference:{name}"));
        if rng.random() >= PREFERENCE_ODDS {
            return None;
        }
        Some(match rng.randint(0, 2) {
            0 => Preference::NoNortheast,
            1 => Preference::NoReefer,
            _ => Preference::LongRunsOnly,
        })
    }

    /// "no Northeast runs", after "asked for".
    pub fn text(self) -> &'static str {
        match self {
            Preference::NoNortheast => "no Northeast runs",
            Preference::NoReefer => "no refrigerated freight",
            Preference::LongRunsOnly => "long runs only, 300 miles or more",
        }
    }

    /// Whether a load is the kind the driver asked not to get.
    pub fn against(
        self,
        job: &ff_core::models::jobs::Job,
        world: &ff_core::data::world::World,
    ) -> bool {
        match self {
            Preference::NoNortheast => world
                .city(&job.destination)
                .is_ok_and(|c| c.region == "northeast"),
            Preference::NoReefer => job.cargo.needs_reefer(),
            Preference::LongRunsOnly => job.distance_mi < LONG_RUN_MI,
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
    /// Absolute minute the hours clock is logged up to.
    #[serde(default)]
    pub hos_at: i64,
    /// Hidden until learned; None in older saves (seeded on load).
    #[serde(default)]
    pub traits: Option<Traits>,
    /// Trait names the dispatcher has learned.
    #[serde(default)]
    pub known: Vec<String>,
    #[serde(default)]
    pub record: crate::law::DriverRecord,
    /// Career day hired; 0 for the starting roster.
    #[serde(default)]
    pub hired_day: i64,
    /// Career day the driver leaves after giving notice; 0 for none.
    #[serde(default)]
    pub leaving_day: i64,
    /// Freight they asked not to get. `None` with `preference_set` is a
    /// driver who takes anything.
    #[serde(default)]
    pub prefers: Option<Preference>,
    #[serde(default)]
    pub preference_set: bool,
    /// Loaded miles since Monday, for Sunday's look at the week. Negative
    /// in a save from before miles were counted, until its first Sunday.
    #[serde(default = "miles_not_counted")]
    pub week_miles: f64,
}

fn miles_not_counted() -> f64 {
    -1.0
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
            hos_at: 0,
            traits: None,
            known: Vec::new(),
            record: crate::law::DriverRecord::default(),
            hired_day: 0,
            leaving_day: 0,
            prefers: None,
            preference_set: false,
            week_miles: 0.0,
        }
    }

    pub fn traits(&self) -> Traits {
        self.traits.unwrap_or_default()
    }

    /// Seed traits an older save never had, from the career seed and the id.
    pub fn ensure_traits(&mut self, seed: i64) {
        if self.traits.is_none() {
            let mut rng = crate::rng(seed, self.id as i64, "traits");
            self.traits = Some(Traits::seeded(&mut rng));
        }
        if !self.preference_set {
            self.preference_set = true;
            self.prefers = Preference::seeded(seed, &self.name)
                // A driver home every night doesn't ask for long runs.
                .filter(|p| {
                    *p != Preference::LongRunsOnly || self.home != Some(HomePattern::Local)
                });
        }
    }

    /// Whether a load is one this driver asked not to get.
    pub fn dislikes(
        &self,
        job: &ff_core::models::jobs::Job,
        world: &ff_core::data::world::World,
    ) -> bool {
        self.prefers.is_some_and(|p| p.against(job, world))
    }

    /// "regional, home on weekends. Asked for no Northeast runs."
    pub fn wants_text(&self) -> String {
        match self.prefers {
            Some(p) => format!("{}. Asked for {}.", self.pattern().text(), p.text()),
            None => format!("{}.", self.pattern().text()),
        }
    }

    /// "solid", "fine", "grumbling", "done". A patient driver stays fine
    /// on less goodwill; an impatient one sours sooner.
    pub fn morale(&self) -> &'static str {
        match self.goodwill + 5 * (self.traits().patience as i32 - 3) {
            g if g >= MORALE_SOLID => "solid",
            g if g >= MORALE_FINE => "fine",
            g if g >= MORALE_GRUMBLING => "grumbling",
            _ => "done",
        }
    }

    pub fn on_probation(&self, today: i64) -> bool {
        self.hired_day > 0 && today - self.hired_day < PROBATION_DAYS
    }

    /// Learn one trait the dispatcher doesn't know yet: "Wes Amato is fast
    /// on the road."
    pub fn reveal(&mut self, name: &str) -> Option<String> {
        if self.known.iter().any(|k| k == name) {
            return None;
        }
        self.known.push(name.to_string());
        Some(format!(
            "You've learned that {} {}.",
            self.name,
            Traits::describe(name, self.traits().get(name))
        ))
    }

    /// Learn an unknown trait at random, with `odds`.
    pub fn reveal_random(&mut self, rng: &mut PyRandom, odds: f64) -> Option<String> {
        let unknown: Vec<&str> = TRAIT_NAMES
            .iter()
            .copied()
            .filter(|n| !self.known.iter().any(|k| k == n))
            .collect();
        if unknown.is_empty() || rng.random() >= odds {
            return None;
        }
        let pick = unknown[rng.randint(0, unknown.len() as i64 - 1) as usize];
        self.reveal(pick)
    }

    /// What the dispatcher knows: "Known: shows up every time; fast on the
    /// road. Three traits still to learn."
    pub fn traits_text(&self) -> String {
        let known: Vec<&str> = TRAIT_NAMES
            .iter()
            .copied()
            .filter(|n| self.known.iter().any(|k| k == n))
            .map(|n| Traits::describe(n, self.traits().get(n)))
            .collect();
        let left = TRAIT_NAMES.len() - known.len();
        let mut s = if known.is_empty() {
            "Nothing learned yet".to_string()
        } else {
            format!("{} {}", self.name, known.join("; "))
        };
        match left {
            0 => s.push_str(". You know them well."),
            1 => s.push_str(". One trait still to learn."),
            n => s.push_str(&format!(". {n} traits still to learn.")),
        }
        s
    }

    /// Speed against the plan: pace 1 runs 6 percent slow, pace 5 runs 6
    /// percent fast.
    pub fn pace_factor(&self) -> f64 {
        1.0 - 0.03 * (self.traits().pace as f64 - 3.0)
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

    /// The 14-hour window runs on the wall clock: once a driver has gone on
    /// duty, time spent free and waiting still uses it up (as off duty).
    pub fn clock_to(&mut self, now: i64) {
        let free = self.status == Status::Idle && self.rest_note.is_none();
        let from = self.hos_at.max(self.available_at);
        if free && self.hos.duty_min > 0.0 && now > from {
            self.hos.off_duty((now - from) as f64);
        }
        self.hos_at = self.hos_at.max(now);
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
    /// time: "Wes Amato, in Gary, available at 3:40 PM after a 10-hour
    /// break, home due in 3 days, morale fine, experience level 2."
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
            "{},{place}, {state}, {}, morale {}, experience level {}.",
            self.name,
            self.home_text(today),
            self.morale(),
            self.level()
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
            let mut d = Driver::new(
                first_id + i as u32,
                name,
                Career::with_xp(rng.randint(0, 2500) as f64),
            );
            d.traits = Some(Traits::seeded(rng));
            d
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
