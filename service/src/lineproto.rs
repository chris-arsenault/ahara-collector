//! InfluxDB line-protocol construction. Field names and escaping mirror the
//! house-sensors collectors exactly, so the TrueNAS pull job can write these
//! lines to the same buckets without translation.

pub enum FieldValue {
    Float(f64),
    Int(i64),
    Str(String),
}

fn escape_key(s: &str) -> String {
    // Measurement, tag keys/values, and field keys escape commas, spaces,
    // and equals signs.
    s.replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace(' ', "\\ ")
        .replace('=', "\\=")
}

fn escape_measurement(s: &str) -> String {
    s.replace('\\', "\\\\").replace(',', "\\,").replace(' ', "\\ ")
}

pub fn line(
    measurement: &str,
    tags: &[(String, String)],
    fields: &[(String, FieldValue)],
    timestamp_ns: i64,
) -> Option<String> {
    if fields.is_empty() {
        return None;
    }
    let mut out = escape_measurement(measurement);
    // Sorted tags are not required by the protocol but make output
    // deterministic and testable.
    let mut sorted_tags: Vec<&(String, String)> =
        tags.iter().filter(|(k, v)| !k.is_empty() && !v.is_empty()).collect();
    sorted_tags.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, value) in sorted_tags {
        out.push(',');
        out.push_str(&escape_key(key));
        out.push('=');
        out.push_str(&escape_key(value));
    }
    out.push(' ');
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&escape_key(key));
        out.push('=');
        match value {
            FieldValue::Float(f) => out.push_str(&format!("{f}")),
            FieldValue::Int(n) => out.push_str(&format!("{n}i")),
            FieldValue::Str(s) => {
                out.push('"');
                out.push_str(&s.replace('\\', "\\\\").replace('"', "\\\""));
                out.push('"');
            }
        }
    }
    out.push(' ');
    out.push_str(&timestamp_ns.to_string());
    Some(out)
}

/// Validate one externally supplied line (the push/ingest path). Cheap shape
/// checks only: non-empty measurement, a field section, no newline, bounded
/// length. The authoritative parser is InfluxDB itself downstream.
pub fn looks_like_line(candidate: &str) -> bool {
    if candidate.is_empty() || candidate.len() > 64 * 1024 {
        return false;
    }
    if candidate.contains('\n') || candidate.contains('\r') {
        return false;
    }
    if candidate.starts_with('#') || candidate.starts_with(' ') || candidate.starts_with(',') {
        return false;
    }
    // measurement[,tags] fields [timestamp]: split on unescaped, unquoted
    // spaces, then require a k=v fields section and a numeric timestamp when
    // one is present.
    let mut sections: Vec<&str> = Vec::new();
    let mut escaped = false;
    let mut in_quotes = false;
    let mut start = 0;
    for (i, c) in candidate.char_indices() {
        match c {
            '\\' => escaped = !escaped,
            '"' if !escaped => {
                in_quotes = !in_quotes;
                escaped = false;
            }
            ' ' if !escaped && !in_quotes => {
                sections.push(&candidate[start..i]);
                start = i + 1;
                if sections.len() > 2 {
                    return false;
                }
            }
            _ => escaped = false,
        }
    }
    sections.push(&candidate[start..]);
    match sections.as_slice() {
        [measurement, fields] => !measurement.is_empty() && fields.contains('='),
        [measurement, fields, timestamp] => {
            !measurement.is_empty() && fields.contains('=') && timestamp.parse::<i64>().is_ok()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_house_sensors_shape() {
        // Mirrors the wire shape asserted by house-sensors'
        // tests/test_environment_sensors.py.
        let built = line(
            "environment",
            &[
                ("device".into(), "Office Sensor".into()),
                ("ip".into(), "192.168.66.42".into()),
                ("room".into(), "office lab".into()),
            ],
            &[
                ("humidity".into(), FieldValue::Float(45.1)),
                ("pressure_pa".into(), FieldValue::Float(101325.0)),
                ("timestamp_iso".into(), FieldValue::Str("2026-06-30T03:00:00Z".into())),
            ],
            1_700_000_000_123_456_789,
        )
        .unwrap();
        assert_eq!(
            built,
            "environment,device=Office\\ Sensor,ip=192.168.66.42,room=office\\ lab \
             humidity=45.1,pressure_pa=101325,timestamp_iso=\"2026-06-30T03:00:00Z\" \
             1700000000123456789"
        );
    }

    #[test]
    fn skips_empty_fields_and_tags() {
        assert!(line("m", &[], &[], 0).is_none());
        let built = line(
            "m",
            &[("empty".into(), String::new())],
            &[("v".into(), FieldValue::Int(1))],
            5,
        )
        .unwrap();
        assert_eq!(built, "m v=1i 5");
    }

    #[test]
    fn validates_pushed_lines() {
        assert!(looks_like_line("m v=1i 5"));
        assert!(looks_like_line("environment,device=x humidity=45.1"));
        assert!(looks_like_line(r#"m note="a b c" 7"#));
        assert!(!looks_like_line(""));
        assert!(!looks_like_line("no_fields_here"));
        assert!(!looks_like_line("not a line"));
        assert!(!looks_like_line("m v=1i not_a_timestamp"));
        assert!(!looks_like_line("bad\nline v=1"));
        assert!(!looks_like_line("# comment"));
        assert!(!looks_like_line(&"x".repeat(70 * 1024)));
    }
}
