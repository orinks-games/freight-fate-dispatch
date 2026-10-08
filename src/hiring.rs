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
}

impl Applicant {
    /// The application includes credentials; an interview reveals goodwill.
    pub fn line(&self) -> String {
        let d = &self.driver;
        let mut s = format!(
            "{}, level {}, signing {}.",
            d.name,
            d.level(),
            crate::money(self.signing)
        );
        s.push(' ');
        s.push_str(&d.credential_lines().join(" "));
        if self.interviewed {
            s.push_str(&format!(" Goodwill {}.", d.goodwill));
        }
        s
    }
}

pub fn daily_pool(game: &Game) -> Vec<Applicant> {
    let mut rng = crate::rng(game.seed, game.day, "applicants");
    let taken: Vec<String> = game.drivers.iter().map(|d| d.name.clone()).collect();
    let mut drivers = crate::roster::seeded_drivers(&mut rng, 0, POOL_SIZE, &taken);
    drivers
        .iter_mut()
        .map(|d| {
            d.career = Career::with_xp(rng.randint(0, 6000) as f64);
            d.goodwill = rng.randint(40, 80) as i32;
            Applicant {
                signing: 1500.0 + 500.0 * d.level() as f64,
                driver: d.clone(),
                interviewed: false,
            }
        })
        .collect()
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
        let application = s.applicants[0].line();
        let line = g.interview(&mut s, 0).unwrap();
        assert_eq!(s.clock.minute, SHIFT_START + 15);
        assert!(line.contains("Goodwill"));
        assert!(line.starts_with(&application));
        assert!(!line.contains('_'));
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
    }
}
