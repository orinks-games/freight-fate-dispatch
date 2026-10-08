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
pub const EVENTS_PATH: &str = "/api/freight-fate/events";
// Off until Dispatch is announced: orinks.net omits these rows from public
// journals, but they count toward its 50-event cap and displace Freight Fate entries.
pub const POST_DAY_JOURNAL: bool = false;

const JSON: &[(&str, &str)] = &[
    ("Content-Type", "application/json"),
    (
        "User-Agent",
        concat!("FreightFateDispatch/", env!("CARGO_PKG_VERSION")),
    ),
];

pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .unwrap_or_else(|_| DEFAULT_BASE_URL.into())
        .trim_end_matches('/')
        .to_string()
}

pub fn machine_key() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
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
    Ready {
        driver_id: String,
        token: String,
        display_name: String,
    },
    Pending,
    Expired,
    Error,
    Retry,
}

fn post_json(
    t: &dyn HttpTransport,
    url: &str,
    body: &Value,
    headers: &[(&str, &str)],
) -> Result<Value, Option<u16>> {
    let bytes = t
        .post(url, body.to_string().as_bytes(), headers, 10.0)
        .map_err(|e| {
            log::warn!("online: {url}: {}", e.message);
            http_status(&e)
        })?;
    serde_json::from_slice(&bytes).map_err(|_| None)
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

pub fn start(t: &dyn HttpTransport, base: &str, machine_key: &str) -> Option<Activation> {
    let v = post_json(
        t,
        &format!("{base}/api/freight-fate/activate/start"),
        &json!({ "machine_key": machine_key }),
        JSON,
    )
    .ok()?;
    Some(Activation {
        device_code: text(&v, "device_code")?,
        user_code: text(&v, "user_code")?,
        verification_uri: text(&v, "verification_uri")?,
        expires_in: v.get("expires_in").and_then(Value::as_u64).unwrap_or(600),
    })
}

pub fn poll(t: &dyn HttpTransport, base: &str, device_code: &str) -> Poll {
    match post_json(
        t,
        &format!("{base}/api/freight-fate/activate/poll"),
        &json!({ "device_code": device_code }),
        JSON,
    ) {
        Err(Some(410)) => Poll::Expired,
        Err(Some(400)) => Poll::Error,
        Err(_) => Poll::Retry,
        Ok(v) => match v.get("status").and_then(Value::as_str) {
            Some("pending") => Poll::Pending,
            Some("ready") => match (text(&v, "driver_id"), text(&v, "token")) {
                (Some(driver_id), Some(token)) => Poll::Ready {
                    driver_id,
                    token,
                    display_name: text(&v, "display_name").unwrap_or_default(),
                },
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

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardDriver {
    pub display_name: String,
}

#[derive(Deserialize)]
struct Board {
    #[serde(default)]
    drivers: Vec<BoardDriver>,
}

pub fn board(t: &dyn HttpTransport, base: &str) -> Result<Vec<BoardDriver>, String> {
    let body = t
        .get(
            &format!("{base}/api/freight-fate/presence"),
            &[JSON[1]],
            10.0,
        )
        .map_err(|error| error.message)?;
    serde_json::from_slice::<Board>(&body)
        .map(|board| board.drivers)
        .map_err(|error| format!("The online board returned invalid data: {error}"))
}

impl BoardDriver {
    pub fn spoken(&self) -> String {
        format!("{}.", self.display_name)
    }
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
    keyring::Entry::new(TOKEN_SERVICE, driver_id)
        .and_then(|e| e.get_password())
        .ok()
}

pub fn day_payload(driver_id: &str, game: &Game) -> Value {
    json!({
        "driverId": driver_id,
        "eventId": format!(
            "dispatch:{}:{}:{}",
            game.carrier_key,
            game.seed,
            game.record.days_worked
        ),
        "eventType": "dispatch_shift_completed",
        "summary": format!(
            "{} closed dispatch day {}; {} deliveries so far, {} percent on time.",
            game.name,
            game.record.days_worked,
            game.record.delivered,
            game.record.on_time_pct()
        ),
    })
}

/// End-of-day road-journal event. The stable event ID makes retries safe.
pub fn post_day(
    t: &dyn HttpTransport,
    base: &str,
    driver_id: &str,
    token: &str,
    game: &Game,
) -> bool {
    let auth = format!("Bearer {token}");
    let headers = [JSON[0], JSON[1], ("Authorization", auth.as_str())];
    t.post(
        &format!("{base}{EVENTS_PATH}"),
        day_payload(driver_id, game).to_string().as_bytes(),
        &headers,
        10.0,
    )
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
        let fake = Fake::new(vec![Ok(
            r#"{"device_code":"d1","user_code":"ABCD-1234","verification_uri":"https://www.orinks.net/activate","expires_in":600,"interval":5}"#,
        )]);
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
        assert_eq!(
            p(Ok(
                r#"{"status":"ready","driver_id":"u1","token":"t","display_name":"Casey"}"#
            )),
            Poll::Ready {
                driver_id: "u1".into(),
                token: "t".into(),
                display_name: "Casey".into()
            }
        );
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
    fn day_line_uses_the_existing_idempotent_event_api() {
        let g = game();
        let body = day_payload("u1", &g);
        assert_eq!(body["driverId"], "u1");
        assert_eq!(body["eventType"], "dispatch_shift_completed");
        assert!(body["eventId"].as_str().unwrap().starts_with("dispatch:"));
        assert_eq!(
            body["summary"],
            format!(
                "{} closed dispatch day 0; 0 deliveries so far, 100 percent on time.",
                g.name
            )
        );
        let ok = Fake::new(vec![Ok("{}")]);
        assert!(post_day(ok.as_ref(), "https://x", "u1", "t", &g));
        assert_eq!(
            ok.seen.lock().unwrap()[0].0,
            format!("https://x{EVENTS_PATH}")
        );
        assert!(!post_day(
            Fake::new(vec![Err("HTTP 404")]).as_ref(),
            "https://x",
            "u1",
            "t",
            &g
        ));
    }

    #[test]
    fn identity_round_trips_without_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity {
            driver_id: "u1".into(),
            display_name: "Casey".into(),
        };
        id.save(dir.path()).unwrap();
        assert_eq!(Identity::load(dir.path()), Some(id));
        assert!(!std::fs::read_to_string(dir.path().join("online.json"))
            .unwrap()
            .contains("token"));
    }

    #[test]
    fn board_ignores_activity_and_detail_in_presence_json() {
        let fake = Fake::new(vec![Ok(
            r#"{"asOf":1,"drivers":[{"driverId":"u1","displayName":"Casey","activity":"Driving: Reno to Boise","detail":"45 percent there","changedAt":1}]}"#,
        )]);
        let drivers = board(fake.as_ref(), "https://x").unwrap();
        assert_eq!(drivers[0].spoken(), "Casey.");
        assert_eq!(
            fake.seen.lock().unwrap()[0].0,
            "https://x/api/freight-fate/presence"
        );
    }
}
