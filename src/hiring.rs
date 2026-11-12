//! The daily applicant pool.

use ff_core::models::career::Career;

use crate::career::Game;
use crate::roster::Driver;

pub const ROSTER_CAP: usize = 10;
pub const POOL_SIZE: usize = 3;

#[derive(Clone, Debug)]
pub struct Applicant {
    pub driver: Driver,
    pub signing: f64,
    pub interviewed: bool,
    /// A roster driver who vouched for them, if any.
    pub referred_by: Option<String>,
}

impl Applicant {
    /// The application includes credentials and the MVR and PSP record;
    /// an interview reveals goodwill and two traits.
    pub fn line(&self, today: i64) -> String {
        let d = &self.driver;
        let mut s = format!(
            "{}, level {}, signing {}.",
            d.name,
            d.level(),
            crate::money(self.signing)
        );
        if let Some(by) = &self.referred_by {
            s.push_str(&format!(" Referred by {by}."));
        }
        match d.prefers {
            Some(p) => s.push_str(&format!(" Wants {}, and {}.", d.pattern().text(), p.text())),
            None => s.push_str(&format!(" Wants {}.", d.pattern().text())),
        }
        s.push(' ');
        s.push_str(&d.credential_lines().join(" "));
        let record = d.record.text(today);
        if record.starts_with("Clean record") {
            s.push_str(&format!(" {record}"));
        } else {
            s.push_str(&format!(" Record: {record}"));
        }
        if self.interviewed {
            s.push_str(&format!(
                " Goodwill {}, {}. {}",
                d.goodwill,
                d.morale(),
                d.traits_text()
            ));
        }
        s
    }
}

/// Today's applicants: anyone a driver referred, then the seeded pool.
pub fn daily_pool(game: &Game) -> Vec<Applicant> {
    let mut rng = crate::rng(game.seed, game.day, "applicants");
    let taken: Vec<String> = game.drivers.iter().map(|d| d.name.clone()).collect();
    let mut out: Vec<Applicant> = game
        .referrals
        .iter()
        .map(|(by, d)| Applicant {
            signing: 1500.0 + 500.0 * d.level() as f64,
            driver: d.clone(),
            interviewed: false,
            referred_by: Some(by.clone()),
        })
        .collect();
    let mut drivers = crate::roster::seeded_drivers(&mut rng, 0, POOL_SIZE, &taken);
    out.extend(drivers.iter_mut().map(|d| {
        // Who they are doesn't change from one day to the next: the same
        // name brings the same experience, record and traits.
        let mut rng = crate::rng(game.seed, 0, &format!("applicant:{}", d.name));
        d.traits = Some(crate::roster::Traits::seeded(&mut rng));
        d.career = Career::with_xp(rng.randint(0, 6000) as f64);
        d.goodwill = rng.randint(40, 80) as i32;
        d.home = Some(if rng.random() < 0.5 {
            crate::roster::HomePattern::Regional
        } else {
            crate::roster::HomePattern::OverTheRoad
        });
        d.record = crate::law::seeded_record(&mut rng, d, game.day);
        d.prefers = crate::roster::Preference::seeded(game.seed, &d.name);
        d.preference_set = true;
        Applicant {
            signing: 1500.0 + 500.0 * d.level() as f64,
            driver: d.clone(),
            interviewed: false,
            referred_by: None,
        }
    }));
    out
}

/// A friend a solid driver brings in: traits like their own, two of them
/// already known, and a record a year or more clean.
pub fn referral(game: &Game, by: &Driver) -> Option<Driver> {
    let mut rng = crate::rng(game.seed, game.day, &format!("referral:{}", by.id));
    let taken: Vec<String> = game
        .drivers
        .iter()
        .map(|d| d.name.clone())
        .chain(game.referrals.iter().map(|(_, d)| d.name.clone()))
        .collect();
    let mut d = crate::roster::seeded_drivers(&mut rng, 0, 1, &taken).pop()?;
    d.career = Career::with_xp(rng.randint(1000, 6000) as f64);
    d.goodwill = rng.randint(55, 80) as i32;
    d.home = by.home;
    let mut t = by.traits();
    t.reliability = (t.reliability as i64 + rng.randint(-1, 1)).clamp(1, 5) as u8;
    t.care = (t.care as i64 + rng.randint(-1, 1)).clamp(1, 5) as u8;
    d.traits = Some(t);
    d.known = vec!["reliability".into(), "care".into()];
    d.prefers = crate::roster::Preference::seeded(game.seed, &d.name).filter(|p| {
        *p != crate::roster::Preference::LongRunsOnly
            || d.home != Some(crate::roster::HomePattern::Local)
    });
    d.preference_set = true;
    d.record = crate::law::DriverRecord {
        medical_card_day: game.day - rng.randint(0, 300),
        ..Default::default()
    };
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::{tests::game, ActError};
    use crate::desk::SHIFT_START;
    use ff_core::data::world::get_world;

    #[test]
    fn pool_replays_and_avoids_roster_names() {
        let g = game();
        let a: Vec<_> = daily_pool(&g)
            .iter()
            .map(|a| a.driver.name.clone())
            .collect();
        assert_eq!(
            a,
            daily_pool(&g)
                .iter()
                .map(|a| a.driver.name.clone())
                .collect::<Vec<_>>()
        );
        assert!(g.drivers.iter().all(|d| !a.contains(&d.name)));
    }

    #[test]
    fn interview_costs_fifteen_minutes_and_reveals_only_goodwill() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let application = s.applicants[0].line(g.day);
        assert!(
            application.contains("Record: ") || application.contains("Clean record"),
            "{application}"
        );
        let line = g.interview(&mut s, 0).unwrap();
        assert_eq!(s.clock.minute, SHIFT_START + 15);
        assert!(line.contains("Goodwill"));
        assert!(
            line.contains(&format!("{} ", s.applicants[0].driver.name)),
            "{line}"
        );
        assert!(line.contains("traits still to learn"), "{line}");
        assert!(line.starts_with(&application));
        assert!(!line.contains('_'));
        assert_eq!(s.applicants[0].driver.known.len(), 2);
    }

    #[test]
    fn hire_pays_signing_and_respects_cap_and_funds() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let cost = s.applicants[0].signing;
        let funds = g.funds;
        let name = s.applicants[0].driver.name.clone();
        let line = g.hire(&mut s, 0).unwrap();
        assert_eq!(g.drivers.len(), 4);
        assert_eq!(g.funds, funds - cost);
        assert_eq!(s.applicants.len(), POOL_SIZE - 1);
        assert_eq!(
            line,
            format!(
                "{name} signed on for {}. The drug test and background checks clear overnight; they start tomorrow morning.",
                crate::money(cost)
            )
        );
        assert_eq!(
            g.drivers.last().unwrap().status,
            crate::roster::Status::OffDuty
        );
        g.funds = 0.0;
        assert_eq!(g.hire(&mut s, 0), Err(ActError::NoFunds));
        g.funds = 1e9;
        while g.drivers.len() < ROSTER_CAP {
            let mut extra = g.drivers[0].clone();
            extra.id = g.take_id();
            g.drivers.push(extra);
        }
        assert_eq!(g.hire(&mut s, 0), Err(ActError::RosterFull));
        g.close_day(&mut s, w);
        assert_eq!(g.drivers[3].status, crate::roster::Status::Idle);
        assert_eq!(g.drivers[3].hired_day, 2, "they start tomorrow");
        assert!(g.drivers[3].on_probation(3));
    }

    #[test]
    fn a_solid_driver_s_referral_leads_the_pool_with_two_known_traits() {
        let w = get_world();
        let mut g = game();
        let by = g.drivers[0].clone();
        let friend = referral(&g, &by).unwrap();
        assert_eq!(friend.known.len(), 2);
        g.referrals.push((by.name.clone(), friend.clone()));
        let s = g.morning(w);
        assert_eq!(s.applicants[0].driver.name, friend.name);
        assert!(s.applicants[0]
            .line(g.day)
            .contains(&format!("Referred by {}.", by.name)));
        let mut g = g;
        let mut s = s;
        g.hire(&mut s, 0).unwrap();
        assert!(g.referrals.is_empty());
    }
}
