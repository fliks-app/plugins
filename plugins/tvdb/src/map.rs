//! JSON helpers for the mappings: JS `??`, `||` and "undefined is absent" on `serde_json::Value`.
use serde_json::{Map, Value};

pub fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

/// `v.k ?? null`.
pub fn nn(v: &Value, k: &str) -> Value {
    get(v, k).clone()
}

/// `v.k` as an image URL: TVDB answers a "missing artwork" placeholder for a work that has none.
pub fn image(v: &Value, k: &str) -> Value {
    match get(v, k) {
        Value::String(s) if s.is_empty() || s.contains("/images/missing/") => Value::Null,
        x => x.clone(),
    }
}

/// `v.k ?? default`.
pub fn or(v: &Value, k: &str, default: Value) -> Value {
    match get(v, k) {
        Value::Null => default,
        x => x.clone(),
    }
}

/// `v.k` kept only when the key exists (a JS `undefined` leaves the key out of the JSON).
pub fn put(out: &mut Map<String, Value>, key: &str, v: &Value, k: &str) {
    if let Some(x) = v.get(k) {
        out.insert(key.into(), x.clone());
    }
}

/// A non-empty string (JS truthy string).
pub fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    get(v, k).as_str().filter(|s| !s.is_empty())
}

pub fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// `date ? parseInt(date) : null`; NaN is `null` in JSON.
pub fn year_of(v: &Value, k: &str) -> Value {
    text(v, k)
        .and_then(js_parse_int)
        .map_or(Value::Null, Value::from)
}

/// `list.map(item => item[key])`: a missing member is `null`.
pub fn pluck(list: &[Value], key: &str) -> Vec<Value> {
    list.iter().map(|i| nn(i, key)).collect()
}

/// JS number ordering key; a missing or null score is 0.
pub fn score(v: &Value, k: &str) -> f64 {
    get(v, k).as_f64().unwrap_or(0.0)
}

pub fn str_lower_or(v: &Value, k: &str, default: &str) -> String {
    get(v, k)
        .as_str()
        .map_or_else(|| default.to_owned(), str::to_lowercase)
}

fn is_ws(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `parseInt(s, 10)`; `None` is NaN. A huge value saturates instead of reading as NaN.
pub fn js_parse_int(s: &str) -> Option<i64> {
    let s = s.trim_matches(is_ws);
    let (neg, digits) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    if end == 0 {
        return None;
    }
    let n: i64 = digits[..end].parse().unwrap_or(i64::MAX);
    Some(if neg { -n } else { n })
}
