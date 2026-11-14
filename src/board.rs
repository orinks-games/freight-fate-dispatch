//! The day's loads, from ff-core's own job board at the terminal city.

use ff_core::data::world::World;
use ff_core::data::world_models::Route;
use ff_core::models::jobs::{Job, JobBoard, OfferOptions};
use ff_core::models::market::Market;

use crate::customers::Source;
use crate::roster::{Driver, Status};

pub const BOARD_SIZE: usize = 8;
/// Saturday's board is half size; Sunday's is three loads.
pub const SATURDAY_BOARD: usize = BOARD_SIZE / 2;
pub const SUNDAY_BOARD: usize = 3;

/// What the two docks ask of a load. A facility is the same every time, so
/// a shipper that drops and hooks or a receiver that charges a lumper can
/// be learned.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Terms {
    /// The receiver holds the truck to its appointment: late is worked in
    /// when they can. Otherwise first come, first served until the deadline.
    pub strict: bool,
    /// The trailer is loaded and waiting: no dock wait at the shipper.
    /// Otherwise a live load.
    pub drop_hook: bool,
    /// The receiver's unloading crew bills the truck; 0 for none.
    pub lumper: f64,
}

/// Share of receivers that hold a truck to its appointment.
pub const STRICT_SHARE: f64 = 0.45;
/// Share of shippers with loaded trailers waiting.
pub const DROP_HOOK_SHARE: f64 = 0.3;
/// Share of receivers with a lumper crew, and what it bills.
pub const LUMPER_SHARE: f64 = 0.18;
pub const LUMPER_FEES: [f64; 4] = [80.0, 120.0, 160.0, 200.0];
/// A truck late to a strict appointment waits this long to be worked in.
pub const WORK_IN_MIN: (i64, i64) = (60, 180);
/// Sitting unpaid at a dock costs the driver's goodwill.
pub const UNPAID_WAIT_GOODWILL: i32 = 2;

impl Terms {
    /// The terms for a job, from the career seed and the two facilities.
    pub fn of(seed: i64, job: &Job) -> Terms {
        let roll = |what: &str, city: &str, facility: &str| {
            crate::rng(seed, 0, &format!("{what}:{city}:{facility}")).random()
        };
        // Tankers load and unload through a hose: no trailer swap, no crew.
        let tank = job.cargo.tank;
        let fee = roll("lumper", &job.destination, &job.destination_location);
        Terms {
            strict: roll("strict", &job.destination, &job.destination_location) < STRICT_SHARE,
            drop_hook: !tank && roll("drop", &job.origin, &job.origin_location) < DROP_HOOK_SHARE,
            lumper: if !tank && fee < LUMPER_SHARE {
                LUMPER_FEES
                    [((fee / LUMPER_SHARE) * LUMPER_FEES.len() as f64) as usize % LUMPER_FEES.len()]
            } else {
                0.0
            },
        }
    }

    /// The part of a board line after the appointment: "Drop and hook.
    /// Lumper $160." Empty for a live load with no lumper.
    pub fn tail(&self) -> String {
        let mut s = String::new();
        if self.drop_hook {
            s.push_str(" Drop and hook.");
        }
        if self.lumper > 0.0 {
            s.push_str(&format!(" Lumper {}.", crate::money(self.lumper)));
        }
        s
    }

    /// For a load's help line: what each dock does, in a sentence.
    pub fn help(&self) -> String {
        // A live load at a first come, first served dock is the ordinary
        // case and goes unsaid.
        let mut parts = Vec::new();
        if self.drop_hook {
            parts.push(
                "Drop and hook: the trailer is loaded and waiting, so the shipper can't hold the truck."
                    .to_string(),
            );
        }
        if self.strict {
            parts.push(
                "The receiver keeps a strict appointment: a late truck is worked in when they can."
                    .to_string(),
            );
        }
        if self.lumper > 0.0 {
            parts.push(format!(
                "The receiver's crew unloads for {}, paid at the dock.",
                crate::money(self.lumper)
            ));
        }
        parts.join(" ")
    }
}

/// One offer on the board, with whether it's flagged hot, who owns it, and
/// for contract freight the receiver's appointment.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Load {
    #[serde(with = "crate::trip::job_json")]
    pub job: Job,
    #[serde(default)]
    pub hot: bool,
    #[serde(default)]
    pub weather: Vec<crate::career::StoredAlert>,
    pub source: Source,
    /// Absolute game minute of the delivery appointment, for contract
    /// freight. Spot loads keep ff-core's countdown from dispatch.
    #[serde(default)]
    pub appointment: Option<i64>,
    /// The broker has heard one counter on this load already.
    #[serde(default)]
    pub countered: bool,
    /// What the docks ask; set from the career seed when the board fills.
    #[serde(default)]
    pub terms: Terms,
}

impl Load {
    /// A spot load from a broker.
    pub fn spot(job: Job, broker: String) -> Load {
        Load {
            job,
            hot: false,
            weather: Vec::new(),
            source: Source::Spot { broker },
            appointment: None,
            countered: false,
            terms: Terms::default(),
        }
    }

    /// An accepted tender on the day it runs.
    pub fn contract(t: &crate::customers::Tender) -> Load {
        Load {
            job: t.job.clone(),
            hot: false,
            weather: Vec::new(),
            source: Source::Contract {
                customer_id: t.customer_id,
            },
            appointment: Some(t.appointment),
            countered: false,
            terms: Terms::default(),
        }
    }

    /// The load with its docks' terms, for this career.
    pub fn with_terms(mut self, seed: i64) -> Load {
        self.terms = Terms::of(seed, &self.job);
        self
    }

    /// Keep the appointment's countdown current as the desk clock moves.
    pub fn refresh(&mut self, now: i64) {
        if let Some(appointment) = self.appointment {
            self.job.deadline_game_h = ((appointment - now) as f64 / 60.0).max(0.0);
        }
    }

    /// Who the load is for: the customer, or the broker.
    pub fn owner(&self, game: &crate::career::Game) -> String {
        match &self.source {
            Source::Spot { broker } => broker.clone(),
            Source::Contract { customer_id } => game.customer_name(*customer_id),
        }
    }
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

/// How many spot loads the terminal's board carries on a career day.
pub fn board_size(day: i64) -> usize {
    match crate::roster::weekday(day) {
        5 => SATURDAY_BOARD,
        6 => SUNDAY_BOARD,
        _ => BOARD_SIZE,
    }
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
    None
}

/// A load the driver's legal run doesn't get to on time.
enum Hours {
    /// Late even on a straight run: legal, just late, by about this many
    /// minutes.
    Late(f64),
    /// On time only by driving past the clock; the legal run needs this
    /// many hours.
    Short(f64),
}

fn hours_check(job: &Job, driver: &Driver, world: &World) -> Option<Hours> {
    let p = crate::trip::plan(job, driver, world);
    if p.need_h <= job.deadline_game_h {
        return None;
    }
    // Sent past the clock, a truck runs straight through plus half an hour.
    let straight_h = (p.deadhead_min + p.drive_min) / 60.0 + 0.5;
    Some(
        if job.deadline_game_h > 0.0 && straight_h <= job.deadline_game_h {
            Hours::Short(p.need_h)
        } else {
            Hours::Late((p.need_h - job.deadline_game_h.max(0.0)) * 60.0)
        },
    )
}

/// The legal run misses the appointment. Not a refusal: the desk can send
/// the driver anyway. Either they deliver late on legal hours, or they make
/// it only by running past the clock, on a log that won't hold up at a scale.
pub fn hours_warning(job: &Job, driver: &Driver, world: &World) -> Option<String> {
    Some(match hours_check(job, driver, world)? {
        Hours::Late(late) if job.deadline_game_h <= 0.0 => format!(
            "The appointment has passed. {} would deliver late, {} from now on legal hours.",
            driver.name,
            crate::hours(late)
        ),
        Hours::Late(late) => format!(
            "{} would deliver about {} late on legal hours: {} needed, {} to the appointment.",
            driver.name,
            crate::desk::spoken_duration(late),
            crate::hours(late + job.deadline_game_h * 60.0),
            crate::hours(job.deadline_game_h * 60.0)
        ),
        Hours::Short(need_h) => format!(
            "{} doesn't have the hours to make this appointment: {} needed, {} to the appointment. Sending them runs past the clock.",
            driver.name,
            crate::hours(need_h * 60.0),
            crate::hours(job.deadline_game_h * 60.0)
        ),
    })
}

/// Whether sending the driver means running past the clock: the
/// appointment can be made, but not on legal hours.
pub fn past_clock(job: &Job, driver: &Driver, world: &World) -> bool {
    matches!(hours_check(job, driver, world), Some(Hours::Short(_)))
}

/// A few words for a menu row when the driver can't make the appointment
/// legally, or is held off it by the 70-hour clock, so it's heard before
/// Enter, not only in help.
pub fn short_note(job: &Job, driver: &Driver, world: &World) -> Option<&'static str> {
    if let Some(why) = refusal(job, driver, world) {
        return (Some(why) == cycle_refusal(job, driver, world))
            .then_some(" Needs a 34-hour restart.");
    }
    hours_check(job, driver, world).map(|h| match h {
        Hours::Late(_) if job.deadline_game_h <= 0.0 => " Appointment passed.",
        Hours::Late(_) => " Delivers late.",
        Hours::Short(_) => " Short on hours.",
    })
}

/// Either gate: what the desk sees before a legal dispatch.
pub fn refusal_or_warning(job: &Job, driver: &Driver, world: &World) -> Option<String> {
    refusal(job, driver, world).or_else(|| hours_warning(job, driver, world))
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

/// A board load as spoken: a contract load leads with its customer and
/// ends with the appointment; a spot load is `load_line`.
pub fn load_text(load: &Load, game: &crate::career::Game, world: &World) -> String {
    let mut text = with_terms(load_text_plain(load, game, world), &load.terms);
    if game.slow_docks.contains(&load.job.origin_location) {
        text.push_str(" This shipper has kept a truck waiting before.");
    }
    text
}

/// A load or tender line with its docks' terms: "deliver by 2 PM tomorrow"
/// and "due in 22 hours" become the strict appointment they are, and a
/// drop and hook or a lumper is said after.
pub fn with_terms(line: String, terms: &Terms) -> String {
    let mut text = if terms.strict {
        line.replacen(", deliver by ", ", strict appointment at ", 1)
            .replacen(", due in ", ", strict appointment in ", 1)
    } else {
        line
    };
    text.push_str(&terms.tail());
    text
}

fn load_text_plain(load: &Load, game: &crate::career::Game, world: &World) -> String {
    match (&load.source, load.appointment) {
        (Source::Contract { customer_id }, Some(at)) => format!(
            "{}. {}, {} to {}, {} miles, pays {}, deliver by {}.",
            game.customer_name(*customer_id),
            load.job.cargo.label,
            world.spoken_city(&load.job.origin, None),
            world.spoken_city(&load.job.destination, None),
            crate::money(load.job.distance_mi).trim_start_matches('$'),
            crate::money(load.job.pay),
            crate::desk::abs_text(at, game.day),
        ),
        _ => load_line(&load.job, load.hot, world),
    }
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
        assert_eq!(refusal(&job, &tired, w), None, "hours are a warning now");
        assert!(hours_warning(&job, &tired, w)
            .unwrap()
            .contains("past the clock"));
        assert_eq!(refusal_or_warning(&job, &fresh(3), w), None);
    }
}
