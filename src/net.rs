//! HTTP for Dispatch: one ureq transport behind ff-core's own trait, and
//! the morning weather fetch that must never hold up the morning.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ff_core::data::world::World;
use ff_core::data::world_models::Route;
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
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        UreqTransport { agent }
    }
}

fn finish(
    resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<Vec<u8>, TransportError> {
    let mut resp = resp.map_err(|e| err(e.to_string()))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        return Err(err(format!("HTTP {status}")));
    }
    resp.body_mut()
        .read_to_vec()
        .map_err(|e| err(e.to_string()))
}

impl HttpTransport for UreqTransport {
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        _timeout_s: f64,
    ) -> Result<Vec<u8>, TransportError> {
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        finish(req.call())
    }

    fn post(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
        _timeout_s: f64,
    ) -> Result<Vec<u8>, TransportError> {
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
    fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        timeout_s: f64,
    ) -> Result<Vec<u8>, TransportError> {
        let r = self.inner.get(url, headers, timeout_s);
        if r.is_err() {
            self.failed.store(true, Ordering::Relaxed);
        }
        r
    }

    fn post(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
        timeout_s: f64,
    ) -> Result<Vec<u8>, TransportError> {
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
    let watch = Arc::new(Watch {
        inner: transport,
        failed: AtomicBool::new(false),
    });
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
    while points
        .iter()
        .any(|&(lat, lon)| provider.get(lat, lon).is_none())
    {
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
                    .map(|a| StoredAlert {
                        event: a.alert.event,
                        city: a.near_city,
                        delay_min: a.delay_h * 60.0,
                        avoid: a.effect == AlertEffect::Avoid,
                    })
                    .collect(),
                None => Vec::new(),
            })
            .collect(),
    )
}

pub fn live_weather(routes: &[Option<Route>], world: &World) -> Option<Vec<Vec<StoredAlert>>> {
    fetch_day_weather(
        Arc::new(UreqTransport::new(WEATHER_BUDGET.as_secs_f64())),
        routes,
        world,
        WEATHER_BUDGET,
        true,
    )
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
            let replies = replies
                .into_iter()
                .map(|r| {
                    r.map(|b| b.as_bytes().to_vec())
                        .map_err(|m| err(m.to_string()))
                })
                .collect();
            Arc::new(Fake {
                replies: Mutex::new(replies),
                seen: Mutex::new(Vec::new()),
                delay: Duration::ZERO,
            })
        }

        fn next(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, TransportError> {
            std::thread::sleep(self.delay);
            self.seen
                .lock()
                .unwrap()
                .push((url.to_string(), body.to_vec()));
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
        fn post(
            &self,
            url: &str,
            body: &[u8],
            _: &[(&str, &str)],
            _: f64,
        ) -> Result<Vec<u8>, TransportError> {
            self.next(url, body)
        }
    }

    const STORM: &str = r#"{"features":[{"id":"urn:1","properties":{"id":"urn:1","event":"Winter Storm Warning","severity":"Severe","headline":"Winter Storm Warning","areaDesc":"Area","description":"Snow."}}]}"#;

    #[test]
    fn live_alerts_are_stored_per_board_route() {
        let w = get_world();
        let routes = game().todays_routes(w);
        let got = fetch_day_weather(
            Fake::new(vec![Ok(STORM)]),
            &routes,
            w,
            WEATHER_BUDGET,
            false,
        )
        .unwrap();
        assert_eq!(got.len(), routes.len());
        assert!(got
            .iter()
            .zip(&routes)
            .all(|(a, r)| r.is_none() || a.iter().any(|a| a.event == "Winter Storm Warning")));
    }

    #[test]
    fn a_failed_fetch_falls_back_to_seeded() {
        let w = get_world();
        let routes = game().todays_routes(w);
        assert!(fetch_day_weather(
            Fake::new(vec![Err("offline")]),
            &routes,
            w,
            WEATHER_BUDGET,
            false
        )
        .is_none());
    }

    #[test]
    fn a_slow_network_never_holds_the_morning_past_the_budget() {
        let w = get_world();
        let routes = game().todays_routes(w);
        let slow = Arc::new(Fake {
            delay: Duration::from_secs(2),
            ..Arc::try_unwrap(Fake::new(vec![Ok(STORM)])).ok().unwrap()
        });
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
