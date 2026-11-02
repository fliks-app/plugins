//! JSON helpers for the mappings: JS `??`, `||` and "undefined is absent" on `serde_json::Value`.
use serde_json::{Map, Value, json};

pub fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

/// `v.k ?? null`.
pub fn nn(v: &Value, k: &str) -> Value {
    get(v, k).clone()
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

/// `v.k || null` for strings.
pub fn text_or_null(v: &Value, k: &str) -> Value {
    text(v, k).map_or(Value::Null, Value::from)
}

/// `v.k || null` for numbers: 0 is falsy.
pub fn num_or_null(v: &Value, k: &str) -> Value {
    match get(v, k) {
        Value::Number(n) if n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()) => {
            Value::Number(n.clone())
        }
        _ => Value::Null,
    }
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

/// `<base>/<size><path>` for a truthy path, else `null`.
pub fn image(base: &str, size: &str, v: &Value, k: &str) -> Value {
    text(v, k).map_or(Value::Null, |p| json!(format!("{base}/{size}{p}")))
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

/// `parseFloat`: the longest decimal prefix; `None` is NaN.
pub fn js_parse_float(s: &str) -> Option<f64> {
    let t = s.trim_start_matches(is_ws);
    let b = t.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let int = digits(i);
    i += int;
    let mut frac = 0;
    if b.get(i) == Some(&b'.') {
        frac = digits(i + 1);
        if int + frac > 0 {
            i += 1 + frac;
        }
    }
    if int + frac == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
        let exp = digits(i + 1 + sign);
        if exp > 0 {
            i += 1 + sign + exp;
        }
    }
    t[..i].parse().ok()
}
