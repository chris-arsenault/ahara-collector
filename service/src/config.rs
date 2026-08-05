//! Configuration loading. Topology arrives as one JSON document rendered by
//! Nix from site.nix; device credentials arrive as a separate host-state
//! file passed through systemd credentials. Neither is trusted blindly —
//! missing or malformed values fail startup with a named error, matching
//! the repo rule that validation happens before anything runs.

use crate::json::{self, Json};
use std::net::Ipv4Addr;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ipv4Cidr {
    network: u32,
    mask: u32,
}

impl Ipv4Cidr {
    pub fn parse(text: &str) -> Result<Ipv4Cidr, String> {
        let (addr, prefix) = text
            .split_once('/')
            .ok_or_else(|| format!("not a CIDR: {text}"))?;
        let addr: Ipv4Addr = addr.parse().map_err(|_| format!("bad CIDR address: {text}"))?;
        let prefix: u32 = prefix.parse().map_err(|_| format!("bad CIDR prefix: {text}"))?;
        if prefix > 32 {
            return Err(format!("bad CIDR prefix: {text}"));
        }
        let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
        Ok(Ipv4Cidr {
            network: u32::from(addr) & mask,
            mask,
        })
    }

    pub fn contains(&self, addr: Ipv4Addr) -> bool {
        u32::from(addr) & self.mask == self.network
    }
}

#[derive(Debug, Clone)]
pub struct AirwaveSsdpConfig {
    pub enable: bool,
    pub airwave_ip: Ipv4Addr,
    pub ssdp_port: u16,
    pub response_port: u16,
    pub relay_port: u16,
    pub response_window_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct PollerConfig {
    pub enable: bool,
    pub discovery_port: u16,
    pub discovery_bind_port: u16,
    pub device_port: u16,
    pub poll_interval_seconds: u64,
    pub discovery_interval_hours: u64,
    pub static_devices: Vec<Ipv4Addr>,
}

#[derive(Debug, Clone)]
pub struct SpoolConfig {
    pub dir: String,
    pub max_bytes: u64,
    pub segment_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_address: Ipv4Addr,
    pub home_cidr: Ipv4Cidr,
    pub home_broadcast: Ipv4Addr,
    pub api_port: u16,
    pub airwave_ssdp: AirwaveSsdpConfig,
    pub env_sensors: PollerConfig,
    pub kasa: PollerConfig,
    pub spool: SpoolConfig,
}

fn req<'a>(doc: &'a Json, section: &str, key: &str) -> Result<&'a Json, String> {
    let node = if section.is_empty() {
        doc
    } else {
        doc.get(section)
            .ok_or_else(|| format!("config missing section {section}"))?
    };
    node.get(key)
        .ok_or_else(|| format!("config missing {section}.{key}"))
}

fn req_str(doc: &Json, section: &str, key: &str) -> Result<String, String> {
    req(doc, section, key)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("config {section}.{key} must be a string"))
}

fn req_ip(doc: &Json, section: &str, key: &str) -> Result<Ipv4Addr, String> {
    req_str(doc, section, key)?
        .parse()
        .map_err(|_| format!("config {section}.{key} must be an IPv4 address"))
}

fn req_port(doc: &Json, section: &str, key: &str) -> Result<u16, String> {
    let value = req(doc, section, key)?
        .as_i64()
        .ok_or_else(|| format!("config {section}.{key} must be an integer"))?;
    u16::try_from(value).map_err(|_| format!("config {section}.{key} must be a port"))
}

fn req_u64(doc: &Json, section: &str, key: &str) -> Result<u64, String> {
    let value = req(doc, section, key)?
        .as_i64()
        .ok_or_else(|| format!("config {section}.{key} must be an integer"))?;
    u64::try_from(value).map_err(|_| format!("config {section}.{key} must be non-negative"))
}

fn req_bool(doc: &Json, section: &str, key: &str) -> Result<bool, String> {
    req(doc, section, key)?
        .as_bool()
        .ok_or_else(|| format!("config {section}.{key} must be a boolean"))
}

fn poller(doc: &Json, section: &str) -> Result<PollerConfig, String> {
    let static_devices = match req(doc, section, "staticDevices") {
        Ok(node) => node
            .as_arr()
            .ok_or_else(|| format!("config {section}.staticDevices must be a list"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("config {section}.staticDevices entries must be IPv4"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Err(_) => Vec::new(),
    };
    Ok(PollerConfig {
        enable: req_bool(doc, section, "enable")?,
        discovery_port: req_port(doc, section, "discoveryPort")?,
        discovery_bind_port: req_port(doc, section, "discoveryBindPort")?,
        device_port: req_port(doc, section, "devicePort")?,
        poll_interval_seconds: req_u64(doc, section, "pollIntervalSeconds")?.max(1),
        discovery_interval_hours: req_u64(doc, section, "discoveryIntervalHours")?.max(1),
        static_devices,
    })
}

impl Config {
    pub fn from_json(text: &str) -> Result<Config, String> {
        let doc = json::parse(text)?;
        let airwave = AirwaveSsdpConfig {
            enable: req_bool(&doc, "airwaveSsdp", "enable")?,
            airwave_ip: req_ip(&doc, "airwaveSsdp", "airwaveIp")?,
            ssdp_port: req_port(&doc, "airwaveSsdp", "ssdpPort")?,
            response_port: req_port(&doc, "airwaveSsdp", "responsePort")?,
            relay_port: req_port(&doc, "airwaveSsdp", "relayPort")?,
            response_window_seconds: req_u64(&doc, "airwaveSsdp", "responseWindowSeconds")?
                .clamp(1, 10),
        };
        Ok(Config {
            bind_address: req_ip(&doc, "", "bindAddress")?,
            home_cidr: Ipv4Cidr::parse(&req_str(&doc, "", "homeCidr")?)?,
            home_broadcast: req_ip(&doc, "", "homeBroadcast")?,
            api_port: req_port(&doc, "", "apiPort")?,
            airwave_ssdp: airwave,
            env_sensors: poller(&doc, "envSensors")?,
            kasa: poller(&doc, "kasa")?,
            spool: SpoolConfig {
                dir: req_str(&doc, "spool", "dir")?,
                max_bytes: req_u64(&doc, "spool", "maxBytes")?,
                segment_bytes: req_u64(&doc, "spool", "segmentBytes")?,
            },
        })
    }
}

#[derive(Debug, Clone)]
pub struct BasicCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Default)]
pub struct Credentials {
    pub env_sensors: Option<BasicCredentials>,
    pub kasa: Option<BasicCredentials>,
}

impl Credentials {
    /// The credentials file is host state uploaded by the operator; before
    /// that upload it is empty. Empty or `{}` mean "no credentials yet" —
    /// modules that need them idle rather than fail.
    pub fn from_json(text: &str) -> Result<Credentials, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(Credentials::default());
        }
        let doc = json::parse(trimmed).map_err(|e| format!("credentials file: {e}"))?;
        let read = |section: &str| -> Result<Option<BasicCredentials>, String> {
            match doc.get(section) {
                None | Some(Json::Null) => Ok(None),
                Some(node) => {
                    let username = node
                        .get("username")
                        .and_then(Json::as_str)
                        .ok_or_else(|| format!("credentials {section}.username missing"))?;
                    let password = node
                        .get("password")
                        .and_then(Json::as_str)
                        .ok_or_else(|| format!("credentials {section}.password missing"))?;
                    if username.is_empty() || password.is_empty() {
                        return Err(format!("credentials {section}: empty username or password"));
                    }
                    Ok(Some(BasicCredentials {
                        username: username.to_string(),
                        password: password.to_string(),
                    }))
                }
            }
        };
        Ok(Credentials {
            env_sensors: read("envSensors")?,
            kasa: read("kasa")?,
        })
    }
}

#[cfg(test)]
pub fn test_config() -> Config {
    Config::from_json(
        r#"{
        "bindAddress": "192.168.65.3",
        "homeCidr": "192.168.65.0/24",
        "homeBroadcast": "192.168.65.255",
        "apiPort": 8850,
        "airwaveSsdp": {"enable": true, "airwaveIp": "192.168.66.3", "ssdpPort": 1900,
                        "responsePort": 1901, "relayPort": 1901, "responseWindowSeconds": 4},
        "envSensors": {"enable": true, "discoveryPort": 12343, "discoveryBindPort": 12344,
                       "devicePort": 80, "pollIntervalSeconds": 1, "discoveryIntervalHours": 4},
        "kasa": {"enable": true, "discoveryPort": 20002, "discoveryBindPort": 20003,
                 "devicePort": 80, "pollIntervalSeconds": 1, "discoveryIntervalHours": 4,
                 "staticDevices": ["192.168.65.40"]},
        "spool": {"dir": "/tmp/spool", "maxBytes": 268435456, "segmentBytes": 4194304}
    }"#,
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_config() {
        let config = test_config();
        assert_eq!(config.api_port, 8850);
        assert_eq!(config.airwave_ssdp.airwave_ip, "192.168.66.3".parse::<Ipv4Addr>().unwrap());
        assert_eq!(config.kasa.static_devices.len(), 1);
        assert!(config.home_cidr.contains("192.168.65.77".parse().unwrap()));
        assert!(!config.home_cidr.contains("192.168.66.3".parse().unwrap()));
    }

    #[test]
    fn names_the_missing_key() {
        let err = Config::from_json(r#"{"bindAddress": "192.168.65.3"}"#).unwrap_err();
        assert!(err.contains("airwaveSsdp"), "{err}");
    }

    #[test]
    fn cidr_edges() {
        let cidr = Ipv4Cidr::parse("10.0.0.0/8").unwrap();
        assert!(cidr.contains("10.255.255.255".parse().unwrap()));
        assert!(!cidr.contains("11.0.0.0".parse().unwrap()));
        assert!(Ipv4Cidr::parse("10.0.0.0/33").is_err());
        assert!(Ipv4Cidr::parse("10.0.0.0").is_err());
        let all = Ipv4Cidr::parse("0.0.0.0/0").unwrap();
        assert!(all.contains("203.0.113.9".parse().unwrap()));
    }

    #[test]
    fn credentials_empty_and_partial() {
        assert!(Credentials::from_json("").unwrap().env_sensors.is_none());
        assert!(Credentials::from_json("{}").unwrap().kasa.is_none());
        let creds = Credentials::from_json(
            r#"{"envSensors": {"username": "admin", "password": "s3cret"}}"#,
        )
        .unwrap();
        assert_eq!(creds.env_sensors.unwrap().username, "admin");
        assert!(creds.kasa.is_none());
        assert!(Credentials::from_json(r#"{"kasa": {"username": ""}}"#).is_err());
    }
}
