//! Minimal JSON: a recursive-descent parser and a serializer. The service is
//! dependency-free, so config files, device payloads, and API bodies all go
//! through this module. Integers and floats are kept apart because config
//! values (ports, byte counts) must round-trip exactly.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Numeric value as f64, accepting either numeric variant. Device
    /// payloads use whatever the firmware emitted.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(i) => Some(*i as f64),
            Json::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Obj(map) => Some(map),
            _ => None,
        }
    }

    pub fn to_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Json::Float(f) => {
                if f.is_finite() {
                    let _ = write!(out, "{f}");
                } else {
                    out.push_str("null");
                }
            }
            Json::Str(s) => write_escaped(out, s),
            Json::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Obj(map) => {
                out.push('{');
                for (i, (key, value)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_escaped(out, key);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_escaped(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn parse(input: &str) -> Result<Json, String> {
    let bytes = input.as_bytes();
    let mut pos = 0;
    let value = parse_value(bytes, &mut pos)?;
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err(format!("trailing data at byte {pos}"));
    }
    Ok(value)
}

fn skip_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && matches!(bytes[*pos], b' ' | b'\t' | b'\n' | b'\r') {
        *pos += 1;
    }
}

fn parse_value(bytes: &[u8], pos: &mut usize) -> Result<Json, String> {
    skip_ws(bytes, pos);
    match bytes.get(*pos) {
        None => Err("unexpected end of input".into()),
        Some(b'{') => parse_object(bytes, pos),
        Some(b'[') => parse_array(bytes, pos),
        Some(b'"') => Ok(Json::Str(parse_string(bytes, pos)?)),
        Some(b't') => parse_literal(bytes, pos, "true", Json::Bool(true)),
        Some(b'f') => parse_literal(bytes, pos, "false", Json::Bool(false)),
        Some(b'n') => parse_literal(bytes, pos, "null", Json::Null),
        Some(_) => parse_number(bytes, pos),
    }
}

fn parse_literal(bytes: &[u8], pos: &mut usize, word: &str, value: Json) -> Result<Json, String> {
    if bytes[*pos..].starts_with(word.as_bytes()) {
        *pos += word.len();
        Ok(value)
    } else {
        Err(format!("invalid literal at byte {}", *pos))
    }
}

fn parse_number(bytes: &[u8], pos: &mut usize) -> Result<Json, String> {
    let start = *pos;
    if bytes.get(*pos) == Some(&b'-') {
        *pos += 1;
    }
    let mut is_float = false;
    while let Some(&b) = bytes.get(*pos) {
        match b {
            b'0'..=b'9' => *pos += 1,
            b'.' | b'e' | b'E' | b'+' | b'-' => {
                is_float = true;
                *pos += 1;
            }
            _ => break,
        }
    }
    let text = std::str::from_utf8(&bytes[start..*pos]).map_err(|_| "bad number".to_string())?;
    if text.is_empty() || text == "-" {
        return Err(format!("invalid number at byte {start}"));
    }
    if is_float {
        text.parse::<f64>()
            .map(Json::Float)
            .map_err(|_| format!("invalid number {text:?}"))
    } else {
        text.parse::<i64>()
            .map(Json::Int)
            // Integers beyond i64 (unlikely in any device payload) degrade to
            // float rather than failing the whole document.
            .or_else(|_| {
                text.parse::<f64>()
                    .map(Json::Float)
                    .map_err(|_| format!("invalid number {text:?}"))
            })
    }
}

fn parse_string(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    if bytes.get(*pos) != Some(&b'"') {
        return Err(format!("expected string at byte {}", *pos));
    }
    *pos += 1;
    let mut out = String::new();
    loop {
        match bytes.get(*pos) {
            None => return Err("unterminated string".into()),
            Some(b'"') => {
                *pos += 1;
                return Ok(out);
            }
            Some(b'\\') => {
                *pos += 1;
                match bytes.get(*pos) {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'b') => out.push('\u{0008}'),
                    Some(b'f') => out.push('\u{000c}'),
                    Some(b'n') => out.push('\n'),
                    Some(b'r') => out.push('\r'),
                    Some(b't') => out.push('\t'),
                    Some(b'u') => {
                        let hex = bytes
                            .get(*pos + 1..*pos + 5)
                            .ok_or("truncated \\u escape".to_string())?;
                        let hex =
                            std::str::from_utf8(hex).map_err(|_| "bad \\u escape".to_string())?;
                        let code =
                            u32::from_str_radix(hex, 16).map_err(|_| "bad \\u escape".to_string())?;
                        *pos += 4;
                        // Surrogate pairs: read the low half when present;
                        // lone surrogates become the replacement character.
                        if (0xd800..0xdc00).contains(&code) {
                            if bytes.get(*pos + 1..*pos + 3) == Some(b"\\u") {
                                let low_hex = bytes
                                    .get(*pos + 3..*pos + 7)
                                    .ok_or("truncated surrogate".to_string())?;
                                let low_hex = std::str::from_utf8(low_hex)
                                    .map_err(|_| "bad surrogate".to_string())?;
                                let low = u32::from_str_radix(low_hex, 16)
                                    .map_err(|_| "bad surrogate".to_string())?;
                                if (0xdc00..0xe000).contains(&low) {
                                    let combined =
                                        0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                                    out.push(char::from_u32(combined).unwrap_or('\u{fffd}'));
                                    *pos += 6;
                                } else {
                                    out.push('\u{fffd}');
                                }
                            } else {
                                out.push('\u{fffd}');
                            }
                        } else {
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                    }
                    _ => return Err("invalid escape".into()),
                }
                *pos += 1;
            }
            Some(_) => {
                // Consume one UTF-8 scalar: find the char boundary.
                let rest = std::str::from_utf8(&bytes[*pos..])
                    .map_err(|_| "invalid utf-8 in string".to_string())?;
                let c = rest.chars().next().ok_or("unterminated string".to_string())?;
                out.push(c);
                *pos += c.len_utf8();
            }
        }
    }
}

fn parse_array(bytes: &[u8], pos: &mut usize) -> Result<Json, String> {
    *pos += 1; // consume [
    let mut items = Vec::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b']') {
        *pos += 1;
        return Ok(Json::Arr(items));
    }
    loop {
        items.push(parse_value(bytes, pos)?);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => {
                *pos += 1;
            }
            Some(b']') => {
                *pos += 1;
                return Ok(Json::Arr(items));
            }
            _ => return Err(format!("expected , or ] at byte {}", *pos)),
        }
    }
}

fn parse_object(bytes: &[u8], pos: &mut usize) -> Result<Json, String> {
    *pos += 1; // consume {
    let mut map = BTreeMap::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b'}') {
        *pos += 1;
        return Ok(Json::Obj(map));
    }
    loop {
        skip_ws(bytes, pos);
        let key = parse_string(bytes, pos)?;
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b':') {
            return Err(format!("expected : at byte {}", *pos));
        }
        *pos += 1;
        let value = parse_value(bytes, pos)?;
        map.insert(key, value);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => {
                *pos += 1;
            }
            Some(b'}') => {
                *pos += 1;
                return Ok(Json::Obj(map));
            }
            _ => return Err(format!("expected , or }} at byte {}", *pos)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars() {
        assert_eq!(parse("null").unwrap(), Json::Null);
        assert_eq!(parse("true").unwrap(), Json::Bool(true));
        assert_eq!(parse("-42").unwrap(), Json::Int(-42));
        assert_eq!(parse("2.5").unwrap(), Json::Float(2.5));
        assert_eq!(parse("\"a\\nb\"").unwrap(), Json::Str("a\nb".into()));
    }

    #[test]
    fn parses_nested_structures() {
        let doc = parse(r#"{"a": [1, {"b": "c"}], "d": false}"#).unwrap();
        assert_eq!(doc.get("d"), Some(&Json::Bool(false)));
        let arr = doc.get("a").unwrap().as_arr().unwrap();
        assert_eq!(arr[0], Json::Int(1));
        assert_eq!(arr[1].get("b").unwrap().as_str(), Some("c"));
    }

    #[test]
    fn round_trips_through_serializer() {
        let text = r#"{"a":[1,2.5,"x\"y"],"b":null,"c":{"d":true}}"#;
        let doc = parse(text).unwrap();
        assert_eq!(parse(&doc.to_string()).unwrap(), doc);
    }

    #[test]
    fn handles_unicode_escapes() {
        assert_eq!(parse(r#""A""#).unwrap(), Json::Str("A".into()));
        assert_eq!(
            parse(r#""😀""#).unwrap(),
            Json::Str("\u{1f600}".into())
        );
        assert_eq!(parse(r#""\ud800x""#).unwrap(), Json::Str("\u{fffd}x".into()));
    }

    #[test]
    fn rejects_malformed_documents() {
        assert!(parse("{").is_err());
        assert!(parse("[1,]").is_err());
        assert!(parse("{\"a\" 1}").is_err());
        assert!(parse("12 34").is_err());
        assert!(parse("").is_err());
    }
}
