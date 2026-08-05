//! Environment-sensor module: the AtomS3U/ENV-III firmware speaks UDP
//! discovery on the home broadcast and an HTTP `/sensors` endpoint behind
//! shared Basic auth. Readings become `environment` measurement lines in
//! this repo's schema (docs/architecture.md): one field per quantity in SI
//! units, normalized at the edge, with the line timestamp corrected by the
//! device-reported sample age.

use crate::config::{BasicCredentials, PollerConfig};
use crate::http;
use crate::json::{self, Json};
use crate::lineproto::{self, FieldValue};
use crate::metrics::{self, Metrics};
use crate::spool::Spool;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, PartialEq)]
pub struct EnvDevice {
    pub ip: Ipv4Addr,
    pub name: String,
    pub model: Option<String>,
    pub tags: Vec<(String, String)>,
}

/// Parse one discovery reply datagram. The firmware answers with a JSON
/// object carrying `deviceId` plus optional metadata and user tags.
pub fn parse_discovery_reply(payload: &[u8], src: Ipv4Addr) -> Option<EnvDevice> {
    let text = std::str::from_utf8(payload).ok()?;
    let doc = json::parse(text).ok()?;
    let name = doc
        .get("deviceId")
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())?
        .to_string();
    Some(EnvDevice {
        ip: src,
        name,
        model: doc.get("model").and_then(Json::as_str).map(str::to_string),
        tags: parse_device_tags(doc.get("m5_tags")),
    })
}

/// User tags arrive as a map, a list of single-entry maps, or "k=v" strings
/// — the same permissive shapes the Python collector accepted.
pub fn parse_device_tags(node: Option<&Json>) -> Vec<(String, String)> {
    let mut tags = Vec::new();
    match node {
        Some(Json::Obj(map)) => {
            for (key, value) in map {
                if let Some(v) = value.as_str() {
                    tags.push((key.clone(), v.to_string()));
                }
            }
        }
        Some(Json::Arr(items)) => {
            for item in items {
                match item {
                    Json::Str(s) => {
                        if let Some((k, v)) = s.split_once('=') {
                            if !k.is_empty() && !v.is_empty() {
                                tags.push((k.to_string(), v.to_string()));
                            }
                        }
                    }
                    Json::Obj(map) => {
                        for (key, value) in map {
                            if let Some(v) = value.as_str() {
                                tags.push((key.clone(), v.to_string()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    tags
}

const SENSOR_FIELDS: [&str; 7] = [
    "temperature_c",
    "temperature_f",
    "humidity",
    "pressure_pa",
    "pressure_hpa",
    "timestamp_ms",
    "sample_age_ms",
];

/// Does a `/sensors` body look like a real sensor payload? Used as the
/// post-discovery validation gate.
pub fn is_sensor_payload(body: &str) -> bool {
    json::parse(body).is_ok_and(|doc| {
        SENSOR_FIELDS
            .iter()
            .any(|f| doc.get(f).is_some())
            || doc.get("temperature").is_some()
            || doc.get("pressure").is_some()
    })
}

/// One reading → one line. Quantities are normalized to one field each at
/// the edge — Celsius, percent relative humidity, Pascals — whatever name
/// the firmware variant used. `now_ns` is the server clock at poll time;
/// the line timestamp is corrected by the device-reported sample age when
/// present, so time lives in the timestamp, not in extra fields.
pub fn build_environment_line(body: &str, device: &EnvDevice, now_ns: i64) -> Option<String> {
    let doc = json::parse(body).ok()?;
    let read = |key: &str| doc.get(key).and_then(Json::as_f64);
    let mut fields: Vec<(String, FieldValue)> = Vec::new();

    if let Some(v) = read("temperature_c").or_else(|| read("temperature")) {
        fields.push(("temperature_c".to_string(), FieldValue::Float(v)));
    }
    if let Some(v) = read("humidity") {
        fields.push(("humidity".to_string(), FieldValue::Float(v)));
    }
    if let Some(pa) = read("pressure_pa")
        .or_else(|| read("pressure"))
        .or_else(|| read("pressure_hpa").map(|hpa| hpa * 100.0))
    {
        fields.push(("pressure_pa".to_string(), FieldValue::Float(pa)));
    }
    if fields.is_empty() {
        return None;
    }

    let sample_age_ms = read("sample_age_ms");
    let timestamp_ns = match sample_age_ms {
        Some(age) => now_ns - (age * 1e6) as i64,
        None => now_ns,
    };
    if let Some(age) = sample_age_ms {
        fields.push(("sample_age_ms".to_string(), FieldValue::Float(age)));
    }

    let mut tags: Vec<(String, String)> = vec![
        ("device".to_string(), device.name.clone()),
        ("ip".to_string(), device.ip.to_string()),
    ];
    if let Some(model) = &device.model {
        tags.push(("model".to_string(), model.clone()));
    }
    tags.extend(device.tags.iter().cloned());

    lineproto::line("environment", &tags, &fields, timestamp_ns)
}

pub struct EnvSensorModule {
    pub cfg: PollerConfig,
    pub creds: Option<BasicCredentials>,
    pub bind_address: Ipv4Addr,
    pub home_broadcast: Ipv4Addr,
    pub spool: Arc<Spool>,
    pub metrics: Arc<Metrics>,
    pub registry: Arc<crate::registry::Registry>,
}

impl EnvSensorModule {
    pub fn spawn(self, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || self.run(&stop))
    }

    fn run(&self, stop: &AtomicBool) {
        let Some(creds) = self.creds.clone() else {
            eprintln!("event=env_sensors_idle reason=no_credentials");
            return;
        };
        let mut devices: Vec<EnvDevice> = Vec::new();
        let mut last_discovery: Option<Instant> = None;
        let discovery_interval =
            Duration::from_secs(self.cfg.discovery_interval_hours * 3600);
        let poll_interval = Duration::from_secs(self.cfg.poll_interval_seconds);

        while !stop.load(Ordering::Relaxed) {
            if last_discovery.is_none_or(|t| t.elapsed() >= discovery_interval) {
                devices = self.discover(&creds, stop);
                *self.registry.env.lock().unwrap() = devices.clone();
                metrics::set(&self.metrics.env_devices, devices.len() as u64);
                metrics::inc(&self.metrics.env_discovery_runs);
                last_discovery = Some(Instant::now());
                eprintln!("event=env_discovery devices={}", devices.len());
            }

            let started = Instant::now();
            let mut lines = Vec::new();
            for device in &devices {
                match self.poll(device, &creds) {
                    Some(line) => {
                        metrics::inc(&self.metrics.env_polls_ok);
                        lines.push(line);
                    }
                    None => metrics::inc(&self.metrics.env_polls_failed),
                }
            }
            if !lines.is_empty() {
                match self.spool.append(&lines) {
                    Ok(written) => {
                        for _ in 0..written {
                            metrics::inc(&self.metrics.spool_lines_written);
                        }
                    }
                    Err(e) => eprintln!("event=spool_error module=env error={e}"),
                }
            }
            let elapsed = started.elapsed();
            if elapsed < poll_interval {
                sleep_interruptible(poll_interval - elapsed, stop);
            }
        }
    }

    fn discover(&self, creds: &BasicCredentials, stop: &AtomicBool) -> Vec<EnvDevice> {
        let mut found: Vec<EnvDevice> = self
            .cfg
            .static_devices
            .iter()
            .map(|&ip| EnvDevice {
                ip,
                name: ip.to_string(),
                model: None,
                tags: Vec::new(),
            })
            .collect();

        let bind = SocketAddr::V4(SocketAddrV4::new(self.bind_address, self.cfg.discovery_bind_port));
        let socket = match UdpSocket::bind(bind) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("event=env_discovery_error stage=bind error={e}");
                return found;
            }
        };
        let _ = socket.set_broadcast(true);
        let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
        let target = SocketAddr::V4(SocketAddrV4::new(self.home_broadcast, self.cfg.discovery_port));
        if let Err(e) = socket.send_to(br#"{"discover": true}"#, target) {
            eprintln!("event=env_discovery_error stage=send error={e}");
            return found;
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
            let Ok((len, src)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let SocketAddr::V4(src) = src else { continue };
            if let Some(device) = parse_discovery_reply(&buf[..len], *src.ip()) {
                if !found.iter().any(|d| d.ip == device.ip) {
                    found.push(device);
                }
            }
        }

        // Validation gate: a discovered address must actually serve sensor
        // data before it is polled every second.
        found.retain(|device| match self.fetch_sensors(device, creds) {
            Some(body) => is_sensor_payload(&body),
            None => false,
        });
        found
    }

    fn fetch_sensors(&self, device: &EnvDevice, creds: &BasicCredentials) -> Option<String> {
        let addr = SocketAddr::V4(SocketAddrV4::new(device.ip, self.cfg.device_port));
        let auth = http::basic_auth_header(&creds.username, &creds.password);
        let response = http::request(
            addr,
            "GET",
            "/sensors",
            &[auth],
            b"",
            Duration::from_secs(3),
        )
        .ok()?;
        if response.status != 200 {
            return None;
        }
        String::from_utf8(response.body).ok()
    }

    fn poll(&self, device: &EnvDevice, creds: &BasicCredentials) -> Option<String> {
        let body = self.fetch_sensors(device, creds)?;
        let now_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_nanos() as i64;
        build_environment_line(&body, device, now_ns)
    }
}

pub fn sleep_interruptible(total: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(200);
    let mut remaining = total;
    while remaining > Duration::ZERO && !stop.load(Ordering::Relaxed) {
        let chunk = remaining.min(step);
        std::thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> EnvDevice {
        EnvDevice {
            ip: "192.168.65.42".parse().unwrap(),
            name: "Office Sensor".into(),
            model: Some("ENV3".into()),
            tags: vec![("room".into(), "office lab".into())],
        }
    }

    #[test]
    fn discovery_reply_parses() {
        let reply = br#"{"deviceId": "ATOM3U-ENV3-005", "model": "ENV3", "m5_tags": {"room": "office"}}"#;
        let parsed = parse_discovery_reply(reply, "192.168.65.42".parse().unwrap()).unwrap();
        assert_eq!(parsed.name, "ATOM3U-ENV3-005");
        assert_eq!(parsed.tags, vec![("room".to_string(), "office".to_string())]);
        assert!(parse_discovery_reply(b"not json", "192.168.65.42".parse().unwrap()).is_none());
        assert!(parse_discovery_reply(b"{}", "192.168.65.42".parse().unwrap()).is_none());
    }

    #[test]
    fn tag_shapes() {
        let list = json::parse(r#"["room=lab", "floor=2", "bad"]"#).unwrap();
        assert_eq!(
            parse_device_tags(Some(&list)),
            vec![
                ("room".to_string(), "lab".to_string()),
                ("floor".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn environment_line_shape() {
        let body = r#"{"temperature_c": 21.5, "humidity": 45.1, "pressure_pa": 101325.0,
                        "sample_age_ms": 50.0}"#;
        let now_ns: i64 = 1_782_788_400_000_000_000;
        let line = build_environment_line(body, &device(), now_ns).unwrap();
        assert_eq!(
            line,
            format!(
                "environment,device=Office\\ Sensor,ip=192.168.65.42,model=ENV3,room=office\\ lab \
                 temperature_c=21.5,humidity=45.1,pressure_pa=101325,sample_age_ms=50 {}",
                // Timestamp corrected 50 ms before the poll; no redundant
                // time fields on the line itself.
                now_ns - 50_000_000
            ),
        );
    }

    #[test]
    fn firmware_unit_variants_normalize() {
        // Bare names and hPa variants collapse to the canonical fields.
        let line = build_environment_line(
            r#"{"temperature": 20.0, "pressure_hpa": 1013.25}"#,
            &device(),
            9,
        )
        .unwrap();
        assert!(line.contains("temperature_c=20"), "{line}");
        assert!(line.contains("pressure_pa=101325"), "{line}");
        assert!(line.ends_with(" 9"), "{line}");
    }

    #[test]
    fn line_requires_a_reading() {
        assert!(build_environment_line(r#"{"uptime": 5}"#, &device(), 0).is_none());
        assert!(build_environment_line("not json", &device(), 0).is_none());
    }

    #[test]
    fn payload_gate() {
        assert!(is_sensor_payload(r#"{"temperature": 20.1}"#));
        assert!(is_sensor_payload(r#"{"pressure_hpa": 1013}"#));
        assert!(!is_sensor_payload(r#"{"status": "ok"}"#));
        assert!(!is_sensor_payload("<html>router admin page</html>"));
    }

}
