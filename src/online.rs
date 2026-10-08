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
    post_json_response(t, url, body, headers).map_err(|failure| {
        if let Some(status) = failure.status {
            log::warn!("online: {url}: HTTP {status}");
        } else {
            log::warn!("online: {url}: invalid response");
        }
        failure.status
    })
}

struct PostFailure {
    status: Option<u16>,
    body: Option<Value>,
}

fn post_json_response(
    t: &dyn HttpTransport,
    url: &str,
    body: &Value,
    headers: &[(&str, &str)],
) -> Result<Value, PostFailure> {
    let bytes = t
        .post(url, body.to_string().as_bytes(), headers, 10.0)
        .map_err(|e| {
            let status = http_status(&e);
            let body = e
                .message
                .split_once('\n')
                .and_then(|(_, body)| serde_json::from_str(body).ok());
            PostFailure { status, body }
        })?;
    serde_json::from_slice(&bytes).map_err(|_| PostFailure {
        status: None,
        body: None,
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Delay,
    Hours,
    RoadConditions,
    TruckTrouble,
    LoadTrouble,
}

impl CallKind {
    fn parse(kind: &str) -> Option<Self> {
        Some(match kind {
            "delay" => Self::Delay,
            "hours" => Self::Hours,
            "road_conditions" => Self::RoadConditions,
            "truck_trouble" => Self::TruckTrouble,
            "load_trouble" => Self::LoadTrouble,
            _ => return None,
        })
    }

    pub fn words(self) -> &'static str {
        match self {
            Self::Delay => "a delay",
            Self::Hours => "their hours",
            Self::RoadConditions => "the road ahead",
            Self::TruckTrouble => "truck trouble",
            Self::LoadTrouble => "load trouble",
        }
    }

    pub fn choices(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Delay => &[
                ("Tell them to keep going", "continue"),
                (
                    "Tell them it's tight and to update you at the next stop",
                    "watch",
                ),
                ("Mark the load late", "late_update"),
            ],
            Self::Hours => &[
                ("Tell them to plan their next rest", "plan_rest"),
                ("Tell them to park now", "stop"),
            ],
            Self::RoadConditions => &[
                ("Tell them the road looks clear", "continue"),
                ("Tell them to drive with caution", "caution"),
            ],
            Self::TruckTrouble => &[
                ("Tell them to watch the gauges", "monitor"),
                ("Authorize a roadside repair", "repair_authorized"),
            ],
            Self::LoadTrouble => &[
                ("Tell them to carry on", "continue"),
                ("Tell them to protect the load", "protect_load"),
            ],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallFacts {
    pub remaining_miles: f64,
    pub hours_left: f64,
    pub truck_damage_pct: f64,
    pub cargo_damage_pct: f64,
    pub hos_remaining_minutes: Option<f64>,
    pub weather_alerts: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpenCall {
    pub call_id: String,
    pub caller_name: String,
    pub kind: CallKind,
    pub facts: CallFacts,
    pub ring_until: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallError {
    NotListed,
    Unauthorized,
    Taken,
    Other,
}

fn finite(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn parse_open_call(v: &Value) -> Option<OpenCall> {
    let kind = CallKind::parse(v.get("kind")?.as_str()?)?;
    let facts = v.get("facts")?;
    let clamp = |key, min, max| finite(facts, key).map(|n| n.clamp(min, max));
    let mut caller_name: String = v
        .get("callerName")
        .and_then(Value::as_str)
        .unwrap_or("driver")
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect();
    if caller_name.is_empty() {
        caller_name = "driver".into();
    }
    let alerts = finite(facts, "weatherAlerts")?.clamp(0.0, 20.0).round() as u8;
    Some(OpenCall {
        call_id: text(v, "callId")?,
        caller_name,
        kind,
        facts: CallFacts {
            remaining_miles: clamp("remainingMiles", 0.0, 10_000.0)?,
            hours_left: clamp("hoursLeft", -1000.0, 1000.0)?,
            truck_damage_pct: clamp("truckDamagePct", 0.0, 100.0)?,
            cargo_damage_pct: clamp("cargoDamagePct", 0.0, 100.0)?,
            hos_remaining_minutes: finite(facts, "hosRemainingMinutes")
                .map(|n| n.clamp(0.0, 10_000.0)),
            weather_alerts: alerts,
        },
        ring_until: v.get("ringUntil")?.as_u64()?,
    })
}

fn dispatch_post(
    t: &dyn HttpTransport,
    base: &str,
    path: &str,
    token: &str,
    body: Value,
) -> Result<Value, CallError> {
    let auth = format!("Bearer {token}");
    let headers = [JSON[0], JSON[1], ("Authorization", auth.as_str())];
    post_json_response(
        t,
        &format!("{base}/api/freight-fate/dispatch-calls/{path}"),
        &body,
        &headers,
    )
    .map_err(|failure| {
        if failure.status == Some(401) {
            CallError::Unauthorized
        } else if failure.status == Some(403)
            && failure
                .body
                .as_ref()
                .and_then(|v| v.get("error"))
                .and_then(Value::as_str)
                == Some("not_listed")
        {
            CallError::NotListed
        } else if matches!(failure.status, Some(409 | 410)) {
            CallError::Taken
        } else {
            CallError::Other
        }
    })
}

pub fn open_calls(
    t: &dyn HttpTransport,
    base: &str,
    identity: &Identity,
    token: &str,
) -> Result<Vec<OpenCall>, CallError> {
    let response = dispatch_post(
        t,
        base,
        "open",
        token,
        json!({ "driverId": identity.driver_id }),
    )?;
    let Some(calls) = response.get("calls").and_then(Value::as_array) else {
        return Err(CallError::Other);
    };
    Ok(calls.iter().filter_map(parse_open_call).collect())
}

pub fn claim_call(
    t: &dyn HttpTransport,
    base: &str,
    identity: &Identity,
    token: &str,
    call_id: &str,
) -> Result<(), CallError> {
    let response = dispatch_post(
        t,
        base,
        "claim",
        token,
        json!({ "driverId": identity.driver_id, "callId": call_id }),
    )?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(CallError::Other)
    }
}

pub fn answer_call(
    t: &dyn HttpTransport,
    base: &str,
    identity: &Identity,
    token: &str,
    call_id: &str,
    decision: &str,
) -> Result<(), CallError> {
    let response = dispatch_post(
        t,
        base,
        "answer",
        token,
        json!({
            "driverId": identity.driver_id,
            "callId": call_id,
            "decision": decision
        }),
    )?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(CallError::Other)
    }
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
    fn open_calls_drop_unknown_kinds_and_sanitize_and_clamp_known_calls() {
        let name = format!("A\n{}", "x".repeat(65));
        let response = json!({
            "calls": [
                {"callId":"unknown","callerName":"Name","kind":"made_up","facts":{},"ringUntil":9},
                {
                    "callId":"known",
                    "callerName":name,
                    "kind":"delay",
                    "ringUntil":1234,
                    "facts":{
                        "remainingMiles":-5,
                        "hoursLeft":2000,
                        "truckDamagePct":-1,
                        "cargoDamagePct":120,
                        "hosRemainingMinutes":12000,
                        "weatherAlerts":99,
                        "summary":"discarded"
                    },
                    "extra":"ignored"
                }
            ],
            "extra":"ignored"
        })
        .to_string();
        let fake = Fake::new_owned(vec![Ok(response)]);
        let calls = open_calls(
            fake.as_ref(),
            "https://x",
            &Identity {
                driver_id: "u1".into(),
                display_name: String::new(),
            },
            "token",
        )
        .unwrap();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.call_id, "known");
        assert_eq!(call.caller_name.len(), 60);
        assert!(!call.caller_name.chars().any(char::is_control));
        assert_eq!(call.ring_until, 1234);
        assert_eq!(call.facts.remaining_miles, 0.0);
        assert_eq!(call.facts.hours_left, 1000.0);
        assert_eq!(call.facts.truck_damage_pct, 0.0);
        assert_eq!(call.facts.cargo_damage_pct, 100.0);
        assert_eq!(call.facts.hos_remaining_minutes, Some(10_000.0));
        assert_eq!(call.facts.weather_alerts, 20);
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].0, "https://x/api/freight-fate/dispatch-calls/open");
        assert_eq!(
            serde_json::from_slice::<Value>(&seen[0].1).unwrap(),
            json!({"driverId":"u1"})
        );
    }

    #[test]
    fn remote_call_choices_match_the_design() {
        assert_eq!(
            CallKind::Delay.choices(),
            [
                ("Tell them to keep going", "continue"),
                (
                    "Tell them it's tight and to update you at the next stop",
                    "watch"
                ),
                ("Mark the load late", "late_update")
            ]
        );
        assert_eq!(
            CallKind::Hours.choices(),
            [
                ("Tell them to plan their next rest", "plan_rest"),
                ("Tell them to park now", "stop")
            ]
        );
        assert_eq!(
            CallKind::RoadConditions.choices(),
            [
                ("Tell them the road looks clear", "continue"),
                ("Tell them to drive with caution", "caution")
            ]
        );
        assert_eq!(
            CallKind::TruckTrouble.choices(),
            [
                ("Tell them to watch the gauges", "monitor"),
                ("Authorize a roadside repair", "repair_authorized")
            ]
        );
        assert_eq!(
            CallKind::LoadTrouble.choices(),
            [
                ("Tell them to carry on", "continue"),
                ("Tell them to protect the load", "protect_load")
            ]
        );
    }

    #[test]
    fn claim_maps_lost_calls_and_answer_sends_only_the_fixed_fields() {
        let identity = Identity {
            driver_id: "u1".into(),
            display_name: String::new(),
        };
        for response in [Err("HTTP 409"), Err("HTTP 410")] {
            assert_eq!(
                claim_call(
                    Fake::new(vec![response]).as_ref(),
                    "https://x",
                    &identity,
                    "token",
                    "call-1"
                ),
                Err(CallError::Taken)
            );
        }
        let fake = Fake::new(vec![Ok(r#"{"ok":true}"#)]);
        answer_call(
            fake.as_ref(),
            "https://x",
            &identity,
            "token",
            "call-1",
            "watch",
        )
        .unwrap();
        let seen = fake.seen.lock().unwrap();
        assert_eq!(
            seen[0].0,
            "https://x/api/freight-fate/dispatch-calls/answer"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&seen[0].1).unwrap(),
            json!({"driverId":"u1","callId":"call-1","decision":"watch"})
        );
    }

    #[test]
    fn open_reports_a_not_listed_forbidden_response() {
        let fake = Fake::new(vec![Err("HTTP 403\n{\"error\":\"not_listed\"}")]);
        assert_eq!(
            open_calls(
                fake.as_ref(),
                "https://x",
                &Identity {
                    driver_id: "u1".into(),
                    display_name: String::new(),
                },
                "token"
            ),
            Err(CallError::NotListed)
        );
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
