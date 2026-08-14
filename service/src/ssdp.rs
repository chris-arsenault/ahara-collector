//! Airwave SSDP relay. The collector sits on the WiiM subnet, so unlike the
//! failed gateway-hosted attempt it speaks real on-link SSDP: multicast with
//! a directed-broadcast copy for Wi-Fi networks that suppress multicast
//! delivery (the reason ahara-vpn ADR-0012 existed), and it can hear
//! everything the renderers send.
//!
//! Four relayed paths:
//!   1. Airwave M-SEARCH (unicast from TrueNAS) → re-originated on-link;
//!      renderer replies within the search window return to Airwave's fixed
//!      response port.
//!   2. Airwave NOTIFY (MediaServer announcements) → re-originated on-link
//!      so WiiMs learn the media server exists.
//!   3. WiiM M-SEARCH (multicast, looking for MediaServers) → forwarded to
//!      Airwave; its unicast answers return to the requesting device.
//!   4. Renderer NOTIFYs are counted but not forwarded — Airwave's discovery
//!      is M-SEARCH-based and ignores them.
//!
//! Packet handling is pure (`process_main` / `process_relay` return send
//! actions); the socket loops just execute them. That is what makes the
//! relay testable without a network.

use crate::config::{AirwaveSsdpConfig, Ipv4Cidr};
use crate::metrics::{self, Metrics};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SSDP_MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const MAX_DATAGRAM: usize = 2048;
pub const MEDIA_RENDERER_URN: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
const HOME_SEARCH_WINDOW: Duration = Duration::from_secs(3);

/// Search targets a home device may ask Airwave about: exactly the NT set
/// Airwave's own SSDP responder advertises.
const RELAYED_HOME_TARGETS: [&str; 5] = [
    "ssdp:all",
    "upnp:rootdevice",
    "urn:schemas-upnp-org:device:MediaServer:1",
    "urn:schemas-upnp-org:service:ContentDirectory:1",
    "urn:schemas-upnp-org:service:ConnectionManager:1",
];

pub struct RelayState {
    /// Deadline until which renderer replies are forwarded to Airwave.
    /// Windows from overlapping searches merge to the latest deadline; SSDP
    /// replies carry nothing that would correlate them to one search.
    pub search_deadline: Option<Instant>,
    /// Home devices with an in-flight MediaServer search, awaiting Airwave's
    /// unicast answers.
    pub pending_home: Vec<(SocketAddr, Instant)>,
}

impl RelayState {
    pub fn new() -> RelayState {
        RelayState {
            search_deadline: None,
            pending_home: Vec::new(),
        }
    }

    fn prune(&mut self, now: Instant) {
        if self.search_deadline.is_some_and(|d| d <= now) {
            self.search_deadline = None;
        }
        self.pending_home.retain(|(_, deadline)| *deadline > now);
    }
}

impl Default for RelayState {
    fn default() -> Self {
        Self::new()
    }
}

/// Which socket an action leaves through.
#[derive(Debug, PartialEq)]
pub enum Via {
    Relay,
}

#[derive(Debug, PartialEq)]
pub struct Outgoing {
    pub via: Via,
    pub dest: SocketAddr,
    pub payload: Vec<u8>,
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
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, PartialEq)]
pub enum AirwaveMessage {
    /// MX seconds from the payload, already clamped.
    Search { window: Duration },
    Notify,
    Invalid,
}

pub fn classify_airwave(payload: &[u8], max_window_seconds: u64) -> AirwaveMessage {
    let Some(msg) = SsdpMessage::parse(payload) else {
        return AirwaveMessage::Invalid;
    };
    match msg.start_line.as_str() {
        "M-SEARCH * HTTP/1.1" => {
            let man_ok = msg.header("man") == Some("\"ssdp:discover\"");
            let st_ok = msg.header("st") == Some(MEDIA_RENDERER_URN);
            if man_ok && st_ok {
                let mx = msg
                    .header("mx")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(max_window_seconds);
                AirwaveMessage::Search {
                    window: Duration::from_secs(mx.clamp(1, max_window_seconds)),
                }
            } else {
                AirwaveMessage::Invalid
            }
        }
        "NOTIFY * HTTP/1.1" => {
            let nts_ok = matches!(msg.header("nts"), Some("ssdp:alive") | Some("ssdp:byebye"));
            if nts_ok && msg.header("nt").is_some() && msg.header("usn").is_some() {
                AirwaveMessage::Notify
            } else {
                AirwaveMessage::Invalid
            }
        }
        _ => AirwaveMessage::Invalid,
    }
}

/// A renderer's answer to the re-originated M-SEARCH: a 200 response for the
/// MediaRenderer URN whose LOCATION host is an address on the home subnet.
/// (The LOCATION is not required to equal the sender — grouped WiiM slaves
/// have answered for their master before — but it must stay inside the
/// subnet so nothing can steer Airwave off the LAN.)
pub fn is_valid_renderer_reply(payload: &[u8], home_cidr: &Ipv4Cidr) -> bool {
    let Some(msg) = SsdpMessage::parse(payload) else {
        return false;
    };
    if msg.start_line != "HTTP/1.1 200 OK" {
        return false;
    }
    if msg.header("st") != Some(MEDIA_RENDERER_URN) {
        return false;
    }
    if msg.header("usn").is_none_or(str::is_empty) {
        return false;
    }
    let Some(location) = msg.header("location") else {
        return false;
    };
    location_host(location)
        .is_some_and(|host| host.parse::<Ipv4Addr>().is_ok_and(|ip| home_cidr.contains(ip)))
}

fn location_host(location: &str) -> Option<&str> {
    let rest = location
        .strip_prefix("http://")
        .or_else(|| location.strip_prefix("https://"))?;
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..end];
    Some(authority.rsplit_once(':').map_or(authority, |(host, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            host
        } else {
            authority
        }
    }))
}

/// Is this M-SEARCH from a home device one Airwave could answer?
fn is_relayable_home_search(payload: &[u8]) -> bool {
    let Some(msg) = SsdpMessage::parse(payload) else {
        return false;
    };
    msg.start_line == "M-SEARCH * HTTP/1.1"
        && msg.header("man") == Some("\"ssdp:discover\"")
        && msg
            .header("st")
            .is_some_and(|st| RELAYED_HOME_TARGETS.contains(&st) || st.starts_with("uuid:"))
}

pub struct Relay {
    pub cfg: AirwaveSsdpConfig,
    pub home_cidr: Ipv4Cidr,
    pub home_broadcast: Ipv4Addr,
    pub bind_address: Ipv4Addr,
    pub metrics: Arc<Metrics>,
}

impl Relay {
    /// A datagram arriving on the main SSDP socket (port 1900): Airwave
    /// unicast, or on-link multicast/broadcast from home devices.
    pub fn process_main(
        &self,
        src: SocketAddr,
        payload: &[u8],
        now: Instant,
        state: &mut RelayState,
    ) -> Vec<Outgoing> {
        state.prune(now);
        let SocketAddr::V4(src_v4) = src else {
            metrics::inc(&self.metrics.ssdp_dropped);
            return Vec::new();
        };
        if payload.len() > MAX_DATAGRAM {
            metrics::inc(&self.metrics.ssdp_dropped);
            return Vec::new();
        }
        let src_ip = *src_v4.ip();
        // Our own re-originated broadcast copies come back to this socket.
        if src_ip == self.bind_address {
            return Vec::new();
        }

        if src_ip == self.cfg.airwave_ip {
            return match classify_airwave(payload, self.cfg.response_window_seconds) {
                AirwaveMessage::Search { window } => {
                    let deadline = now + window;
                    state.search_deadline = Some(match state.search_deadline {
                        Some(existing) if existing > deadline => existing,
                        _ => deadline,
                    });
                    metrics::inc(&self.metrics.ssdp_airwave_msearch);
                    self.fan_out_home(payload)
                }
                AirwaveMessage::Notify => {
                    metrics::inc(&self.metrics.ssdp_airwave_notify);
                    self.fan_out_home(payload)
                }
                AirwaveMessage::Invalid => {
                    metrics::inc(&self.metrics.ssdp_dropped);
                    Vec::new()
                }
            };
        }

        if self.home_cidr.contains(src_ip) {
            if is_relayable_home_search(payload) {
                metrics::inc(&self.metrics.ssdp_home_msearch);
                state.pending_home.retain(|(addr, _)| *addr != src);
                state.pending_home.push((src, now + HOME_SEARCH_WINDOW));
                // Bounded: a chatty LAN cannot grow this without limit.
                if state.pending_home.len() > 64 {
                    state.pending_home.remove(0);
                }
                return vec![Outgoing {
                    via: Via::Relay,
                    dest: SocketAddr::V4(SocketAddrV4::new(
                        self.cfg.airwave_ip,
                        self.cfg.ssdp_port,
                    )),
                    payload: payload.to_vec(),
                }];
            }
            // Renderer NOTIFYs land here too; Airwave has no consumer for
            // them, so they are observed, not relayed.
            metrics::inc(&self.metrics.ssdp_dropped);
            return Vec::new();
        }

        metrics::inc(&self.metrics.ssdp_dropped);
        Vec::new()
    }

    /// A datagram arriving on the relay socket (the fixed port our
    /// re-originated traffic uses as source): renderer search replies, or
    /// Airwave's answers to forwarded home searches.
    pub fn process_relay(
        &self,
        src: SocketAddr,
        payload: &[u8],
        now: Instant,
        state: &mut RelayState,
    ) -> Vec<Outgoing> {
        state.prune(now);
        let SocketAddr::V4(src_v4) = src else {
            metrics::inc(&self.metrics.ssdp_dropped);
            return Vec::new();
        };
        if payload.len() > MAX_DATAGRAM {
            metrics::inc(&self.metrics.ssdp_dropped);
            return Vec::new();
        }
        let src_ip = *src_v4.ip();

        if src_ip == self.cfg.airwave_ip {
            // Airwave answering a forwarded home search: fan the reply out
            // to every pending requester.
            if SsdpMessage::parse(payload)
                .is_some_and(|m| m.start_line == "HTTP/1.1 200 OK" && m.header("st").is_some())
            {
                metrics::inc(&self.metrics.ssdp_home_replies);
                return state
                    .pending_home
                    .iter()
                    .map(|(requester, _)| Outgoing {
                        via: Via::Relay,
                        dest: *requester,
                        payload: payload.to_vec(),
                    })
                    .collect();
            }
            metrics::inc(&self.metrics.ssdp_dropped);
            return Vec::new();
        }

        if self.home_cidr.contains(src_ip) && src_ip != self.bind_address {
            let window_open = state.search_deadline.is_some_and(|d| d > now);
            if window_open && is_valid_renderer_reply(payload, &self.home_cidr) {
                metrics::inc(&self.metrics.ssdp_renderer_replies);
                return vec![Outgoing {
                    via: Via::Relay,
                    dest: SocketAddr::V4(SocketAddrV4::new(
                        self.cfg.airwave_ip,
                        self.cfg.response_port,
                    )),
                    payload: payload.to_vec(),
                }];
            }
        }

        metrics::inc(&self.metrics.ssdp_dropped);
        Vec::new()
    }

    /// On-link fan-out: multicast plus the directed broadcast, because home
    /// Wi-Fi has been observed suppressing multicast delivery.
    fn fan_out_home(&self, payload: &[u8]) -> Vec<Outgoing> {
        vec![
            Outgoing {
                via: Via::Relay,
                dest: SocketAddr::V4(SocketAddrV4::new(SSDP_MULTICAST, self.cfg.ssdp_port)),
                payload: payload.to_vec(),
            },
            Outgoing {
                via: Via::Relay,
                dest: SocketAddr::V4(SocketAddrV4::new(self.home_broadcast, self.cfg.ssdp_port)),
                payload: payload.to_vec(),
            },
        ]
    }
}

/// Socket loops. Two threads share the relay state; each executes the send
/// actions its processor returns.
pub fn run(
    relay: Relay,
    stop: Arc<AtomicBool>,
) -> std::io::Result<Vec<std::thread::JoinHandle<()>>> {
    let main_sock = bind_retry(SocketAddr::from((Ipv4Addr::UNSPECIFIED, relay.cfg.ssdp_port)))?;
    main_sock.join_multicast_v4(&SSDP_MULTICAST, &relay.bind_address)?;
    main_sock.set_multicast_loop_v4(false)?;
    main_sock.set_read_timeout(Some(Duration::from_millis(200)))?;

    let relay_sock = bind_retry(SocketAddr::from((relay.bind_address, relay.cfg.relay_port)))?;
    relay_sock.set_broadcast(true)?;
    relay_sock.set_multicast_ttl_v4(2)?;
    relay_sock.set_read_timeout(Some(Duration::from_millis(200)))?;

    let state = Arc::new(Mutex::new(RelayState::new()));
    let relay = Arc::new(relay);
    let mut handles = Vec::new();

    for main_loop in [true, false] {
        let receiver = if main_loop {
            main_sock.try_clone()?
        } else {
            relay_sock.try_clone()?
        };
        let sender = relay_sock.try_clone()?;
        let relay = Arc::clone(&relay);
        let state = Arc::clone(&state);
        let stop = Arc::clone(&stop);
        handles.push(std::thread::spawn(move || {
            let mut buf = [0u8; MAX_DATAGRAM + 1];
            while !stop.load(Ordering::Relaxed) {
                let (len, src) = match receiver.recv_from(&mut buf) {
                    Ok(ok) => ok,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        continue;
                    }
                    Err(e) => {
                        eprintln!("event=ssdp_recv_error error={e}");
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                };
                let now = Instant::now();
                let actions = {
                    let mut state = state.lock().unwrap();
                    if main_loop {
                        relay.process_main(src, &buf[..len], now, &mut state)
                    } else {
                        relay.process_relay(src, &buf[..len], now, &mut state)
                    }
                };
                for action in actions {
                    if let Err(e) = sender.send_to(&action.payload, action.dest) {
                        eprintln!(
                            "event=ssdp_send_error destination={} error={e}",
                            action.dest
                        );
                    }
                }
            }
        }));
    }
    Ok(handles)
}

/// systemd starts us alongside the network; a not-yet-assigned address is a
/// wait, not a failure.
fn bind_retry(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    for _ in 0..30 {
        match UdpSocket::bind(addr) {
            Ok(sock) => return Ok(sock),
            Err(e) if e.kind() == std::io::ErrorKind::AddrNotAvailable => {
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(e) => return Err(e),
        }
    }
    UdpSocket::bind(addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;

    fn relay() -> Relay {
        let cfg = test_config();
        Relay {
            cfg: cfg.airwave_ssdp,
            home_cidr: cfg.home_cidr,
            home_broadcast: cfg.home_broadcast,
            bind_address: cfg.bind_address,
            metrics: Arc::new(Metrics::default()),
        }
    }

    fn addr(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(ip.parse().unwrap(), port))
    }

    const AIRWAVE_MSEARCH: &[u8] = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 3\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
    const AIRWAVE_NOTIFY: &[u8] = b"NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: upnp:rootdevice\r\nNTS: ssdp:alive\r\nUSN: uuid:x::upnp:rootdevice\r\nLOCATION: http://192.168.66.3:7882/device.xml\r\n\r\n";
    const RENDERER_REPLY: &[u8] = b"HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\nEXT:\r\nLOCATION: http://192.168.65.60:49152/description.xml\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\nUSN: uuid:wiim-1::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";

    #[test]
    fn classifies_airwave_messages() {
        assert_eq!(
            classify_airwave(AIRWAVE_MSEARCH, 4),
            AirwaveMessage::Search {
                window: Duration::from_secs(3)
            }
        );
        assert_eq!(classify_airwave(AIRWAVE_NOTIFY, 4), AirwaveMessage::Notify);
        assert_eq!(classify_airwave(b"GET / HTTP/1.1\r\n\r\n", 4), AirwaveMessage::Invalid);
        // MX beyond the cap clamps down.
        let big_mx = AIRWAVE_MSEARCH
            .to_vec()
            .windows(1)
            .count(); // silence unused warning path
        let _ = big_mx;
        let msearch_mx9 = String::from_utf8_lossy(AIRWAVE_MSEARCH).replace("MX: 3", "MX: 9");
        assert_eq!(
            classify_airwave(msearch_mx9.as_bytes(), 4),
            AirwaveMessage::Search {
                window: Duration::from_secs(4)
            }
        );
    }

    #[test]
    fn validates_renderer_replies() {
        let cidr = Ipv4Cidr::parse("192.168.65.0/24").unwrap();
        assert!(is_valid_renderer_reply(RENDERER_REPLY, &cidr));
        let off_subnet = String::from_utf8_lossy(RENDERER_REPLY)
            .replace("http://192.168.65.60:49152", "http://10.9.9.9:49152");
        assert!(!is_valid_renderer_reply(off_subnet.as_bytes(), &cidr));
        let wrong_st = String::from_utf8_lossy(RENDERER_REPLY)
            .replace("device:MediaRenderer:1", "device:MediaServer:1");
        assert!(!is_valid_renderer_reply(wrong_st.as_bytes(), &cidr));
        let hostname_location = String::from_utf8_lossy(RENDERER_REPLY)
            .replace("http://192.168.65.60:49152", "http://evil.example:80");
        assert!(!is_valid_renderer_reply(hostname_location.as_bytes(), &cidr));
    }

    #[test]
    fn airwave_msearch_fans_out_and_opens_window() {
        let relay = relay();
        let mut state = RelayState::new();
        let now = Instant::now();
        let actions = relay.process_main(addr("192.168.66.3", 1901), AIRWAVE_MSEARCH, now, &mut state);
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().any(|a| a.dest == addr("239.255.255.250", 1900)));
        assert!(actions.iter().any(|a| a.dest == addr("192.168.65.255", 1900)));
        assert!(state.search_deadline.is_some());

        // A renderer reply inside the window forwards to Airwave's fixed
        // response port; outside it, nothing.
        let forwarded =
            relay.process_relay(addr("192.168.65.60", 50000), RENDERER_REPLY, now, &mut state);
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].dest, addr("192.168.66.3", 1901));

        let late = now + Duration::from_secs(30);
        let dropped =
            relay.process_relay(addr("192.168.65.60", 50000), RENDERER_REPLY, late, &mut state);
        assert!(dropped.is_empty());
    }

    #[test]
    fn home_msearch_reaches_airwave_and_reply_returns() {
        let relay = relay();
        let mut state = RelayState::new();
        let now = Instant::now();
        let wiim_search = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\n\r\n";
        let requester = addr("192.168.65.60", 41234);
        let actions = relay.process_main(requester, wiim_search, now, &mut state);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].dest, addr("192.168.66.3", 1900));

        let airwave_reply = b"HTTP/1.1 200 OK\r\nST: urn:schemas-upnp-org:device:MediaServer:1\r\nUSN: uuid:airwave::urn:schemas-upnp-org:device:MediaServer:1\r\nLOCATION: http://192.168.66.3:7882/device.xml\r\n\r\n";
        let replies =
            relay.process_relay(addr("192.168.66.3", 1900), airwave_reply, now, &mut state);
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].dest, requester);
    }

    #[test]
    fn renderer_notify_and_foreign_sources_are_dropped() {
        let relay = relay();
        let mut state = RelayState::new();
        let now = Instant::now();
        let renderer_notify = b"NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: urn:schemas-upnp-org:device:MediaRenderer:1\r\nNTS: ssdp:alive\r\nUSN: uuid:wiim\r\n\r\n";
        assert!(relay
            .process_main(addr("192.168.65.60", 1900), renderer_notify, now, &mut state)
            .is_empty());
        // Off-subnet source: dropped even with a valid payload.
        assert!(relay
            .process_main(addr("10.0.0.1", 1900), AIRWAVE_MSEARCH, now, &mut state)
            .is_empty());
        // Our own reflected broadcast: silently ignored.
        assert!(relay
            .process_main(addr("192.168.65.3", 1901), AIRWAVE_MSEARCH, now, &mut state)
            .is_empty());
    }

    #[test]
    fn oversized_datagrams_are_dropped() {
        let relay = relay();
        let mut state = RelayState::new();
        let huge = vec![b'x'; MAX_DATAGRAM + 1];
        assert!(relay
            .process_main(addr("192.168.66.3", 1901), &huge, Instant::now(), &mut state)
            .is_empty());
    }
}
