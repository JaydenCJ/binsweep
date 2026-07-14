//! Minimal recursive-descent JSON parser, just enough to read the manifests
//! binsweep cares about: cargo's `.crates2.json`, pipx's
//! `pipx_metadata.json` and npm's `package.json`. Std-only on purpose —
//! pulling in serde for three read-only files would be the heaviest part
//! of the binary.

use std::collections::BTreeMap;

/// A parsed JSON value. Objects use a `BTreeMap` so iteration order is
/// deterministic regardless of the input's key order.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    /// Member lookup on an object; `None` for other value kinds.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Object(map) => Some(map),
            _ => None,
        }
    }
}

/// Parse a complete JSON document. Trailing garbage is an error — a
/// truncated or concatenated manifest should never half-parse silently.
pub fn parse(text: &str) -> Result<Json, String> {
    let bytes = text.as_bytes();
    let mut pos = 0usize;
    let value = parse_value(bytes, &mut pos)?;
    skip_ws(bytes, &mut pos);
    if pos != bytes.len() {
        return Err(format!("trailing characters at byte {pos}"));
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
        None => Err("unexpected end of input".to_string()),
        Some(b'{') => parse_object(bytes, pos),
        Some(b'[') => parse_array(bytes, pos),
        Some(b'"') => Ok(Json::String(parse_string(bytes, pos)?)),
        Some(b't') => parse_literal(bytes, pos, "true", Json::Bool(true)),
        Some(b'f') => parse_literal(bytes, pos, "false", Json::Bool(false)),
        Some(b'n') => parse_literal(bytes, pos, "null", Json::Null),
        Some(_) => parse_number(bytes, pos),
    }
}

fn parse_literal(bytes: &[u8], pos: &mut usize, lit: &str, value: Json) -> Result<Json, String> {
    if bytes[*pos..].starts_with(lit.as_bytes()) {
        *pos += lit.len();
        Ok(value)
    } else {
        Err(format!("invalid literal at byte {}", *pos))
    }
}

fn parse_number(bytes: &[u8], pos: &mut usize) -> Result<Json, String> {
    let start = *pos;
    while *pos < bytes.len()
        && matches!(bytes[*pos], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
    {
        *pos += 1;
    }
    let text = std::str::from_utf8(&bytes[start..*pos]).map_err(|_| "bad number".to_string())?;
    text.parse::<f64>()
        .map(Json::Number)
        .map_err(|_| format!("invalid number '{text}' at byte {start}"))
}

fn parse_string(bytes: &[u8], pos: &mut usize) -> Result<String, String> {
    debug_assert_eq!(bytes[*pos], b'"');
    *pos += 1;
    let mut out = String::new();
    loop {
        match bytes.get(*pos) {
            None => return Err("unterminated string".to_string()),
            Some(b'"') => {
                *pos += 1;
                return Ok(out);
            }
            Some(b'\\') => {
                *pos += 1;
                let esc = bytes.get(*pos).ok_or("unterminated escape")?;
                match esc {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let cp = parse_hex4(bytes, *pos + 1)?;
                        // Surrogate pair: a high surrogate must be followed
                        // by \uDC00–\uDFFF, or the manifest is malformed.
                        if (0xD800..0xDC00).contains(&cp) {
                            if bytes.get(*pos + 5) != Some(&b'\\')
                                || bytes.get(*pos + 6) != Some(&b'u')
                            {
                                return Err("lone high surrogate".to_string());
                            }
                            let low = parse_hex4(bytes, *pos + 7)?;
                            if !(0xDC00..0xE000).contains(&low) {
                                return Err("invalid low surrogate".to_string());
                            }
                            let combined = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                            out.push(char::from_u32(combined).ok_or("invalid surrogate pair")?);
                            *pos += 10;
                        } else if (0xDC00..0xE000).contains(&cp) {
                            return Err("lone low surrogate".to_string());
                        } else {
                            out.push(char::from_u32(cp).ok_or("invalid \\u escape")?);
                            *pos += 4;
                        }
                    }
                    other => return Err(format!("invalid escape '\\{}'", *other as char)),
                }
                *pos += 1;
            }
            Some(_) => {
                // Consume one UTF-8 encoded character, not one byte.
                let rest = std::str::from_utf8(&bytes[*pos..])
                    .map_err(|_| "invalid UTF-8 in string".to_string())?;
                let ch = rest.chars().next().unwrap();
                out.push(ch);
                *pos += ch.len_utf8();
            }
        }
    }
}

fn parse_hex4(bytes: &[u8], at: usize) -> Result<u32, String> {
    let hex = bytes.get(at..at + 4).ok_or("truncated \\u escape")?;
    let text = std::str::from_utf8(hex).map_err(|_| "bad \\u escape".to_string())?;
    u32::from_str_radix(text, 16).map_err(|_| format!("bad \\u escape '{text}'"))
}

fn parse_array(bytes: &[u8], pos: &mut usize) -> Result<Json, String> {
    *pos += 1; // consume '['
    let mut items = Vec::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b']') {
        *pos += 1;
        return Ok(Json::Array(items));
    }
    loop {
        items.push(parse_value(bytes, pos)?);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => *pos += 1,
            Some(b']') => {
                *pos += 1;
                return Ok(Json::Array(items));
            }
            _ => return Err(format!("expected ',' or ']' at byte {}", *pos)),
        }
    }
}

fn parse_object(bytes: &[u8], pos: &mut usize) -> Result<Json, String> {
    *pos += 1; // consume '{'
    let mut map = BTreeMap::new();
    skip_ws(bytes, pos);
    if bytes.get(*pos) == Some(&b'}') {
        *pos += 1;
        return Ok(Json::Object(map));
    }
    loop {
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b'"') {
            return Err(format!("expected object key at byte {}", *pos));
        }
        let key = parse_string(bytes, pos)?;
        skip_ws(bytes, pos);
        if bytes.get(*pos) != Some(&b':') {
            return Err(format!("expected ':' at byte {}", *pos));
        }
        *pos += 1;
        let value = parse_value(bytes, pos)?;
        map.insert(key, value);
        skip_ws(bytes, pos);
        match bytes.get(*pos) {
            Some(b',') => *pos += 1,
            Some(b'}') => {
                *pos += 1;
                return Ok(Json::Object(map));
            }
            _ => return Err(format!("expected ',' or '}}' at byte {}", *pos)),
        }
    }
}

/// Escape a string for inclusion in JSON output.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalars() {
        assert_eq!(parse("null").unwrap(), Json::Null);
        assert_eq!(parse("true").unwrap(), Json::Bool(true));
        assert_eq!(parse("false").unwrap(), Json::Bool(false));
        assert_eq!(parse("42").unwrap(), Json::Number(42.0));
        assert_eq!(parse("-3.5e2").unwrap(), Json::Number(-350.0));
        assert_eq!(parse("\"hi\"").unwrap(), Json::String("hi".into()));
    }

    #[test]
    fn parses_nested_structures() {
        let doc = parse(r#"{"a": [1, {"b": null}], "c": "d"}"#).unwrap();
        assert_eq!(doc.get("c").unwrap().as_str(), Some("d"));
        let arr = doc.get("a").unwrap().as_array().unwrap();
        assert_eq!(arr[0], Json::Number(1.0));
        assert_eq!(arr[1].get("b"), Some(&Json::Null));
    }

    #[test]
    fn parses_empty_containers_and_whitespace() {
        assert_eq!(parse(" { } ").unwrap(), Json::Object(BTreeMap::new()));
        assert_eq!(parse("[\n]").unwrap(), Json::Array(vec![]));
    }

    #[test]
    fn decodes_string_escapes() {
        let doc = parse(r#""a\"b\\c\nd\teé""#).unwrap();
        assert_eq!(doc.as_str(), Some("a\"b\\c\nd\te\u{e9}"));
    }

    #[test]
    fn decodes_surrogate_pairs() {
        // A package description can legally contain an emoji.
        let doc = parse(r#""😀""#).unwrap();
        assert_eq!(doc.as_str(), Some("\u{1F600}"));
    }

    #[test]
    fn rejects_lone_surrogates() {
        assert!(parse(r#""\ud83d""#).is_err());
        assert!(parse(r#""\ude00""#).is_err());
    }

    #[test]
    fn handles_raw_multibyte_utf8() {
        let doc = parse("\"caf\u{e9} \u{1F980}\"").unwrap();
        assert_eq!(doc.as_str(), Some("caf\u{e9} \u{1F980}"));
    }

    #[test]
    fn rejects_trailing_garbage() {
        // Two concatenated documents means a corrupt manifest; refuse it.
        assert!(parse("{} {}").is_err());
        assert!(parse("1 2").is_err());
    }

    #[test]
    fn rejects_truncated_input() {
        assert!(parse("{\"a\": ").is_err());
        assert!(parse("[1, 2").is_err());
        assert!(parse("\"unterminated").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn rejects_bad_numbers_and_literals() {
        assert!(parse("1.2.3").is_err());
        assert!(parse("truthy").is_err());
        assert!(parse("nul").is_err());
    }

    #[test]
    fn object_keys_win_last_and_iterate_sorted() {
        let doc = parse(r#"{"z": 1, "a": 2, "z": 3}"#).unwrap();
        let keys: Vec<&String> = doc.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["a", "z"]);
        assert_eq!(doc.get("z"), Some(&Json::Number(3.0)));
    }

    #[test]
    fn escape_round_trips_through_parse() {
        let nasty = "he said \"hi\"\n\ttab\\slash\u{1}";
        let encoded = format!("\"{}\"", escape(nasty));
        assert_eq!(parse(&encoded).unwrap().as_str(), Some(nasty));
    }
}
