//! The appliance's single-port API. The house-sensors drain pulls reading
//! envelopes here (bearer token, generated on the host at first boot);
//! future push-capable sensor firmware lands envelopes on /ingest with the
//! same Basic credentials the devices already hold. /health stays
//! unauthenticated for the deploy gate.
//!
//! Routes:
//!   GET  /health          liveness + module states (no auth)
//!   GET  /metrics         Prometheus text (bearer)
//!   GET  /devices         discovered devices per module (bearer)
//!   GET  /readings/next?module=<name>   oldest closed segment of that
//!                         module's spool (bearer)
//!   POST /readings/ack    {"module": ..., "batchId": ...} deletes a
//!                         drained batch (bearer)
//!   POST /ingest          envelope JSON lines from devices, routed to
//!                         each envelope's module spool (Basic auth)

use crate::config::{BasicCredentials, Config};
use crate::crypto;
use crate::envelope;
use crate::http::{self, Request, Response};
use crate::json::Json;
use crate::metrics::{self, Metrics};
use crate::registry::Registry;
use crate::spool::SpoolSet;
use std::collections::BTreeMap;
use std::net::{SocketAddr, SocketAddrV4, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct Api {
    pub token: Vec<u8>,
    pub ingest_creds: Option<BasicCredentials>,
    pub spools: Arc<SpoolSet>,
    pub metrics: Arc<Metrics>,
    pub registry: Arc<Registry>,
    pub modules: ModuleFlags,
}

pub struct ModuleFlags {
    pub airwave_ssdp: bool,
    pub env_sensors: bool,
    pub kasa: bool,
}

impl Api {
    fn authorized_bearer(&self, request: &Request) -> bool {
        let Some(header) = request.headers.get("authorization") else {
            return false;
        };
        let Some(presented) = header.strip_prefix("Bearer ") else {
            return false;
        };
        crypto::eq_constant_time(presented.trim().as_bytes(), &self.token)
    }

    fn authorized_basic(&self, request: &Request) -> bool {
        let Some(creds) = &self.ingest_creds else {
            return false;
        };
        let Some(header) = request.headers.get("authorization") else {
            return false;
        };
        let Some(encoded) = header.strip_prefix("Basic ") else {
            return false;
        };
        let Ok(decoded) = crypto::base64_decode(encoded.trim()) else {
            return false;
        };
        let expected = format!("{}:{}", creds.username, creds.password);
        crypto::eq_constant_time(&decoded, expected.as_bytes())
    }

    pub fn handle(&self, request: &Request) -> Response {
        metrics::inc(&self.metrics.api_requests);
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/health") => self.health(),
            ("GET", "/metrics") => self.gated(request, |api| {
                Response::text(200, &api.metrics.render(&api.spools))
            }),
            ("GET", "/devices") => self.gated(request, Api::devices),
            ("GET", "/readings/next") => self.gated(request, |api| api.readings_next(request)),
            ("POST", "/readings/ack") => self.gated(request, |api| api.readings_ack(request)),
            ("POST", "/ingest") => self.ingest(request),
            ("GET" | "POST", _) => Response::empty(404),
            _ => Response::empty(405),
        }
    }

    fn gated(&self, request: &Request, handler: impl Fn(&Api) -> Response) -> Response {
        if self.authorized_bearer(request) {
            handler(self)
        } else {
            metrics::inc(&self.metrics.api_unauthorized);
            Response::empty(401)
        }
    }

    fn health(&self) -> Response {
        let stats = self.spools.stats();
        let mut modules = BTreeMap::new();
        modules.insert("airwaveSsdp".to_string(), Json::Bool(self.modules.airwave_ssdp));
        modules.insert("envSensors".to_string(), Json::Bool(self.modules.env_sensors));
        modules.insert("kasa".to_string(), Json::Bool(self.modules.kasa));
        let mut body = BTreeMap::new();
        body.insert("status".to_string(), Json::Str("ok".to_string()));
        body.insert("modules".to_string(), Json::Obj(modules));
        body.insert(
            "spoolBytes".to_string(),
            Json::Int(stats.total_bytes as i64),
        );
        Response::json(200, Json::Obj(body).to_string())
    }

    fn devices(&self) -> Response {
        let env: Vec<Json> = self
            .registry
            .env
            .lock()
            .unwrap()
            .iter()
            .map(|d| {
                let mut map = BTreeMap::new();
                map.insert("ip".to_string(), Json::Str(d.ip.to_string()));
                map.insert("name".to_string(), Json::Str(d.name.clone()));
                if let Some(model) = &d.model {
                    map.insert("model".to_string(), Json::Str(model.clone()));
                }
                Json::Obj(map)
            })
            .collect();
        let kasa: Vec<Json> = self
            .registry
            .kasa
            .lock()
            .unwrap()
            .iter()
            .map(|d| {
                let mut map = BTreeMap::new();
                map.insert("ip".to_string(), Json::Str(d.ip.to_string()));
                if let Some(name) = &d.name {
                    map.insert("name".to_string(), Json::Str(name.clone()));
                }
                if let Some(model) = &d.model {
                    map.insert("model".to_string(), Json::Str(model.clone()));
                }
                Json::Obj(map)
            })
            .collect();
        let mut body = BTreeMap::new();
        body.insert("envSensors".to_string(), Json::Arr(env));
        body.insert("kasa".to_string(), Json::Arr(kasa));
        Response::json(200, Json::Obj(body).to_string())
    }

    fn readings_next(&self, request: &Request) -> Response {
        let Some(module) = request.query.get("module") else {
            return Response::empty(400);
        };
        if !envelope::valid_module(module) {
            return Response::empty(400);
        }
        // A module nobody has produced for yet is an empty stream, not an
        // error — the consumer may deploy before its producer.
        let Some(spool) = self.spools.get(module) else {
            return Response::empty(204);
        };
        match spool.next_batch() {
            Ok(Some((batch_id, lines))) => {
                metrics::inc(&self.metrics.batches_served);
                let mut body = BTreeMap::new();
                body.insert("batchId".to_string(), Json::Str(batch_id));
                body.insert("module".to_string(), Json::Str(module.clone()));
                body.insert("lines".to_string(), Json::Str(lines));
                Response::json(200, Json::Obj(body).to_string())
            }
            Ok(None) => Response::empty(204),
            Err(e) => {
                eprintln!("event=spool_error op=next_batch module={module} error={e}");
                Response::empty(500)
            }
        }
    }

    fn readings_ack(&self, request: &Request) -> Response {
        let Ok(text) = std::str::from_utf8(&request.body) else {
            return Response::empty(400);
        };
        let Ok(doc) = crate::json::parse(text) else {
            return Response::empty(400);
        };
        let Some(module) = doc.get("module").and_then(Json::as_str) else {
            return Response::empty(400);
        };
        let Some(batch_id) = doc.get("batchId").and_then(Json::as_str) else {
            return Response::empty(400);
        };
        let Some(spool) = self.spools.get(module) else {
            let mut body = BTreeMap::new();
            body.insert("acked".to_string(), Json::Bool(false));
            return Response::json(200, Json::Obj(body).to_string());
        };
        match spool.ack(batch_id) {
            Ok(acked) => {
                if acked {
                    metrics::inc(&self.metrics.batches_acked);
                }
                let mut body = BTreeMap::new();
                body.insert("acked".to_string(), Json::Bool(acked));
                Response::json(200, Json::Obj(body).to_string())
            }
            Err(e) => {
                eprintln!("event=spool_error op=ack module={module} error={e}");
                Response::empty(500)
            }
        }
    }

    fn ingest(&self, request: &Request) -> Response {
        if !self.authorized_basic(request) {
            metrics::inc(&self.metrics.api_unauthorized);
            return Response::empty(401);
        }
        let Ok(text) = std::str::from_utf8(&request.body) else {
            return Response::empty(400);
        };
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        let mut by_module: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut accepted = 0i64;
        let mut rejected = 0i64;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match envelope::normalize_pushed(line, now_ns) {
                Some((module, normalized)) => {
                    by_module.entry(module).or_default().push(normalized);
                    accepted += 1;
                }
                None => rejected += 1,
            }
        }
        for _ in 0..accepted {
            metrics::inc(&self.metrics.ingest_lines_accepted);
        }
        for _ in 0..rejected {
            metrics::inc(&self.metrics.ingest_lines_rejected);
        }
        for (module, lines) in &by_module {
            let append = self
                .spools
                .for_module(module)
                .and_then(|spool| spool.append(lines));
            if let Err(e) = append {
                eprintln!("event=spool_error op=ingest module={module} error={e}");
                return Response::empty(500);
            }
        }
        let mut body = BTreeMap::new();
        body.insert("accepted".to_string(), Json::Int(accepted));
        body.insert("rejected".to_string(), Json::Int(rejected));
        Response::json(200, Json::Obj(body).to_string())
    }
}

/// Accept loop; one thread per connection. LAN-scale service — the firewall
/// already restricts who can reach it.
pub fn run(api: Arc<Api>, config: &Config, stop: Arc<AtomicBool>) -> std::io::Result<()> {
    let addr = SocketAddr::V4(SocketAddrV4::new(config.bind_address, config.api_port));
    let listener = TcpListener::bind(addr)?;
    listener.set_nonblocking(true)?;
    eprintln!("event=api_listening address={addr}");
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let api = Arc::clone(&api);
                std::thread::spawn(move || {
                    let _ = stream.set_nodelay(true);
                    match http::read_request(&mut stream) {
                        Ok(request) => {
                            let response = api.handle(&request);
                            http::write_response(&mut stream, &response);
                        }
                        Err(_) => {
                            http::write_response(&mut stream, &Response::empty(400));
                        }
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                eprintln!("event=api_accept_error error={e}");
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_with_spool(dir: &str) -> Api {
        let path = std::env::temp_dir().join(format!("ahara-api-test-{dir}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Api {
            token: b"testtoken".to_vec(),
            ingest_creds: Some(BasicCredentials {
                username: "admin".into(),
                password: "pw".into(),
            }),
            spools: Arc::new(SpoolSet::open(&path, 1024, 65536).unwrap()),
            metrics: Arc::new(Metrics::default()),
            registry: Arc::new(Registry::default()),
            modules: ModuleFlags {
                airwave_ssdp: true,
                env_sensors: true,
                kasa: false,
            },
        }
    }

    fn request(method: &str, target: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
        let (path, query_text) = target.split_once('?').unwrap_or((target, ""));
        let query = query_text
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Request {
            method: method.to_string(),
            path: path.to_string(),
            query,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: body.to_vec(),
        }
    }

    #[test]
    fn health_is_open_everything_else_gated() {
        let api = api_with_spool("gate");
        assert_eq!(api.handle(&request("GET", "/health", &[], b"")).status, 200);
        assert_eq!(api.handle(&request("GET", "/metrics", &[], b"")).status, 401);
        assert_eq!(
            api.handle(&request("GET", "/metrics", &[("authorization", "Bearer wrong")], b""))
                .status,
            401
        );
        assert_eq!(
            api.handle(&request(
                "GET",
                "/metrics",
                &[("authorization", "Bearer testtoken")],
                b""
            ))
            .status,
            200
        );
        assert_eq!(api.handle(&request("GET", "/nope", &[], b"")).status, 404);
    }

    #[test]
    fn drain_and_ack_cycle() {
        let api = api_with_spool("drain");
        let auth = [("authorization", "Bearer testtoken")];
        // The module parameter is required, and its charset is validated.
        assert_eq!(api.handle(&request("GET", "/readings/next", &auth, b"")).status, 400);
        assert_eq!(
            api.handle(&request("GET", "/readings/next?module=a/b", &auth, b"")).status,
            400
        );
        // A module with no readings yet: empty stream.
        assert_eq!(
            api.handle(&request("GET", "/readings/next?module=m", &auth, b"")).status,
            204
        );

        let envelope_line = r#"{"module":"m","timestampNs":1,"values":{"v":1}}"#;
        let other_line = r#"{"module":"other","timestampNs":2,"values":{"w":2}}"#;
        api.spools.for_module("m").unwrap().append(&[envelope_line.to_string()]).unwrap();
        api.spools.for_module("other").unwrap().append(&[other_line.to_string()]).unwrap();
        let response = api.handle(&request("GET", "/readings/next?module=m", &auth, b""));
        assert_eq!(response.status, 200);
        let body = String::from_utf8(response.body).unwrap();
        let doc = crate::json::parse(&body).unwrap();
        let batch_id = doc.get("batchId").unwrap().as_str().unwrap().to_string();
        assert_eq!(doc.get("module").unwrap().as_str(), Some("m"));
        let lines = doc.get("lines").unwrap().as_str().unwrap();
        assert!(lines.contains(envelope_line));
        // The other module's readings never leak into this stream.
        assert!(!lines.contains("other"));

        let ack_body = format!(r#"{{"module": "m", "batchId": "{batch_id}"}}"#);
        let response = api.handle(&request("POST", "/readings/ack", &auth, ack_body.as_bytes()));
        assert_eq!(response.status, 200);
        assert!(String::from_utf8(response.body).unwrap().contains("true"));
        assert_eq!(
            api.handle(&request("GET", "/readings/next?module=m", &auth, b"")).status,
            204
        );
        // The other module's batch is still there.
        assert_eq!(
            api.handle(&request("GET", "/readings/next?module=other", &auth, b"")).status,
            200
        );

        // An ack without a module is malformed; garbage ids delete nothing.
        let response = api.handle(&request(
            "POST",
            "/readings/ack",
            &auth,
            format!(r#"{{"batchId": "{batch_id}"}}"#).as_bytes(),
        ));
        assert_eq!(response.status, 400);
        let response = api.handle(&request(
            "POST",
            "/readings/ack",
            &auth,
            br#"{"module": "m", "batchId": "../etc/passwd"}"#,
        ));
        assert!(String::from_utf8(response.body).unwrap().contains("false"));
    }

    #[test]
    fn ingest_validates_and_authenticates() {
        let api = api_with_spool("ingest");
        let envelope_body = br#"{"module": "push", "values": {"v": 42}}"#;
        // No auth → 401. Bearer is not accepted here — devices hold Basic.
        assert_eq!(api.handle(&request("POST", "/ingest", &[], envelope_body)).status, 401);
        let basic = [("authorization", "Basic YWRtaW46cHc=")]; // admin:pw
        let body = b"{\"module\": \"push\", \"values\": {\"v\": 42}}\nnot an envelope\n\n{\"module\": \"push\", \"timestampNs\": 7, \"values\": {\"x\": 1}}\n";
        let response = api.handle(&request("POST", "/ingest", &basic, body));
        assert_eq!(response.status, 200);
        let text = String::from_utf8(response.body).unwrap();
        assert!(text.contains("\"accepted\":2"), "{text}");
        assert!(text.contains("\"rejected\":1"), "{text}");

        // Pushed envelopes land in their declared module's stream,
        // normalized so every one carries a timestamp.
        let auth = [("authorization", "Bearer testtoken")];
        let drained = api.handle(&request("GET", "/readings/next?module=push", &auth, b""));
        assert_eq!(drained.status, 200);
        let drained_body = String::from_utf8(drained.body).unwrap();
        let doc = crate::json::parse(&drained_body).unwrap();
        let lines = doc.get("lines").unwrap().as_str().unwrap();
        assert!(
            lines.lines().all(|l| l.contains("\"timestampNs\":")),
            "{lines}"
        );
    }
}
