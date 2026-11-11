//! The law, measured in game days: roadside inspections, citations, out of
//! service orders, CDL status, medical cards, drug tests, and the carrier's
//! safety score.

use serde::{Deserialize, Serialize};

use crate::roster::Driver;

/// Odds a trip meets a scale or a patrol at all, before care, the safety
/// score and running over hours move it.
pub const INSPECTION_BASE: f64 = 0.08;
/// A driver dispatched past their clock is stopped this much more often:
/// the log is the first thing a scale looks at.
pub const OVER_HOURS_INSPECTION: f64 = 0.25;
pub const LEVEL_1_MIN: f64 = 60.0;
pub const LEVEL_2_MIN: f64 = 30.0;
pub const LEVEL_3_MIN: f64 = 20.0;
/// An hours violation found at a scale: 10 hours out of service on the spot.
pub const HOURS_OOS_MIN: f64 = 600.0;
/// An equipment out-of-service order holds the truck for the repair.
pub const EQUIPMENT_OOS_MIN: f64 = 180.0;
pub const EQUIPMENT_OOS_COST: f64 = 450.0;
/// Odds a trip draws a speeding stop, per point of pace over 3.
pub const SPEEDING_PER_PACE: f64 = 0.04;
/// A citation stays on the driver's record for 3 years.
pub const DRIVER_RECORD_DAYS: i64 = 3 * 365;
/// Points count on the carrier's score for 2 years, at half weight after
/// the first.
pub const CARRIER_FULL_DAYS: i64 = 365;
pub const CARRIER_SCORE_DAYS: i64 = 2 * 365;
/// Over this the carrier is audited and customers get nervous.
pub const AUDIT_THRESHOLD: f64 = 30.0;
/// An audit with hours citations on the record costs this.
pub const AUDIT_FINE: f64 = 2_000.0;
/// Two serious violations in 3 years suspend a CDL for 60 days; three, 120.
pub const SUSPENSION_DAYS: [i64; 2] = [60, 120];
/// Medical cards last 2 years; renewing one parks the driver for a day.
pub const MEDICAL_CARD_DAYS: i64 = 2 * 365;
/// One driver a quarter draws a random drug test and loses the day.
pub const DRUG_TEST_EVERY_DAYS: i64 = 90;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    UnsafeDriving,
    Hours,
    DriverFitness,
    Substances,
    VehicleMaintenance,
    Hazmat,
    Crashes,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::UnsafeDriving,
        Category::Hours,
        Category::DriverFitness,
        Category::Substances,
        Category::VehicleMaintenance,
        Category::Hazmat,
        Category::Crashes,
    ];

    pub fn text(self) -> &'static str {
        match self {
            Category::UnsafeDriving => "unsafe driving",
            Category::Hours => "hours of service",
            Category::DriverFitness => "driver fitness",
            Category::Substances => "drugs and alcohol",
            Category::VehicleMaintenance => "vehicle maintenance",
            Category::Hazmat => "hazardous materials",
            Category::Crashes => "crashes",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Violation {
    Speeding,
    /// Logbook form and manner: a sloppy log, no violation of the hours.
    Logbook,
    /// Driving past the clock.
    Hours,
    /// A lamp, a tire, a brake adjustment.
    Equipment,
}

impl Violation {
    pub fn text(self) -> &'static str {
        match self {
            Violation::Speeding => "speeding",
            Violation::Logbook => "a logbook form-and-manner violation",
            Violation::Hours => "an hours-of-service violation",
            Violation::Equipment => "an equipment violation",
        }
    }

    pub fn category(self) -> Category {
        match self {
            Violation::Speeding => Category::UnsafeDriving,
            Violation::Logbook | Violation::Hours => Category::Hours,
            Violation::Equipment => Category::VehicleMaintenance,
        }
    }

    pub fn points(self) -> f64 {
        match self {
            Violation::Speeding => 5.0,
            Violation::Logbook => 2.0,
            Violation::Hours => 7.0,
            Violation::Equipment => 4.0,
        }
    }

    pub fn fine(self) -> f64 {
        match self {
            Violation::Speeding => 250.0,
            Violation::Logbook => 150.0,
            Violation::Hours => 400.0,
            Violation::Equipment => 200.0,
        }
    }

    /// The driver pays moving and log violations; the carrier pays for
    /// its equipment.
    pub fn carrier_pays(self) -> bool {
        self == Violation::Equipment
    }
}

/// One citation on a driver's record and the carrier's score.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub day: i64,
    pub violation: Violation,
    /// Counts toward a CDL suspension.
    pub serious: bool,
}

/// A driver's legal standing, kept on the roster.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriverRecord {
    pub citations: Vec<Citation>,
    pub inspections: u32,
    pub clean_inspections: u32,
    /// Career day the CDL is suspended until; 0 when it isn't.
    pub suspended_until: i64,
    /// Career day the medical card was issued; 0 means a fresh card on hire.
    pub medical_card_day: i64,
    pub suspensions: u32,
}

impl DriverRecord {
    /// Citations still on the record on `today`.
    pub fn active(&self, today: i64) -> impl Iterator<Item = &Citation> {
        self.citations
            .iter()
            .filter(move |c| today - c.day < DRIVER_RECORD_DAYS)
    }

    pub fn serious_count(&self, today: i64) -> usize {
        self.active(today).filter(|c| c.serious).count()
    }

    pub fn suspended(&self, today: i64) -> bool {
        self.suspended_until > today
    }

    pub fn medical_card_expired(&self, today: i64) -> bool {
        today - self.medical_card_day >= MEDICAL_CARD_DAYS
    }

    /// "Clean record." or "2 citations in 3 years: speeding, an equipment
    /// violation. 4 inspections, 3 clean."
    pub fn text(&self, today: i64) -> String {
        let active: Vec<&Citation> = self.active(today).collect();
        let mut s = if active.is_empty() {
            "Clean record".to_string()
        } else {
            format!(
                "{} in 3 years: {}",
                match active.len() {
                    1 => "1 citation".to_string(),
                    n => format!("{n} citations"),
                },
                active
                    .iter()
                    .map(|c| c.violation.text())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        if self.inspections > 0 {
            s.push_str(&format!(
                ". {} inspections, {} clean",
                self.inspections, self.clean_inspections
            ));
        }
        if self.suspended(today) {
            s.push_str(&format!(
                ". CDL suspended for {} more days",
                self.suspended_until - today
            ));
        }
        s.push('.');
        s
    }

    /// Add a citation; returns the suspension in days it triggers, if any.
    pub fn cite(&mut self, c: Citation, today: i64) -> Option<i64> {
        self.citations.push(c);
        let serious = self.serious_count(today);
        if serious >= 2 && !self.suspended(today) {
            let days = SUSPENSION_DAYS[(serious - 2).min(1)];
            self.suspended_until = today + days;
            self.suspensions += 1;
            Some(days)
        } else {
            None
        }
    }
}

/// A seeded history for an applicant: the sloppier the driver, the more
/// on the record.
pub fn seeded_record(
    rng: &mut ff_core::pyrandom::PyRandom,
    d: &Driver,
    today: i64,
) -> DriverRecord {
    let t = d.traits();
    let mut r = DriverRecord {
        medical_card_day: today - rng.randint(0, MEDICAL_CARD_DAYS - 30),
        ..DriverRecord::default()
    };
    let inspections = rng.randint(0, 6) as u32;
    r.inspections = inspections;
    let mut clean = 0;
    for _ in 0..inspections {
        if rng.random() < 0.55 + 0.1 * (t.care as f64 - 3.0) {
            clean += 1;
        } else {
            r.citations.push(Citation {
                day: today - rng.randint(1, DRIVER_RECORD_DAYS - 1),
                violation: if rng.random() < 0.5 {
                    Violation::Equipment
                } else {
                    Violation::Logbook
                },
                serious: false,
            });
        }
    }
    r.clean_inspections = clean;
    for _ in 0..(t.pace as i64 - 3).max(0) {
        if rng.random() < 0.4 {
            r.citations.push(Citation {
                day: today - rng.randint(1, DRIVER_RECORD_DAYS - 1),
                violation: Violation::Speeding,
                serious: rng.random() < 0.3,
            });
        }
    }
    r
}

/// Points against the carrier, each with the day they were earned.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyScore {
    pub points: Vec<(i64, Category, f64)>,
    /// Career day of the last audit; 0 for none.
    pub audited_day: i64,
}

impl SafetyScore {
    pub fn add(&mut self, day: i64, category: Category, points: f64) {
        self.points.push((day, category, points));
    }

    /// A clean inspection takes a point off the categories it looked at.
    pub fn credit(&mut self, day: i64, category: Category) {
        self.points.push((day, category, -1.0));
    }

    fn weight(day: i64, today: i64) -> f64 {
        match today - day {
            age if age < 0 => 0.0,
            age if age < CARRIER_FULL_DAYS => 1.0,
            age if age < CARRIER_SCORE_DAYS => 0.5,
            _ => 0.0,
        }
    }

    pub fn category(&self, category: Category, today: i64) -> f64 {
        self.points
            .iter()
            .filter(|(_, c, _)| *c == category)
            .map(|(d, _, p)| p * Self::weight(*d, today))
            .sum::<f64>()
            .max(0.0)
    }

    pub fn total(&self, today: i64) -> f64 {
        Category::ALL.iter().map(|c| self.category(*c, today)).sum()
    }

    pub fn over_threshold(&self, today: i64) -> bool {
        self.total(today) >= AUDIT_THRESHOLD
    }

    /// Forget points too old to count, so the save doesn't grow forever.
    pub fn prune(&mut self, today: i64) {
        self.points
            .retain(|(d, _, _)| today - d < CARRIER_SCORE_DAYS);
    }

    /// "Safety score 12: hours of service 7, unsafe driving 5."
    pub fn text(&self, today: i64) -> String {
        let parts: Vec<String> = Category::ALL
            .iter()
            .filter_map(|c| {
                let v = self.category(*c, today);
                (v > 0.0).then(|| format!("{} {}", c.text(), v.round()))
            })
            .collect();
        let total = self.total(today).round();
        if parts.is_empty() {
            "Safety score 0, clean.".into()
        } else {
            format!("Safety score {total}: {}.", parts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_age_off_the_driver_record_after_three_years() {
        let mut r = DriverRecord::default();
        assert_eq!(r.cite(speeding(10, true), 10), None);
        assert_eq!(r.text(10), "1 citation in 3 years: speeding.");
        assert_eq!(r.cite(speeding(20, true), 20), Some(60));
        assert!(r.suspended(50) && !r.suspended(80));
        assert!(r.text(50).contains("CDL suspended for 30 more days"));
        assert_eq!(r.active(20 + DRIVER_RECORD_DAYS).count(), 0);
        assert_eq!(r.text(20 + DRIVER_RECORD_DAYS), "Clean record.");
        let mut r = DriverRecord::default();
        r.cite(speeding(1, true), 1);
        r.cite(speeding(2, true), 2);
        r.suspended_until = 0;
        assert_eq!(r.cite(speeding(3, true), 3), Some(120));
    }

    fn speeding(day: i64, serious: bool) -> Citation {
        Citation {
            day,
            violation: Violation::Speeding,
            serious,
        }
    }

    #[test]
    fn the_safety_score_halves_after_a_year_and_clears_after_two() {
        let mut s = SafetyScore::default();
        s.add(1, Category::Hours, 7.0);
        s.add(1, Category::UnsafeDriving, 5.0);
        assert_eq!(s.total(1), 12.0);
        assert_eq!(
            s.text(1),
            "Safety score 12: unsafe driving 5, hours of service 7."
        );
        assert_eq!(s.total(1 + CARRIER_FULL_DAYS), 6.0);
        assert_eq!(s.total(1 + CARRIER_SCORE_DAYS), 0.0);
        s.credit(2, Category::Hours);
        assert_eq!(s.category(Category::Hours, 2), 6.0);
        s.credit(3, Category::Hazmat);
        assert_eq!(s.category(Category::Hazmat, 3), 0.0, "never below zero");
        s.prune(3 + CARRIER_SCORE_DAYS);
        assert!(s.points.is_empty());
        assert_eq!(s.text(5000), "Safety score 0, clean.");
    }

    #[test]
    fn medical_cards_last_two_years() {
        let r = DriverRecord {
            medical_card_day: 10,
            ..DriverRecord::default()
        };
        assert!(!r.medical_card_expired(10 + MEDICAL_CARD_DAYS - 1));
        assert!(r.medical_card_expired(10 + MEDICAL_CARD_DAYS));
    }
}
