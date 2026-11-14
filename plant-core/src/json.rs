//! Dependency-free JSON parser/serializer, just enough for parameter archives
//! and hierarchical plant exports. Integers are kept as `u64` so 64-bit seeds
//! survive a round trip without float truncation.

use crate::error::PlantError;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    UInt(u64),
    Int(i64),
    Float(f32),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::UInt(v) => Some(*v),
            Json::Int(v) if *v >= 0 => Some(*v as u64),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Json::Float(v) => Some(*v),
            Json::UInt(v) => Some(*v as f32),
            Json::Int(v) => Some(*v as f32),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn must_get(&self, key: &str) -> Result<&Json, PlantError> {
        self.get(key)
            .ok_or_else(|| PlantError::Json(format!("missing field '{key}'")))
    }
}

pub fn parse(input: &str) -> Result<Json, PlantError> {
    let bytes = input.as_bytes();
    let mut p = Parser { bytes, pos: 0 };
    p.skip_ws();
    let v = p.parse_value()?;
    p.skip_ws();
    if p.pos != bytes.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, msg: &str) -> PlantError {
        PlantError::Json(format!("json parse error at byte {}: {msg}", self.pos))
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self) -> Result<Json, PlantError> {
        self.skip_ws();
        if self.pos >= self.bytes.len() {
            return Err(self.err("unexpected end of input"));
        }
        match self.bytes[self.pos] {
            b'{' => self.parse_object(),
            b'[' => self.parse_array(),
            b'"' => Ok(Json::Str(self.parse_string()?)),
            b't' | b'f' => self.parse_bool(),
            b'n' => self.parse_null(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            c => Err(self.err(&format!("unexpected character '{}'", c as char))),
        }
    }

    fn parse_object(&mut self) -> Result<Json, PlantError> {
        self.pos += 1; // {
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected string key"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.next() != Some(b':') {
                return Err(self.err("expected ':'"));
            }
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b'}') => break,
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
        Ok(Json::Object(entries))
    }

    fn parse_array(&mut self) -> Result<Json, PlantError> {
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b']') => break,
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
        Ok(Json::Array(items))
    }

    fn parse_string(&mut self) -> Result<String, PlantError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            match self.next() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => break,
                Some(b'\\') => match self.next() {
                    Some(b'"') => out.push('"'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'/') => out.push('/'),
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(b'r') => out.push('\r'),
                    Some(b'b') => out.push('\u{0008}'),
                    Some(b'f') => out.push('\u{000C}'),
                    Some(b'u') => {
                        let mut code: u32 = 0;
                        for _ in 0..4 {
                            let c = self.next().ok_or_else(|| self.err("bad unicode escape"))?;
                            let d = (c as char).to_digit(16).ok_or_else(|| self.err("bad hex digit"))?;
                            code = code * 16 + d;
                        }
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        }
                    }
                    _ => return Err(self.err("bad escape")),
                },
                Some(b) => {
                    // UTF-8 multibyte: collect the full sequence.
                    if b < 0x80 {
                        out.push(b as char);
                    } else {
                        let start = self.pos - 1;
                        let len = if b >= 0xF0 { 4 } else if b >= 0xE0 { 3 } else { 2 };
                        while self.pos < start + len && self.pos < self.bytes.len() {
                            self.pos += 1;
                        }
                        let slice = &self.bytes[start..self.pos.min(start + len)];
                        out.push_str(std::str::from_utf8(slice).map_err(|_| self.err("bad utf-8"))?);
                    }
                }
            }
        }
        Ok(out)
    }

    fn parse_bool(&mut self) -> Result<Json, PlantError> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(Json::Bool(true))
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(Json::Bool(false))
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn parse_null(&mut self) -> Result<Json, PlantError> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(Json::Null)
        } else {
            Err(self.err("invalid literal"))
        }
    }

    fn parse_number(&mut self) -> Result<Json, PlantError> {
        let start = self.pos;
        let mut is_float = false;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' => self.pos += 1,
                b'.' | b'e' | b'E' | b'+' | b'-' => {
                    is_float = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("bad number"))?;
        if is_float {
            text.parse::<f32>()
                .map(Json::Float)
                .map_err(|_| self.err("bad float"))
        } else if text.starts_with('-') {
            text.parse::<i64>()
                .map(Json::Int)
                .map_err(|_| self.err("bad integer"))
        } else {
            text.parse::<u64>()
                .map(Json::UInt)
                .map_err(|_| self.err("bad integer"))
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let v = self.bytes.get(self.pos).copied();
        if v.is_some() {
            self.pos += 1;
        }
        v
    }
}

pub fn to_string(value: &Json) -> String {
    let mut out = String::new();
    write_value(value, &mut out, 0);
    out.push('\n');
    out
}

fn write_value(value: &Json, out: &mut String, indent: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::UInt(v) => out.push_str(&v.to_string()),
        Json::Int(v) => out.push_str(&v.to_string()),
        Json::Float(v) => out.push_str(&format_f32(*v)),
        Json::Str(s) => write_string(s, out),
        Json::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                push_indent(out, indent + 1);
                write_value(item, out, indent + 1);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push(']');
        }
        Json::Object(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            for (i, (k, v)) in entries.iter().enumerate() {
                push_indent(out, indent + 1);
                write_string(k, out);
                out.push_str(": ");
                write_value(v, out, indent + 1);
                if i + 1 < entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            push_indent(out, indent);
            out.push('}');
        }
    }
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// Shortest representation that round-trips through f32.
pub fn format_f32(v: f32) -> String {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15 {
        // Keep whole numbers explicit as floats with `.0`.
        format!("{v:.1}")
    } else {
        v.to_string()
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basic() {
        let doc = r#"{"a": 1.5, "b": "hi", "c": [true, null, -3], "seed": 18446744073709551615}"#;
        let v = parse(doc).unwrap();
        assert_eq!(v.get("a").unwrap().as_f32(), Some(1.5));
        assert_eq!(v.get("b").unwrap().as_str(), Some("hi"));
        assert_eq!(v.get("seed").unwrap().as_u64(), Some(u64::MAX));
        let again = parse(&to_string(&v)).unwrap();
        assert_eq!(v, again);
    }

    #[test]
    fn float_roundtrip_preserves_value() {
        for f in [0.1f32, 1.0 / 3.0, -7.25, 1337.0, std::f32::consts::PI] {
            let s = format_f32(f);
            assert_eq!(s.parse::<f32>().unwrap(), f);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("{,").is_err());
        assert!(parse("[1 2]").is_err());
        assert!(parse("nul").is_err());
    }
}
