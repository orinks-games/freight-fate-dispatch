# Freight Fate: Dispatch — Rust Slice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `ff-dispatch`, an audio-first dispatcher sim in Rust on Freight Fate's `ff-core`. It should be playable end to end: start or import a career, work the board against a desk clock, answer the phone when the board talks back, close the day, sleep, and save.

**Architecture:** This is one crate with a library (`src/lib.rs`, all logic and UI state) and a thin binary (`src/main.rs`, the SDL window and speech). The logic modules are pure and seeded, and they are tested headlessly against the real `ff-core` world. `ui::app::App` turns keys into spoken lines, so the whole game can also be driven from a key script. That script mode is how the boot smoke test runs the binary.

**Tech Stack:** Rust 1.98.0 (edition 2021), `ff-core` (git, pinned tag), `prismer` 0.1.3 (static Prism), `sdl2` 0.38 (vendored Windows SDL2), `ureq` 3, `serde`/`serde_json`, `keyring` 3, `sha2`/`hex`, `log`/`env_logger`.

**Spec:** `docs/superpowers/specs/2026-09-22-dispatch-rust-slice-design.md`

**Rules source:** The only sources are this plan and the spec, applied on top of `ff-core`'s models. There is no earlier implementation to port, so do not go looking for one.

## Global Constraints

- Toolchain: `rust-toolchain.toml` with `channel = "1.98.0"`, `profile = "minimal"`, `components = ["rustfmt", "clippy"]`. Edition 2021.
- `ff-core` is a git dependency on `https://github.com/Orinks/Freight-fate`, pinned with `tag = "1.9-tester-20260922"`. Move to the 1.9.0 tag when FF ships it. `Cargo.toml` is the only place the ff-core pin is written; CI reads its tag or rev from there.
- Nothing Dispatch-specific is committed to the Freight Fate or `orinks-net` repos.
- Drivers use the real `ff_core::sim::hos::HosClock`, and HOS mode is always `"realistic"`. The dispatcher has no HOS.
- Desk clock: 06:00–18:00. Action costs: assign 10 min, take a call 5, hiring interview 15, check on a truck 2. The clock moves **only** on actions.
- Roster cap: 10. A new career starts with 3 seeded drivers.
- Determinism: every random stream comes from `crate::rng(career_seed, day, purpose)`. A reloaded morning replays the board, calls, applicants and incident rolls exactly.
- A Freight Fate save is never opened for writing. Read it with `std::fs::read` and `ff_core::models::profile::decode_save_bytes` followed by `Profile::from_dict`. **Never** call `Profile::load`: it re-signs, rewrites and quarantines saves.
- Career saves are written as JSON with a temp file and a rename. Every struct tolerates missing fields (`#[serde(default)]`) and unknown fields (serde's default behavior). A corrupt save is reported by name and never deleted.
- Network: weather has a 3 s budget, and online calls time out after 10 s. A network failure never interrupts play; it is logged with `log::warn!`.
- Env vars:

  | Variable | Effect |
  |---|---|
  | `FF_DISPATCH_NO_SPEECH=1` | Print speech to the console instead of speaking |
  | `FF_DISPATCH_DATA_DIR` | Override the career save and log directory |
  | `FF_DISPATCH_FREIGHT_FATE_SAVES` | Override where FF profiles are looked for |
  | `FF_DISPATCH_ONLINE_URL` | Override the online base URL |
  | `FREIGHT_FATE_DATA_ROOT` | Read by `ff-core` to locate world data |

- Online: device-code flow on `https://www.orinks.net`, using `/api/freight-fate/activate/start` and `/api/freight-fate/activate/poll`. The token lives in the OS keyring under service `"Freight Fate Dispatch token"`, never in a file. Every payload carries `schema_version`.
- Menu grammar:

  | Key | Effect |
  |---|---|
  | Up / Down | Move, with wraparound |
  | Home / End | First / last item |
  | Letter | Jump to the next item starting with it |
  | Enter | Select |
  | Escape | Back |
  | F1 | Help for the current item |
  | Control | Stop speech |

- Tests: run the focused tests for the files you touched while iterating. Run the full suite **once**, at the end (Task 18). After editing any `.github/workflows/*.yml`, run `actionlint` from the repo root and fix whatever it reports.
- The first `cargo build` compiles `ff-core`, including zstd's C code, and the first test that touches the world parses the checkout's JSON tree. Expect it to be slow once. Do not "fix" that.

## Tuning numbers (first pass; they live as `pub const` in the named module)

| Constant | Value | Module |
|---|---|---|
| `STARTING_FUNDS` | 25_000.0 | career |
| `TRUCK_COST_PER_DAY` (per driver) | 180.0 | career |
| `DEADHEAD_RATE` ($/mi) | 0.65 | career |
| `BOARD_SIZE` | 8 | board |
| `GOODWILL_START` | 60 | roster |
| `BREAKDOWN_PER_LEG` | 0.03 | incidents |
| `DETENTION_BASE` / `DETENTION_SLOW_SHIPPER` / `DETENTION_SOURED` | 0.12 / 0.35 / +0.10 | incidents |
| `SEEDED_STORM` (per load, offline) | 0.06 | incidents |
| `REPAIR_COST` / `REPAIR_MIN` | 450.0 / 120.0 | incidents |
| `RESET_MIN` (10-hour reset) | 600.0 | incidents |
| `HOLD_MIN` | 240.0 | incidents |
| `DETENTION_MIN` / `DETENTION_BILL` | 120.0 / 150.0 | incidents |
| `WEATHER_FINE` | 300.0 | incidents |
| `SICK_ODDS` / `PERSONAL_ODDS` | 0.05 / 0.04 | phone |
| `HOT_PAY` / `HOT_DEADLINE` | 1.35 / 0.85 | phone |
| `POOL_SIZE` / signing | 3 / 1500 + 500 × level | hiring |

## Review Focus

These are the five inputs the spec implies but no happy-path test exercises. Each one is pinned by a test in the task named.

1. **No driver can legally take a load** (every driver busy, short of hours, or under-endorsed). Assigning must refuse with the reason and must not spend desk time. *Task 6.*
2. **An action that would carry the clock past 18:00.** The clock clamps at 18:00 and further actions report that the desk is closed. Closing the day still works, and incidents scheduled after the close never ring. *Tasks 3 and 7.*
3. **A ring whose subject is gone.** If the load was pulled or delivered, or the driver is now on a load, the call must be dropped rather than offered. *Tasks 7 and 9.*
4. **Relay or "send another truck" when no idle driver qualifies.** The choice is simply not offered, and the incident stays answerable. *Task 7.*
5. **Damaged or foreign saves.** This covers a career save with unknown or missing fields, a corrupt career save, and a corrupt or too-new `.ffsave`. Each is handled with a spoken message, and no file is deleted or written. *Tasks 11 and 13.*

---

## File map

| File | Responsibility |
|---|---|
| `Cargo.toml`, `rust-toolchain.toml`, `build.rs`, `vendor/sdl2/windows-x86_64/{SDL2.dll,SDL2.lib}` | Crate, pins, vendored SDL2, Prism delay-load |
| `src/lib.rs` | Module list, `rng`, `seed_for`, `money`, `hours` |
| `src/roster.rs` | `Driver`, `Status`, names, seeded drivers, HOS serde |
| `src/desk.rs` | Desk clock, action costs, time text, `Schedule<T>` |
| `src/board.rs` | Morning board, refusal gates, load lines |
| `src/trip.rs` | `Trip`: timing, position, hours-short test, status line |
| `src/career.rs` | `Game` (persistent), `Shift` (today), `Ring`, actions, day close and overnight |
| `src/incidents.rs` | Four incidents: rolls, scripts, choices, effects |
| `src/phone.rs` | Driver calls, broker hot loads |
| `src/hiring.rs` | Applicant pool |
| `src/saves.rs` | Data dir, atomic save, tolerant load, slot list |
| `src/net.rs` | `UreqTransport`, morning NWS weather fetch |
| `src/import.rs` | Read-only FF profile import |
| `src/online.rs` | Device-code sign-in, keyring token, day-end post |
| `src/ui/mod.rs` | `Key`, `Out`, `Menu` |
| `src/ui/speech.rs` | `Speaker`: Prism or console |
| `src/ui/app.rs` | `App`: screens, the phone interrupt, day flow |
| `src/main.rs` | Args, logging, world check, SDL loop, script mode |
| `tests/fixtures/ff/*.ffsave`, `tests/import.rs`, `tests/smoke.rs`, `tests/fixtures/first_shift.keys` | Integration tests |
| `.github/workflows/ci.yml` | fmt, clippy, test; Windows zip on `v*` |
| `README.md` | Player readme (ships in the zip) |

---

### Task 1: Crate scaffold, pins, CI

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `build.rs`, `src/lib.rs`, `src/main.rs`, `.github/workflows/ci.yml`
- Copy: `vendor/sdl2/windows-x86_64/SDL2.dll` and `SDL2.lib` from `C:\Users\joshu\gh-projects\Freight-Fate\vendor\sdl2\windows-x86_64\` (SDL2 2.32.10 VC prebuilt)

**Interfaces:**
- Produces: `ff_dispatch::rng(seed: i64, day: i64, purpose: &str) -> PyRandom`, `ff_dispatch::seed_for(seed: i64, day: i64, purpose: &str) -> i64`, `ff_dispatch::money(x: f64) -> String`, `ff_dispatch::hours(min: f64) -> String`

- [ ] **Step 1: Write the toolchain file and manifest**

`rust-toolchain.toml`:
```toml
[toolchain]
# Same compiler as Freight Fate; bump together.
channel = "1.98.0"
profile = "minimal"
components = ["rustfmt", "clippy"]
```

`Cargo.toml`:
```toml
[package]
name = "ff-dispatch"
version = "0.1.0"
edition = "2021"
rust-version = "1.95"
publish = false

[dependencies]
# The one place the ff-core pin lives; CI reads the tag from this line.
ff-core = { git = "https://github.com/Orinks/Freight-fate", tag = "1.9-tester-20260922" }
serde = { version = "1", features = ["derive"] }
serde_json = { version = "1", features = ["float_roundtrip"] }
ureq = { version = "3", default-features = false, features = ["rustls", "json", "gzip", "platform-verifier"] }
keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }
sha2 = "0.10"
hex = "0.4"
log = "0.4"
env_logger = "0.11"
# 0.1.3 at least: older releases drop static backends or overrun the stack in Prism::new.
prismer = { version = "0.1.3", features = ["static"] }
# Windows links the vendored prebuilt (build.rs); Linux/macOS builds come after the beta.
sdl2 = "0.38"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Write `build.rs`**

```rust
//! Link the vendored SDL2 and stage SDL2.dll next to the build output, and
//! delay-load Prism's screen-reader client DLLs so the game starts on a
//! machine missing any of them (the same arrangement as Freight Fate).
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DEP_PRISMER_DELAY_LOAD_DLLS");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        if let Ok(dlls) = env::var("DEP_PRISMER_DELAY_LOAD_DLLS") {
            for dll in dlls.split(';').filter(|d| !d.is_empty()) {
                println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
            }
        }
    }
    let vendor = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("vendor/sdl2/windows-x86_64");
    println!("cargo:rerun-if-changed={}", vendor.display());
    println!("cargo:rustc-link-search=native={}", vendor.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    if let Some(profile) = out.ancestors().nth(3) {
        for entry in fs::read_dir(&vendor).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "dll") {
                let _ = fs::copy(&p, profile.join(p.file_name().unwrap()));
            }
        }
    }
}
```

- [ ] **Step 3: Write the failing tests in `src/lib.rs`**

```rust
//! Freight Fate: Dispatch — logic and UI state. `main.rs` is the window.

use ff_core::pyrandom::PyRandom;

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
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if x < -0.5 { format!("-${out}") } else { format!("${out}") }
}

/// Minutes as spoken hours: "9.5 hours", "1 hour".
pub fn hours(min: f64) -> String {
    let h = (min / 30.0).round() / 2.0;
    if (h - 1.0).abs() < f64::EPSILON { "1 hour".into() } else { format!("{h} hours") }
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
    }

    #[test]
    fn world_loads_from_the_pinned_ff_core() {
        let world = ff_core::data::world::get_world();
        assert!(world.city_names().len() > 100);
        assert!(world.city("Chicago").is_ok());
    }
}
```

`src/main.rs` (temporary, replaced in Task 17):
```rust
fn main() {
    println!("Freight Fate: Dispatch");
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib tests::`
Expected: all 3 PASS. The first build is slow. If `world_loads...` fails because the data root is missing, the git checkout is incomplete; do not work around it.

- [ ] **Step 5: Write `.github/workflows/ci.yml` (the release job is added in Task 18)**

```yaml
name: CI
on:
  push:
    branches: [main]
    tags: ["v*"]
  pull_request:

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: rustup show
      - run: cargo fmt --all --check

  test:
    runs-on: windows-latest
    env:
      FF_DISPATCH_NO_SPEECH: "1"
      SDL_VIDEODRIVER: dummy
    steps:
      - uses: actions/checkout@v4
      - run: rustup show
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --all-targets --locked -- -D warnings
      - run: cargo test --locked
```

- [ ] **Step 6: Lint and commit**

Run: `actionlint`, `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`
Expected: clean.

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml build.rs vendor src .github
git commit -m "feat: ff-dispatch crate on pinned ff-core, CI"
```

---

### Task 2: Roster

**Files:**
- Create: `src/roster.rs`
- Modify: `src/lib.rs` (add `pub mod roster;`)

**Interfaces:**
- Consumes: `ff_core::models::career::Career`, `ff_core::sim::hos::HosClock`, `ff_core::pyrandom::PyRandom`
- Produces:
  - `Status::{Idle, OnLoad, OffDuty, OutOnRun}`
  - `Driver { id: u32, name: String, career: Career, hos: HosClock, status: Status, goodwill: i32 }`
  - `Driver::new(id, &str, Career) -> Driver`, `level() -> i64`, `endorsements() -> Vec<String>`, `legal_left_min() -> f64`, `adjust_goodwill(i32)`, `card() -> String`
  - `seeded_drivers(&mut PyRandom, first_id: u32, count: usize, taken: &[String]) -> Vec<Driver>`
  - `NAMES`, `GOODWILL_START`

- [ ] **Step 1: Write `src/roster.rs` with its tests**

```rust
//! Drivers on the roster: an ff-core career and a real HOS clock each.

use ff_core::models::career::Career;
use ff_core::pyrandom::PyRandom;
use ff_core::sim::hos::HosClock;
use serde::{Deserialize, Serialize};

pub const GOODWILL_START: i32 = 60;
const DRIVE_LIMIT_MIN: f64 = 11.0 * 60.0;
const WINDOW_LIMIT_MIN: f64 = 14.0 * 60.0;

pub const NAMES: &[&str] = &[
    "Alma Reyes", "Bo Whitaker", "Casey Lin", "Dee Okafor", "Eli Brandt", "Faye Monroe",
    "Gus Tran", "Hattie Cole", "Ike Serrano", "Jo Petrov", "Kit Mercer", "Lou Abara",
    "Mack Dunn", "Nell Ibarra", "Otis Grant", "Pru Salas", "Quinn Harlow", "Rae Wojcik",
    "Sal Mendez", "Tess Oduya", "Ugo Farris", "Val Kincaid", "Wes Amato", "Xena Pruitt",
    "Yuri Bell", "Zed Carver", "Ada Nwosu", "Bram Keller", "Cora Vance", "Dom Rinaldi",
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
        Driver { id, name: name.to_string(), career, hos: HosClock::new(), status: Status::Idle, goodwill: GOODWILL_START }
    }

    pub fn level(&self) -> i64 {
        self.career.level()
    }

    pub fn endorsements(&self) -> Vec<String> {
        self.career.endorsements().into_iter().map(String::from).collect()
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
pub fn seeded_drivers(rng: &mut PyRandom, first_id: u32, count: usize, taken: &[String]) -> Vec<Driver> {
    let mut pool: Vec<&str> = NAMES.iter().copied().filter(|n| !taken.iter().any(|t| t == n)).collect();
    rng.shuffle(&mut pool);
    pool.into_iter()
        .take(count)
        .enumerate()
        .map(|(i, name)| Driver::new(first_id + i as u32, name, Career::with_xp(rng.randint(0, 2500) as f64)))
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
        let bare: Driver = serde_json::from_str(r#"{"id":2,"name":"Casey Lin","extra":true}"#).unwrap();
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
```

- [ ] **Step 2: Add `pub mod roster;` to `src/lib.rs` and run the tests**

Run: `cargo test --lib roster::`
Expected: 4 PASS. If `Career` is not `Default`, replace `#[serde(default)]` on `career` with `#[serde(default = "Career::new")]`.

- [ ] **Step 3: Commit**

```bash
git add src/roster.rs src/lib.rs
git commit -m "feat: roster drivers with ff-core career and HOS clock"
```

---

### Task 3: Desk clock and schedule

**Files:**
- Create: `src/desk.rs`
- Modify: `src/lib.rs` (add `pub mod desk;`)

**Interfaces:**
- Produces:
  - Constants: `SHIFT_START: u32 = 360`, `SHIFT_END: u32 = 1080`, `MIN_PER_DAY: i64 = 1440`
  - `Action::{Assign, TakeCall, Interview, CheckTruck}` with `cost_min(self) -> u32`
  - `DeskClock { minute: u32 }` with `morning()`, `spend(Action)`, `closed() -> bool`, `spoken() -> String`
  - `clock_text(u32) -> String`, `abs_min(day: i64, minute: u32) -> i64`, `abs_text(abs: i64, today: i64) -> String`
  - `Schedule<T>` with `push(i64, T)`, `due(i64) -> Vec<T>`, `len()`, `is_empty()`, `iter() -> impl Iterator<Item = &(i64, T)>`

- [ ] **Step 1: Write `src/desk.rs` with its tests**

```rust
//! The dispatcher's desk clock. It only moves when the player acts.

pub const SHIFT_START: u32 = 6 * 60;
pub const SHIFT_END: u32 = 18 * 60;
pub const MIN_PER_DAY: i64 = 24 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Assign,
    TakeCall,
    Interview,
    CheckTruck,
}

impl Action {
    pub const fn cost_min(self) -> u32 {
        match self {
            Action::Assign => 10,
            Action::TakeCall => 5,
            Action::Interview => 15,
            Action::CheckTruck => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeskClock {
    /// Minutes since midnight.
    pub minute: u32,
}

impl DeskClock {
    pub fn morning() -> Self {
        DeskClock { minute: SHIFT_START }
    }

    pub fn spend(&mut self, action: Action) {
        self.minute = (self.minute + action.cost_min()).min(SHIFT_END);
    }

    pub fn closed(&self) -> bool {
        self.minute >= SHIFT_END
    }

    pub fn spoken(&self) -> String {
        clock_text(self.minute)
    }
}

/// "6 AM", "9:35 AM", "12 PM", "6:05 PM".
pub fn clock_text(minute_of_day: u32) -> String {
    let h = (minute_of_day / 60) % 24;
    let m = minute_of_day % 60;
    let (h12, half) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    if m == 0 { format!("{h12} {half}") } else { format!("{h12}:{m:02} {half}") }
}

/// Absolute game minute for a desk minute on a career day.
pub fn abs_min(day: i64, minute_of_day: u32) -> i64 {
    day * MIN_PER_DAY + minute_of_day as i64
}

/// "3:10 PM today", "3:10 PM tomorrow", "3:10 PM in 2 days".
pub fn abs_text(abs: i64, today: i64) -> String {
    let t = clock_text(abs.rem_euclid(MIN_PER_DAY) as u32);
    match abs.div_euclid(MIN_PER_DAY) - today {
        n if n <= 0 => format!("{t} today"),
        1 => format!("{t} tomorrow"),
        n => format!("{t} in {n} days"),
    }
}

/// Things due at an absolute game minute, handed out in time order.
#[derive(Clone, Debug)]
pub struct Schedule<T> {
    items: Vec<(i64, T)>,
}

impl<T> Default for Schedule<T> {
    fn default() -> Self {
        Schedule { items: Vec::new() }
    }
}

impl<T> Schedule<T> {
    pub fn push(&mut self, at: i64, item: T) {
        self.items.push((at, item));
        self.items.sort_by_key(|(t, _)| *t); // stable: equal times keep push order
    }

    pub fn due(&mut self, now: i64) -> Vec<T> {
        let split = self.items.partition_point(|(t, _)| *t <= now);
        self.items.drain(..split).map(|(_, item)| item).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &(i64, T)> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_costs_match_the_spec() {
        assert_eq!(Action::Assign.cost_min(), 10);
        assert_eq!(Action::TakeCall.cost_min(), 5);
        assert_eq!(Action::Interview.cost_min(), 15);
        assert_eq!(Action::CheckTruck.cost_min(), 2);
    }

    #[test]
    fn clock_clamps_at_six_pm_and_closes() {
        let mut c = DeskClock { minute: SHIFT_END - 4 };
        assert!(!c.closed());
        c.spend(Action::Assign);
        assert_eq!(c.minute, SHIFT_END);
        assert!(c.closed());
    }

    #[test]
    fn times_read_aloud() {
        assert_eq!(clock_text(360), "6 AM");
        assert_eq!(clock_text(575), "9:35 AM");
        assert_eq!(clock_text(720), "12 PM");
        assert_eq!(clock_text(1085), "6:05 PM");
        assert_eq!(abs_text(abs_min(3, 910), 2), "3:10 PM tomorrow");
        assert_eq!(abs_text(abs_min(5, 360), 2), "6 AM in 3 days");
    }

    #[test]
    fn schedule_hands_out_due_items_in_order_and_keeps_the_rest() {
        let mut s = Schedule::default();
        s.push(50, "late");
        s.push(10, "first");
        s.push(10, "second");
        assert_eq!(s.due(20), vec!["first", "second"]);
        assert_eq!(s.len(), 1);
        assert!(s.due(49).is_empty());
        assert_eq!(s.due(50), vec!["late"]);
    }
}
```

- [ ] **Step 2: Register the module and run the tests**

Run: `cargo test --lib desk::`
Expected: 4 PASS.

- [ ] **Step 3: Commit**

```bash
git add src/desk.rs src/lib.rs
git commit -m "feat: desk clock, action costs, schedule"
```

---

### Task 4: Load board and refusal gates

**Files:**
- Create: `src/board.rs`
- Modify: `src/lib.rs` (add `pub mod board;`)

**Interfaces:**
- Consumes: `roster::{Driver, Status}`, `ff_core::models::jobs::{Job, JobBoard, OfferOptions, required_hours}`, `ff_core::models::market::Market`, `ff_core::data::world::World`
- Produces:
  - `BOARD_SIZE`
  - `Load { job: Job, hot: bool, weather: Vec<career::StoredAlert> }`: the `weather` field is added in Task 6. In this task, define `Load { job, hot }` and Task 6 adds the field.
  - `morning_board(&World, terminal: &str, carrier_key: &str, &Market, seed: i64, roster: &[Driver]) -> Vec<Job>`
  - `route_for(&World, &Job) -> Option<Route>`
  - `refusal(&Job, &Driver, &World) -> Option<String>`
  - `load_line(&Job, hot: bool, &World) -> String`

- [ ] **Step 1: Write `src/board.rs` with its tests**

```rust
//! The day's loads, from ff-core's own job board at the terminal city.

use ff_core::data::world::World;
use ff_core::data::world_models::leg::Route;
use ff_core::models::jobs::{required_hours, Job, JobBoard, OfferOptions};
use ff_core::models::market::Market;

use crate::roster::{Driver, Status};

pub const BOARD_SIZE: usize = 8;

/// Offers for the whole roster: every credential anyone holds, the top level.
pub fn morning_board(world: &World, terminal: &str, carrier_key: &str, market: &Market, seed: i64, roster: &[Driver]) -> Vec<Job> {
    let mut endorsements: Vec<String> = roster.iter().flat_map(Driver::endorsements).collect();
    endorsements.sort();
    endorsements.dedup();
    let level = roster.iter().map(Driver::level).max().unwrap_or(1);
    JobBoard::seeded(world, seed).offers(
        terminal,
        &endorsements,
        OfferOptions { count: BOARD_SIZE, level, market: Some(market), carrier_key: Some(carrier_key), direct_freight: false },
    )
}

pub fn route_for(world: &World, job: &Job) -> Option<Route> {
    world.supported_route(&job.origin, &job.destination, None).ok().flatten()
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
    let need_h = required_hours(job.distance_mi, route.as_ref(), Some(world), Some(&driver.hos));
    if need_h > job.deadline_game_h {
        return Some(format!("{} doesn't have the hours to make this appointment.", driver.name));
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
    use ff_core::models::career::Career;
    use ff_core::data::world::get_world;

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
        assert_eq!(a, morning_board(w, &chicago(), "northstar", &m, 11, &roster));
        assert_ne!(a, morning_board(w, &chicago(), "northstar", &m, 12, &roster));
    }

    #[test]
    fn busy_driver_is_refused() {
        let w = get_world();
        let job = morning_board(w, &chicago(), "northstar", &Market::with_seed(1), 5, &[fresh(1)]).remove(0);
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
                b.offers(&chicago(), &senior.endorsements(), OfferOptions { count: 20, level: senior.level(), ..Default::default() })
            })
            .find(|j| j.cargo.min_level > rookie.level() || !j.cargo.missing_credentials(&rookie.endorsements()).is_empty())
            .expect("some load a rookie cannot take");
        assert!(refusal(&job, &rookie, w).unwrap().starts_with("Rookie can't take it."));
    }

    #[test]
    fn out_of_hours_driver_is_refused_but_fresh_driver_is_not() {
        let w = get_world();
        let mut job = morning_board(w, &chicago(), "northstar", &Market::with_seed(1), 5, &[fresh(1)]).remove(0);
        let route = route_for(w, &job);
        job.deadline_game_h = required_hours(job.distance_mi, route.as_ref(), Some(w), None) + 0.5;
        let mut tired = fresh(2);
        tired.hos.drive(660.0);
        assert!(refusal(&job, &tired, w).unwrap().contains("hours"));
        assert_eq!(refusal(&job, &fresh(3), w), None);
    }
}
```

Check the `Route` import path. The survey places `Route` at `data/world_models/leg.rs`. If `ff_core::data::world_models::leg::Route` is not public, use the re-export: `grep -rn "pub use.*Route" <ff-core>/src/data`.

- [ ] **Step 2: Register the module and run the tests**

Run: `cargo test --lib board::`
Expected: 4 PASS.

- [ ] **Step 3: Commit**

```bash
git add src/board.rs src/lib.rs
git commit -m "feat: morning board from ff-core JobBoard, FF refusal gates"
```

---

### Task 5: Trips

**Files:**
- Create: `src/trip.rs`
- Modify: `src/lib.rs` (add `pub mod trip;`)

**Interfaces:**
- Consumes: `board::route_for`, `roster::Driver`, `ff_core::models::jobs::{route_drive_hours, required_hours, job_payload, job_from_payload}`
- Produces: `Trip` (fields below), `Trip::start(id: u32, &Driver, Job, depart: i64, &World) -> Trip`, `deadline()`, `eta()`, `on_time()`, `progress(now)`, `at_fraction(f64) -> i64`, `fraction_at_city(&str) -> f64`, `city_at(now) -> String` (a route city key), `hours_short() -> bool`, `status_line(driver_name: &str, now, today, &World) -> String`

- [ ] **Step 1: Write `src/trip.rs` with its tests**

```rust
//! A load in progress. Its position is derived from time and the route's
//! miles; nothing moves in real time.

use ff_core::data::world::World;
use ff_core::models::jobs::{required_hours, route_drive_hours, Job};
use serde::{Deserialize, Serialize};

use crate::board::route_for;
use crate::roster::Driver;

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
}

impl Trip {
    pub fn start(id: u32, driver: &Driver, job: Job, depart: i64, world: &World) -> Trip {
        let route = route_for(world, &job);
        let drive_min = route_drive_hours(route.as_ref(), 0.0, Some(world)) * 60.0;
        let plan_min = required_hours(job.distance_mi, route.as_ref(), Some(world), Some(&driver.hos)) * 60.0;
        let (cities, city_mi) = match &route {
            Some(r) => {
                let mut cum = vec![0.0];
                for leg in &r.legs {
                    cum.push(cum.last().unwrap() + leg.miles);
                }
                (r.cities.clone(), cum)
            }
            None => (vec![job.origin.clone(), job.destination.clone()], vec![0.0, job.distance_mi]),
        };
        Trip {
            id,
            driver_id: driver.id,
            job,
            depart,
            drive_min,
            plan_min,
            legal_min: driver.legal_left_min(),
            delay_min: 0.0,
            hours_resolved: false,
            hot: false,
            cities,
            city_mi,
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
        let total = (self.plan_min + self.delay_min).max(1.0);
        ((now - self.depart) as f64 / total).clamp(0.0, 1.0)
    }

    /// The absolute minute the truck reaches a fraction of the route.
    pub fn at_fraction(&self, f: f64) -> i64 {
        self.depart + (f.clamp(0.0, 1.0) * (self.plan_min + self.delay_min)).round() as i64
    }

    pub fn fraction_at_city(&self, city: &str) -> f64 {
        let total = self.city_mi.last().copied().unwrap_or(0.0).max(1.0);
        self.cities.iter().position(|c| c == city).map(|i| self.city_mi[i] / total).unwrap_or(0.5)
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
        !rest_planned && !self.hours_resolved && self.drive_min + self.delay_min > self.legal_min
    }

    /// "Alma Reyes: near Des Moines, 42 percent, due 3:10 PM tomorrow, on time."
    pub fn status_line(&self, driver_name: &str, now: i64, today: i64, world: &World) -> String {
        format!(
            "{driver_name}: {} to {}, near {}, {} percent, arriving {}, {}.",
            self.job.cargo.label,
            world.spoken_city(&self.job.destination, None),
            world.spoken_city(&self.city_at(now), None),
            (self.progress(now) * 100.0).round(),
            crate::desk::abs_text(self.eta(), today),
            if self.on_time() { "on time" } else { "running late" },
        )
    }
}

/// Job has no serde; ff-core's own payload form round-trips it.
mod job_json {
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
        let job = morning_board(w, &chicago(), "northstar", &Market::with_seed(1), 5, &[d.clone()]).remove(0);
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
    fn trip_round_trips_through_json() {
        let t = trip();
        let back: Trip = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.job, t.job);
        assert_eq!(back.eta(), t.eta());
    }
}
```

- [ ] **Step 2: Register the module and run the tests**

Run: `cargo test --lib trip::`
Expected: 4 PASS. If `trip_round_trips` fails on `back.job == t.job` because a field such as `origin_spoken` is not in the payload, compare `job_payload(&back.job) == job_payload(&t.job)` instead. The payload is the persisted contract.

- [ ] **Step 3: Commit**

```bash
git add src/trip.rs src/lib.rs
git commit -m "feat: trips with ff-core HOS plan, position and ETA"
```

---

### Task 6: Career core: new career, briefing, assign, check, close and overnight

**Files:**
- Create: `src/career.rs`
- Modify: `src/board.rs` (add `weather: Vec<StoredAlert>` to `Load`), `src/lib.rs` (add `pub mod career;`)

**Interfaces:**
- Consumes: Tasks 2–5, `ff_core::models::start_options::{start_option, pay_plan_for_key, START_OPTIONS}`
- Produces:
  - Constants: `STARTING_FUNDS`, `TRUCK_COST_PER_DAY`, `DEADHEAD_RATE`, `SCHEMA_VERSION: u32 = 1`
  - `Record { delivered, on_time, abandoned, days_worked: u32 }` with `on_time_pct() -> f64`
  - `StoredAlert { event: String, city: String, delay_min: f64, avoid: bool }`
  - `Game { schema_version, name, seed, day, carrier_key, terminal, funds, reputation, record, drivers, trips, market, shipper_goodwill: BTreeMap<String,i32>, weather: Vec<Vec<StoredAlert>>, weather_live: bool, news: Vec<String>, next_id: u32 }`
  - `Ring`: an empty enum here (`pub enum Ring {}`). Task 7 adds `Ring::Incident` and Task 9 adds `Ring::Call`.
  - `Shift { clock: DeskClock, board: Vec<Load>, schedule: Schedule<Ring>, pending: VecDeque<Ring> }` with `now(day) -> i64`. Task 9 adds `hot_offers: Vec<Load>` and Task 10 adds `applicants: Vec<hiring::Applicant>`.
  - `ActError::{PhoneRinging, Closed, Refused(String), NoSuchItem, RosterFull, NoFunds}` with `spoken() -> String`
  - `Game::new(carrier_key: &str, seed: i64, &World) -> Game`
  - `morning(&World) -> Shift`, `todays_routes(&World) -> Vec<Option<Route>>`, `set_weather(Option<Vec<Vec<StoredAlert>>>)`, `briefing(&Shift, &World) -> Vec<String>`
  - `assign(&mut Shift, load: usize, driver_id: u32, &World) -> Result<Vec<String>, ActError>`
  - `check_truck(&mut Shift, trip_id: u32, &World) -> Result<String, ActError>`
  - `close_day(&mut Shift, &World) -> Vec<String>`
  - `driver(id) -> Option<&Driver>`, `driver_mut(id)`, `trip_mut(id)`, `take_id() -> u32`, `ensure_open(&Shift) -> Result<(), ActError>`, `ring_due(&mut Shift)`, `add_delay(&mut Shift, trip_id, minutes)`

- [ ] **Step 1: Add the weather field to `Load` in `src/board.rs`**

```rust
#[derive(Clone, Debug)]
pub struct Load {
    pub job: Job,
    pub hot: bool,
    pub weather: Vec<crate::career::StoredAlert>,
}
```

- [ ] **Step 2: Write `src/career.rs`**

```rust
//! The career (persistent, saved at day boundaries) and today's shift
//! (rebuilt from the career's seed every morning).

use std::collections::{BTreeMap, VecDeque};

use ff_core::data::world::World;
use ff_core::data::world_models::leg::Route;
use ff_core::models::market::Market;
use ff_core::models::start_options::{pay_plan_for_key, start_option};
use serde::{Deserialize, Serialize};

use crate::board::{morning_board, refusal, route_for, Load};
use crate::desk::{abs_min, abs_text, Action, DeskClock, Schedule, SHIFT_END, SHIFT_START};
use crate::roster::{seeded_drivers, Driver, Status};
use crate::trip::Trip;

pub const STARTING_FUNDS: f64 = 25_000.0;
pub const STARTING_DRIVERS: usize = 3;
pub const TRUCK_COST_PER_DAY: f64 = 180.0;
pub const DEADHEAD_RATE: f64 = 0.65;
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub delivered: u32,
    pub on_time: u32,
    pub abandoned: u32,
    pub days_worked: u32,
}

impl Record {
    pub fn on_time_pct(&self) -> f64 {
        if self.delivered == 0 { 100.0 } else { (self.on_time as f64 * 100.0 / self.delivered as f64).round() }
    }
}

/// One weather warning on a board load's route, as stored in the day's save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredAlert {
    pub event: String,
    /// Route city (key or name) the alert was found at.
    pub city: String,
    pub delay_min: f64,
    pub avoid: bool,
}

/// Something that makes the phone ring. Filled in by incidents and phone.
#[derive(Clone, Debug, PartialEq)]
pub enum Ring {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    pub schema_version: u32,
    pub name: String,
    pub seed: i64,
    pub day: i64,
    pub carrier_key: String,
    pub terminal: String,
    pub funds: f64,
    pub reputation: f64,
    #[serde(default)]
    pub record: Record,
    pub drivers: Vec<Driver>,
    #[serde(default)]
    pub trips: Vec<Trip>,
    #[serde(default)]
    pub market: Market,
    #[serde(default)]
    pub shipper_goodwill: BTreeMap<String, i32>,
    /// Today's board weather, by morning board position.
    #[serde(default)]
    pub weather: Vec<Vec<StoredAlert>>,
    #[serde(default)]
    pub weather_live: bool,
    /// Overnight news read in the next morning's briefing.
    #[serde(default)]
    pub news: Vec<String>,
    #[serde(default)]
    pub next_id: u32,
}

pub struct Shift {
    pub clock: DeskClock,
    pub board: Vec<Load>,
    pub schedule: Schedule<Ring>,
    pub pending: VecDeque<Ring>,
}

impl Shift {
    pub fn now(&self, day: i64) -> i64 {
        abs_min(day, self.clock.minute)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActError {
    PhoneRinging,
    Closed,
    Refused(String),
    NoSuchItem,
    RosterFull,
    NoFunds,
}

impl ActError {
    pub fn spoken(&self) -> String {
        match self {
            ActError::PhoneRinging => "The phone is ringing. Press Enter to answer.".into(),
            ActError::Closed => "The desk is closed for the day. Close the day to go on.".into(),
            ActError::Refused(why) => why.clone(),
            ActError::NoSuchItem => "That is no longer available.".into(),
            ActError::RosterFull => "The roster is full at 10 drivers.".into(),
            ActError::NoFunds => "There isn't enough money for that.".into(),
        }
    }
}

impl Game {
    pub fn new(carrier_key: &str, seed: i64, world: &World) -> Game {
        let opt = start_option(Some(carrier_key));
        let drivers = seeded_drivers(&mut crate::rng(seed, 0, "roster"), 1, STARTING_DRIVERS, &[]);
        Game {
            schema_version: SCHEMA_VERSION,
            name: opt.carrier_name.to_string(),
            seed,
            day: 1,
            carrier_key: opt.key.to_string(),
            terminal: world.resolve_city_key(opt.default_city),
            funds: STARTING_FUNDS,
            reputation: opt.starting_reputation,
            record: Record::default(),
            next_id: drivers.len() as u32 + 1,
            drivers,
            trips: Vec::new(),
            market: Market::with_seed(seed),
            shipper_goodwill: BTreeMap::new(),
            weather: Vec::new(),
            weather_live: false,
            news: Vec::new(),
        }
    }

    pub fn take_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id - 1
    }

    pub fn driver(&self, id: u32) -> Option<&Driver> {
        self.drivers.iter().find(|d| d.id == id)
    }

    pub fn driver_mut(&mut self, id: u32) -> Option<&mut Driver> {
        self.drivers.iter_mut().find(|d| d.id == id)
    }

    pub fn trip_mut(&mut self, id: u32) -> Option<&mut Trip> {
        self.trips.iter_mut().find(|t| t.id == id)
    }

    fn board_jobs(&self, world: &World) -> Vec<ff_core::models::jobs::Job> {
        let seed = crate::seed_for(self.seed, self.day, "board");
        morning_board(world, &self.terminal, &self.carrier_key, &self.market, seed, &self.drivers)
    }

    pub fn todays_routes(&self, world: &World) -> Vec<Option<Route>> {
        self.board_jobs(world).iter().map(|j| route_for(world, j)).collect()
    }

    /// `Some` = live NWS alerts for today's board; `None` = seeded weather.
    pub fn set_weather(&mut self, fetched: Option<Vec<Vec<StoredAlert>>>) {
        self.weather_live = fetched.is_some();
        self.weather = fetched.unwrap_or_default();
    }

    pub fn morning(&self, world: &World) -> Shift {
        let board = self
            .board_jobs(world)
            .into_iter()
            .enumerate()
            .map(|(i, job)| Load { job, hot: false, weather: self.weather.get(i).cloned().unwrap_or_default() })
            .collect();
        Shift { clock: DeskClock::morning(), board, schedule: Schedule::default(), pending: VecDeque::new() }
    }

    pub fn briefing(&self, shift: &Shift, world: &World) -> Vec<String> {
        let mut lines = vec![format!(
            "Day {} at {}, {} terminal. {}.",
            self.day,
            self.name,
            world.spoken_city(&self.terminal, None),
            shift.clock.spoken()
        )];
        lines.push(format!("Funds {}. On time {} percent.", crate::money(self.funds), self.record.on_time_pct()));
        let idle: Vec<&str> = self.drivers.iter().filter(|d| d.status == Status::Idle).map(|d| d.name.as_str()).collect();
        lines.push(match idle.len() {
            0 => "No drivers available.".into(),
            n => format!("{n} available: {}.", idle.join(", ")),
        });
        for t in &self.trips {
            let name = self.driver(t.driver_id).map(|d| d.name.as_str()).unwrap_or("A driver");
            lines.push(format!("{name} is still out, arriving {}.", abs_text(t.eta(), self.day)));
        }
        lines.extend(self.news.iter().cloned());
        lines.push(format!("{} loads on the board.", shift.board.len()));
        for load in shift.board.iter().filter(|l| !l.weather.is_empty()) {
            let a = &load.weather[0];
            lines.push(format!(
                "Weather: {} near {} on the {} load.",
                a.event,
                world.spoken_city(&a.city, None),
                world.spoken_city(&load.job.destination, None)
            ));
        }
        lines
    }

    pub fn ensure_open(&self, shift: &Shift) -> Result<(), ActError> {
        if !shift.pending.is_empty() {
            return Err(ActError::PhoneRinging);
        }
        if shift.clock.closed() {
            return Err(ActError::Closed);
        }
        Ok(())
    }

    /// Move everything now due onto the ringing phone, dropping stale rings.
    pub fn ring_due(&mut self, shift: &mut Shift) {
        let now = shift.now(self.day);
        for ring in shift.schedule.due(now) {
            if self.ring_live(&ring) {
                shift.pending.push_back(ring);
            }
        }
    }

    fn ring_live(&self, ring: &Ring) -> bool {
        match *ring {}
    }

    /// Delay a trip; a delay that breaks the driver's hours rings the phone.
    pub fn add_delay(&mut self, shift: &mut Shift, trip_id: u32, minutes: f64) {
        let Some(trip) = self.trip_mut(trip_id) else { return };
        trip.delay_min += minutes;
        let _ = shift; // Task 7 schedules the hours-short incident here.
    }

    pub fn assign(&mut self, shift: &mut Shift, load: usize, driver_id: u32, world: &World) -> Result<Vec<String>, ActError> {
        self.ensure_open(shift)?;
        let job = &shift.board.get(load).ok_or(ActError::NoSuchItem)?.job;
        let driver = self.driver(driver_id).ok_or(ActError::NoSuchItem)?;
        if let Some(why) = refusal(job, driver, world) {
            return Err(ActError::Refused(why));
        }
        shift.clock.spend(Action::Assign);
        let now = shift.now(self.day);
        let id = self.take_id();
        let loaded = shift.board.remove(load);
        let driver = self.driver(driver_id).unwrap();
        let mut trip = Trip::start(id, driver, loaded.job, now, world);
        trip.hot = loaded.hot;
        let line = format!(
            "{} is rolling: {} to {}, arriving {}.",
            driver.name,
            trip.job.cargo.label,
            world.spoken_city(&trip.job.destination, None),
            abs_text(trip.eta(), self.day)
        );
        self.driver_mut(driver_id).unwrap().status = Status::OnLoad;
        self.trips.push(trip);
        self.ring_due(shift);
        Ok(vec![line])
    }

    pub fn check_truck(&mut self, shift: &mut Shift, trip_id: u32, world: &World) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let trip = self.trips.iter().find(|t| t.id == trip_id).ok_or(ActError::NoSuchItem)?;
        shift.clock.spend(Action::CheckTruck);
        let name = self.driver(trip.driver_id).map(|d| d.name.clone()).unwrap_or_default();
        let line = trip.status_line(&name, shift.now(self.day), self.day, world);
        self.ring_due(shift);
        Ok(line)
    }

    /// Close the desk, settle what arrives overnight, rest everyone, and
    /// roll to the next morning. Returns what was said.
    pub fn close_day(&mut self, shift: &mut Shift, world: &World) -> Vec<String> {
        shift.clock.minute = shift.clock.minute.max(SHIFT_START);
        let next_morning = abs_min(self.day + 1, SHIFT_START);
        let pay_share = pay_plan_for_key(Some(&self.carrier_key)).pay_share;
        let mut news = Vec::new();
        let (arrived, rolling): (Vec<Trip>, Vec<Trip>) = std::mem::take(&mut self.trips).into_iter().partition(|t| t.eta() <= next_morning);
        for trip in arrived {
            let hours_taken = (trip.eta() - trip.depart) as f64 / 60.0;
            let gross = trip.job.payout(hours_taken, 0.0, 0.0);
            let driver_pay = gross * pay_share;
            self.funds += gross - driver_pay;
            self.record.delivered += 1;
            self.record.on_time += trip.on_time() as u32;
            if let Some(d) = self.driver_mut(trip.driver_id) {
                d.career.record_delivery(trip.job.distance_mi, driver_pay, trip.on_time(), 0.0, 1.0, 1.0);
                d.status = Status::Idle;
                news.push(format!(
                    "{} delivered {} to {}, {}. {} to the carrier.",
                    d.name,
                    trip.job.cargo.label,
                    world.spoken_city(&trip.job.destination, None),
                    if trip.on_time() { "on time" } else { "late" },
                    crate::money(gross - driver_pay)
                ));
            }
        }
        for trip in &rolling {
            if let Some(d) = self.driver_mut(trip.driver_id) {
                d.status = Status::OutOnRun;
            }
        }
        self.trips = rolling;
        let truck_costs = TRUCK_COST_PER_DAY * self.drivers.len() as f64;
        self.funds -= truck_costs;
        news.push(format!("Truck costs today: {}.", crate::money(truck_costs)));
        for d in &mut self.drivers {
            d.hos.sleep(); // everyone gets a 10-hour reset overnight
            if d.status == Status::OffDuty {
                d.status = Status::Idle;
            }
        }
        self.record.days_worked += 1;
        self.day += 1;
        self.market.advance_to(self.day);
        self.news = news.clone();
        shift.clock.minute = SHIFT_END;
        news
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ff_core::data::world::get_world;

    pub(crate) fn game() -> Game {
        Game::new("northstar", 42, get_world())
    }

    /// First (load, driver) pair the gates allow.
    pub(crate) fn legal_pair(g: &Game, s: &Shift) -> (usize, u32) {
        let w = get_world();
        for (i, l) in s.board.iter().enumerate() {
            for d in &g.drivers {
                if refusal(&l.job, d, w).is_none() {
                    return (i, d.id);
                }
            }
        }
        panic!("seed 42 has no legal pair; pick another seed");
    }

    #[test]
    fn new_career_matches_the_carrier() {
        let g = game();
        assert_eq!(g.drivers.len(), 3);
        assert_eq!(g.funds, STARTING_FUNDS);
        assert_eq!(g.terminal, get_world().resolve_city_key(start_option(Some("northstar")).default_city));
    }

    #[test]
    fn briefing_names_drivers_loads_and_funds() {
        let g = game();
        let s = g.morning(get_world());
        let text = g.briefing(&s, get_world()).join(" ");
        assert!(text.contains("Day 1"));
        assert!(text.contains("Funds $25,000"));
        assert!(text.contains(&g.drivers[0].name));
        assert!(text.contains("8 loads on the board"));
    }

    #[test]
    fn assign_costs_ten_minutes_and_puts_a_driver_on_the_road() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        let lines = g.assign(&mut s, load, driver, w).unwrap();
        assert!(lines[0].contains("is rolling"));
        assert_eq!(s.clock.minute, SHIFT_START + 10);
        assert_eq!(s.board.len(), 7);
        assert_eq!(g.driver(driver).unwrap().status, Status::OnLoad);
        assert_eq!(g.trips.len(), 1);
    }

    #[test]
    fn refused_assignment_spends_no_time() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        for d in &mut g.drivers {
            d.status = Status::OffDuty;
        }
        let err = g.assign(&mut s, 0, g.drivers[0].id, w).unwrap_err();
        assert!(matches!(err, ActError::Refused(_)));
        assert_eq!(s.clock.minute, SHIFT_START);
        assert_eq!(s.board.len(), 8);
    }

    #[test]
    fn closed_desk_refuses_actions_but_the_day_still_closes() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        s.clock.minute = SHIFT_END;
        let (load, driver) = (0, g.drivers[0].id);
        assert_eq!(g.assign(&mut s, load, driver, w), Err(ActError::Closed));
        g.close_day(&mut s, w);
        assert_eq!(g.day, 2);
    }

    #[test]
    fn overnight_settles_arrivals_and_carries_long_runs() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        let eta = g.trips[0].eta();
        let funds = g.funds;
        let news = g.close_day(&mut s, w);
        if eta <= abs_min(2, SHIFT_START) {
            assert_eq!(g.record.delivered, 1);
            assert!(g.trips.is_empty());
            assert!(news[0].contains("delivered"));
            assert!(g.funds > funds - TRUCK_COST_PER_DAY * 3.0);
        } else {
            assert_eq!(g.driver(driver).unwrap().status, Status::OutOnRun);
            assert_eq!(g.trips.len(), 1);
        }
        assert_eq!(g.day, 2);
        assert!(g.drivers.iter().all(|d| d.hos.driving_min == 0.0), "overnight reset");
    }
}
```

- [ ] **Step 3: Register the module and run the tests**

Run: `cargo test --lib career::`
Expected: 6 PASS. If `legal_pair` panics, change the seed in `tests::game()` to one that has a legal pair. Do not weaken the gates.

- [ ] **Step 4: Commit**

```bash
git add src/career.rs src/board.rs src/lib.rs
git commit -m "feat: career day loop: briefing, assign, check, overnight settlement"
```

---

### Task 7: Incidents: hours short, breakdown, detention, and the phone queue

**Files:**
- Create: `src/incidents.rs`
- Modify: `src/career.rs` (fill `Ring`, `ring_live`, and `add_delay`; roll incidents in `assign`; add `answer` and `resolve`), `src/lib.rs` (add `pub mod incidents;`)

**Interfaces:**
- Consumes: `career::{Game, Shift, Ring, StoredAlert, DEADHEAD_RATE}`, `trip::Trip`, `ff_core::models::jobs::relay::RELAY_RANGE_MI`
- Produces:
  - `Kind::{HoursShort, Breakdown, Weather { event: String, city: String, delay_min: f64, avoid: bool }, Detention}`: the `Weather` variant and its choices are added in Task 8.
  - `Incident { trip_id: u32, kind: Kind }`
  - `Choice::{Relay(u32), ParkLate, CallReceiver, Repair, SendTruck(u32), Wait, BillDetention, PullDriver}` (Task 8 adds `Hold`, `Reroute` and `PushThrough`), with `label(&Game, &World) -> String`
  - `roll_for_trip(&Game, &Trip, alerts: &[StoredAlert]) -> Vec<(i64, Incident)>`
  - `script(&Game, &Incident, now: i64, &World) -> String`
  - `choices(&Game, &Incident, now: i64, &World) -> Vec<Choice>`
  - `apply(&mut Game, &mut Shift, &Incident, Choice, &World) -> Vec<String>`
  - `Ring::Incident(Incident)`, `Ring::title(&Game, &Shift, &World) -> String`, `Ring::labels(&Game, &Shift, &World) -> Vec<String>`
  - `Game::answer(&mut Shift) -> Result<Ring, ActError>`, `Game::resolve(&mut Shift, &Ring, choice: usize, &World) -> Vec<String>`

- [ ] **Step 1: Write the failing tests at the bottom of `src/incidents.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::tests::{game, legal_pair};
    use crate::career::{ActError, Ring};
    use crate::desk::SHIFT_START;
    use ff_core::data::world::get_world;

    /// A game with one trip rolling, and that trip's id.
    fn rolling() -> (Game, Shift, u32) {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let (load, driver) = legal_pair(&g, &s);
        g.assign(&mut s, load, driver, w).unwrap();
        s.schedule = Default::default();
        s.pending.clear();
        let id = g.trips[0].id;
        (g, s, id)
    }

    #[test]
    fn rolls_are_deterministic_for_a_reloaded_morning() {
        let (g, _, _) = rolling();
        let t = &g.trips[0];
        assert_eq!(roll_for_trip(&g, t, &[]), roll_for_trip(&g, t, &[]));
    }

    #[test]
    fn detention_wait_delays_and_bill_pays_and_sours_the_shipper() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let inc = Incident { trip_id: id, kind: Kind::Detention };
        apply(&mut g, &mut s, &inc, Choice::Wait, w);
        assert_eq!(g.trips[0].delay_min, DETENTION_MIN);
        let funds = g.funds;
        apply(&mut g, &mut s, &inc, Choice::BillDetention, w);
        assert_eq!(g.funds, funds + DETENTION_BILL);
        assert_eq!(g.shipper_goodwill[&g.trips[0].job.origin_location], -1);
    }

    #[test]
    fn pulling_the_driver_abandons_the_load_and_drops_its_later_rings() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let driver = g.trips[0].driver_id;
        let later = g.trips[0].depart + 30;
        s.schedule.push(later, Ring::Incident(Incident { trip_id: id, kind: Kind::Breakdown }));
        apply(&mut g, &mut s, &Incident { trip_id: id, kind: Kind::Detention }, Choice::PullDriver, w);
        assert!(g.trips.is_empty());
        assert_eq!(g.record.abandoned, 1);
        assert_eq!(g.driver(driver).unwrap().status, Status::Idle);
        s.clock.minute = SHIFT_START + 60;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty(), "a ring about a pulled load never rings");
    }

    #[test]
    fn repair_costs_money_and_time() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let funds = g.funds;
        apply(&mut g, &mut s, &Incident { trip_id: id, kind: Kind::Breakdown }, Choice::Repair, w);
        assert_eq!(g.funds, funds - REPAIR_COST);
        assert_eq!(g.trips[0].delay_min, REPAIR_MIN);
    }

    #[test]
    fn send_truck_swaps_drivers_and_is_offered_only_with_an_idle_driver() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let inc = Incident { trip_id: id, kind: Kind::Breakdown };
        let now = s.now(g.day);
        let spare = g.drivers.iter().find(|d| d.status == Status::Idle).unwrap().id;
        assert!(choices(&g, &inc, now, w).contains(&Choice::SendTruck(spare)));
        let first = g.trips[0].driver_id;
        apply(&mut g, &mut s, &inc, Choice::SendTruck(spare), w);
        assert_eq!(g.trips[0].driver_id, spare);
        assert_eq!(g.driver(first).unwrap().status, Status::OffDuty);
        for d in &mut g.drivers {
            if d.status == Status::Idle {
                d.status = Status::OffDuty;
            }
        }
        assert!(!choices(&g, &inc, now, w).iter().any(|c| matches!(c, Choice::SendTruck(_))));
        assert!(choices(&g, &inc, now, w).contains(&Choice::Repair), "still answerable");
    }

    #[test]
    fn a_delay_that_breaks_the_hours_rings_hours_short() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let t = g.trip_mut(id).unwrap();
        t.plan_min = t.drive_min + 30.0;
        t.legal_min = t.drive_min + 60.0;
        g.add_delay(&mut s, id, 90.0);
        assert!(s.schedule.iter().any(|(_, r)| matches!(r, Ring::Incident(Incident { kind: Kind::HoursShort, .. }))));
        let inc = Incident { trip_id: id, kind: Kind::HoursShort };
        apply(&mut g, &mut s, &inc, Choice::ParkLate, w);
        assert!(g.trips[0].hours_resolved);
        assert_eq!(g.trips[0].delay_min, 90.0 + RESET_MIN);
    }

    #[test]
    fn call_receiver_always_parks_and_success_moves_the_appointment() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let deadline = g.trips[0].job.deadline_game_h;
        let rep = g.reputation;
        let lines = apply(&mut g, &mut s, &Incident { trip_id: id, kind: Kind::HoursShort }, Choice::CallReceiver, w);
        assert_eq!(g.reputation, rep - 1.0);
        assert_eq!(g.trips[0].delay_min, RESET_MIN);
        let moved = g.trips[0].job.deadline_game_h == deadline + RESET_MIN / 60.0;
        assert_eq!(moved, lines.join(" ").contains("moved the appointment"));
    }

    #[test]
    fn a_ringing_phone_blocks_the_desk_until_answered() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        s.pending.push_back(Ring::Incident(Incident { trip_id: id, kind: Kind::Detention }));
        assert_eq!(g.check_truck(&mut s, id, w), Err(ActError::PhoneRinging));
        let minute = s.clock.minute;
        let ring = g.answer(&mut s).unwrap();
        assert_eq!(s.clock.minute, minute + 5);
        let labels = ring.labels(&g, &s, w);
        assert_eq!(labels.len(), 3);
        g.resolve(&mut s, &ring, 0, w);
        assert!(g.check_truck(&mut s, id, w).is_ok());
    }

    #[test]
    fn rings_after_the_close_never_ring() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        s.schedule.push(crate::desk::abs_min(g.day, crate::desk::SHIFT_END + 30), Ring::Incident(Incident { trip_id: id, kind: Kind::Breakdown }));
        s.clock.minute = crate::desk::SHIFT_END;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib incidents::`
Expected: FAIL to compile (`roll_for_trip`, `apply` and the rest are not defined yet).

- [ ] **Step 3: Write the implementation at the top of `src/incidents.rs`**

```rust
//! The board talks back: the launch incidents, their triggers, and what
//! each choice does to the trip, funds, goodwill and service record.

use ff_core::data::world::World;
use ff_core::models::jobs::relay::RELAY_RANGE_MI;
use ff_core::models::jobs::DEADLINE_AVG_MPH;

use crate::career::{Game, Shift, StoredAlert, DEADHEAD_RATE};
use crate::roster::Status;
use crate::trip::Trip;

pub const BREAKDOWN_PER_LEG: f64 = 0.03;
pub const DETENTION_BASE: f64 = 0.12;
pub const DETENTION_SLOW_SHIPPER: f64 = 0.35;
pub const DETENTION_SOURED: f64 = 0.10;
pub const REPAIR_COST: f64 = 450.0;
pub const REPAIR_MIN: f64 = 120.0;
pub const RESET_MIN: f64 = 600.0;
pub const DETENTION_MIN: f64 = 120.0;
pub const DETENTION_BILL: f64 = 150.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    HoursShort,
    Breakdown,
    Detention,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Incident {
    pub trip_id: u32,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Relay(u32),
    ParkLate,
    CallReceiver,
    Repair,
    SendTruck(u32),
    Wait,
    BillDetention,
    PullDriver,
}

impl Choice {
    pub fn label(&self, game: &Game) -> String {
        let name = |id: u32| game.driver(id).map(|d| d.name.clone()).unwrap_or_default();
        match *self {
            Choice::Relay(id) => format!("Relay the load to {}", name(id)),
            Choice::ParkLate => "Park for a 10-hour reset and deliver late".into(),
            Choice::CallReceiver => "Call the receiver for a new appointment".into(),
            Choice::Repair => format!("Roadside repair, {} and about 2 hours", crate::money(REPAIR_COST)),
            Choice::SendTruck(id) => format!("Send {} out with another truck", name(id)),
            Choice::Wait => "Wait it out".into(),
            Choice::BillDetention => format!("Wait and bill {} detention", crate::money(DETENTION_BILL)),
            Choice::PullDriver => "Pull the driver and drop the load".into(),
        }
    }
}

/// Some shippers are slow to load, and it's the same ones every day.
fn slow_shipper(facility: &str) -> bool {
    facility.bytes().map(u32::from).sum::<u32>() % 4 == 0
}

/// Incidents seeded for a trip at dispatch, keyed by absolute game minute.
pub fn roll_for_trip(game: &Game, trip: &Trip, alerts: &[StoredAlert]) -> Vec<(i64, Incident)> {
    let _ = alerts; // weather: Task 8
    let mut rng = crate::rng(game.seed, game.day, &format!("incidents:{}", trip.id));
    let mut out = Vec::new();
    let soured = game.shipper_goodwill.get(&trip.job.origin_location).copied().unwrap_or(0) < 0;
    let detention = if slow_shipper(&trip.job.origin_location) { DETENTION_SLOW_SHIPPER } else { DETENTION_BASE }
        + if soured { DETENTION_SOURED } else { 0.0 };
    if rng.random() < detention {
        out.push((trip.depart + 60, Incident { trip_id: trip.id, kind: Kind::Detention }));
    }
    let total = trip.city_mi.last().copied().unwrap_or(0.0).max(1.0);
    for leg in 0..trip.city_mi.len().saturating_sub(1) {
        if rng.random() < BREAKDOWN_PER_LEG {
            let mid = (trip.city_mi[leg] + trip.city_mi[leg + 1]) / 2.0 / total;
            out.push((trip.at_fraction(mid), Incident { trip_id: trip.id, kind: Kind::Breakdown }));
            break;
        }
    }
    out
}

fn trip<'a>(game: &'a Game, inc: &Incident) -> Option<&'a Trip> {
    game.trips.iter().find(|t| t.id == inc.trip_id)
}

fn driver_name(game: &Game, inc: &Incident) -> String {
    trip(game, inc).and_then(|t| game.driver(t.driver_id)).map(|d| d.name.clone()).unwrap_or_default()
}

pub fn script(game: &Game, inc: &Incident, now: i64, world: &World) -> String {
    let who = driver_name(game, inc);
    let Some(t) = trip(game, inc) else { return String::new() };
    let near = world.spoken_city(&t.city_at(now), None);
    match &inc.kind {
        Kind::HoursShort => format!(
            "{who} here, near {near}. With the delays I'm out of hours before the {} appointment. What do you want me to do?",
            world.spoken_city(&t.job.destination, None)
        ),
        Kind::Breakdown => format!("{who} here. I've broken down near {near}. The truck isn't going anywhere without a mechanic."),
        Kind::Detention => format!(
            "{who} here, at the shipper in {}. They're not ready to load. They say at least two hours.",
            world.spoken_city(&t.job.origin, None)
        ),
    }
}

/// Deadhead miles from the terminal to where the truck is now.
fn deadhead_mi(game: &Game, t: &Trip, now: i64, world: &World) -> f64 {
    world
        .supported_route(&game.terminal, &t.city_at(now), None)
        .ok()
        .flatten()
        .map(|r| r.miles())
        .unwrap_or(0.0)
}

fn first_idle(game: &Game) -> Option<u32> {
    game.drivers.iter().find(|d| d.status == Status::Idle).map(|d| d.id)
}

pub fn choices(game: &Game, inc: &Incident, now: i64, world: &World) -> Vec<Choice> {
    let Some(t) = trip(game, inc) else { return Vec::new() };
    match inc.kind {
        Kind::HoursShort => {
            let mut c = Vec::new();
            if let Some(id) = first_idle(game).filter(|_| deadhead_mi(game, t, now, world) <= RELAY_RANGE_MI) {
                c.push(Choice::Relay(id));
            }
            c.extend([Choice::ParkLate, Choice::CallReceiver]);
            c
        }
        Kind::Breakdown => {
            let mut c = vec![Choice::Repair];
            if let Some(id) = first_idle(game) {
                c.push(Choice::SendTruck(id));
            }
            c
        }
        Kind::Detention => vec![Choice::Wait, Choice::BillDetention, Choice::PullDriver],
    }
}

/// Put `new_driver` on the load where it stands: they deadhead out from the
/// terminal, the truck waits for them, and the first driver is done today.
fn swap_driver(game: &mut Game, shift: &mut Shift, trip_id: u32, new_driver: u32, world: &World) -> String {
    let now = shift.now(game.day);
    let t = game.trips.iter().find(|t| t.id == trip_id).unwrap();
    let miles = deadhead_mi(game, t, now, world);
    let minutes = miles / DEADLINE_AVG_MPH * 60.0;
    let old = t.driver_id;
    game.funds -= miles * DEADHEAD_RATE;
    if let Some(d) = game.driver_mut(old) {
        d.status = Status::OffDuty;
    }
    let d = game.driver_mut(new_driver).unwrap();
    d.status = Status::OnLoad;
    d.hos.drive(minutes);
    let (legal, name) = (d.legal_left_min(), d.name.clone());
    let t = game.trip_mut(trip_id).unwrap();
    t.driver_id = new_driver;
    t.legal_min = legal + t.drive_min * t.progress(now);
    t.hours_resolved = true;
    game.add_delay(shift, trip_id, minutes);
    format!("{name} is heading out, {} miles of deadhead.", miles.round())
}

pub fn apply(game: &mut Game, shift: &mut Shift, inc: &Incident, choice: Choice, world: &World) -> Vec<String> {
    let id = inc.trip_id;
    let Some(t) = game.trips.iter().find(|t| t.id == id) else { return Vec::new() };
    let origin = t.job.origin_location.clone();
    let mut out = Vec::new();
    match choice {
        Choice::Relay(driver) | Choice::SendTruck(driver) => {
            if choice == Choice::SendTruck(driver) {
                game.funds -= REPAIR_COST; // the broken truck still gets fixed
            }
            out.push(swap_driver(game, shift, id, driver, world));
        }
        Choice::ParkLate => {
            game.trip_mut(id).unwrap().hours_resolved = true;
            game.add_delay(shift, id, RESET_MIN);
            out.push("Parking for a 10-hour reset. The load will be late.".into());
        }
        Choice::CallReceiver => {
            game.reputation -= 1.0;
            let odds = (0.5 + (game.reputation - 50.0) / 200.0).clamp(0.2, 0.8);
            let mut rng = crate::rng(game.seed, game.day, &format!("receiver:{id}:{}", shift.clock.minute));
            let t = game.trip_mut(id).unwrap();
            t.hours_resolved = true;
            if rng.random() < odds {
                t.job.deadline_game_h += RESET_MIN / 60.0;
                out.push("The receiver moved the appointment. Parking for a reset.".into());
            } else {
                out.push("The receiver won't move it. Parking for a reset; the load will be late.".into());
            }
            game.add_delay(shift, id, RESET_MIN);
        }
        Choice::Repair => {
            game.funds -= REPAIR_COST;
            game.add_delay(shift, id, REPAIR_MIN);
            out.push("A mechanic is on the way.".into());
        }
        Choice::Wait => {
            game.add_delay(shift, id, DETENTION_MIN);
            out.push("Waiting on the dock.".into());
        }
        Choice::BillDetention => {
            game.funds += DETENTION_BILL;
            *game.shipper_goodwill.entry(origin).or_insert(0) -= 1;
            game.add_delay(shift, id, DETENTION_MIN);
            out.push("Detention billed. The shipper won't forget it.".into());
        }
        Choice::PullDriver => {
            let t = game.trips.iter().position(|t| t.id == id).unwrap();
            let t = game.trips.remove(t);
            if let Some(d) = game.driver_mut(t.driver_id) {
                d.status = Status::Idle;
            }
            game.record.abandoned += 1;
            game.reputation -= 3.0;
            out.push("Load dropped. It goes on the record.".into());
        }
    }
    out
}

// tests (Step 1) go here
```

- [ ] **Step 4: Wire incidents into `src/career.rs`**

Replace the empty `Ring` and add its methods:
```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Ring {
    Incident(crate::incidents::Incident),
}

impl Ring {
    pub fn title(&self, game: &Game, shift: &Shift, world: &World) -> String {
        match self {
            Ring::Incident(i) => crate::incidents::script(game, i, shift.now(game.day), world),
        }
    }

    pub fn labels(&self, game: &Game, shift: &Shift, world: &World) -> Vec<String> {
        match self {
            Ring::Incident(i) => crate::incidents::choices(game, i, shift.now(game.day), world)
                .iter()
                .map(|c| c.label(game))
                .collect(),
        }
    }
}
```

Replace `ring_live` and `add_delay`:
```rust
    fn ring_live(&self, ring: &Ring) -> bool {
        match ring {
            Ring::Incident(i) => self.trips.iter().any(|t| t.id == i.trip_id),
        }
    }

    pub fn add_delay(&mut self, shift: &mut Shift, trip_id: u32, minutes: f64) {
        let at = shift.now(self.day) + 30;
        let Some(trip) = self.trip_mut(trip_id) else { return };
        trip.delay_min += minutes;
        if trip.hours_short() {
            shift.schedule.push(at, Ring::Incident(crate::incidents::Incident { trip_id, kind: crate::incidents::Kind::HoursShort }));
        }
    }
```

In `ring_due`, drop anything due after the close before it can queue, so that rings scheduled after 18:00 never ring:
```rust
    pub fn ring_due(&mut self, shift: &mut Shift) {
        let now = shift.now(self.day).min(abs_min(self.day, SHIFT_END));
        for ring in shift.schedule.due(now) {
            if self.ring_live(&ring) {
                shift.pending.push_back(ring);
            }
        }
    }
```

In `assign`, after building `trip` and before `self.trips.push(trip)`:
```rust
        for (at, inc) in crate::incidents::roll_for_trip(self, &trip, &loaded.weather) {
            shift.schedule.push(at, Ring::Incident(inc));
        }
```
(`loaded.weather` must still be available here, so clone it out of `loaded` before `loaded.job` is moved: `let weather = loaded.weather.clone();`.)

Add `answer` and `resolve`:
```rust
    /// Pick up the phone: the oldest ring, five minutes of desk time.
    pub fn answer(&mut self, shift: &mut Shift) -> Result<Ring, ActError> {
        let ring = shift.pending.pop_front().ok_or(ActError::NoSuchItem)?;
        shift.clock.spend(Action::TakeCall);
        Ok(ring)
    }

    pub fn resolve(&mut self, shift: &mut Shift, ring: &Ring, choice: usize, world: &World) -> Vec<String> {
        let lines = match ring {
            Ring::Incident(i) => {
                let now = shift.now(self.day);
                match crate::incidents::choices(self, i, now, world).get(choice) {
                    Some(&c) => crate::incidents::apply(self, shift, i, c, world),
                    None => vec![ActError::NoSuchItem.spoken()],
                }
            }
        };
        self.ring_due(shift);
        lines
    }
```

- [ ] **Step 5: Register the module and run the tests**

Run: `cargo test --lib incidents:: career::`
Expected: every test PASSES.

- [ ] **Step 6: Commit**

```bash
git add src/incidents.rs src/career.rs src/lib.rs
git commit -m "feat: hours-short, breakdown and detention incidents over the phone"
```

---

### Task 8: Weather incidents (stored alerts and seeded storms)

**Files:**
- Modify: `src/incidents.rs` (the `Kind::Weather` variant, `Hold`, `Reroute`, `PushThrough`, rolls, script, effects)

**Interfaces:**
- Consumes: `StoredAlert`, `Game::weather_live`, `ff_core::data::world::World::route_options`
- Produces:
  - `Kind::Weather { event: String, city: String, delay_min: f64, avoid: bool }`
  - `Choice::{Hold, Reroute, PushThrough}`
  - Constants: `SEEDED_STORM`, `HOLD_MIN`, `WEATHER_FINE`

- [ ] **Step 1: Add the failing tests to the `incidents` tests module**

```rust
    fn storm(id: u32, avoid: bool) -> Incident {
        Incident { trip_id: id, kind: Kind::Weather { event: "Winter Storm Warning".into(), city: String::new(), delay_min: 45.0, avoid } }
    }

    #[test]
    fn live_alerts_on_the_route_schedule_a_weather_call_at_that_city() {
        let (mut g, _, _) = rolling();
        g.weather_live = true;
        let t = g.trips[0].clone();
        let city = t.cities[t.cities.len() / 2].clone();
        let alert = StoredAlert { event: "Winter Storm Warning".into(), city: city.clone(), delay_min: 45.0, avoid: false };
        let rolls = roll_for_trip(&g, &t, &[alert]);
        let (at, inc) = rolls.iter().find(|(_, i)| matches!(i.kind, Kind::Weather { .. })).unwrap();
        assert_eq!(*at, t.at_fraction(t.fraction_at_city(&city)));
        assert!(matches!(&inc.kind, Kind::Weather { event, .. } if event == "Winter Storm Warning"));
    }

    #[test]
    fn live_day_without_alerts_rolls_no_weather() {
        let (mut g, _, _) = rolling();
        g.weather_live = true;
        let t = g.trips[0].clone();
        assert!(!roll_for_trip(&g, &t, &[]).iter().any(|(_, i)| matches!(i.kind, Kind::Weather { .. })));
    }

    #[test]
    fn hold_waits_and_push_through_takes_the_alert_delay() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        apply(&mut g, &mut s, &storm(id, false), Choice::Hold, w);
        assert_eq!(g.trips[0].delay_min, HOLD_MIN);
        apply(&mut g, &mut s, &storm(id, false), Choice::PushThrough, w);
        assert!(g.trips[0].delay_min >= HOLD_MIN + 45.0);
    }

    #[test]
    fn reroute_adds_wheel_time_not_just_waiting() {
        let w = get_world();
        let (mut g, mut s, id) = rolling();
        let (drive, plan) = (g.trips[0].drive_min, g.trips[0].plan_min);
        apply(&mut g, &mut s, &storm(id, true), Choice::Reroute, w);
        assert!(g.trips[0].drive_min > drive);
        assert_eq!(g.trips[0].plan_min - plan, g.trips[0].drive_min - drive);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib incidents::`
Expected: FAIL to compile (no `Kind::Weather`).

- [ ] **Step 3: Implement the weather pieces in `src/incidents.rs`**

Change the career import to `use crate::career::{Game, Ring, Shift, StoredAlert, DEADHEAD_RATE};`, then add the constants:
```rust
pub const SEEDED_STORM: f64 = 0.06;
pub const HOLD_MIN: f64 = 240.0;
pub const WEATHER_FINE: f64 = 300.0;
const SEEDED_EVENTS: &[&str] = &["Winter Storm Warning", "Severe Thunderstorm Warning", "High Wind Warning"];
```

Add the variant `Weather { event: String, city: String, delay_min: f64, avoid: bool }` to `Kind`, and `Hold`, `Reroute`, `PushThrough` to `Choice`. Add these arms to `label`:
```rust
            Choice::Hold => "Hold until it clears, about 4 hours".into(),
            Choice::Reroute => "Reroute around it".into(),
            Choice::PushThrough => "Push through".into(),
```

At the end of `roll_for_trip`, before `out`, replace the `let _ = alerts;` line with:
```rust
    if game.weather_live {
        if let Some(a) = alerts.first() {
            let at = trip.at_fraction(trip.fraction_at_city(&a.city));
            out.push((at, Incident { trip_id: trip.id, kind: Kind::Weather { event: a.event.clone(), city: a.city.clone(), delay_min: a.delay_min, avoid: a.avoid } }));
        }
    } else if rng.random() < SEEDED_STORM {
        let f = 0.2 + rng.random() * 0.6;
        let event = rng.choice(SEEDED_EVENTS).to_string();
        let miles = f * trip.city_mi.last().copied().unwrap_or(0.0);
        let city = trip.cities[trip.city_mi.iter().rposition(|&m| m <= miles).unwrap_or(0)].clone();
        out.push((trip.at_fraction(f), Incident { trip_id: trip.id, kind: Kind::Weather { event, city, delay_min: 90.0, avoid: false } }));
    }
```
The existing detention and breakdown rolls stay first, so weather goes last in the stream and they replay exactly as before.

Add to `script`:
```rust
        Kind::Weather { event, city, avoid, .. } => format!(
            "{who} here. There's a {event} near {} on my route.{}",
            world.spoken_city(city, None),
            if *avoid { " They're saying stay off the road." } else { "" }
        ),
```

Add to `choices`:
```rust
        Kind::Weather { .. } => vec![Choice::Hold, Choice::Reroute, Choice::PushThrough],
```

Add to `apply`:
```rust
        Choice::Hold => {
            game.add_delay(shift, id, HOLD_MIN);
            out.push("Holding until it clears.".into());
        }
        Choice::Reroute => {
            let t = game.trips.iter().find(|t| t.id == id).unwrap();
            let routes = world.route_options(&t.job.origin, &t.job.destination, 2, false).unwrap_or_default();
            let extra_mi = match routes.as_slice() {
                [a, b, ..] => (b.miles() - a.miles()).max(0.0),
                _ => 0.0,
            }
            .max(t.job.distance_mi * 0.10);
            let extra_min = extra_mi / DEADLINE_AVG_MPH * 60.0;
            let at = shift.now(game.day) + 30;
            let t = game.trip_mut(id).unwrap();
            t.drive_min += extra_min;
            t.plan_min += extra_min;
            if t.hours_short() {
                shift.schedule.push(at, Ring::Incident(Incident { trip_id: id, kind: Kind::HoursShort }));
            }
            out.push(format!("Rerouting, about {} extra miles.", extra_mi.round()));
        }
        Choice::PushThrough => {
            let Kind::Weather { delay_min, avoid, .. } = &inc.kind else { return out };
            let (delay_min, risk) = (*delay_min, if *avoid { 0.5 } else { 0.2 });
            game.add_delay(shift, id, delay_min);
            let mut rng = crate::rng(game.seed, game.day, &format!("push:{id}:{}", shift.clock.minute));
            if rng.random() < risk {
                if rng.random() < 0.5 {
                    game.funds -= WEATHER_FINE;
                    out.push(format!("Pushed through and got a {} citation.", crate::money(WEATHER_FINE)));
                } else {
                    let at = shift.now(game.day) + 60;
                    shift.schedule.push(at, Ring::Incident(Incident { trip_id: id, kind: Kind::Breakdown }));
                    out.push("Pushing through. The truck is taking a beating.".into());
                }
            } else {
                out.push("Pushing through, slow and steady.".into());
            }
        }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib incidents::`
Expected: every test PASSES.

- [ ] **Step 5: Commit**

```bash
git add src/incidents.rs
git commit -m "feat: weather incidents from stored alerts or seeded storms"
```

---

### Task 9: Phone: driver calls and broker hot loads

**Files:**
- Create: `src/phone.rs`
- Modify: `src/career.rs` (add `Ring::Call`, `Shift.hot_offers`, morning scheduling, `ring_live`, `title`, `labels`, `resolve`), `src/lib.rs` (add `pub mod phone;`)

**Interfaces:**
- Consumes: `career::{Game, Shift, Ring}`, `board::{Load, load_line}`, `ff_core::models::jobs::{JobBoard, OfferOptions}`
- Produces:
  - `Call::{Sick { driver_id: u32 }, PersonalDay { driver_id: u32 }, HotLoad { offer: usize }}`
  - `CallChoice::{LetRest, AskIn, Grant, Deny, Accept, Decline}` with `label()`
  - `morning_calls(&Game) -> Vec<(i64, Call)>`, `hot_offers(&Game, &World, count: usize) -> Vec<Load>`
  - `script(&Game, &Shift, &Call, &World) -> String`, `choices(&Call) -> Vec<CallChoice>`
  - `apply(&mut Game, &mut Shift, &Call, CallChoice) -> Vec<String>`
  - Constants: `SICK_ODDS`, `PERSONAL_ODDS`, `HOT_PAY`, `HOT_DEADLINE`

- [ ] **Step 1: Write `src/phone.rs` with its tests**

```rust
//! Calls that aren't about a truck on the road: drivers and brokers.

use ff_core::data::world::World;
use ff_core::models::jobs::{JobBoard, OfferOptions};

use crate::board::{load_line, Load};
use crate::career::{Game, Shift};
use crate::desk::abs_min;
use crate::roster::{Driver, Status};

pub const SICK_ODDS: f64 = 0.05;
pub const PERSONAL_ODDS: f64 = 0.04;
pub const HOT_PAY: f64 = 1.35;
pub const HOT_DEADLINE: f64 = 0.85;

#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Sick { driver_id: u32 },
    PersonalDay { driver_id: u32 },
    HotLoad { offer: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallChoice {
    LetRest,
    AskIn,
    Grant,
    Deny,
    Accept,
    Decline,
}

impl CallChoice {
    pub fn label(self) -> &'static str {
        match self {
            CallChoice::LetRest => "Tell them to rest up",
            CallChoice::AskIn => "Ask them to come in anyway",
            CallChoice::Grant => "Give them the day",
            CallChoice::Deny => "Ask them to work",
            CallChoice::Accept => "Take the load",
            CallChoice::Decline => "Pass on it",
        }
    }
}

/// Today's calls: drivers early in the shift, brokers any time.
pub fn morning_calls(game: &Game) -> Vec<(i64, Call)> {
    let mut rng = crate::rng(game.seed, game.day, "calls");
    let mut out = Vec::new();
    for d in game.drivers.iter().filter(|d| d.status == Status::Idle) {
        let roll = rng.random();
        let at = abs_min(game.day, rng.randint(370, 480) as u32);
        if roll < SICK_ODDS {
            out.push((at, Call::Sick { driver_id: d.id }));
        } else if roll < SICK_ODDS + PERSONAL_ODDS {
            out.push((at, Call::PersonalDay { driver_id: d.id }));
        }
    }
    for offer in 0..rng.randint(0, 2) as usize {
        out.push((abs_min(game.day, rng.randint(420, 960) as u32), Call::HotLoad { offer }));
    }
    out
}

/// Broker loads: the terminal's freight, paid up, on a tighter clock.
pub fn hot_offers(game: &Game, world: &World, count: usize) -> Vec<Load> {
    (0..count)
        .filter_map(|n| {
            let seed = crate::seed_for(game.seed, game.day, &format!("hot:{n}"));
            let endorsements: Vec<String> = game.drivers.iter().flat_map(Driver::endorsements).collect();
            let level = game.drivers.iter().map(Driver::level).max().unwrap_or(1);
            let mut job = JobBoard::seeded(world, seed)
                .offers(&game.terminal, &endorsements, OfferOptions { count: 1, level, market: Some(&game.market), carrier_key: Some(&game.carrier_key), direct_freight: false })
                .into_iter()
                .next()?;
            job.pay = (job.pay * HOT_PAY).round();
            job.deadline_game_h *= HOT_DEADLINE;
            Some(Load { job, hot: true, weather: Vec::new() })
        })
        .collect()
}

pub fn script(game: &Game, shift: &Shift, call: &Call, world: &World) -> String {
    let name = |id: u32| game.driver(id).map(|d| d.name.clone()).unwrap_or_default();
    match call {
        Call::Sick { driver_id } => format!("{} is calling in sick today.", name(*driver_id)),
        Call::PersonalDay { driver_id } => format!("{} is asking for a personal day.", name(*driver_id)),
        Call::HotLoad { offer } => match shift.hot_offers.get(*offer) {
            Some(l) => format!("A broker has a hot load. {}", load_line(&l.job, true, world)),
            None => String::new(),
        },
    }
}

pub fn choices(call: &Call) -> Vec<CallChoice> {
    match call {
        Call::Sick { .. } => vec![CallChoice::LetRest, CallChoice::AskIn],
        Call::PersonalDay { .. } => vec![CallChoice::Grant, CallChoice::Deny],
        Call::HotLoad { .. } => vec![CallChoice::Accept, CallChoice::Decline],
    }
}

pub fn apply(game: &mut Game, shift: &mut Shift, call: &Call, choice: CallChoice) -> Vec<String> {
    let mut rng = crate::rng(game.seed, game.day, &format!("call:{}", shift.clock.minute));
    let off = |game: &mut Game, id: u32, goodwill: i32, line: &str| {
        if let Some(d) = game.driver_mut(id) {
            d.status = Status::OffDuty;
            d.adjust_goodwill(goodwill);
        }
        vec![line.to_string()]
    };
    match (call, choice) {
        (Call::Sick { driver_id }, CallChoice::LetRest) => off(game, *driver_id, 3, "They'll be back tomorrow."),
        (Call::Sick { driver_id }, CallChoice::AskIn) => {
            if rng.random() < 0.5 {
                if let Some(d) = game.driver_mut(*driver_id) {
                    d.adjust_goodwill(-10);
                }
                vec!["They'll come in. They aren't happy about it.".into()]
            } else {
                off(game, *driver_id, -5, "They can't. They're staying home.")
            }
        }
        (Call::PersonalDay { driver_id }, CallChoice::Grant) => off(game, *driver_id, 5, "Day granted."),
        (Call::PersonalDay { driver_id }, CallChoice::Deny) => {
            if let Some(d) = game.driver_mut(*driver_id) {
                d.adjust_goodwill(-8);
            }
            vec!["They'll work. You'll hear about it.".into()]
        }
        (Call::HotLoad { offer }, CallChoice::Accept) => match shift.hot_offers.get(*offer).cloned() {
            Some(load) => {
                shift.board.insert(0, load);
                vec!["The hot load is at the top of the board.".into()]
            }
            None => Vec::new(),
        },
        _ => vec!["Passed.".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::{tests::game, Ring};
    use ff_core::data::world::get_world;

    #[test]
    fn calls_replay_for_the_same_morning() {
        let g = game();
        assert_eq!(morning_calls(&g), morning_calls(&g));
        assert!(morning_calls(&g).iter().all(|(at, _)| *at > abs_min(g.day, crate::desk::SHIFT_START)));
    }

    #[test]
    fn hot_load_pays_more_on_a_tighter_clock_and_lands_on_the_board() {
        let w = get_world();
        let g = game();
        let offer = hot_offers(&g, w, 1).remove(0);
        assert!(offer.hot);
        let mut g = g;
        let mut s = g.morning(w);
        s.hot_offers = vec![offer];
        apply(&mut g, &mut s, &Call::HotLoad { offer: 0 }, CallChoice::Accept);
        assert!(s.board[0].hot);
    }

    #[test]
    fn granting_a_day_takes_the_driver_off_and_raises_goodwill() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let id = g.drivers[0].id;
        apply(&mut g, &mut s, &Call::PersonalDay { driver_id: id }, CallChoice::Grant);
        assert_eq!(g.driver(id).unwrap().status, Status::OffDuty);
        assert_eq!(g.driver(id).unwrap().goodwill, crate::roster::GOODWILL_START + 5);
    }

    #[test]
    fn a_call_about_a_driver_now_on_a_load_is_dropped() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let id = g.drivers[0].id;
        g.driver_mut(id).unwrap().status = Status::OnLoad;
        s.schedule.push(abs_min(g.day, 361), Ring::Call(Call::Sick { driver_id: id }));
        s.clock.minute = 400;
        g.ring_due(&mut s);
        assert!(s.pending.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib phone::`
Expected: FAIL (`Ring::Call` and `Shift.hot_offers` don't exist yet).

- [ ] **Step 3: Wire it into `src/career.rs`**

Add the `pub hot_offers: Vec<Load>` field to `Shift`, and add a `Call(crate::phone::Call)` variant to `Ring`. Then extend the match arms:

```rust
// Ring::title
            Ring::Call(c) => crate::phone::script(game, shift, c, world),
// Ring::labels
            Ring::Call(c) => crate::phone::choices(c).iter().map(|c| c.label().to_string()).collect(),
// Game::ring_live
            Ring::Call(crate::phone::Call::Sick { driver_id } | crate::phone::Call::PersonalDay { driver_id }) => {
                self.driver(*driver_id).is_some_and(|d| d.status == Status::Idle)
            }
            Ring::Call(crate::phone::Call::HotLoad { .. }) => true,
// Game::resolve
            Ring::Call(c) => match crate::phone::choices(c).get(choice) {
                Some(&ch) => crate::phone::apply(self, shift, c, ch),
                None => vec![ActError::NoSuchItem.spoken()],
            },
```

In `Game::morning`, build the shift with its calls:

```rust
        let calls = crate::phone::morning_calls(self);
        let hot = calls.iter().filter(|(_, c)| matches!(c, crate::phone::Call::HotLoad { .. })).count();
        let mut shift = Shift { clock: DeskClock::morning(), board, schedule: Schedule::default(), pending: VecDeque::new(), hot_offers: crate::phone::hot_offers(self, world, hot) };
        for (at, call) in calls {
            shift.schedule.push(at, Ring::Call(call));
        }
        shift
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib phone:: career:: incidents::`
Expected: every test PASSES.

- [ ] **Step 5: Commit**

```bash
git add src/phone.rs src/career.rs src/lib.rs
git commit -m "feat: driver calls and broker hot loads"
```

---

### Task 10: Hiring

**Files:**
- Create: `src/hiring.rs`
- Modify: `src/career.rs` (`Shift.applicants`, morning pool, `interview`, `hire`), `src/lib.rs` (add `pub mod hiring;`)

**Interfaces:**
- Produces:
  - `ROSTER_CAP = 10`, `POOL_SIZE = 3`
  - `Applicant { driver: Driver, signing: f64, interviewed: bool }` with `line() -> String`
  - `daily_pool(&Game) -> Vec<Applicant>`
  - `Game::interview(&mut Shift, idx) -> Result<String, ActError>`, `Game::hire(&mut Shift, idx) -> Result<String, ActError>`

- [ ] **Step 1: Write `src/hiring.rs` with its tests**

```rust
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
    /// Name, level and signing cost up front; endorsements and attitude
    /// only after an interview.
    pub fn line(&self) -> String {
        let d = &self.driver;
        let mut s = format!("{}, level {}, signing {}.", d.name, d.level(), crate::money(self.signing));
        if self.interviewed {
            let e = d.endorsements();
            s.push_str(&format!(
                " {}. Goodwill {}.",
                if e.is_empty() { "No endorsements".to_string() } else { format!("Endorsements: {}", e.join(", ")) },
                d.goodwill
            ));
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
            Applicant { signing: 1500.0 + 500.0 * d.level() as f64, driver: d.clone(), interviewed: false }
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
        let a: Vec<_> = daily_pool(&g).iter().map(|a| a.driver.name.clone()).collect();
        assert_eq!(a, daily_pool(&g).iter().map(|a| a.driver.name.clone()).collect::<Vec<_>>());
        assert!(g.drivers.iter().all(|d| !a.contains(&d.name)));
    }

    #[test]
    fn interview_costs_fifteen_minutes_and_reveals_more() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let line = g.interview(&mut s, 0).unwrap();
        assert_eq!(s.clock.minute, SHIFT_START + 15);
        assert!(line.contains("Goodwill"));
    }

    #[test]
    fn hire_pays_signing_and_respects_cap_and_funds() {
        let w = get_world();
        let mut g = game();
        let mut s = g.morning(w);
        let cost = s.applicants[0].signing;
        let funds = g.funds;
        g.hire(&mut s, 0).unwrap();
        assert_eq!(g.drivers.len(), 4);
        assert_eq!(g.funds, funds - cost);
        assert_eq!(s.applicants.len(), POOL_SIZE - 1);
        g.funds = 0.0;
        assert_eq!(g.hire(&mut s, 0), Err(ActError::NoFunds));
        g.funds = 1e9;
        while g.drivers.len() < ROSTER_CAP {
            let mut extra = g.drivers[0].clone();
            extra.id = g.take_id();
            g.drivers.push(extra);
        }
        assert_eq!(g.hire(&mut s, 0), Err(ActError::RosterFull));
    }
}
```

- [ ] **Step 2: Wire it into `src/career.rs`**

Add the `pub applicants: Vec<crate::hiring::Applicant>` field to `Shift`, and set it in `morning` with `applicants: crate::hiring::daily_pool(self)`. Then add:

```rust
    pub fn interview(&mut self, shift: &mut Shift, idx: usize) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let a = shift.applicants.get_mut(idx).ok_or(ActError::NoSuchItem)?;
        shift.clock.spend(Action::Interview);
        a.interviewed = true;
        let line = a.line();
        self.ring_due(shift);
        Ok(line)
    }

    pub fn hire(&mut self, shift: &mut Shift, idx: usize) -> Result<String, ActError> {
        self.ensure_open(shift)?;
        let a = shift.applicants.get(idx).ok_or(ActError::NoSuchItem)?;
        if self.drivers.len() >= crate::hiring::ROSTER_CAP {
            return Err(ActError::RosterFull);
        }
        if self.funds < a.signing {
            return Err(ActError::NoFunds);
        }
        let mut a = shift.applicants.remove(idx);
        self.funds -= a.signing;
        a.driver.id = self.take_id();
        let line = format!("{} signed on for {}. They're available now.", a.driver.name, crate::money(a.signing));
        self.drivers.push(a.driver);
        Ok(line)
    }
```

- [ ] **Step 3: Register the module and run the tests**

Run: `cargo test --lib hiring::`
Expected: 3 PASS.

- [ ] **Step 4: Commit**

```bash
git add src/hiring.rs src/career.rs src/lib.rs
git commit -m "feat: daily applicants, interviews and hiring"
```

---

### Task 11: Saves and morning replay

**Files:**
- Create: `src/saves.rs`
- Modify: `src/lib.rs` (add `pub mod saves;`)

**Interfaces:**
- Produces:
  - `DATA_DIR_ENV = "FF_DISPATCH_DATA_DIR"`
  - `data_dir() -> PathBuf`, `careers_dir() -> PathBuf`
  - `save_game(&Game, dir: &Path) -> io::Result<PathBuf>`, `load_game(&Path) -> Result<Game, String>`
  - `Slot::{Ok { path: PathBuf, game: Game }, Corrupt { path: PathBuf, name: String }}` with `label() -> String`
  - `list_slots(dir: &Path) -> Vec<Slot>`

- [ ] **Step 1: Write `src/saves.rs` with its tests**

```rust
//! Career saves: versioned JSON, atomic writes, tolerant loads.

use std::io;
use std::path::{Path, PathBuf};

use crate::career::Game;

pub const DATA_DIR_ENV: &str = "FF_DISPATCH_DATA_DIR";

/// Where Dispatch keeps its own files (never Freight Fate's folder).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("FreightFateDispatch")
}

pub fn careers_dir() -> PathBuf {
    data_dir().join("careers")
}

fn file_name(game: &Game) -> String {
    let slug: String = game.name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    format!("{}-{}.json", slug.trim_matches('-'), game.seed)
}

/// Write the career atomically: a temp file, then a rename over the old save.
pub fn save_game(game: &Game, dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name(game));
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(game)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn load_game(path: &Path) -> Result<Game, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

pub enum Slot {
    Ok { path: PathBuf, game: Game },
    Corrupt { path: PathBuf, name: String },
}

impl Slot {
    pub fn label(&self) -> String {
        match self {
            Slot::Ok { game, .. } => format!("{}, day {}, {}.", game.name, game.day, crate::money(game.funds)),
            Slot::Corrupt { name, .. } => format!("{name}: this save could not be read. It has been left as it is."),
        }
    }
}

/// Every career save in `dir`, sorted by file name. Corrupt ones are listed, never touched.
pub fn list_slots(dir: &Path) -> Vec<Slot> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| match load_game(&path) {
            Ok(game) => Slot::Ok { path, game },
            Err(err) => {
                log::warn!("unreadable career save {}: {err}", path.display());
                let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                Slot::Corrupt { path, name }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::tests::{game, legal_pair};
    use ff_core::data::world::get_world;

    #[test]
    fn save_and_load_round_trip_without_leaving_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let g = game();
        let path = save_game(&g, dir.path()).unwrap();
        let back = load_game(&path).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), serde_json::to_string(&g).unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn corrupt_saves_are_listed_by_name_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        save_game(&game(), dir.path()).unwrap();
        let bad = dir.path().join("broken-1.json");
        std::fs::write(&bad, b"{not json").unwrap();
        let slots = list_slots(dir.path());
        assert_eq!(slots.len(), 2);
        assert!(slots.iter().any(|s| matches!(s, Slot::Corrupt { name, .. } if name == "broken-1")));
        assert_eq!(std::fs::read(&bad).unwrap(), b"{not json");
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let mut v = serde_json::to_value(game()).unwrap();
        let obj = v.as_object_mut().unwrap();
        obj.insert("from_the_future".into(), serde_json::json!({"x": 1}));
        for field in ["record", "trips", "market", "shipper_goodwill", "weather", "weather_live", "news", "next_id"] {
            obj.remove(field);
        }
        let g: Game = serde_json::from_value(v).unwrap();
        assert_eq!(g.record.delivered, 0);
    }

    #[test]
    fn a_reloaded_morning_replays_board_calls_applicants_and_incidents() {
        let w = get_world();
        let dir = tempfile::tempdir().unwrap();
        let mut g1 = game();
        g1.set_weather(None);
        let path = save_game(&g1, dir.path()).unwrap();
        let mut s1 = g1.morning(w);
        let mut g2 = load_game(&path).unwrap();
        let mut s2 = g2.morning(w);
        let jobs = |s: &crate::career::Shift| s.board.iter().map(|l| format!("{:?}", l.job)).collect::<Vec<_>>();
        assert_eq!(jobs(&s1), jobs(&s2));
        assert_eq!(format!("{:?}", s1.schedule), format!("{:?}", s2.schedule));
        let names = |s: &crate::career::Shift| s.applicants.iter().map(|a| a.driver.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&s1), names(&s2));
        let (load, driver) = legal_pair(&g1, &s1);
        g1.assign(&mut s1, load, driver, w).unwrap();
        g2.assign(&mut s2, load, driver, w).unwrap();
        assert_eq!(format!("{:?}", s1.schedule), format!("{:?}", s2.schedule));
    }
}
```

`Schedule` already derives `Debug` (Task 3). If `Shift`'s `Ring` does not, add `#[derive(Debug)]` to `Ring` (it already has `Debug` from Task 7).

- [ ] **Step 2: Register the module and run the tests**

Run: `cargo test --lib saves::`
Expected: 4 PASS.

- [ ] **Step 3: Commit**

```bash
git add src/saves.rs src/lib.rs
git commit -m "feat: atomic career saves, tolerant loads, morning replay"
```

---

### Task 12: Network transport and real morning weather

**Files:**
- Create: `src/net.rs`
- Modify: `src/lib.rs` (add `pub mod net;`)

**Interfaces:**
- Consumes: `ff_core::sim::real_traffic::{HttpTransport, TransportError}`, `ff_core::sim::real_weather_alerts::{WeatherAlertsProvider, scan_route_weather, AlertEffect}`
- Produces:
  - `UreqTransport::new(timeout_s: f64) -> UreqTransport` (implements `HttpTransport`; a status ≥ 400 becomes `TransportError { message: "HTTP <code>" }`)
  - `http_status(&TransportError) -> Option<u16>`
  - `WEATHER_BUDGET: Duration = 3 s`
  - `fetch_day_weather(Arc<dyn HttpTransport>, routes: &[Option<Route>], &World, budget: Duration, threaded: bool) -> Option<Vec<Vec<StoredAlert>>>`
  - `live_weather(routes: &[Option<Route>], &World) -> Option<Vec<Vec<StoredAlert>>>`

- [ ] **Step 1: Write `src/net.rs` with its tests**

```rust
//! HTTP for Dispatch: one ureq transport behind ff-core's own trait, and
//! the morning weather fetch that must never hold up the morning.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ff_core::data::world::World;
use ff_core::data::world_models::leg::Route;
use ff_core::models::jobs::DEADLINE_AVG_MPH;
use ff_core::sim::real_traffic::{HttpTransport, TransportError};
use ff_core::sim::real_weather_alerts::{scan_route_weather, AlertEffect, WeatherAlertsProvider};

use crate::career::StoredAlert;

pub const WEATHER_BUDGET: Duration = Duration::from_secs(3);

fn err(message: String) -> TransportError {
    TransportError { message }
}

pub fn http_status(e: &TransportError) -> Option<u16> {
    e.message.strip_prefix("HTTP ")?.parse().ok()
}

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new(timeout_s: f64) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs_f64(timeout_s)))
            .http_status_as_error(false)
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build()
            .into();
        UreqTransport { agent }
    }
}

fn finish(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Vec<u8>, TransportError> {
    let mut resp = resp.map_err(|e| err(e.to_string()))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        return Err(err(format!("HTTP {status}")));
    }
    resp.body_mut().read_to_vec().map_err(|e| err(e.to_string()))
}

impl HttpTransport for UreqTransport {
    fn get(&self, url: &str, headers: &[(&str, &str)], _timeout_s: f64) -> Result<Vec<u8>, TransportError> {
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        finish(req.call())
    }

    fn post(&self, url: &str, body: &[u8], headers: &[(&str, &str)], _timeout_s: f64) -> Result<Vec<u8>, TransportError> {
        let mut req = self.agent.post(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        finish(req.send(body))
    }
}

/// Remembers whether any fetch failed, so a partial answer counts as none.
struct Watch {
    inner: Arc<dyn HttpTransport>,
    failed: AtomicBool,
}

impl HttpTransport for Watch {
    fn get(&self, url: &str, headers: &[(&str, &str)], timeout_s: f64) -> Result<Vec<u8>, TransportError> {
        let r = self.inner.get(url, headers, timeout_s);
        if r.is_err() {
            self.failed.store(true, Ordering::Relaxed);
        }
        r
    }

    fn post(&self, url: &str, body: &[u8], headers: &[(&str, &str)], timeout_s: f64) -> Result<Vec<u8>, TransportError> {
        self.inner.post(url, body, headers, timeout_s)
    }
}

/// NWS alerts along each board route, or `None` (seeded weather) if any
/// point fails or the budget runs out.
pub fn fetch_day_weather(
    transport: Arc<dyn HttpTransport>,
    routes: &[Option<Route>],
    world: &World,
    budget: Duration,
    threaded: bool,
) -> Option<Vec<Vec<StoredAlert>>> {
    let watch = Arc::new(Watch { inner: transport, failed: AtomicBool::new(false) });
    let provider = WeatherAlertsProvider::new(watch.clone()).with_threaded(threaded);
    let points: Vec<(f64, f64)> = routes
        .iter()
        .flatten()
        .flat_map(|r| r.cities.iter())
        .filter_map(|k| world.cities.get(k))
        .map(|c| (c.lat, c.lon))
        .collect();
    for &(lat, lon) in &points {
        provider.request(lat, lon);
    }
    let deadline = Instant::now() + budget;
    while points.iter().any(|&(lat, lon)| provider.get(lat, lon).is_none()) {
        if watch.failed.load(Ordering::Relaxed) || Instant::now() >= deadline {
            log::warn!("weather fetch fell back to seeded weather");
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if watch.failed.load(Ordering::Relaxed) {
        return None;
    }
    Some(
        routes
            .iter()
            .map(|r| match r {
                Some(route) => scan_route_weather(route, &provider, world, DEADLINE_AVG_MPH)
                    .alerts
                    .into_iter()
                    .map(|a| StoredAlert { event: a.alert.event, city: a.near_city, delay_min: a.delay_h * 60.0, avoid: a.effect == AlertEffect::Avoid })
                    .collect(),
                None => Vec::new(),
            })
            .collect(),
    )
}

pub fn live_weather(routes: &[Option<Route>], world: &World) -> Option<Vec<Vec<StoredAlert>>> {
    fetch_day_weather(Arc::new(UreqTransport::new(WEATHER_BUDGET.as_secs_f64())), routes, world, WEATHER_BUDGET, true)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::career::tests::game;
    use ff_core::data::world::get_world;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Replies from a queue (the last one repeats); records every request.
    pub(crate) struct Fake {
        pub replies: Mutex<VecDeque<Result<Vec<u8>, TransportError>>>,
        pub seen: Mutex<Vec<(String, Vec<u8>)>>,
        pub delay: Duration,
    }

    impl Fake {
        pub(crate) fn new(replies: Vec<Result<&str, &str>>) -> Arc<Fake> {
            let replies = replies.into_iter().map(|r| r.map(|b| b.as_bytes().to_vec()).map_err(|m| err(m.to_string()))).collect();
            Arc::new(Fake { replies: Mutex::new(replies), seen: Mutex::new(Vec::new()), delay: Duration::ZERO })
        }

        fn next(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, TransportError> {
            std::thread::sleep(self.delay);
            self.seen.lock().unwrap().push((url.to_string(), body.to_vec()));
            let mut q = self.replies.lock().unwrap();
            if q.len() > 1 {
                return q.pop_front().unwrap();
            }
            match q.front().unwrap() {
                Ok(bytes) => Ok(bytes.clone()),
                Err(e) => Err(err(e.message.clone())),
            }
        }
    }

    impl HttpTransport for Fake {
        fn get(&self, url: &str, _: &[(&str, &str)], _: f64) -> Result<Vec<u8>, TransportError> {
            self.next(url, &[])
        }
        fn post(&self, url: &str, body: &[u8], _: &[(&str, &str)], _: f64) -> Result<Vec<u8>, TransportError> {
            self.next(url, body)
        }
    }

    const STORM: &str = r#"{"features":[{"id":"urn:1","properties":{"id":"urn:1","event":"Winter Storm Warning","severity":"Severe","headline":"Winter Storm Warning","areaDesc":"Area","description":"Snow."}}]}"#;

    #[test]
    fn live_alerts_are_stored_per_board_route() {
        let w = get_world();
        let routes = game().todays_routes(w);
        let got = fetch_day_weather(Fake::new(vec![Ok(STORM)]), &routes, w, WEATHER_BUDGET, false).unwrap();
        assert_eq!(got.len(), routes.len());
        assert!(got.iter().zip(&routes).all(|(a, r)| r.is_none() || a.iter().any(|a| a.event == "Winter Storm Warning")));
    }

    #[test]
    fn a_failed_fetch_falls_back_to_seeded() {
        let w = get_world();
        let routes = game().todays_routes(w);
        assert!(fetch_day_weather(Fake::new(vec![Err("offline")]), &routes, w, WEATHER_BUDGET, false).is_none());
    }

    #[test]
    fn a_slow_network_never_holds_the_morning_past_the_budget() {
        let w = get_world();
        let routes = game().todays_routes(w);
        let slow = Arc::new(Fake { delay: Duration::from_secs(2), ..Arc::try_unwrap(Fake::new(vec![Ok(STORM)])).ok().unwrap() });
        let start = Instant::now();
        assert!(fetch_day_weather(slow, &routes, w, Duration::from_millis(200), true).is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn status_codes_read_back() {
        assert_eq!(http_status(&err("HTTP 410".into())), Some(410));
        assert_eq!(http_status(&err("dns".into())), None);
    }
}
```

`WeatherAlertsProvider::new` takes an `Arc<dyn HttpTransport>`, and `watch.clone()` coerces to that. If `with_threaded(true)` spawns detached workers, the slow-network test leaves them sleeping. That is harmless: the test only asserts that the call returns inside the budget.

- [ ] **Step 2: Register the module and run the tests**

Run: `cargo test --lib net::`
Expected: 4 PASS. If `live_alerts...` finds no alerts, check that `alert_effect("Winter Storm Warning")` is `Some` in the pinned ff-core. If it isn't, switch `STORM` to an event it returns `Some` for, rather than weakening the assertion.

- [ ] **Step 3: Commit**

```bash
git add src/net.rs src/lib.rs
git commit -m "feat: ureq transport and budgeted NWS morning weather"
```

---

### Task 13: Freight Fate driver import

**Files:**
- Create: `src/import.rs`, `tests/import.rs`, `tests/fixtures/ff/Casey Rivers.ffsave` (generated), `tests/fixtures/ff/Newer.ffsave` (generated)
- Modify: `src/lib.rs` (add `pub mod import;`)

**Interfaces:**
- Consumes: `ff_core::models::profile::{decode_save_bytes, Profile, SAVE_VERSION, FIRST_1_9_SAVE_VERSION}`, `ff_core::settings::paths::legacy_data_dir`, `ff_core::models::start_options::START_OPTIONS`
- Produces:
  - `FF_SAVES_ENV = "FF_DISPATCH_FREIGHT_FATE_SAVES"`
  - `ff_profile_dirs() -> Vec<PathBuf>`, `list_ff_saves(&[PathBuf]) -> Vec<PathBuf>`
  - `ImportError::{Unreadable(String), Newer { found: i64, supported: i64 }, TooOld}` with `spoken(&self, file: &str) -> String`
  - `read_profile(&Path) -> Result<Profile, ImportError>`
  - `import_game(&Profile, seed: i64, &World) -> Game`

- [ ] **Step 1: Write `src/import.rs`**

```rust
//! Read-only import of a Freight Fate driver. The save is read into memory
//! and never opened for writing. Profile::load is not used: it re-signs
//! and rewrites saves.

use std::path::{Path, PathBuf};

use ff_core::data::world::World;
use ff_core::models::profile::{decode_save_bytes, Profile, FIRST_1_9_SAVE_VERSION, SAVE_VERSION};
use ff_core::models::start_options::START_OPTIONS;

use crate::career::Game;
use crate::roster::Driver;

pub const FF_SAVES_ENV: &str = "FF_DISPATCH_FREIGHT_FATE_SAVES";

/// Where FF keeps profiles: the override, FF's per-user folder, and a
/// portable FreightFate folder unzipped beside Dispatch's.
pub fn ff_profile_dirs() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os(FF_SAVES_ENV).filter(|d| !d.is_empty()) {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = vec![ff_core::settings::paths::legacy_data_dir().join("profiles")];
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent()?.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("FreightFate").join("saves").join("profiles"));
    }
    dirs
}

pub fn list_ff_saves(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "ffsave"))
        .collect();
    out.sort();
    out
}

#[derive(Debug, PartialEq)]
pub enum ImportError {
    Unreadable(String),
    Newer { found: i64, supported: i64 },
    TooOld,
}

impl ImportError {
    pub fn spoken(&self, file: &str) -> String {
        match self {
            ImportError::Unreadable(_) => format!("{file} could not be read. It has been left as it is."),
            ImportError::Newer { found, supported } => format!(
                "{file} is from a newer Freight Fate than this Dispatch understands (save format {found}; Dispatch reads up to {supported}). Update Dispatch to import it."
            ),
            ImportError::TooOld => format!("{file} is from before Freight Fate 1.9. Open it in Freight Fate once to update it, then import it."),
        }
    }
}

pub fn read_profile(path: &Path) -> Result<Profile, ImportError> {
    let raw = std::fs::read(path).map_err(|e| ImportError::Unreadable(e.to_string()))?;
    let (map, _packed) = decode_save_bytes(&raw).map_err(|e| ImportError::Unreadable(format!("{e:?}")))?;
    let version = map.get("version").and_then(serde_json::Value::as_i64).unwrap_or(0);
    if version > SAVE_VERSION {
        return Err(ImportError::Newer { found: version, supported: SAVE_VERSION });
    }
    if version < FIRST_1_9_SAVE_VERSION {
        return Err(ImportError::TooOld);
    }
    // The integrity flag is ignored on purpose: this is a local continuation.
    Ok(Profile::from_dict(&map))
}

/// A career at the driver's carrier, with the driver on the roster as they are in FF.
pub fn import_game(profile: &Profile, seed: i64, world: &World) -> Game {
    let carrier = START_OPTIONS.iter().find(|o| o.key == profile.carrier_key).map(|o| o.key).unwrap_or("northstar");
    let mut game = Game::new(carrier, seed, world);
    let mut driver = Driver::new(game.drivers[0].id, &profile.name, profile.career.clone());
    driver.hos = profile.hos.clone();
    let keep = driver.id;
    game.drivers[0] = driver;
    game.drivers.retain(|d| d.id == keep || d.name != profile.name);
    game.reputation = profile.standing();
    game.name = format!("{} with {}", game.name, profile.name);
    game
}
```

- [ ] **Step 2: Write `tests/import.rs`, including the fixture writer**

```rust
use std::path::PathBuf;

use ff_core::models::career::Career;
use ff_core::models::profile::{encode_save_bytes, Profile, SAVE_VERSION};
use ff_dispatch::import::{import_game, list_ff_saves, read_profile, ImportError};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ff")
}

/// Regenerate the committed fixtures with the pinned ff-core's own encoder.
/// Run after every ff-core pin bump:
/// `cargo test --test import write_fixtures -- --ignored`
#[test]
#[ignore]
fn write_fixtures() {
    let tmp = tempfile::tempdir().unwrap();
    ff_core::settings::set_thread_data_dir(Some(tmp.path().to_path_buf()));
    let mut p = Profile::named("Casey Rivers");
    p.career = Career::with_xp(4_000.0);
    p.career.deliveries = 40;
    p.career.on_time_deliveries = 36;
    p.carrier_key = "prairie_link".into();
    p.hos.drive(240.0);
    p.integrity_modified = true;
    let mut dict = p.to_dict();
    std::fs::create_dir_all(fixtures()).unwrap();
    std::fs::write(fixtures().join("Casey Rivers.ffsave"), encode_save_bytes(&dict)).unwrap();
    dict.insert("version".into(), (SAVE_VERSION + 1).into());
    std::fs::write(fixtures().join("Newer.ffsave"), encode_save_bytes(&dict)).unwrap();
}

#[test]
fn a_real_ffsave_imports_with_career_hours_and_carrier_intact() {
    let path = fixtures().join("Casey Rivers.ffsave");
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let profile = read_profile(&path).unwrap();
    assert!(profile.integrity_modified, "integrity flag set, and it still imports");
    let game = import_game(&profile, 9, ff_core::data::world::get_world());
    let d = &game.drivers[0];
    assert_eq!(d.name, "Casey Rivers");
    assert_eq!(d.career.on_time_deliveries, 36);
    assert_eq!(d.hos.driving_min, 240.0);
    assert_eq!(game.carrier_key, "prairie_link");
    assert_eq!(game.drivers.len(), 3);
    assert_eq!(std::fs::read(&path).unwrap(), before, "the FF save is never written");
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);
}

#[test]
fn a_newer_save_is_refused_plainly_and_untouched() {
    let path = fixtures().join("Newer.ffsave");
    let before = std::fs::read(&path).unwrap();
    let err = read_profile(&path).unwrap_err();
    assert_eq!(err, ImportError::Newer { found: SAVE_VERSION + 1, supported: SAVE_VERSION });
    assert!(err.spoken("Newer").contains("newer Freight Fate"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_corrupt_ffsave_is_refused_not_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("Bad.ffsave");
    std::fs::write(&bad, b"garbage").unwrap();
    assert!(matches!(read_profile(&bad), Err(ImportError::Unreadable(_))));
    assert!(bad.exists(), "left where it was");
    assert_eq!(list_ff_saves(&[dir.path().to_path_buf()]), vec![bad]);
}
```

- [ ] **Step 3: Generate the fixtures, then run the tests**

Run: `cargo test --test import write_fixtures -- --ignored`
Then: `cargo test --test import`
Expected: 3 PASS, and the 2 fixture files now exist. If `Profile::to_dict` or `Profile::named` needs a data dir beyond `set_thread_data_dir`, set the `FREIGHT_FATE_DATA_DIR` env var to the temp dir inside the test.

- [ ] **Step 4: Commit**

```bash
git add src/import.rs src/lib.rs tests/import.rs tests/fixtures/ff
git commit -m "feat: read-only Freight Fate driver import"
```

---

### Task 14: Online sign-in and the day-end profile line

**Files:**
- Create: `src/online.rs`
- Modify: `src/lib.rs` (add `pub mod online;`)

**Interfaces:**
- Consumes: `net::{UreqTransport, http_status}`, `saves::data_dir`, `career::Game`
- Produces:
  - Constants: `DEFAULT_BASE_URL`, `BASE_URL_ENV`, `TOKEN_SERVICE`, `SCHEMA_VERSION: u32 = 1`
  - `base_url() -> String`, `machine_key() -> String`
  - `Activation { device_code, user_code, verification_uri: String, expires_in: u64 }`
  - `Poll::{Ready { driver_id, token, display_name: String }, Pending, Expired, Error, Retry}`
  - `start(&dyn HttpTransport, base: &str, machine_key: &str) -> Option<Activation>`
  - `poll(&dyn HttpTransport, base: &str, device_code: &str) -> Poll`
  - `poll_interval(elapsed_s: f64) -> Duration`
  - `Identity { driver_id, display_name }` with `load(dir: &Path) -> Option<Identity>`, `save(&self, dir: &Path) -> io::Result<()>`
  - `store_token(driver_id, token) -> bool`, `load_token(driver_id) -> Option<String>`
  - `day_payload(&Game) -> serde_json::Value`, `post_day(&dyn HttpTransport, base: &str, token: &str, &Game) -> bool`

- [ ] **Step 1: Write `src/online.rs` with its tests**

```rust
//! Optional orinks.net sign-in (the same device-code flow as Freight Fate)
//! and the end-of-day profile line. Nothing here may interrupt play.

use std::io;
use std::path::Path;
use std::time::Duration;

use ff_core::sim::real_traffic::HttpTransport;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::career::Game;
use crate::net::http_status;

pub const DEFAULT_BASE_URL: &str = "https://www.orinks.net";
pub const BASE_URL_ENV: &str = "FF_DISPATCH_ONLINE_URL";
pub const TOKEN_SERVICE: &str = "Freight Fate Dispatch token";
pub const SCHEMA_VERSION: u32 = 1;
/// The day-line endpoint orinks-net adds in release week. Until it is
/// live, the post fails with 404 and is skipped quietly.
pub const DAY_PATH: &str = "/api/freight-fate/dispatch/day";

const JSON: &[(&str, &str)] = &[("Content-Type", "application/json"), ("User-Agent", concat!("FreightFateDispatch/", env!("CARGO_PKG_VERSION")))];

pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV).unwrap_or_else(|_| DEFAULT_BASE_URL.into()).trim_end_matches('/').to_string()
}

pub fn machine_key() -> String {
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    hex::encode(Sha256::digest(format!("node:{host}")))[..32].to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Activation {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Poll {
    Ready { driver_id: String, token: String, display_name: String },
    Pending,
    Expired,
    Error,
    Retry,
}

fn post_json(t: &dyn HttpTransport, url: &str, body: &Value, headers: &[(&str, &str)]) -> Result<Value, Option<u16>> {
    let bytes = t.post(url, body.to_string().as_bytes(), headers, 10.0).map_err(|e| {
        log::warn!("online: {url}: {}", e.message);
        http_status(&e)
    })?;
    serde_json::from_slice(&bytes).map_err(|_| None)
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

pub fn start(t: &dyn HttpTransport, base: &str, machine_key: &str) -> Option<Activation> {
    let v = post_json(t, &format!("{base}/api/freight-fate/activate/start"), &json!({ "machine_key": machine_key }), JSON).ok()?;
    Some(Activation {
        device_code: text(&v, "device_code")?,
        user_code: text(&v, "user_code")?,
        verification_uri: text(&v, "verification_uri")?,
        expires_in: v.get("expires_in").and_then(Value::as_u64).unwrap_or(600),
    })
}

pub fn poll(t: &dyn HttpTransport, base: &str, device_code: &str) -> Poll {
    match post_json(t, &format!("{base}/api/freight-fate/activate/poll"), &json!({ "device_code": device_code }), JSON) {
        Err(Some(410)) => Poll::Expired,
        Err(Some(400)) => Poll::Error,
        Err(_) => Poll::Retry,
        Ok(v) => match v.get("status").and_then(Value::as_str) {
            Some("pending") => Poll::Pending,
            Some("ready") => match (text(&v, "driver_id"), text(&v, "token")) {
                (Some(driver_id), Some(token)) => Poll::Ready { driver_id, token, display_name: text(&v, "display_name").unwrap_or_default() },
                _ => Poll::Retry,
            },
            _ => Poll::Retry,
        },
    }
}

/// Every 3 s for the first 30 s, then every 8 s (Freight Fate's schedule).
pub fn poll_interval(elapsed_s: f64) -> Duration {
    Duration::from_secs(if elapsed_s < 30.0 { 3 } else { 8 })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub driver_id: String,
    #[serde(default)]
    pub display_name: String,
}

impl Identity {
    pub fn load(dir: &Path) -> Option<Identity> {
        serde_json::from_slice(&std::fs::read(dir.join("online.json")).ok()?).ok()
    }

    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("online.json"), serde_json::to_vec_pretty(self)?)
    }
}

pub fn store_token(driver_id: &str, token: &str) -> bool {
    keyring::Entry::new(TOKEN_SERVICE, driver_id)
        .and_then(|e| e.set_password(token))
        .map_err(|e| log::warn!("online: keyring store failed: {e}"))
        .is_ok()
}

pub fn load_token(driver_id: &str) -> Option<String> {
    keyring::Entry::new(TOKEN_SERVICE, driver_id).and_then(|e| e.get_password()).ok()
}

pub fn day_payload(game: &Game) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "carrier": game.name,
        "carrier_key": game.carrier_key,
        "days_worked": game.record.days_worked,
        "on_time_pct": game.record.on_time_pct(),
    })
}

/// End-of-day profile line. `false` (logged) on any failure, including a 404
/// before the endpoint exists.
pub fn post_day(t: &dyn HttpTransport, base: &str, token: &str, game: &Game) -> bool {
    let auth = format!("Bearer {token}");
    let headers = [JSON[0], JSON[1], ("Authorization", auth.as_str())];
    t.post(&format!("{base}{DAY_PATH}"), day_payload(game).to_string().as_bytes(), &headers, 10.0)
        .map_err(|e| log::info!("online: day line skipped: {}", e.message))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::career::tests::game;
    use crate::net::tests::Fake;

    #[test]
    fn start_reads_the_activation() {
        let fake = Fake::new(vec![Ok(r#"{"device_code":"d1","user_code":"ABCD-1234","verification_uri":"https://www.orinks.net/activate","expires_in":600,"interval":5}"#)]);
        let a = start(fake.as_ref(), "https://x", "k").unwrap();
        assert_eq!(a.user_code, "ABCD-1234");
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].0, "https://x/api/freight-fate/activate/start");
        assert!(String::from_utf8_lossy(&seen[0].1).contains("\"machine_key\":\"k\""));
    }

    #[test]
    fn start_fails_quietly_offline() {
        assert!(start(Fake::new(vec![Err("offline")]).as_ref(), "https://x", "k").is_none());
    }

    #[test]
    fn poll_maps_every_server_answer() {
        let p = |r: Result<&str, &str>| poll(Fake::new(vec![r]).as_ref(), "https://x", "d1");
        assert_eq!(p(Ok(r#"{"status":"pending"}"#)), Poll::Pending);
        assert_eq!(p(Ok(r#"{"status":"ready","driver_id":"u1","token":"t","display_name":"Casey"}"#)), Poll::Ready { driver_id: "u1".into(), token: "t".into(), display_name: "Casey".into() });
        assert_eq!(p(Ok(r#"{"status":"ready","driver_id":"u1"}"#)), Poll::Retry);
        assert_eq!(p(Err("HTTP 410")), Poll::Expired);
        assert_eq!(p(Err("HTTP 400")), Poll::Error);
        assert_eq!(p(Err("HTTP 503")), Poll::Retry);
        assert_eq!(p(Err("dns")), Poll::Retry);
    }

    #[test]
    fn poll_schedule_matches_freight_fate() {
        assert_eq!(poll_interval(0.0), Duration::from_secs(3));
        assert_eq!(poll_interval(31.0), Duration::from_secs(8));
    }

    #[test]
    fn day_line_is_schema_versioned_and_404_is_skipped() {
        let g = game();
        let body = day_payload(&g);
        assert_eq!(body["schema_version"], 1);
        assert_eq!(body["on_time_pct"], 100.0);
        let ok = Fake::new(vec![Ok("{}")]);
        assert!(post_day(ok.as_ref(), "https://x", "t", &g));
        assert_eq!(ok.seen.lock().unwrap()[0].0, format!("https://x{DAY_PATH}"));
        assert!(!post_day(Fake::new(vec![Err("HTTP 404")]).as_ref(), "https://x", "t", &g));
    }

    #[test]
    fn identity_round_trips_without_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity { driver_id: "u1".into(), display_name: "Casey".into() };
        id.save(dir.path()).unwrap();
        assert_eq!(Identity::load(dir.path()), Some(id));
        assert!(!std::fs::read_to_string(dir.path().join("online.json")).unwrap().contains("token"));
    }
}
```

- [ ] **Step 2: Register the module and run the tests**

Run: `cargo test --lib online::`
Expected: 6 PASS. The keyring is not exercised in tests. It is only touched from the UI after a real sign-in.

- [ ] **Step 3: Commit**

```bash
git add src/online.rs src/lib.rs
git commit -m "feat: optional orinks.net sign-in and day-end profile line"
```

---

### Task 15: Menu grammar and speech

**Files:**
- Create: `src/ui/mod.rs`, `src/ui/speech.rs`
- Modify: `src/lib.rs` (add `pub mod ui;`)

**Interfaces:**
- Produces:
  - `ui::Key::{Up, Down, Home, End, Enter, Escape, F1, Ctrl, Char(char)}` with `Key::parse(&str) -> Option<Key>` (script words: `up down home end enter escape f1 ctrl`, or a single character)
  - `ui::Out::{Say(String), Queue(String), Stop, Quit}`
  - `ui::Item { label, help: String }`
  - `ui::Menu { title, items, index }` with `new(title, Vec<Item>)`, `current_text()`, `intro() -> Vec<Out>`, `lines() -> Vec<String>`, `key(Key) -> MenuEvent`
  - `ui::MenuEvent::{Speak(String), Activate(usize), Back, Stop, None}`
  - `ui::speech::Speaker` trait (`say(&mut self, &str, interrupt: bool)`, `stop(&mut self)`), `Console` (with `lines: Vec<String>` and `echo: bool`), `speaker() -> Box<dyn Speaker>`, `NO_SPEECH_ENV`

- [ ] **Step 1: Write `src/ui/mod.rs` with its tests**

```rust
//! Menu grammar shared with Freight Fate, and what the UI asks the
//! speaker to do. Every screen is a Menu, so every screen is also lines.

pub mod app;
pub mod speech;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Home,
    End,
    Enter,
    Escape,
    F1,
    Ctrl,
    Char(char),
}

impl Key {
    /// Script words for the key-script mode and tests.
    pub fn parse(word: &str) -> Option<Key> {
        Some(match word.trim().to_ascii_lowercase().as_str() {
            "up" => Key::Up,
            "down" => Key::Down,
            "home" => Key::Home,
            "end" => Key::End,
            "enter" => Key::Enter,
            "escape" => Key::Escape,
            "f1" => Key::F1,
            "ctrl" => Key::Ctrl,
            w if w.chars().count() == 1 => Key::Char(w.chars().next()?),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Interrupt and speak.
    Say(String),
    /// Speak after what's already speaking.
    Queue(String),
    Stop,
    Quit,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub help: String,
}

impl Item {
    pub fn new(label: impl Into<String>, help: impl Into<String>) -> Item {
        Item { label: label.into(), help: help.into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEvent {
    Speak(String),
    Activate(usize),
    Back,
    Stop,
    None,
}

#[derive(Clone, Debug)]
pub struct Menu {
    pub title: String,
    pub items: Vec<Item>,
    pub index: usize,
}

impl Menu {
    pub fn new(title: impl Into<String>, items: Vec<Item>) -> Menu {
        Menu { title: title.into(), items, index: 0 }
    }

    pub fn current_text(&self) -> String {
        match self.items.get(self.index) {
            Some(item) => format!("{}, {} of {}.", item.label.trim_end_matches('.'), self.index + 1, self.items.len()),
            None => "No options available.".into(),
        }
    }

    pub fn intro(&self) -> Vec<Out> {
        vec![Out::Say(self.title.clone()), Out::Queue(self.current_text())]
    }

    pub fn lines(&self) -> Vec<String> {
        std::iter::once(self.title.clone()).chain(self.items.iter().map(|i| i.label.clone())).collect()
    }

    pub fn key(&mut self, key: Key) -> MenuEvent {
        let n = self.items.len();
        match key {
            Key::Ctrl => return MenuEvent::Stop,
            Key::Escape => return MenuEvent::Back,
            Key::Enter if n > 0 => return MenuEvent::Activate(self.index),
            Key::F1 => {
                return MenuEvent::Speak(match self.items.get(self.index) {
                    Some(i) if !i.help.is_empty() => i.help.clone(),
                    Some(i) => format!("{}.", i.label.trim_end_matches('.')),
                    None => "No options available.".into(),
                })
            }
            _ if n == 0 => return MenuEvent::Speak(self.current_text()),
            Key::Down => self.index = (self.index + 1) % n,
            Key::Up => self.index = (self.index + n - 1) % n,
            Key::Home => self.index = 0,
            Key::End => self.index = n - 1,
            Key::Char(c) if c.is_alphanumeric() => {
                let c = c.to_lowercase().next().unwrap();
                let hit = (1..=n).map(|off| (self.index + off) % n).find(|&i| self.items[i].label.to_lowercase().starts_with(c));
                match hit {
                    Some(i) => self.index = i,
                    None => return MenuEvent::None,
                }
            }
            _ => return MenuEvent::None,
        }
        MenuEvent::Speak(self.current_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> Menu {
        Menu::new("Desk", vec![Item::new("Load board", "The day's loads."), Item::new("Trips", ""), Item::new("Tally", "")])
    }

    #[test]
    fn arrows_wrap_and_home_end_jump() {
        let mut m = menu();
        assert_eq!(m.key(Key::Up), MenuEvent::Speak("Tally, 3 of 3.".into()));
        assert_eq!(m.key(Key::Down), MenuEvent::Speak("Load board, 1 of 3.".into()));
        m.key(Key::End);
        assert_eq!(m.index, 2);
        m.key(Key::Home);
        assert_eq!(m.index, 0);
    }

    #[test]
    fn first_letter_cycles_matches_from_the_next_item() {
        let mut m = menu();
        m.key(Key::Char('T'));
        assert_eq!(m.index, 1);
        m.key(Key::Char('t'));
        assert_eq!(m.index, 2);
        assert_eq!(m.key(Key::Char('z')), MenuEvent::None);
    }

    #[test]
    fn enter_escape_help_and_stop() {
        let mut m = menu();
        assert_eq!(m.key(Key::Enter), MenuEvent::Activate(0));
        assert_eq!(m.key(Key::Escape), MenuEvent::Back);
        assert_eq!(m.key(Key::F1), MenuEvent::Speak("The day's loads.".into()));
        m.key(Key::Down);
        assert_eq!(m.key(Key::F1), MenuEvent::Speak("Trips.".into()));
        assert_eq!(m.key(Key::Ctrl), MenuEvent::Stop);
    }

    #[test]
    fn empty_menu_says_so_and_screens_are_lines() {
        let mut m = Menu::new("Board", vec![]);
        assert_eq!(m.key(Key::Enter), MenuEvent::Speak("No options available.".into()));
        assert_eq!(menu().lines(), vec!["Desk", "Load board", "Trips", "Tally"]);
    }

    #[test]
    fn script_words_parse() {
        assert_eq!(Key::parse("enter"), Some(Key::Enter));
        assert_eq!(Key::parse("F1"), Some(Key::F1));
        assert_eq!(Key::parse("c"), Some(Key::Char('c')));
        assert_eq!(Key::parse("nonsense"), None);
    }
}
```

- [ ] **Step 2: Write `src/ui/speech.rs`**

```rust
//! Speech through Prism, or the console when Prism isn't there or
//! FF_DISPATCH_NO_SPEECH=1.

pub const NO_SPEECH_ENV: &str = "FF_DISPATCH_NO_SPEECH";

pub trait Speaker {
    fn say(&mut self, text: &str, interrupt: bool);
    fn stop(&mut self);
}

/// Prints each line (when `echo`) and keeps them (for tests and scripts).
#[derive(Default)]
pub struct Console {
    pub lines: Vec<String>,
    pub echo: bool,
}

impl Speaker for Console {
    fn say(&mut self, text: &str, _interrupt: bool) {
        if self.echo {
            println!("{text}");
        }
        self.lines.push(text.to_string());
    }

    fn stop(&mut self) {}
}

struct Prism {
    backend: prismer::Backend<'static>,
}

impl Speaker for Prism {
    fn say(&mut self, text: &str, interrupt: bool) {
        if text.trim().is_empty() {
            return;
        }
        if self.backend.output(text, interrupt).is_err() {
            let _ = self.backend.speak(text, interrupt);
        }
    }

    fn stop(&mut self) {
        let _ = self.backend.stop();
    }
}

pub fn speaker() -> Box<dyn Speaker> {
    if std::env::var(NO_SPEECH_ENV).is_ok_and(|v| v == "1") {
        return Box::new(Console { lines: Vec::new(), echo: true });
    }
    // ponytail: one Prism context per process, leaked so its backend can be
    // 'static; add backend refresh (as FF does) if screen-reader switching
    // mid-session matters to testers.
    let backend = prismer::Prism::new().ok().and_then(|p| {
        let prism: &'static prismer::Prism = Box::leak(Box::new(p));
        let backend = prism.acquire_best().ok()?;
        match backend.initialize() {
            Ok(()) | Err(prismer::Error::AlreadyInitialized) => Some(backend),
            Err(_) => None,
        }
    });
    match backend {
        Some(backend) => Box::new(Prism { backend }),
        None => {
            log::warn!("Prism unavailable; speech goes to the console");
            Box::new(Console { lines: Vec::new(), echo: true })
        }
    }
}
```

Create `src/ui/app.rs` with only `//! Screens (Task 16).` so the module compiles.

- [ ] **Step 3: Register the module and run the tests**

Run: `cargo test --lib ui::`
Expected: 5 PASS. If `prismer::Error::AlreadyInitialized` has a different name in 0.1.3, check `~/.cargo/registry/src/*/prismer-0.1.3/src/error.rs` and use the real name.

- [ ] **Step 4: Commit**

```bash
git add src/ui src/lib.rs
git commit -m "feat: Freight Fate menu grammar and Prism speech with console fallback"
```

---

### Task 16: Screens, the phone interrupt, and the day flow

**Files:**
- Modify: `src/ui/app.rs`

**Interfaces:**
- Consumes: everything above
- Produces:
  - `Dirs { careers: PathBuf, ff_saves: Vec<PathBuf>, data: PathBuf }`
  - `App::new(world: &'static World, dirs: Dirs, seed: Option<i64>, live: bool) -> App` (`live` = network allowed for weather and online)
  - `App::start() -> Vec<Out>`, `App::key(Key) -> Vec<Out>`, `App::tick() -> Vec<Out>`
  - `App::screen_lines() -> Vec<String>`

- [ ] **Step 1: Write the failing tests at the bottom of `src/ui/app.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ff_core::data::world::get_world;

    fn app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let dirs = Dirs { careers: dir.path().join("careers"), ff_saves: vec![], data: dir.path().to_path_buf() };
        (App::new(get_world(), dirs, Some(42), false), dir)
    }

    fn said(out: &[Out]) -> String {
        out.iter().filter_map(|o| match o { Out::Say(s) | Out::Queue(s) => Some(s.as_str()), _ => None }).collect::<Vec<_>>().join(" | ")
    }

    #[test]
    fn start_menu_opens_with_the_title() {
        let (mut a, _d) = app();
        let out = said(&a.start());
        assert!(out.starts_with("Freight Fate: Dispatch"));
        assert!(a.screen_lines().contains(&"New career".to_string()));
    }

    #[test]
    fn new_career_briefs_saves_and_lands_on_the_desk() {
        let (mut a, d) = app();
        a.start();
        a.key(Key::Enter); // New career
        let out = said(&a.key(Key::Enter)); // first carrier
        assert!(out.contains("Day 1"));
        assert!(out.contains("loads on the board"));
        assert!(a.screen_lines()[0].starts_with("Desk"));
        assert_eq!(std::fs::read_dir(d.path().join("careers")).unwrap().count(), 1);
    }

    #[test]
    fn closing_the_day_rolls_to_day_two_and_saves() {
        let (mut a, _d) = app();
        a.start();
        a.key(Key::Enter);
        a.key(Key::Enter);
        a.key(Key::End); // Close the day
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("Day 2"), "{out}");
    }

    #[test]
    fn a_ringing_phone_is_announced_and_enter_answers_from_any_menu() {
        let (mut a, _d) = app();
        a.start();
        a.key(Key::Enter);
        a.key(Key::Enter);
        let (g, s) = (a.game.as_mut().unwrap(), a.shift.as_mut().unwrap());
        let id = g.drivers[0].id;
        s.pending.push_back(crate::career::Ring::Call(crate::phone::Call::PersonalDay { driver_id: id }));
        let out = said(&a.key(Key::Down));
        assert!(out.contains("phone is ringing"), "{out}");
        let out = said(&a.key(Key::Enter));
        assert!(out.contains("personal day"), "{out}");
        let out = said(&a.key(Key::Enter)); // Give them the day
        assert!(out.contains("Day granted"));
        assert!(a.shift.as_ref().unwrap().pending.is_empty());
    }

    #[test]
    fn escape_from_the_start_menu_quits() {
        let (mut a, _d) = app();
        a.start();
        assert!(a.key(Key::Escape).contains(&Out::Quit));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib ui::app::`
Expected: FAIL to compile.

- [ ] **Step 3: Write the implementation at the top of `src/ui/app.rs`**

```rust
//! Screens. The App owns the career and today's shift and turns keys into
//! spoken lines. It never touches SDL or Prism.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use ff_core::data::world::World;
use ff_core::models::start_options::START_OPTIONS;

use super::{Item, Key, Menu, MenuEvent, Out};
use crate::board::{load_line, refusal};
use crate::career::{Game, Ring, Shift};
use crate::import::{import_game, list_ff_saves, read_profile};
use crate::online::{self, Identity, Poll};
use crate::saves::{list_slots, load_game, save_game, Slot};

pub struct Dirs {
    pub careers: PathBuf,
    pub ff_saves: Vec<PathBuf>,
    pub data: PathBuf,
}

#[derive(Clone)]
enum Screen {
    Start,
    NewCareer,
    Continue,
    Import,
    Desk,
    Board,
    Drivers(usize),
    Trips,
    Roster,
    Hiring,
    Phone(Ring),
}

enum SignIn {
    Code(String),
    Done(Result<Identity, String>),
}

pub struct App {
    world: &'static World,
    dirs: Dirs,
    seed: Option<i64>,
    live: bool,
    stack: Vec<Screen>,
    menu: Menu,
    pub(crate) game: Option<Game>,
    pub(crate) shift: Option<Shift>,
    announced: usize,
    identity: Option<Identity>,
    sign_in: Option<Receiver<SignIn>>,
}

const DESK_HELP: [&str; 6] = [
    "Assign the day's loads to drivers. Assigning takes 10 minutes.",
    "Trucks on the road. Checking on one takes 2 minutes.",
    "Everyone on the roster and their hours.",
    "Today's applicants. An interview takes 15 minutes.",
    "Hear the morning briefing again.",
    "End the shift. Trips still rolling finish overnight, and the career saves.",
];

impl App {
    pub fn new(world: &'static World, dirs: Dirs, seed: Option<i64>, live: bool) -> App {
        let identity = if live { Identity::load(&dirs.data) } else { None };
        App { world, dirs, seed, live, stack: vec![Screen::Start], menu: Menu::new("", vec![]), game: None, shift: None, announced: 0, identity, sign_in: None }
    }

    pub fn screen_lines(&self) -> Vec<String> {
        self.menu.lines()
    }

    pub fn start(&mut self) -> Vec<Out> {
        self.rebuild();
        self.menu.intro()
    }

    fn rebuild(&mut self) {
        let index = self.menu.index;
        let top = self.stack.last().cloned().unwrap_or(Screen::Start);
        self.menu = self.build(&top);
        self.menu.index = index.min(self.menu.items.len().saturating_sub(1));
    }

    fn open(&mut self, screen: Screen) -> Vec<Out> {
        self.stack.push(screen);
        self.menu.index = 0;
        self.rebuild();
        self.menu.intro()
    }

    fn back(&mut self) -> Vec<Out> {
        if self.stack.len() <= 1 {
            return vec![Out::Quit];
        }
        self.stack.pop();
        self.menu.index = 0;
        self.rebuild();
        self.menu.intro()
    }

    fn build(&self, screen: &Screen) -> Menu {
        let w = self.world;
        match screen {
            Screen::Start => {
                let online = match &self.identity {
                    Some(id) => Item::new(format!("Signed in as {}", id.display_name), "Your orinks.net account. Dispatch posts your carrier line at the end of each day."),
                    None => Item::new("Sign in to orinks.net", "Optional. Uses your Freight Fate account."),
                };
                Menu::new("Freight Fate: Dispatch", vec![
                    Item::new("New career", "Pick a carrier and start with three drivers."),
                    Item::new("Continue", "Saved careers. Each saves at the start of a day."),
                    Item::new("Import a Freight Fate driver", "Bring your Freight Fate driver onto the roster. Your Freight Fate save is only read."),
                    online,
                    Item::new("Quit", ""),
                ])
            }
            Screen::NewCareer => Menu::new("Pick a carrier", START_OPTIONS.iter().map(|o| Item::new(o.label, o.help_text)).collect()),
            Screen::Continue => Menu::new("Continue", list_slots(&self.dirs.careers).iter().map(|s| Item::new(s.label(), "")).collect()),
            Screen::Import => Menu::new(
                "Import a Freight Fate driver",
                list_ff_saves(&self.dirs.ff_saves)
                    .iter()
                    .map(|p| Item::new(p.file_stem().unwrap_or_default().to_string_lossy(), "Read only. Your Freight Fate save is never changed."))
                    .collect(),
            ),
            Screen::Desk => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let labels = [
                    format!("Load board, {} loads", s.board.len()),
                    format!("Trips, {} rolling", g.trips.len()),
                    format!("Roster, {} drivers", g.drivers.len()),
                    format!("Hiring, {} applicants", s.applicants.len()),
                    "Briefing".to_string(),
                    "Close the day".to_string(),
                ];
                Menu::new(format!("Desk, {}", s.clock.spoken()), labels.into_iter().zip(DESK_HELP).map(|(l, h)| Item::new(l, h)).collect())
            }
            Screen::Board => {
                let s = self.shift.as_ref().unwrap();
                Menu::new("Load board", s.board.iter().map(|l| Item::new(load_line(&l.job, l.hot, w), "Enter picks a driver.")).collect())
            }
            Screen::Drivers(load) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let job = s.board.get(*load).map(|l| &l.job);
                Menu::new(
                    "Who takes it?",
                    g.drivers
                        .iter()
                        .map(|d| {
                            let why = job.and_then(|j| refusal(j, d, w));
                            Item::new(d.card(), why.unwrap_or_else(|| "Legal for this load.".into()))
                        })
                        .collect(),
                )
            }
            Screen::Trips => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                let name = |id: u32| g.driver(id).map(|d| d.name.clone()).unwrap_or_default();
                Menu::new(
                    "Trips",
                    g.trips
                        .iter()
                        .map(|t| Item::new(format!("{}, {} to {}", name(t.driver_id), t.job.cargo.label, w.spoken_city(&t.job.destination, None)), "Enter checks on the truck."))
                        .collect(),
                )
            }
            Screen::Roster => Menu::new("Roster", self.game.as_ref().unwrap().drivers.iter().map(|d| Item::new(d.card(), "")).collect()),
            Screen::Hiring => {
                let s = self.shift.as_ref().unwrap();
                let items = s
                    .applicants
                    .iter()
                    .flat_map(|a| {
                        [
                            Item::new(format!("Interview {}", a.driver.name), a.line()),
                            Item::new(format!("Hire {}, {}", a.driver.name, crate::money(a.signing)), a.line()),
                        ]
                    })
                    .collect();
                Menu::new("Hiring", items)
            }
            Screen::Phone(ring) => {
                let (g, s) = (self.game.as_ref().unwrap(), self.shift.as_ref().unwrap());
                Menu::new(ring.title(g, s, w), ring.labels(g, s, w).into_iter().map(|l| Item::new(l, "")).collect())
            }
        }
    }

    pub fn key(&mut self, key: Key) -> Vec<Out> {
        let ringing = self.shift.as_ref().is_some_and(|s| !s.pending.is_empty());
        let on_phone = matches!(self.stack.last(), Some(Screen::Phone(_)));
        if ringing && !on_phone && key == Key::Enter {
            return self.answer();
        }
        let mut out = match self.menu.key(key) {
            MenuEvent::Speak(text) => vec![Out::Say(text)],
            MenuEvent::Stop => vec![Out::Stop],
            MenuEvent::Back if on_phone => vec![Out::Say("Pick an answer; the caller is waiting.".into())],
            MenuEvent::Back => self.back(),
            MenuEvent::Activate(i) => self.activate(i),
            MenuEvent::None => vec![],
        };
        out.extend(self.announce_ring());
        out
    }

    fn announce_ring(&mut self) -> Vec<Out> {
        let Some(s) = self.shift.as_ref() else { return vec![] };
        let on_phone = matches!(self.stack.last(), Some(Screen::Phone(_)));
        if s.pending.len() > self.announced && !on_phone {
            self.announced = s.pending.len();
            return vec![Out::Queue("The phone is ringing. Press Enter to answer.".into())];
        }
        self.announced = self.announced.min(s.pending.len());
        vec![]
    }

    fn answer(&mut self) -> Vec<Out> {
        let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
        match g.answer(s) {
            Ok(ring) => {
                self.announced = self.announced.saturating_sub(1);
                self.open(Screen::Phone(ring))
            }
            Err(e) => vec![Out::Say(e.spoken())],
        }
    }

    /// Say lines after an action, then refresh the screen underneath.
    fn after(&mut self, lines: Vec<String>) -> Vec<Out> {
        self.rebuild();
        let mut out: Vec<Out> = lines.into_iter().map(Out::Queue).collect();
        if let Some(Out::Queue(first)) = out.first().cloned() {
            out[0] = Out::Say(first);
        }
        out
    }

    fn activate(&mut self, i: usize) -> Vec<Out> {
        let w = self.world;
        match self.stack.last().cloned().unwrap() {
            Screen::Start => match i {
                0 => self.open(Screen::NewCareer),
                1 => self.open(Screen::Continue),
                2 => self.open(Screen::Import),
                3 if self.identity.is_none() => self.begin_sign_in(),
                3 => vec![Out::Say(self.menu.items[3].help.clone())],
                _ => vec![Out::Quit],
            },
            Screen::NewCareer => {
                let seed = self.seed.unwrap_or_else(now_seed);
                self.begin(Game::new(START_OPTIONS[i].key, seed, w), true)
            }
            Screen::Continue => match list_slots(&self.dirs.careers).into_iter().nth(i) {
                Some(Slot::Ok { game, .. }) => self.begin(game, false),
                Some(slot) => vec![Out::Say(slot.label())],
                None => vec![],
            },
            Screen::Import => {
                let Some(path) = list_ff_saves(&self.dirs.ff_saves).into_iter().nth(i) else { return vec![] };
                let file = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                match read_profile(&path) {
                    Ok(profile) => {
                        let seed = self.seed.unwrap_or_else(now_seed);
                        self.begin(import_game(&profile, seed, w), true)
                    }
                    Err(e) => vec![Out::Say(e.spoken(&file))],
                }
            }
            Screen::Desk => match i {
                0 => self.open(Screen::Board),
                1 => self.open(Screen::Trips),
                2 => self.open(Screen::Roster),
                3 => self.open(Screen::Hiring),
                4 => {
                    let lines = self.game.as_ref().unwrap().briefing(self.shift.as_ref().unwrap(), w);
                    self.after(lines)
                }
                _ => self.close_day(),
            },
            Screen::Board => self.open(Screen::Drivers(i)),
            Screen::Drivers(load) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let id = g.drivers[i].id;
                match g.assign(s, load, id, w) {
                    Ok(lines) => {
                        self.stack.pop();
                        self.stack.pop();
                        self.after(lines)
                    }
                    Err(e) => vec![Out::Say(e.spoken())],
                }
            }
            Screen::Trips => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let id = g.trips[i].id;
                match g.check_truck(s, id, w) {
                    Ok(line) => self.after(vec![line]),
                    Err(e) => vec![Out::Say(e.spoken())],
                }
            }
            Screen::Roster => vec![Out::Say(self.menu.items[i].label.clone())],
            Screen::Hiring => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let r = if i % 2 == 0 { g.interview(s, i / 2) } else { g.hire(s, i / 2) };
                match r {
                    Ok(line) => self.after(vec![line]),
                    Err(e) => vec![Out::Say(e.spoken())],
                }
            }
            Screen::Phone(ring) => {
                let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
                let lines = g.resolve(s, &ring, i, w);
                self.stack.pop();
                self.after(lines)
            }
        }
    }

    /// Start a career day: weather (fresh days only), save, brief, desk.
    fn begin(&mut self, game: Game, fresh_day: bool) -> Vec<Out> {
        self.game = Some(game);
        self.stack.truncate(1);
        self.stack.push(Screen::Desk);
        self.menu.index = 0;
        let mut out = self.morning(fresh_day);
        self.rebuild();
        out.extend(self.menu.intro().into_iter().map(|o| match o {
            Out::Say(s) => Out::Queue(s),
            o => o,
        }));
        out
    }

    fn morning(&mut self, fresh_day: bool) -> Vec<Out> {
        let w = self.world;
        let game = self.game.as_mut().unwrap();
        if fresh_day {
            let fetched = if self.live { crate::net::live_weather(&game.todays_routes(w), w) } else { None };
            game.set_weather(fetched);
        }
        let mut out = Vec::new();
        if let Err(e) = save_game(game, &self.dirs.careers) {
            log::error!("save failed: {e}");
            out.push(Out::Say(format!("The career could not be saved: {e}")));
        }
        let shift = game.morning(w);
        let lines = game.briefing(&shift, w);
        self.shift = Some(shift);
        self.announced = 0;
        out.extend(lines.into_iter().map(Out::Queue));
        if let Some(Out::Queue(first)) = out.first().cloned() {
            out[0] = Out::Say(first);
        }
        out
    }

    fn close_day(&mut self) -> Vec<Out> {
        let w = self.world;
        let (g, s) = (self.game.as_mut().unwrap(), self.shift.as_mut().unwrap());
        if !s.pending.is_empty() {
            return vec![Out::Say(crate::career::ActError::PhoneRinging.spoken())];
        }
        let mut out: Vec<Out> = vec![Out::Say("Closing the desk.".into())];
        out.extend(g.close_day(s, w).into_iter().map(Out::Queue));
        if let (Some(id), true) = (&self.identity, self.live) {
            if let Some(token) = online::load_token(&id.driver_id) {
                let game = g.clone();
                std::thread::spawn(move || {
                    online::post_day(&crate::net::UreqTransport::new(10.0), &online::base_url(), &token, &game);
                });
            }
        }
        let morning = self.morning(true);
        out.extend(morning.into_iter().map(|o| match o {
            Out::Say(s) => Out::Queue(s),
            o => o,
        }));
        self.rebuild();
        out
    }

    fn begin_sign_in(&mut self) -> Vec<Out> {
        if !self.live {
            return vec![Out::Say("Online is off in this mode.".into())];
        }
        let (tx, rx) = channel();
        self.sign_in = Some(rx);
        let data = self.dirs.data.clone();
        std::thread::spawn(move || {
            let t = crate::net::UreqTransport::new(10.0);
            let base = online::base_url();
            let Some(a) = online::start(&t, &base, &online::machine_key()) else {
                let _ = tx.send(SignIn::Done(Err("orinks.net could not be reached. You can keep playing offline.".into())));
                return;
            };
            let _ = tx.send(SignIn::Code(format!("Go to {} and enter the code {}.", a.verification_uri, a.user_code.chars().map(String::from).collect::<Vec<_>>().join(" "))));
            let started = Instant::now();
            let result = loop {
                let elapsed = started.elapsed().as_secs_f64();
                if elapsed >= a.expires_in as f64 {
                    break Err("The sign-in code expired.".to_string());
                }
                std::thread::sleep(online::poll_interval(elapsed));
                match online::poll(&t, &base, &a.device_code) {
                    Poll::Ready { driver_id, token, display_name } => {
                        if !online::store_token(&driver_id, &token) {
                            break Err("The sign-in worked, but the token could not be stored in the system keyring.".into());
                        }
                        let id = Identity { driver_id, display_name };
                        let _ = id.save(&data);
                        break Ok(id);
                    }
                    Poll::Expired => break Err("The sign-in code expired.".into()),
                    Poll::Error => break Err("orinks.net refused the sign-in.".into()),
                    Poll::Pending | Poll::Retry => {}
                }
            };
            let _ = tx.send(SignIn::Done(result));
        });
        vec![Out::Say("Contacting orinks.net.".into())]
    }

    /// Background results (sign-in). The window calls this between key events.
    pub fn tick(&mut self) -> Vec<Out> {
        let Some(rx) = &self.sign_in else { return vec![] };
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                SignIn::Code(text) => out.push(Out::Say(text)),
                SignIn::Done(Ok(id)) => {
                    out.push(Out::Say(format!("Signed in as {}.", id.display_name)));
                    self.identity = Some(id);
                    self.sign_in = None;
                    self.rebuild();
                    break;
                }
                SignIn::Done(Err(e)) => {
                    out.push(Out::Say(e));
                    self.sign_in = None;
                    break;
                }
            }
        }
        out
    }
}

fn now_seed() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i64 & 0xFFFF_FFFF_FFFF).unwrap_or(1)
}

// tests (Step 1) go here
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib ui::`
Expected: every test PASSES. In `a_ringing_phone...`, the down key should announce the ring, because `announce_ring` runs after every key.

- [ ] **Step 5: Commit**

```bash
git add src/ui/app.rs
git commit -m "feat: screens, phone interrupt, day flow, sign-in"
```

---

### Task 17: The binary: SDL window, script mode, boot smoke test

**Files:**
- Modify: `src/main.rs`
- Create: `tests/smoke.rs`, `tests/fixtures/first_shift.keys`

**Interfaces:**
- Consumes: `ui::{app::{App, Dirs}, Key, Out, speech::{speaker, Console, Speaker}}`, `saves::{careers_dir, data_dir}`, `import::ff_profile_dirs`, `ff_core::data::data_resources::data_root`
- Produces: `ff-dispatch [--seed N] [--script FILE]`. Script mode reads one key word per line, ignoring blank lines and `#` comments. It prints every spoken line to stdout and never opens a window or the network.

- [ ] **Step 1: Write the failing smoke test**

`tests/fixtures/first_shift.keys`:
```
# New career, first carrier, hear help, look at the board, close the day, quit.
enter
enter
f1
enter
escape
end
enter
escape
escape
```

`tests/smoke.rs`:
```rust
use std::process::Command;

#[test]
fn boots_without_speech_and_plays_a_scripted_shift() {
    let dir = tempfile::tempdir().unwrap();
    let keys = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/first_shift.keys");
    let out = Command::new(env!("CARGO_BIN_EXE_ff-dispatch"))
        .args(["--seed", "7", "--script", keys])
        .env("FF_DISPATCH_DATA_DIR", dir.path())
        .env("FF_DISPATCH_NO_SPEECH", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    for line in ["Freight Fate: Dispatch", "Day 1 at", "loads on the board", "Assign the day's loads", "Load board", "Closing the desk.", "Day 2 at"] {
        assert!(text.contains(line), "missing {line:?} in:\n{text}");
    }
    assert_eq!(std::fs::read_dir(dir.path().join("careers")).unwrap().count(), 1);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test smoke`
Expected: FAIL (the binary still only prints a title).

- [ ] **Step 3: Write `src/main.rs`**

```rust
//! Freight Fate: Dispatch. The window, the keyboard and speech; all the
//! game is in the library.
// ponytail: console subsystem on purpose, so the no-speech fallback is
// visible; switch to windows_subsystem = "windows" once Prism is proven on testers.

use std::path::PathBuf;
use std::time::Duration;

use ff_dispatch::import::ff_profile_dirs;
use ff_dispatch::saves::{careers_dir, data_dir};
use ff_dispatch::ui::app::{App, Dirs};
use ff_dispatch::ui::speech::{speaker, Console, Speaker};
use ff_dispatch::ui::{Key, Out};
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let seed = arg("--seed").and_then(|s| s.parse().ok());
    let script = arg("--script").map(PathBuf::from);
    configure_logging(script.is_some());

    let mut voice: Box<dyn Speaker> = if script.is_some() { Box::new(Console { lines: Vec::new(), echo: true }) } else { speaker() };
    let root = ff_core::data::data_resources::data_root();
    if !root.join("world.ffdata").is_file() && !root.join("world_data/index.json").is_file() {
        let msg = format!("World data is missing. Dispatch expected it at {}.", root.join("world.ffdata").display());
        voice.say(&msg, true);
        eprintln!("{msg}");
        std::thread::sleep(Duration::from_secs(4)); // let the speech finish
        std::process::exit(1);
    }
    let world = ff_core::data::world::get_world();
    let dirs = Dirs { careers: careers_dir(), ff_saves: ff_profile_dirs(), data: data_dir() };
    let mut app = App::new(world, dirs, seed, script.is_none());

    match script {
        Some(path) => {
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            if flush(app.start(), voice.as_mut()) {
                return;
            }
            for key in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).filter_map(Key::parse) {
                if flush(app.key(key), voice.as_mut()) {
                    return;
                }
            }
        }
        None => {
            if let Err(e) = run_window(&mut app, voice.as_mut()) {
                log::error!("window: {e}");
                eprintln!("{e}");
            }
        }
    }
}

/// Speak what the app said; `true` means quit.
fn flush(out: Vec<Out>, voice: &mut dyn Speaker) -> bool {
    for o in out {
        match o {
            Out::Say(t) => voice.say(&t, true),
            Out::Queue(t) => voice.say(&t, false),
            Out::Stop => voice.stop(),
            Out::Quit => return true,
        }
    }
    false
}

fn translate(keycode: Keycode, keymod: Mod) -> Option<Key> {
    // sdl2 0.38: Keycode is a struct with associated consts (FF matches the same way).
    Some(match keycode {
        Keycode::UP => Key::Up,
        Keycode::DOWN => Key::Down,
        Keycode::HOME => Key::Home,
        Keycode::END => Key::End,
        Keycode::RETURN | Keycode::KP_ENTER => Key::Enter,
        Keycode::ESCAPE => Key::Escape,
        Keycode::F1 => Key::F1,
        Keycode::LCTRL | Keycode::RCTRL => Key::Ctrl,
        _ if keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) => return None,
        k => {
            let name = k.name();
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => Key::Char(c.to_ascii_lowercase()),
                _ => return None,
            }
        }
    })
}

fn run_window(app: &mut App, voice: &mut dyn Speaker) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let _window = video.window("Freight Fate: Dispatch", 640, 480).position_centered().build().map_err(|e| e.to_string())?;
    let mut pump = sdl.event_pump()?;
    if flush(app.start(), voice) {
        return Ok(());
    }
    loop {
        if let Some(event) = pump.wait_event_timeout(100) {
            for event in std::iter::once(event).chain(pump.poll_iter()) {
                match event {
                    Event::Quit { .. } => return Ok(()),
                    Event::KeyDown { keycode: Some(k), keymod, repeat: false, .. } => {
                        if let Some(key) = translate(k, keymod) {
                            if flush(app.key(key), voice) {
                                return Ok(());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if flush(app.tick(), voice) {
            return Ok(());
        }
    }
}

fn configure_logging(script: bool) {
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if !script {
        let dir = data_dir().join("logs");
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(file) = std::fs::File::create(dir.join("dispatch.log")) {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
        }
    }
    let _ = builder.try_init();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("Fatal error: {info}");
        default_hook(info);
    }));
}
```

- [ ] **Step 4: Run the smoke test**

Run: `cargo test --test smoke`
Expected: PASS. If a line is missing, read the printed stdout. The key script avoids any ring: the desk closes at 6 AM, before the first scheduled call at 6:10. Fix the app, not the assertion, unless the wording of a line changed on purpose.

- [ ] **Step 5: Try the real window once by hand**

Run: `cargo run`
Expected: a window opens and speech (or console lines) announces "Freight Fate: Dispatch". Arrows, Enter and Escape work, and Escape at the start menu quits.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs tests/smoke.rs tests/fixtures/first_shift.keys
git commit -m "feat: SDL window, key-script mode, boot smoke test"
```

---

### Task 18: Windows portable zip, README, full sweep

**Files:**
- Modify: `.github/workflows/ci.yml` (add the `release` job)
- Create: `README.md`

- [ ] **Step 1: Write `README.md` (player-facing, ships in the zip)**

```markdown
# Freight Fate: Dispatch (beta)

Run the desk at a Freight Fate carrier. Assign loads, then handle what the
road throws back over the phone: hours running out, breakdowns, weather and
detention. Time only moves when you act.

## Keys

| Key | Effect |
|---|---|
| Up / Down | Move, with wraparound |
| Home / End | First / last item |
| Letter | Jump to the next item starting with it |
| Enter | Select, or answer a ringing phone |
| Escape | Back |
| F1 | Help for the current item |
| Control | Stop speech |

## Files

- Careers save at the start of each day in `%APPDATA%\FreightFateDispatch\careers`.
- Importing a Freight Fate driver only reads the Freight Fate save.
  - Dispatch looks in `%APPDATA%\FreightFate\profiles` and in a `FreightFate` folder unzipped beside this one.
  - To use another folder, set `FF_DISPATCH_FREIGHT_FATE_SAVES`.
- `FF_DISPATCH_NO_SPEECH=1` prints speech to the console instead of speaking.
- Sign-in to orinks.net is optional and uses your Freight Fate account.
```

- [ ] **Step 2: Add the release job to `.github/workflows/ci.yml`**

```yaml
  release:
    if: startsWith(github.ref, 'refs/tags/v')
    needs: [fmt, test]
    runs-on: windows-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
      - run: rustup show
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --release --locked
      - name: Read the ff-core pin
        id: pin
        shell: bash
        run: echo "ref=$(grep -oP 'ff-core = \{[^}]*(tag|rev) = "\K[^"]+' Cargo.toml)" >> "$GITHUB_OUTPUT"
      - uses: actions/checkout@v4
        with:
          repository: Orinks/Freight-fate
          ref: ${{ steps.pin.outputs.ref }}
          path: ff
      - name: Bake world.ffdata from the pinned Freight Fate
        working-directory: ff
        run: cargo run --release -p ff-core --bin ff-bake -- --data-dir data --out ../world.ffdata
      - name: Package
        shell: pwsh
        run: |
          $stage = "stage/FreightFateDispatch"
          New-Item -ItemType Directory -Force "$stage/freight_fate/data" | Out-Null
          Copy-Item target/release/ff-dispatch.exe "$stage/FreightFateDispatch.exe"
          Copy-Item target/release/SDL2.dll $stage
          Copy-Item world.ffdata "$stage/freight_fate/data/world.ffdata"
          Copy-Item README.md, LICENSE $stage
          $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
          $crt = & $vswhere -latest -find "VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT\*.dll"
          foreach ($dll in "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll", "msvcp140_atomic_wait.dll") {
            $src = $crt | Where-Object { (Split-Path $_ -Leaf) -eq $dll } | Select-Object -First 1
            if (-not $src) { throw "missing $dll" }
            Copy-Item $src $stage
          }
          Compress-Archive -Path $stage -DestinationPath "FreightFateDispatch-${{ github.ref_name }}-windows-portable.zip"
      - name: Smoke the packaged exe
        shell: pwsh
        env:
          FF_DISPATCH_NO_SPEECH: "1"
          FF_DISPATCH_DATA_DIR: ${{ runner.temp }}/dispatch
        run: |
          $said = ./stage/FreightFateDispatch/FreightFateDispatch.exe --seed 7 --script tests/fixtures/first_shift.keys
          if (-not ($said | Select-String -Quiet "Day 2 at")) { $said; throw "packaged smoke failed" }
      - name: Publish
        shell: bash
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release create "${{ github.ref_name }}" FreightFateDispatch-*-windows-portable.zip --prerelease --title "Dispatch ${{ github.ref_name }}" --notes "Tester beta."
```

The packaged smoke step checks that the zip layout puts `world.ffdata` where `ff-core`'s `data_root()` looks: `<exe dir>/freight_fate/data`.

- [ ] **Step 3: Lint the workflow**

Run: `actionlint`
Expected: no findings. Fix anything it reports.

- [ ] **Step 4: Full sweep, once**

Run: `cargo fmt --all --check`, then `cargo clippy --all-targets --locked -- -D warnings`, then `cargo test --locked`
Expected: fmt and clippy clean, and every test passes. Report the pass count.

- [ ] **Step 5: Commit and push**

```bash
git add README.md .github/workflows/ci.yml
git commit -m "ci: Windows portable zip on v* tags; player README"
git push
```

Watch the CI run on the push. Tag `v0.1.0-beta.1` only when the user asks.

---

## Spec coverage

| Spec section | Task |
|---|---|
| Toolchain, pins, deps | 1 |
| World data from `get_world`; missing-data exit | 1, 17 |
| `roster` | 2 |
| `desk`, action costs, clock only moves on actions | 3, 6 |
| `board`, refusal gates | 4 |
| `trip` | 5 |
| Shift loop: briefing, work, close, overnight | 6, 16 |
| Determinism and replay | 1, 11 |
| Incidents: hours short, breakdown, detention | 7 |
| Incidents: weather, real NWS weather stored in the save | 8, 12 |
| `phone` | 9 |
| `hiring`, cap of 10 | 10 |
| `saves` | 11 |
| Start menu: new, continue, import (newer refused, integrity imports) | 13, 16 |
| Online: sign-in, keyring, day line, schema_version, silent skip | 14, 16 |
| UI grammar, phone interrupt, lines, speech fallback | 15, 16, 17 |
| Error handling | 11, 12, 13, 14, 17 |
| Testing: headless, import fixtures, fake transport, boot smoke, CI | every task, 17, 18 |
| Release zip | 18 |
