//! Kasa smart-plug module (KP125M energy monitoring). These devices speak
//! TP-Link's KLAP v2 transport: a seed handshake authenticated by the Kasa
//! cloud account credentials, then AES-128-CBC payloads over plain HTTP.
//! The protocol here follows python-kasa's KlapTransportV2, which the
//! TrueNAS voltage collector uses today.
//!
//! EXPERIMENTAL until validated against real hardware: the handshake and
//! key derivation are implemented from the python-kasa semantics and
//! self-tested for internal consistency, but no KP125M has confirmed them
//! end-to-end from this codebase yet. The module fails per-device and
//! per-poll, never taking the service down.

use crate::config::{BasicCredentials, PollerConfig};
use crate::crypto;
use crate::envelope;
use crate::http;
use crate::json::{self, Json};
use crate::metrics::{self, Metrics};
use crate::sensors::sleep_interruptible;
use crate::spool::Spool;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// The fixed 16-byte probe python-kasa broadcasts to UDP 20002 for
/// new-protocol devices.
pub const DISCOVERY_PROBE: [u8; 16] = [
    0x02, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46, 0x3c, 0xb5,
    0xd3,
];

#[derive(Debug, Clone, PartialEq)]
pub struct KasaDevice {
    pub ip: Ipv4Addr,
    pub http_port: u16,
    pub device_id: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
}

/// Parse a 20002 discovery response: 16-byte header, then JSON. Only KLAP
/// devices are kept — the older XOR "IOT" protocol plugs are not supported
/// here.
pub fn parse_discovery_response(payload: &[u8], src: Ipv4Addr) -> Option<KasaDevice> {
    let body = payload.get(16..)?;
    let text = std::str::from_utf8(body).ok()?;
    let doc = json::parse(text).ok()?;
    let result = doc.get("result")?;
    let schm = result.get("mgt_encrypt_schm")?;
    if schm.get("encrypt_type").and_then(Json::as_str) != Some("KLAP") {
        return None;
    }
    let http_port = schm
        .get("http_port")
        .and_then(Json::as_i64)
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(80);
    Some(KasaDevice {
        ip: src,
        http_port,
        device_id: result
            .get("device_id")
            .and_then(Json::as_str)
            .map(str::to_string),
        model: result
            .get("device_model")
            .and_then(Json::as_str)
            .map(str::to_string),
        name: None,
    })
}

/// KLAP v2 auth hash: sha256(sha1(username) + sha1(password)).
pub fn auth_hash(creds: &BasicCredentials) -> [u8; 32] {
    let mut seed = Vec::with_capacity(40);
    seed.extend_from_slice(&crypto::sha1(creds.username.as_bytes()));
    seed.extend_from_slice(&crypto::sha1(creds.password.as_bytes()));
    crypto::sha256(&seed)
}

pub struct KlapKeys {
    pub key: [u8; 16],
    pub iv: [u8; 12],
    pub sig: [u8; 28],
    pub seq: i32,
}

/// Session keys, derived exactly as python-kasa's KlapEncryptionSession.
pub fn derive_keys(local_seed: &[u8], remote_seed: &[u8], auth: &[u8; 32]) -> KlapKeys {
    let material = |label: &[u8]| {
        let mut buf = Vec::with_capacity(label.len() + local_seed.len() + remote_seed.len() + 32);
        buf.extend_from_slice(label);
        buf.extend_from_slice(local_seed);
        buf.extend_from_slice(remote_seed);
        buf.extend_from_slice(auth);
        crypto::sha256(&buf)
    };
    let key_full = material(b"lsk");
    let iv_full = material(b"iv");
    let sig_full = material(b"ldk");
    KlapKeys {
        key: key_full[..16].try_into().unwrap(),
        iv: iv_full[..12].try_into().unwrap(),
        sig: sig_full[..28].try_into().unwrap(),
        seq: i32::from_be_bytes(iv_full[28..32].try_into().unwrap()),
    }
}

pub fn encrypt_payload(keys: &KlapKeys, seq: i32, plaintext: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; 16];
    iv[..12].copy_from_slice(&keys.iv);
    iv[12..].copy_from_slice(&seq.to_be_bytes());
    let cipher = crypto::aes128_cbc_encrypt(&keys.key, &iv, plaintext);
    let mut signed = Vec::with_capacity(28 + 4 + cipher.len());
    signed.extend_from_slice(&keys.sig);
    signed.extend_from_slice(&seq.to_be_bytes());
    signed.extend_from_slice(&cipher);
    let signature = crypto::sha256(&signed);
    let mut out = Vec::with_capacity(32 + cipher.len());
    out.extend_from_slice(&signature);
    out.extend_from_slice(&cipher);
    out
}

pub fn decrypt_payload(keys: &KlapKeys, seq: i32, payload: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = payload
        .get(32..)
        .ok_or_else(|| "response shorter than its signature".to_string())?;
    let mut iv = [0u8; 16];
    iv[..12].copy_from_slice(&keys.iv);
    iv[12..].copy_from_slice(&seq.to_be_bytes());
    crypto::aes128_cbc_decrypt(&keys.key, &iv, cipher)
}

fn random_seed() -> [u8; 16] {
    use std::io::Read;
    let mut seed = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut seed))
        .is_ok()
    {
        return seed;
    }
    // Fallback entropy: clock and pid through sha256. Never expected on the
    // appliance.
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let digest = crypto::sha256(format!("{now}-{}", std::process::id()).as_bytes());
    seed.copy_from_slice(&digest[..16]);
    seed
}

pub struct KlapSession {
    addr: SocketAddr,
    cookie: String,
    keys: KlapKeys,
}

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

impl KlapSession {
    pub fn establish(addr: SocketAddr, creds: &BasicCredentials) -> Result<KlapSession, String> {
        let auth = auth_hash(creds);
        let local_seed = random_seed();

        let h1 = http::request(addr, "POST", "/app/handshake1", &[], &local_seed, HTTP_TIMEOUT)?;
        if h1.status != 200 {
            return Err(format!("handshake1 returned {}", h1.status));
        }
        if h1.body.len() < 48 {
            return Err(format!("handshake1 body too short: {}", h1.body.len()));
        }
        let remote_seed: [u8; 16] = h1.body[..16].try_into().unwrap();
        let server_hash = &h1.body[16..48];

        let mut expected = Vec::with_capacity(64);
        expected.extend_from_slice(&local_seed);
        expected.extend_from_slice(&remote_seed);
        expected.extend_from_slice(&auth);
        if !crypto::eq_constant_time(&crypto::sha256(&expected), server_hash) {
            return Err("handshake1 hash mismatch (wrong credentials or KLAP v1 device)".into());
        }

        let cookie = h1
            .header_all("set-cookie")
            .iter()
            .find_map(|c| {
                c.split(';')
                    .map(str::trim)
                    .find(|part| part.starts_with("TP_SESSIONID="))
            })
            .map(str::to_string)
            .ok_or_else(|| "handshake1 set no TP_SESSIONID cookie".to_string())?;

        let mut h2_body = Vec::with_capacity(64);
        h2_body.extend_from_slice(&remote_seed);
        h2_body.extend_from_slice(&local_seed);
        h2_body.extend_from_slice(&auth);
        let h2 = http::request(
            addr,
            "POST",
            "/app/handshake2",
            &[("cookie".to_string(), cookie.clone())],
            &crypto::sha256(&h2_body),
            HTTP_TIMEOUT,
        )?;
        if h2.status != 200 {
            return Err(format!("handshake2 returned {}", h2.status));
        }

        Ok(KlapSession {
            addr,
            cookie,
            keys: derive_keys(&local_seed, &remote_seed, &auth),
        })
    }

    pub fn execute(&mut self, request_body: &str) -> Result<Json, String> {
        self.keys.seq = self.keys.seq.wrapping_add(1);
        let seq = self.keys.seq;
        let payload = encrypt_payload(&self.keys, seq, request_body.as_bytes());
        let response = http::request(
            self.addr,
            "POST",
            &format!("/app/request?seq={seq}"),
            &[("cookie".to_string(), self.cookie.clone())],
            &payload,
            HTTP_TIMEOUT,
        )?;
        if response.status != 200 {
            return Err(format!("request returned {}", response.status));
        }
        let plain = decrypt_payload(&self.keys, seq, &response.body)?;
        let text = String::from_utf8(plain).map_err(|_| "response not UTF-8".to_string())?;
        let doc = json::parse(&text)?;
        match doc.get("error_code").and_then(Json::as_i64) {
            Some(0) | None => Ok(doc),
            Some(code) => Err(format!("device error_code {code}")),
        }
    }
}

/// Keys whose presence marks an energy payload as carrying a reading, in
/// the vendor's vocabulary and units (mW, Wh, mV, mA).
const ENERGY_KEYS: [&str; 4] = ["current_power", "today_energy", "voltage_mv", "current_ma"];

/// Build the reading envelope from get_energy_usage: the result object
/// verbatim, vendor keys and units untouched. house-sensors owns the
/// storage names and conversions (ADR-0006).
pub fn build_power_reading(
    energy: &Json,
    device: &KasaDevice,
    now_ns: i64,
) -> Option<String> {
    let result = energy.get("result").unwrap_or(energy);
    if !ENERGY_KEYS
        .iter()
        .any(|k| result.get(k).and_then(Json::as_f64).is_some())
    {
        return None;
    }
    let identity = envelope::device(
        &device.ip.to_string(),
        device.name.as_deref(),
        device.model.as_deref(),
        device.device_id.as_deref(),
        &[],
    );
    envelope::reading(envelope::MODULE_KASA, identity, now_ns, result.clone())
}

/// Kasa nicknames arrive base64-encoded.
pub fn decode_nickname(info: &Json) -> Option<String> {
    let result = info.get("result").unwrap_or(info);
    let encoded = result.get("nickname").and_then(Json::as_str)?;
    let decoded = crypto::base64_decode(encoded).ok()?;
    String::from_utf8(decoded).ok().filter(|s| !s.is_empty())
}

pub struct KasaModule {
    pub cfg: PollerConfig,
    pub creds: Option<BasicCredentials>,
    pub bind_address: Ipv4Addr,
    pub home_broadcast: Ipv4Addr,
    pub spool: Arc<Spool>,
    pub metrics: Arc<Metrics>,
    pub registry: Arc<crate::registry::Registry>,
}

impl KasaModule {
    pub fn spawn(self, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || self.run(&stop))
    }

    fn run(&self, stop: &AtomicBool) {
        let Some(creds) = self.creds.clone() else {
            eprintln!("event=kasa_idle reason=no_credentials");
            return;
        };
        let mut devices: Vec<KasaDevice> = Vec::new();
        let mut sessions: Vec<Option<KlapSession>> = Vec::new();
        let mut last_discovery: Option<Instant> = None;
        let discovery_interval = Duration::from_secs(self.cfg.discovery_interval_hours * 3600);
        let poll_interval = Duration::from_secs(self.cfg.poll_interval_seconds);

        while !stop.load(Ordering::Relaxed) {
            if last_discovery.is_none_or(|t| t.elapsed() >= discovery_interval) {
                devices = self.discover(stop);
                sessions = devices.iter().map(|_| None).collect();
                *self.registry.kasa.lock().unwrap() = devices.clone();
                metrics::set(&self.metrics.kasa_devices, devices.len() as u64);
                metrics::inc(&self.metrics.kasa_discovery_runs);
                last_discovery = Some(Instant::now());
                eprintln!("event=kasa_discovery devices={}", devices.len());
            }

            let started = Instant::now();
            let mut lines = Vec::new();
            for (device, session_slot) in devices.iter_mut().zip(sessions.iter_mut()) {
                match Self::poll(device, session_slot, &creds) {
                    Ok(line) => {
                        metrics::inc(&self.metrics.kasa_polls_ok);
                        lines.push(line);
                    }
                    Err(e) => {
                        metrics::inc(&self.metrics.kasa_polls_failed);
                        // A dead session re-handshakes next tick.
                        *session_slot = None;
                        eprintln!("event=kasa_poll_failed device={} error={e}", device.ip);
                    }
                }
            }
            if !lines.is_empty() {
                match self.spool.append(&lines) {
                    Ok(written) => {
                        for _ in 0..written {
                            metrics::inc(&self.metrics.spool_lines_written);
                        }
                    }
                    Err(e) => eprintln!("event=spool_error module=kasa error={e}"),
                }
            }
            let elapsed = started.elapsed();
            if elapsed < poll_interval {
                sleep_interruptible(poll_interval - elapsed, stop);
            }
        }
    }

    fn discover(&self, stop: &AtomicBool) -> Vec<KasaDevice> {
        let mut found: Vec<KasaDevice> = self
            .cfg
            .static_devices
            .iter()
            .map(|&ip| KasaDevice {
                ip,
                http_port: self.cfg.device_port,
                device_id: None,
                model: None,
                name: None,
            })
            .collect();

        let bind = SocketAddr::V4(SocketAddrV4::new(self.bind_address, self.cfg.discovery_bind_port));
        let socket = match UdpSocket::bind(bind) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("event=kasa_discovery_error stage=bind error={e}");
                return found;
            }
        };
        let _ = socket.set_broadcast(true);
        let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
        let target = SocketAddr::V4(SocketAddrV4::new(self.home_broadcast, self.cfg.discovery_port));
        if let Err(e) = socket.send_to(&DISCOVERY_PROBE, target) {
            eprintln!("event=kasa_discovery_error stage=send error={e}");
            return found;
        }

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
            let Ok((len, src)) = socket.recv_from(&mut buf) else {
                continue;
            };
            let SocketAddr::V4(src) = src else { continue };
            if let Some(device) = parse_discovery_response(&buf[..len], *src.ip()) {
                if !found.iter().any(|d| d.ip == device.ip) {
                    found.push(device);
                }
            }
        }
        found
    }

    fn poll(
        device: &mut KasaDevice,
        session_slot: &mut Option<KlapSession>,
        creds: &BasicCredentials,
    ) -> Result<String, String> {
        let addr = SocketAddr::V4(SocketAddrV4::new(device.ip, device.http_port));
        if session_slot.is_none() {
            let mut session = KlapSession::establish(addr, creds)?;
            // First contact: learn the device's display name for the tag.
            if device.name.is_none() {
                if let Ok(info) = session.execute(r#"{"method": "get_device_info"}"#) {
                    device.name = decode_nickname(&info);
                }
            }
            *session_slot = Some(session);
        }
        let session = session_slot.as_mut().unwrap();
        let energy = session.execute(r#"{"method": "get_energy_usage"}"#)?;
        let now_ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0);
        build_power_reading(&energy, device, now_ns)
            .ok_or_else(|| "energy payload had no usable fields".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_response_parses_and_filters() {
        let json_body = r#"{"error_code":0,"result":{"device_id":"8012ABC","device_model":"KP125M(US)","device_type":"SMART.KASAPLUG","ip":"192.168.65.40","mgt_encrypt_schm":{"encrypt_type":"KLAP","http_port":80,"lv":2}}}"#;
        let mut payload = vec![0u8; 16];
        payload.extend_from_slice(json_body.as_bytes());
        let device = parse_discovery_response(&payload, "192.168.65.40".parse().unwrap()).unwrap();
        assert_eq!(device.http_port, 80);
        assert_eq!(device.model.as_deref(), Some("KP125M(US)"));

        let aes_body = json_body.replace("KLAP", "AES");
        let mut aes_payload = vec![0u8; 16];
        aes_payload.extend_from_slice(aes_body.as_bytes());
        assert!(parse_discovery_response(&aes_payload, "192.168.65.40".parse().unwrap()).is_none());
        assert!(parse_discovery_response(b"short", "192.168.65.40".parse().unwrap()).is_none());
    }

    #[test]
    fn klap_payloads_round_trip() {
        let creds = BasicCredentials {
            username: "user@example.com".into(),
            password: "hunter2".into(),
        };
        let auth = auth_hash(&creds);
        let local = [1u8; 16];
        let remote = [2u8; 16];
        let keys = derive_keys(&local, &remote, &auth);
        let seq = keys.seq.wrapping_add(1);
        let message = br#"{"method": "get_energy_usage"}"#;
        let payload = encrypt_payload(&keys, seq, message);
        // 32-byte signature + at least one cipher block.
        assert!(payload.len() >= 48);
        let decrypted = decrypt_payload(&keys, seq, &payload).unwrap();
        assert_eq!(decrypted, message);
        // The wrong sequence number must not decrypt.
        assert_ne!(
            decrypt_payload(&keys, seq.wrapping_add(1), &payload).ok(),
            Some(message.to_vec())
        );
    }

    #[test]
    fn key_derivation_is_deterministic_and_label_separated() {
        let auth = [3u8; 32];
        let keys_a = derive_keys(&[1; 16], &[2; 16], &auth);
        let keys_b = derive_keys(&[1; 16], &[2; 16], &auth);
        assert_eq!(keys_a.key, keys_b.key);
        assert_eq!(keys_a.seq, keys_b.seq);
        // Different labels must yield unrelated material.
        assert_ne!(keys_a.key.to_vec(), keys_a.sig[..16].to_vec());
    }

    #[test]
    fn power_reading_from_energy_usage() {
        let device = KasaDevice {
            ip: "192.168.65.40".parse().unwrap(),
            http_port: 80,
            device_id: Some("8012ABC".into()),
            model: Some("KP125M(US)".into()),
            name: Some("Dryer".into()),
        };
        let energy = json::parse(
            r#"{"error_code":0,"result":{"current_power":1500,"today_energy":2500,"today_runtime":300}}"#,
        )
        .unwrap();
        let reading = build_power_reading(&energy, &device, 42).unwrap();
        assert_eq!(
            reading,
            concat!(
                r#"{"device":{"deviceId":"8012ABC","ip":"192.168.65.40","model":"KP125M(US)","name":"Dryer"},"#,
                r#""module":"kasa","timestampNs":42,"#,
                r#""values":{"current_power":1500,"today_energy":2500,"today_runtime":300}}"#
            )
        );
        let empty = json::parse(r#"{"error_code":0,"result":{"today_runtime":300}}"#).unwrap();
        assert!(build_power_reading(&empty, &device, 42).is_none());
    }

    #[test]
    fn nickname_decodes_from_base64() {
        let info = json::parse(r#"{"result":{"nickname":"RHJ5ZXI="}}"#).unwrap();
        assert_eq!(decode_nickname(&info).as_deref(), Some("Dryer"));
        let bad = json::parse(r#"{"result":{"nickname":"!!!"}}"#).unwrap();
        assert!(decode_nickname(&bad).is_none());
    }
}
