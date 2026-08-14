//! `WiiM` inventory module. Discovery and endpoint validation happen on the
//! `IoT` LAN; Airwave consumes the resulting native device inventory through
//! the collector API. This module does not interpret playback or grouping
//! state and never writes a readings spool.

use crate::config::{Ipv4Cidr, WiimConfig};
use crate::json::{self, Json};
use crate::metrics::{self, Metrics};
use crate::ssdp::{SsdpMessage, MEDIA_RENDERER_URN, SSDP_MULTICAST};
use reqwest::blocking::Client;
use reqwest::Url;
use roxmltree::{Document, Node};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_DATAGRAM: usize = 2048;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WiimServices {
    pub av_transport: Option<String>,
    pub rendering_control: Option<String>,
    pub play_queue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiimDevice {
    pub id: String,
    pub udn: String,
    pub ip: Ipv4Addr,
    pub name: String,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub description_port: u16,
    pub description_path: String,
    pub services: WiimServices,
    pub last_seen_seconds: u64,
    pub reachable: bool,
}

impl WiimDevice {
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut services = BTreeMap::new();
        insert_optional(
            &mut services,
            "avTransport",
            self.services.av_transport.as_deref(),
        );
        insert_optional(
            &mut services,
            "renderingControl",
            self.services.rendering_control.as_deref(),
        );
        insert_optional(
            &mut services,
            "playQueue",
            self.services.play_queue.as_deref(),
        );

        let mut device = BTreeMap::new();
        device.insert("id".into(), Json::Str(self.id.clone()));
        device.insert("udn".into(), Json::Str(self.udn.clone()));
        device.insert("ip".into(), Json::Str(self.ip.to_string()));
        device.insert("name".into(), Json::Str(self.name.clone()));
        insert_optional(&mut device, "model", self.model.as_deref());
        insert_optional(&mut device, "firmware", self.firmware.as_deref());
        device.insert(
            "descriptionPort".into(),
            Json::Int(i64::from(self.description_port)),
        );
        device.insert(
            "descriptionPath".into(),
            Json::Str(self.description_path.clone()),
        );
        device.insert("services".into(), Json::Obj(services));
        device.insert(
            "lastSeenSeconds".into(),
            Json::Int(self.last_seen_seconds.try_into().unwrap_or(i64::MAX)),
        );
        device.insert("reachable".into(), Json::Bool(self.reachable));
        Json::Obj(device)
    }

    fn from_json(value: &Json) -> Option<WiimDevice> {
        let services = value.get("services")?;
        let ip = value.get("ip")?.as_str()?.parse().ok()?;
        let port = u16::try_from(value.get("descriptionPort")?.as_i64()?).ok()?;
        Some(WiimDevice {
            id: value.get("id")?.as_str()?.to_string(),
            udn: value.get("udn")?.as_str()?.to_string(),
            ip,
            name: value.get("name")?.as_str()?.to_string(),
            model: optional_string(value, "model"),
            firmware: optional_string(value, "firmware"),
            description_port: port,
            description_path: value.get("descriptionPath")?.as_str()?.to_string(),
            services: WiimServices {
                av_transport: optional_string(services, "avTransport"),
                rendering_control: optional_string(services, "renderingControl"),
                play_queue: optional_string(services, "playQueue"),
            },
            last_seen_seconds: u64::try_from(value.get("lastSeenSeconds")?.as_i64()?).ok()?,
            reachable: false,
        })
    }
}

fn insert_optional(map: &mut BTreeMap<String, Json>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        map.insert(key.to_string(), Json::Str(value.to_string()));
    }
}

fn optional_string(value: &Json, key: &str) -> Option<String> {
    value.get(key).and_then(Json::as_str).map(str::to_string)
}

pub struct WiimModule {
    pub cfg: WiimConfig,
    pub bind_address: Ipv4Addr,
    pub iot_cidr: Ipv4Cidr,
    pub iot_broadcast: Ipv4Addr,
    pub metrics: Arc<Metrics>,
    pub registry: Arc<crate::registry::Registry>,
}

impl WiimModule {
    pub fn spawn(self, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || self.run(&stop))
    }

    fn run(&self, stop: &AtomicBool) {
        let mut devices = load_inventory(Path::new(&self.cfg.state_file)).unwrap_or_else(|error| {
            eprintln!(
                "event=wiim_inventory_load_failed path={} error={error}",
                self.cfg.state_file
            );
            Vec::new()
        });
        self.registry.wiim.lock().unwrap().clone_from(&devices);
        metrics::set(&self.metrics.wiim_devices, devices.len() as u64);

        let client = match Client::builder().timeout(Duration::from_secs(5)).build() {
            Ok(client) => client,
            Err(error) => {
                eprintln!("event=wiim_start_failed reason=http_client error={error}");
                return;
            }
        };
        let interval = Duration::from_secs(self.cfg.discovery_interval_seconds);

        while !stop.load(Ordering::Relaxed) {
            metrics::inc(&self.metrics.wiim_discovery_runs);
            match self.discover(&client, stop) {
                Ok(discovered) => {
                    devices = merge_inventory(devices, discovered);
                    if let Err(error) = save_inventory(Path::new(&self.cfg.state_file), &devices) {
                        eprintln!(
                            "event=wiim_inventory_save_failed path={} error={error}",
                            self.cfg.state_file
                        );
                    }
                    self.registry.wiim.lock().unwrap().clone_from(&devices);
                    metrics::set(&self.metrics.wiim_devices, devices.len() as u64);
                    metrics::set(
                        &self.metrics.wiim_reachable_devices,
                        devices.iter().filter(|device| device.reachable).count() as u64,
                    );
                    eprintln!(
                        "event=wiim_discovery devices={} reachable={}",
                        devices.len(),
                        devices.iter().filter(|device| device.reachable).count()
                    );
                }
                Err(error) => {
                    metrics::inc(&self.metrics.wiim_discovery_failed);
                    eprintln!("event=wiim_discovery_error error={error}");
                }
            }
            sleep_interruptible(interval, stop);
        }
    }

    fn discover(&self, client: &Client, stop: &AtomicBool) -> Result<Vec<WiimDevice>, String> {
        let bind = SocketAddr::V4(SocketAddrV4::new(
            self.bind_address,
            self.cfg.discovery_bind_port,
        ));
        let socket = UdpSocket::bind(bind).map_err(|error| error.to_string())?;
        socket.set_broadcast(true).map_err(|error| error.to_string())?;
        socket
            .set_multicast_ttl_v4(2)
            .map_err(|error| error.to_string())?;
        socket
            .set_read_timeout(Some(Duration::from_millis(250)))
            .map_err(|error| error.to_string())?;

        let search = renderer_search(self.cfg.response_window_seconds);
        for target in [SSDP_MULTICAST, self.iot_broadcast] {
            socket
                .send_to(
                    search.as_bytes(),
                    SocketAddr::V4(SocketAddrV4::new(target, self.cfg.ssdp_port)),
                )
                .map_err(|error| error.to_string())?;
        }

        let deadline = Instant::now() + Duration::from_secs(self.cfg.response_window_seconds);
        let mut locations = HashSet::new();
        let mut buffer = [0u8; MAX_DATAGRAM + 1];
        while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
            let Ok((length, source)) = socket.recv_from(&mut buffer) else {
                continue;
            };
            let SocketAddr::V4(source) = source else {
                continue;
            };
            if length > MAX_DATAGRAM || !self.iot_cidr.contains(*source.ip()) {
                continue;
            }
            if let Some(location) = renderer_location(&buffer[..length], &self.iot_cidr) {
                locations.insert(location);
            }
        }

        let now = unix_seconds();
        let mut devices = BTreeMap::new();
        for location in locations {
            match fetch_description(client, &location, &self.iot_cidr, now) {
                Ok(device) => {
                    devices.insert(device.id.clone(), device);
                }
                Err(error) => {
                    metrics::inc(&self.metrics.wiim_descriptions_failed);
                    eprintln!("event=wiim_description_failed location={location} error={error}");
                }
            }
        }
        Ok(devices.into_values().collect())
    }
}

fn renderer_search(window_seconds: u64) -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: {window_seconds}\r\nST: {MEDIA_RENDERER_URN}\r\n\r\n"
    )
}

fn renderer_location(payload: &[u8], iot_cidr: &Ipv4Cidr) -> Option<String> {
    let message = SsdpMessage::parse(payload)?;
    if message.start_line != "HTTP/1.1 200 OK"
        || message.header("st") != Some(MEDIA_RENDERER_URN)
        || message.header("usn").is_none_or(str::is_empty)
    {
        return None;
    }
    let location = message.header("location")?;
    validated_location(location, iot_cidr).ok()?;
    Some(location.to_string())
}

fn validated_location(location: &str, iot_cidr: &Ipv4Cidr) -> Result<Url, String> {
    let url = Url::parse(location).map_err(|error| format!("invalid LOCATION: {error}"))?;
    if url.scheme() != "http" {
        return Err("renderer LOCATION must use HTTP".into());
    }
    let ip: Ipv4Addr = url
        .host_str()
        .ok_or_else(|| "renderer LOCATION has no host".to_string())?
        .parse()
        .map_err(|_| "renderer LOCATION host must be IPv4".to_string())?;
    if !iot_cidr.contains(ip) {
        return Err("renderer LOCATION is outside the IoT CIDR".into());
    }
    if url.port_or_known_default().is_none() {
        return Err("renderer LOCATION has no usable port".into());
    }
    Ok(url)
}

fn fetch_description(
    client: &Client,
    location: &str,
    iot_cidr: &Ipv4Cidr,
    now_seconds: u64,
) -> Result<WiimDevice, String> {
    let location_url = validated_location(location, iot_cidr)?;
    let response = client
        .get(location_url.clone())
        .send()
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let body = response.text().map_err(|error| error.to_string())?;
    parse_description(&body, &location_url, iot_cidr, now_seconds)
}

fn parse_description(
    xml: &str,
    location: &Url,
    iot_cidr: &Ipv4Cidr,
    now_seconds: u64,
) -> Result<WiimDevice, String> {
    let location = validated_location(location.as_str(), iot_cidr)?;
    let document = Document::parse(xml).map_err(|error| error.to_string())?;
    let device = document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "device")
        .ok_or_else(|| "description has no device".to_string())?;
    let device_type = child_text(device, "deviceType").unwrap_or_default();
    if device_type != MEDIA_RENDERER_URN {
        return Err("description is not a MediaRenderer".into());
    }
    let udn = child_text(device, "UDN").ok_or_else(|| "description has no UDN".to_string())?;
    let id = udn
        .strip_prefix("uuid:")
        .unwrap_or(&udn)
        .trim()
        .to_string();
    if id.is_empty() {
        return Err("description has an empty UDN".into());
    }

    let mut services = WiimServices::default();
    for service in device
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "service")
    {
        let service_type = child_text(service, "serviceType").unwrap_or_default();
        let Some(control_url) = child_text(service, "controlURL") else {
            continue;
        };
        let control_path = validated_control_path(&location, &control_url)?;
        if service_type.contains("AVTransport") {
            services.av_transport = Some(control_path);
        } else if service_type.contains("RenderingControl") {
            services.rendering_control = Some(control_path);
        } else if service_type.contains("PlayQueue") {
            services.play_queue = Some(control_path);
        }
    }
    if services.av_transport.is_none() && services.rendering_control.is_none() {
        return Err("description has no supported renderer services".into());
    }

    let ip: Ipv4Addr = location
        .host_str()
        .ok_or_else(|| "renderer LOCATION has no host".to_string())?
        .parse()
        .map_err(|_| "renderer LOCATION host must be IPv4".to_string())?;
    let description_port = location
        .port_or_known_default()
        .ok_or_else(|| "renderer LOCATION has no usable port".to_string())?;
    Ok(WiimDevice {
        id,
        udn,
        ip,
        name: child_text(device, "friendlyName").unwrap_or_else(|| ip.to_string()),
        model: child_text(device, "modelName"),
        firmware: child_text(device, "modelNumber"),
        description_port,
        description_path: location.path().to_string(),
        services,
        last_seen_seconds: now_seconds,
        reachable: true,
    })
}

fn child_text(node: Node<'_, '_>, local_name: &str) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == local_name)
        .and_then(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn validated_control_path(location: &Url, value: &str) -> Result<String, String> {
    let resolved = location
        .join(value)
        .map_err(|error| format!("invalid control URL: {error}"))?;
    if resolved.scheme() != "http"
        || resolved.host_str() != location.host_str()
        || resolved.port_or_known_default() != location.port_or_known_default()
    {
        return Err("control URL leaves the renderer endpoint".into());
    }
    let mut path = resolved.path().to_string();
    if let Some(query) = resolved.query() {
        path.push('?');
        path.push_str(query);
    }
    Ok(path)
}

#[must_use]
fn merge_inventory(
    previous: Vec<WiimDevice>,
    discovered: Vec<WiimDevice>,
) -> Vec<WiimDevice> {
    let mut merged: BTreeMap<String, WiimDevice> = previous
        .into_iter()
        .map(|mut device| {
            device.reachable = false;
            (device.id.clone(), device)
        })
        .collect();
    for device in discovered {
        merged.insert(device.id.clone(), device);
    }
    merged.into_values().collect()
}

fn load_inventory(path: &Path) -> Result<Vec<WiimDevice>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let document = json::parse(&text)?;
    let devices = document
        .as_arr()
        .ok_or_else(|| "WiiM inventory must be a JSON array".to_string())?;
    devices
        .iter()
        .map(|device| {
            WiimDevice::from_json(device)
                .ok_or_else(|| "WiiM inventory contains an invalid device".to_string())
        })
        .collect()
}

fn save_inventory(path: &Path, devices: &[WiimDevice]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "WiiM state file has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("tmp");
    let body = Json::Arr(devices.iter().map(WiimDevice::to_json).collect()).to_string();
    fs::write(&temporary, body).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
    <friendlyName>Office WiiM</friendlyName>
    <modelName>WiiM Mini</modelName>
    <modelNumber>Linkplay.4.6</modelNumber>
    <UDN>uuid:wiim-office</UDN>
    <serviceList>
      <service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/upnp/control/avtransport1</controlURL></service>
      <service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/upnp/control/rendercontrol1</controlURL></service>
      <service><serviceType>urn:schemas-wiimu-com:service:PlayQueue:1</serviceType><controlURL>/upnp/control/PlayQueue1</controlURL></service>
    </serviceList>
  </device>
</root>"#;

    fn cidr() -> Ipv4Cidr {
        Ipv4Cidr::parse("192.168.30.0/24").unwrap()
    }

    #[test]
    fn renderer_reply_requires_an_iot_location() {
        let valid = b"HTTP/1.1 200 OK\r\nLOCATION: http://192.168.30.20:59152/description.xml\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\nUSN: uuid:wiim-office::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
        assert_eq!(
            renderer_location(valid, &cidr()).as_deref(),
            Some("http://192.168.30.20:59152/description.xml")
        );
        let off_subnet = String::from_utf8_lossy(valid)
            .replace("192.168.30.20", "192.168.66.3");
        assert!(renderer_location(off_subnet.as_bytes(), &cidr()).is_none());
    }

    #[test]
    fn description_becomes_native_inventory() {
        let location = Url::parse("http://192.168.30.20:59152/description.xml").unwrap();
        let device = parse_description(DESCRIPTION, &location, &cidr(), 42).unwrap();
        assert_eq!(device.id, "wiim-office");
        assert_eq!(device.name, "Office WiiM");
        assert_eq!(device.description_port, 59152);
        assert_eq!(
            device.services.rendering_control.as_deref(),
            Some("/upnp/control/rendercontrol1")
        );
        assert!(device.reachable);
    }

    #[test]
    fn description_rejects_an_external_control_url() {
        let location = Url::parse("http://192.168.30.20:59152/description.xml").unwrap();
        let xml = DESCRIPTION.replace(
            "/upnp/control/avtransport1",
            "http://192.168.30.99:80/control",
        );
        assert!(parse_description(&xml, &location, &cidr(), 42).is_err());
    }

    #[test]
    fn cache_round_trips_and_marks_entries_unreachable() {
        let location = Url::parse("http://192.168.30.20:59152/description.xml").unwrap();
        let device = parse_description(DESCRIPTION, &location, &cidr(), 42).unwrap();
        let path = std::env::temp_dir().join(format!(
            "ahara-wiim-inventory-{}-{}.json",
            std::process::id(),
            unix_seconds()
        ));
        save_inventory(&path, std::slice::from_ref(&device)).unwrap();
        let loaded = load_inventory(&path).unwrap();
        let _ = fs::remove_file(path);
        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].reachable);

        let merged = merge_inventory(loaded, Vec::new());
        assert_eq!(merged[0].last_seen_seconds, 42);
        assert!(!merged[0].reachable);
    }
}
