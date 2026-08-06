//! Reading envelopes: the spool and wire format for every reading the
//! collector produces. One JSON object per line — `module`, `device`
//! identity, `timestampNs`, and `values` carrying the device's payload
//! verbatim. The collector never emits a measurement, field, or bucket
//! name; the data schema belongs to house-sensors (ADR-0006).

use crate::json::{self, Json};
use std::collections::BTreeMap;

/// Module names of the built-in pollers; pushed envelopes may declare
/// others (each module is its own spool and drain stream).
pub const MODULE_ENV_SENSORS: &str = "envSensors";
pub const MODULE_KASA: &str = "kasa";

/// Largest envelope accepted on the push path; poller-built envelopes are
/// far smaller.
const MAX_PUSHED_BYTES: usize = 64 * 1024;

/// A module name becomes a spool directory name and a query parameter, so
/// the charset stays filesystem- and URL-safe.
pub fn valid_module(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Build one envelope line. `values` must be a non-empty object — a
/// reading with nothing in it is not a reading.
pub fn reading(module: &str, device: Json, timestamp_ns: i64, values: Json) -> Option<String> {
    match &values {
        Json::Obj(map) if !map.is_empty() => {}
        _ => return None,
    }
    let mut out = BTreeMap::new();
    out.insert("module".to_string(), Json::Str(module.to_string()));
    out.insert("device".to_string(), device);
    out.insert("timestampNs".to_string(), Json::Int(timestamp_ns));
    out.insert("values".to_string(), values);
    Some(Json::Obj(out).to_string())
}

/// Device identity block: `ip` always, the rest as discovered.
pub fn device(
    ip: &str,
    name: Option<&str>,
    model: Option<&str>,
    device_id: Option<&str>,
    tags: &[(String, String)],
) -> Json {
    let mut map = BTreeMap::new();
    map.insert("ip".to_string(), Json::Str(ip.to_string()));
    if let Some(name) = name {
        map.insert("name".to_string(), Json::Str(name.to_string()));
    }
    if let Some(model) = model {
        map.insert("model".to_string(), Json::Str(model.to_string()));
    }
    if let Some(id) = device_id {
        map.insert("deviceId".to_string(), Json::Str(id.to_string()));
    }
    let tag_map: BTreeMap<String, Json> = tags
        .iter()
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .map(|(k, v)| (k.clone(), Json::Str(v.clone())))
        .collect();
    if !tag_map.is_empty() {
        map.insert("tags".to_string(), Json::Obj(tag_map));
    }
    Json::Obj(map)
}

/// Validate one pushed line (the /ingest path) and stamp `timestampNs`
/// with the server clock when the device did not supply one. Returns the
/// declared module (which routes the reading to its spool) and the
/// normalized single-line envelope.
pub fn normalize_pushed(line: &str, now_ns: i64) -> Option<(String, String)> {
    if line.len() > MAX_PUSHED_BYTES {
        return None;
    }
    let Ok(Json::Obj(mut map)) = json::parse(line) else {
        return None;
    };
    let module = match map.get("module") {
        Some(Json::Str(module)) if valid_module(module) => module.clone(),
        _ => return None,
    };
    match map.get("values") {
        Some(Json::Obj(values)) if !values.is_empty() => {}
        _ => return None,
    }
    match map.get("timestampNs") {
        Some(Json::Int(_)) => {}
        None => {
            map.insert("timestampNs".to_string(), Json::Int(now_ns));
        }
        Some(_) => return None,
    }
    Some((module, Json::Obj(map).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_envelope_shape() {
        let identity = device(
            "192.168.65.42",
            Some("Office Sensor"),
            Some("ENV3"),
            None,
            &[("room".into(), "lab".into())],
        );
        let values = json::parse(r#"{"temperature_c": 21.5}"#).unwrap();
        let built = reading("envSensors", identity, 42, values).unwrap();
        assert_eq!(
            built,
            r#"{"device":{"ip":"192.168.65.42","model":"ENV3","name":"Office Sensor","tags":{"room":"lab"}},"module":"envSensors","timestampNs":42,"values":{"temperature_c":21.5}}"#
        );
    }

    #[test]
    fn reading_requires_values() {
        let identity = device("192.168.65.42", None, None, None, &[]);
        assert!(reading("m", identity.clone(), 0, json::parse("{}").unwrap()).is_none());
        assert!(reading("m", identity, 0, Json::Str("not an object".into())).is_none());
    }

    #[test]
    fn pushed_envelopes_validate_and_get_stamped() {
        let (module, normalized) =
            normalize_pushed(r#"{"module": "push", "values": {"v": 42}}"#, 7).unwrap();
        assert_eq!(module, "push");
        assert_eq!(normalized, r#"{"module":"push","timestampNs":7,"values":{"v":42}}"#);
        // A supplied timestamp is kept.
        let (_, kept) =
            normalize_pushed(r#"{"module": "push", "timestampNs": 5, "values": {"v": 1}}"#, 7)
                .unwrap();
        assert!(kept.contains("\"timestampNs\":5"), "{kept}");

        assert!(normalize_pushed("not json", 0).is_none());
        assert!(normalize_pushed(r#"{"values": {"v": 1}}"#, 0).is_none());
        assert!(normalize_pushed(r#"{"module": "", "values": {"v": 1}}"#, 0).is_none());
        assert!(normalize_pushed(r#"{"module": "../push", "values": {"v": 1}}"#, 0).is_none());
        assert!(normalize_pushed(r#"{"module": "m", "values": {}}"#, 0).is_none());
        assert!(normalize_pushed(r#"{"module": "m", "values": 3}"#, 0).is_none());
        assert!(
            normalize_pushed(r#"{"module": "m", "timestampNs": "soon", "values": {"v": 1}}"#, 0)
                .is_none()
        );
        let oversized = format!(r#"{{"module": "m", "values": {{"v": "{}"}}}}"#, "x".repeat(70 * 1024));
        assert!(normalize_pushed(&oversized, 0).is_none());
    }

    #[test]
    fn module_names_stay_path_and_url_safe() {
        assert!(valid_module("envSensors"));
        assert!(valid_module("radio-433"));
        assert!(!valid_module(""));
        assert!(!valid_module("a/b"));
        assert!(!valid_module(".."));
        assert!(!valid_module(&"m".repeat(33)));
    }
}
