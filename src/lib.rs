//! Freight Fate: Dispatch — logic and UI state. `shell` puts it on the
//! Hearken engine; `main.rs` only starts it.

use ff_core::pyrandom::PyRandom;

pub mod audio;
pub mod board;
pub mod career;
pub mod customers;
pub mod desk;
pub mod growth;
pub mod hiring;
pub mod import;
pub mod incidents;
pub mod law;
pub mod money;
pub mod net;
pub mod online;
pub mod paths;
pub mod phone;
pub mod roster;
pub mod saves;
pub mod shell;
pub mod strategy;
pub mod trip;
pub mod ui;

/// One random stream per (career, day, purpose): a reloaded morning replays.
pub fn rng(seed: i64, day: i64, purpose: &str) -> PyRandom {
    PyRandom::new_from_str(&format!("{seed}:{day}:{purpose}"))
}

/// An i64 seed for `ff-core` APIs that take one (JobBoard).
pub fn seed_for(seed: i64, day: i64, purpose: &str) -> i64 {
    rng(seed, day, purpose).randint(0, 1 << 40)
}

/// "$25,000", "-$1,250".
pub fn money(x: f64) -> String {
    let whole = x.abs().round() as i64;
    let digits = whole.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if x < -0.5 {
        format!("-${out}")
    } else {
        format!("${out}")
    }
}

/// Minutes as spoken hours: "9.5 hours", "1 hour".
pub fn hours(min: f64) -> String {
    let h = (min / 30.0).round() / 2.0;
    if (h - 1.0).abs() < f64::EPSILON {
        "1 hour".into()
    } else {
        format!("{h} hours")
    }
}

/// "1 driver", "3 drivers": a count with its noun, `many` for any count but one.
pub fn count(n: impl Into<i64>, one: &str, many: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "a", "a and b", "a, b and c".
pub fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// "Gus Tran's", "Meridian Plastics'".
pub fn possessive(name: &str) -> String {
    if name.ends_with('s') {
        format!("{name}'")
    } else {
        format!("{name}'s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_streams_are_stable_and_distinct() {
        let a = rng(7, 1, "board").random();
        assert_eq!(a, rng(7, 1, "board").random());
        assert_ne!(a, rng(7, 2, "board").random());
        assert_ne!(a, rng(7, 1, "calls").random());
    }

    #[test]
    fn money_and_hours_read_aloud() {
        assert_eq!(money(25_000.0), "$25,000");
        assert_eq!(money(-1250.4), "-$1,250");
        assert_eq!(money(999.0), "$999");
        assert_eq!(hours(570.0), "9.5 hours");
        assert_eq!(hours(60.0), "1 hour");
        assert_eq!(count(1, "driver", "drivers"), "1 driver");
        assert_eq!(count(0, "driver", "drivers"), "0 drivers");
        assert_eq!(
            and_list(&["a".into(), "b".into(), "c".into()]),
            "a, b and c"
        );
        assert_eq!(possessive("Meridian Plastics"), "Meridian Plastics'");
        assert_eq!(possessive("Gus Tran"), "Gus Tran's");
    }

    #[test]
    fn world_loads_from_the_pinned_ff_core() {
        let world = ff_core::data::world::get_world();
        assert!(world.city_names().len() > 100);
        assert!(world.city("Chicago").is_ok());
    }
}
