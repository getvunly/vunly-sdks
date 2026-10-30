//! Checking and reading response bodies without a derive macro: a 2xx must carry the fields the SDK promises, with the
//! right types, or the call is `invalid_response` (fail closed, never a zero value that looks like an answer).

use serde_json::Value;

/// The type of a promised field.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    Str,
    /// A string that may be `null` or absent.
    StrOpt,
    /// A whole number that fits an `i64`.
    Int,
    Bool,
    Array,
}

/// `("a.b", Kind)`: a dotted path into the body.
pub(crate) type Shape = &'static [(&'static str, Kind)];

fn lookup<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    let mut v = body;
    for part in path.split('.') {
        v = v.as_object()?.get(part)?;
    }
    Some(v)
}

/// Returns a description of the first problem, or `None` when the body has every promised field.
pub(crate) fn shape_problem(body: &Value, shape: Shape) -> Option<String> {
    if !body.is_object() {
        return Some("the body is not a JSON object".into());
    }
    for (path, kind) in shape {
        let v = lookup(body, path).filter(|v| !v.is_null());
        let Some(v) = v else {
            if matches!(kind, Kind::StrOpt) {
                continue;
            }
            return Some(format!("field \"{path}\" is missing"));
        };
        let good = match kind {
            Kind::Str | Kind::StrOpt => v.is_string(),
            Kind::Int => v.is_i64(),
            Kind::Bool => v.is_boolean(),
            Kind::Array => v.is_array(),
        };
        if !good {
            return Some(format!("field \"{path}\" has the wrong type"));
        }
    }
    None
}

/// Readers for fields that were already checked (or are optional): they never fail, they default.
pub(crate) trait Read {
    fn string(&self, key: &str) -> String;
    fn opt_string(&self, key: &str) -> Option<String>;
    fn int(&self, key: &str) -> i64;
    fn opt_int(&self, key: &str) -> Option<i64>;
    fn boolean(&self, key: &str) -> bool;
    fn child(&self, key: &str) -> &Value;
    fn strings(&self, key: &str) -> Vec<String>;
}

impl Read for Value {
    fn string(&self, key: &str) -> String {
        self.opt_string(key).unwrap_or_default()
    }
    fn opt_string(&self, key: &str) -> Option<String> {
        self.get(key).and_then(Value::as_str).map(str::to_owned)
    }
    fn int(&self, key: &str) -> i64 {
        self.opt_int(key).unwrap_or(0)
    }
    fn opt_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(Value::as_i64)
    }
    fn boolean(&self, key: &str) -> bool {
        self.get(key).and_then(Value::as_bool).unwrap_or(false)
    }
    fn child(&self, key: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.get(key).unwrap_or(&NULL)
    }
    fn strings(&self, key: &str) -> Vec<String> {
        self.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    }
}

/// Percent-encodes one URL path segment or query component (everything but the unreserved characters).
pub(crate) fn escape(s: &str) -> String {
    // `.` and `..` are unreserved, but as a whole path segment they mean "this directory" and "the parent": a server or
    // proxy that normalises the path would answer for another resource. Encode the dots.
    if s == "." || s == ".." {
        return s.replace('.', "%2E");
    }
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shape_checks_types_and_missing_fields() {
        let shape: Shape = &[("a", Kind::Str), ("b.c", Kind::Int), ("d", Kind::StrOpt)];
        assert_eq!(shape_problem(&json!({"a": "x", "b": {"c": 1}}), shape), None);
        assert_eq!(shape_problem(&json!({"a": "x", "b": {"c": 1}, "d": null}), shape), None);
        assert!(shape_problem(&json!({"b": {"c": 1}}), shape)
            .unwrap()
            .contains("\"a\" is missing"));
        assert!(shape_problem(&json!({"a": "x", "b": {"c": 1.5}}), shape)
            .unwrap()
            .contains("wrong type"));
        assert!(shape_problem(&json!({"a": "x", "b": {"c": "1"}}), shape).is_some());
        assert!(shape_problem(&json!([1]), shape).is_some());
    }

    #[test]
    fn integers_stay_integers() {
        let v: Value = serde_json::from_str(r#"{"n": 9007199254740993}"#).unwrap();
        assert_eq!(v.int("n"), 9_007_199_254_740_993);
        assert!(shape_problem(&v, &[("n", Kind::Int)]).is_none());
    }

    #[test]
    fn escape_encodes_everything_but_unreserved() {
        assert_eq!(escape("a b/c?d#é"), "a%20b%2Fc%3Fd%23%C3%A9");
        assert_eq!(escape("cus_1.2-3~"), "cus_1.2-3~");
    }
}
