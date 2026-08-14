//! WiiM-facing SSDP for Airwave's UPnP MediaServer. Airwave registers a
//! short-lived description lease through the authenticated collector API;
//! this module answers MediaServer searches and emits alive announcements on
//! the IoT LAN. No packet crosses a VLAN and no arbitrary SSDP payload is
//! forwarded.

use crate::config::Ipv4Cidr;
use crate::metrics::{self, Metrics};
use crate::registry::Registry;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SSDP_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const MEDIA_RENDERER_URN: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
pub const MAX_DATAGRAM: usize = 2048;
const MEDIA_SERVER_CACHE_SECONDS: u64 = 1800;
const MEDIA_SERVER_TARGETS: [&str; 5] = [
    "ssdp:all",
    "upnp:rootdevice",
    "urn:schemas-upnp-org:device:MediaServer:1",
    "urn:schemas-upnp-org:service:ContentDirectory:1",
    "urn:schemas-upnp-org:service:ConnectionManager:1",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaServerLease {
    pub uuid: String,
    pub location: String,
    pub server: String,
    pub expires_at_seconds: u64,
}

impl MediaServerLease {
    #[must_use]
    pub fn active(&self, now_seconds: u64) -> bool {
        self.expires_at_seconds > now_seconds
    }

    fn nts(&self) -> Vec<(String, String)> {
        let udn = format!("uuid:{}", self.uuid);
        vec![
            (
                "upnp:rootdevice".into(),
                format!("{udn}::upnp:rootdevice"),
            ),
            (udn.clone(), udn.clone()),
            (
                "urn:schemas-upnp-org:device:MediaServer:1".into(),
                format!("{udn}::urn:schemas-upnp-org:device:MediaServer:1"),
            ),
            (
                "urn:schemas-upnp-org:service:ContentDirectory:1".into(),
                format!("{udn}::urn:schemas-upnp-org:service:ContentDirectory:1"),
            ),
            (
                "urn:schemas-upnp-org:service:ConnectionManager:1".into(),
                format!("{udn}::urn:schemas-upnp-org:service:ConnectionManager:1"),
            ),
        ]
    }

    fn cache_seconds(&self, now_seconds: u64) -> u64 {
        self.expires_at_seconds
            .saturating_sub(now_seconds)
            .clamp(1, MEDIA_SERVER_CACHE_SECONDS)
    }

    fn responses(&self, payload: &[u8], now_seconds: u64) -> Vec<Vec<u8>> {
        if !self.active(now_seconds) {
            return Vec::new();
        }
        let Some(message) = SsdpMessage::parse(payload) else {
            return Vec::new();
        };
        let Some(st) = message.header("st") else {
            return Vec::new();
        };
        self.nts()
            .into_iter()
            .filter(|(nt, _)| st == "ssdp:all" || st == nt)
            .map(|(nt, usn)| self.search_response(&nt, &usn, now_seconds).into_bytes())
            .collect()
    }

    fn alive_messages(&self, now_seconds: u64) -> Vec<Vec<u8>> {
        if !self.active(now_seconds) {
            return Vec::new();
        }
        self.nts()
            .into_iter()
            .map(|(nt, usn)| {
                format!(
                    "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nCACHE-CONTROL: max-age={}\r\nLOCATION: {}\r\nNT: {nt}\r\nNTS: ssdp:alive\r\nSERVER: {}\r\nUSN: {usn}\r\n\r\n",
                    self.cache_seconds(now_seconds),
                    self.location,
                    self.server,
                )
                .into_bytes()
            })
            .collect()
    }

    fn search_response(&self, st: &str, usn: &str, now_seconds: u64) -> String {
        let date = httpdate::fmt_http_date(SystemTime::now());
        format!(
            "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age={}\r\nDATE: {date}\r\nEXT:\r\nLOCATION: {}\r\nSERVER: {}\r\nST: {st}\r\nUSN: {usn}\r\n\r\n",
            self.cache_seconds(now_seconds),
            self.location,
            self.server,
        )
    }
}

pub struct SsdpMessage {
    pub start_line: String,
    headers: Vec<(String, String)>,
}

impl SsdpMessage {
    pub fn parse(payload: &[u8]) -> Option<SsdpMessage> {
        let text = std::str::from_utf8(payload).ok()?;
        let mut lines = text.split("\r\n");
        let start_line = lines.next()?.trim().to_string();
        if start_line.is_empty() {
            return None;
        }
        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                headers.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
            }
        }
        Some(SsdpMessage {
            start_line,
            headers,
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn is_media_server_search(payload: &[u8]) -> bool {
    let Some(message) = SsdpMessage::parse(payload) else {
        return false;
    };
    message.start_line == "M-SEARCH * HTTP/1.1"
        && message.header("man") == Some("\"ssdp:discover\"")
        && message.header("st").is_some_and(|st| {
            MEDIA_SERVER_TARGETS.contains(&st) || st.starts_with("uuid:")
        })
}

#[derive(Debug, PartialEq)]
pub struct Outgoing {
    pub dest: SocketAddr,
    pub payload: Vec<u8>,
}

pub struct Responder {
    pub ssdp_port: u16,
    pub home_cidr: Ipv4Cidr,
    pub home_broadcast: Ipv4Addr,
    pub bind_address: Ipv4Addr,
    pub metrics: Arc<Metrics>,
    pub registry: Arc<Registry>,
}

impl Responder {
    pub fn process(
        &self,
        src: SocketAddr,
        payload: &[u8],
        now_seconds: u64,
    ) -> Vec<Outgoing> {
        let SocketAddr::V4(src_v4) = src else {
            metrics::inc(&self.metrics.wiim_media_dropped);
            return Vec::new();
        };
        if payload.len() > MAX_DATAGRAM {
            metrics::inc(&self.metrics.wiim_media_dropped);
            return Vec::new();
        }
        let src_ip = *src_v4.ip();
        if src_ip == self.bind_address {
            return Vec::new();
        }
        if !self.home_cidr.contains(src_ip) || !is_media_server_search(payload) {
            metrics::inc(&self.metrics.wiim_media_dropped);
            return Vec::new();
        }

        metrics::inc(&self.metrics.wiim_media_searches);
        let replies = self
            .registry
            .media_server
            .lock()
            .unwrap()
            .as_ref()
            .map_or_else(Vec::new, |lease| lease.responses(payload, now_seconds));
        if !replies.is_empty() {
            metrics::inc(&self.metrics.wiim_media_replies);
        }
        replies
            .into_iter()
            .map(|payload| Outgoing {
                dest: src,
                payload,
            })
            .collect()
    }
}

pub fn run(
    responder: Responder,
    stop: Arc<AtomicBool>,
) -> std::io::Result<Vec<std::thread::JoinHandle<()>>> {
    let socket = bind_retry(SocketAddr::from((
        Ipv4Addr::UNSPECIFIED,
        responder.ssdp_port,
    )))?;
    socket.join_multicast_v4(&SSDP_MULTICAST, &responder.bind_address)?;
    socket.set_broadcast(true)?;
    socket.set_multicast_loop_v4(false)?;
    socket.set_multicast_ttl_v4(2)?;
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;

    let responder = Arc::new(responder);
    let mut handles = Vec::new();
    let receiver = socket.try_clone()?;
    let sender = socket.try_clone()?;
    let processor = Arc::clone(&responder);
    let receiver_stop = Arc::clone(&stop);
    handles.push(std::thread::spawn(move || {
        let mut buf = [0u8; MAX_DATAGRAM + 1];
        while !receiver_stop.load(Ordering::Relaxed) {
            let (len, src) = match receiver.recv_from(&mut buf) {
                Ok(result) => result,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(error) => {
                    eprintln!("event=ssdp_recv_error error={error}");
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };
            for reply in processor.process(src, &buf[..len], unix_seconds()) {
                if let Err(error) = sender.send_to(&reply.payload, reply.dest) {
                    eprintln!(
                        "event=ssdp_send_error destination={} error={error}",
                        reply.dest
                    );
                }
            }
        }
    }));

    let sender = socket.try_clone()?;
    let advertiser = Arc::clone(&responder);
    handles.push(std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let messages = advertiser
                .registry
                .media_server
                .lock()
                .unwrap()
                .as_ref()
                .map_or_else(Vec::new, |lease| lease.alive_messages(unix_seconds()));
            for payload in messages {
                for target in [SSDP_MULTICAST, advertiser.home_broadcast] {
                    let destination =
                        SocketAddr::V4(SocketAddrV4::new(target, advertiser.ssdp_port));
                    if let Err(error) = sender.send_to(&payload, destination) {
                        eprintln!(
                            "event=ssdp_media_server_send_error destination={destination} error={error}"
                        );
                    }
                }
            }
            sleep_interruptible(Duration::from_secs(60), &stop);
        }
    }));
    Ok(handles)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn sleep_interruptible(total: Duration, stop: &AtomicBool) {
    let step = Duration::from_millis(200);
    let mut remaining = total;
    while remaining > Duration::ZERO && !stop.load(Ordering::Relaxed) {
        let chunk = remaining.min(step);
        std::thread::sleep(chunk);
        remaining = remaining.saturating_sub(chunk);
    }
}

/// systemd starts us alongside the network; a not-yet-assigned address is a
/// wait, not a failure.
fn bind_retry(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    for _ in 0..30 {
        match UdpSocket::bind(addr) {
            Ok(socket) => return Ok(socket),
            Err(error) if error.kind() == std::io::ErrorKind::AddrNotAvailable => {
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(error) => return Err(error),
        }
    }
    UdpSocket::bind(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;

    fn responder() -> Responder {
        let config = test_config();
        Responder {
            ssdp_port: config.wiim.ssdp_port,
            home_cidr: config.home_cidr,
            home_broadcast: config.home_broadcast,
            bind_address: config.bind_address,
            metrics: Arc::new(Metrics::default()),
            registry: Arc::new(Registry::default()),
        }
    }

    fn addr(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(ip.parse().unwrap(), port))
    }

    fn lease(expires_at_seconds: u64) -> MediaServerLease {
        MediaServerLease {
            uuid: "airwave".into(),
            location: "http://192.168.66.3:7882/device.xml".into(),
            server: "Linux/1.0 UPnP/1.0 Airwave/0.1.0".into(),
            expires_at_seconds,
        }
    }

    const MEDIA_SEARCH: &[u8] = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";

    #[test]
    fn active_media_server_answers_iot_search_locally() {
        let responder = responder();
        *responder.registry.media_server.lock().unwrap() = Some(lease(1_600));
        let requester = addr("192.168.65.60", 41234);
        let replies = responder.process(requester, MEDIA_SEARCH, 1_000);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].dest, requester);
        let payload = String::from_utf8_lossy(&replies[0].payload);
        assert!(payload.contains("ST: urn:schemas-upnp-org:device:MediaServer:1"));
        assert!(payload.contains("LOCATION: http://192.168.66.3:7882/device.xml"));
    }

    #[test]
    fn expired_or_unregistered_media_server_does_not_answer() {
        let responder = responder();
        let requester = addr("192.168.65.60", 41234);
        assert!(responder.process(requester, MEDIA_SEARCH, 1_000).is_empty());
        *responder.registry.media_server.lock().unwrap() = Some(lease(999));
        assert!(responder.process(requester, MEDIA_SEARCH, 1_000).is_empty());
    }

    #[test]
    fn only_iot_media_server_searches_are_accepted() {
        let responder = responder();
        *responder.registry.media_server.lock().unwrap() = Some(lease(1_600));
        assert!(responder
            .process(addr("10.0.0.1", 41234), MEDIA_SEARCH, 1_000)
            .is_empty());
        assert!(responder
            .process(
                addr("192.168.65.60", 41234),
                b"NOTIFY * HTTP/1.1\r\n\r\n",
                1_000,
            )
            .is_empty());
        let renderer_search = String::from_utf8_lossy(MEDIA_SEARCH)
            .replace("device:MediaServer:1", "device:MediaRenderer:1");
        assert!(responder
            .process(
                addr("192.168.65.60", 41234),
                renderer_search.as_bytes(),
                1_000,
            )
            .is_empty());
        assert!(responder
            .process(addr("192.168.65.3", 1900), MEDIA_SEARCH, 1_000)
            .is_empty());
    }

    #[test]
    fn alive_messages_cover_the_advertised_device_and_services() {
        let messages = lease(1_600).alive_messages(1_000);
        assert_eq!(messages.len(), 5);
        assert!(messages.iter().all(|payload| String::from_utf8_lossy(payload)
            .contains("LOCATION: http://192.168.66.3:7882/device.xml")));
    }

    #[test]
    fn oversized_datagrams_are_dropped() {
        let responder = responder();
        let huge = vec![b'x'; MAX_DATAGRAM + 1];
        assert!(responder
            .process(addr("192.168.65.60", 41234), &huge, 1_000)
            .is_empty());
    }
}
