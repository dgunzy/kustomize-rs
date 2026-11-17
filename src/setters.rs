// Portions of this file are ported from kustomize api/filters/filtersutil/setters.go
// and kyaml/yaml/{fns,compatibility}.go (FieldSetter, IsValueNonString),
// Copyright The Kubernetes Authors, licensed under Apache-2.0.
// Modified: translated from Go to Rust and adapted to this crate.

//! Setters: what a transformer does at each field a field spec selects.
//!
//! kustomize sets every value from a string and lets YAML quoting decide its
//! type. Without quoting styles in `serde_json::Value`, these setters
//! reconstruct the same outcome:
//!
//! - A new scalar is a string. kustomize quotes it whenever YAML 1.1 would read
//!   it as something else (`8080`, `true`, `yes`), so it stays a string.
//! - Replacing a scalar keeps the old node's quoting. An old string that only
//!   parses as a string when quoted keeps the new value a string. Otherwise
//!   the new value is written plain, so it gets whatever type YAML gives it
//!   (`replicas: 3` set to `5` stays a number).
//! - A value with an explicit non-string tag (such as `!!int`) is always plain.

use serde_json::Value;

use crate::{Error, Result, yaml};

/// The YAML tag a setter gives a new scalar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tag {
    /// No tag (`NodeTagEmpty`): a string, quoted when it would read as another type.
    #[default]
    Untagged,
    /// `!!str`: always a string.
    Str,
    /// Any other tag (`!!int`, `!!bool`, ...): written plain.
    Typed,
}

/// A function applied at each selected field.
pub type SetFn<'a> = dyn FnMut(&mut Value) -> Result<()> + 'a;

/// Replace the selected scalar with `text` (`filtersutil.SetScalar`).
pub fn set_scalar(text: &str) -> impl FnMut(&mut Value) -> Result<()> + '_ {
    move |node| replace_scalar(node, text, Tag::Untagged)
}

/// Replace the selected scalar with `text`, unless it already has a non-empty
/// value (`SetScalarIfEmpty`).
pub fn set_scalar_if_empty(text: &str) -> impl FnMut(&mut Value) -> Result<()> + '_ {
    move |node| {
        if has_value(node) {
            return Ok(());
        }
        replace_scalar(node, text, Tag::Untagged)
    }
}

/// Set `key` to `text` in the selected map (`filtersutil.SetEntry`).
pub fn set_entry<'a>(
    key: &'a str,
    text: &'a str,
    tag: Tag,
) -> impl FnMut(&mut Value) -> Result<()> + 'a {
    move |node| set_map_entry(node, key, text, tag)
}

/// Set `key` to `text` in the selected map, unless it already has a non-empty
/// value (`SetEntryIfEmpty`).
pub fn set_entry_if_empty<'a>(
    key: &'a str,
    text: &'a str,
    tag: Tag,
) -> impl FnMut(&mut Value) -> Result<()> + 'a {
    move |node| {
        if node.get(key).is_some_and(has_value) {
            return Ok(());
        }
        set_map_entry(node, key, text, tag)
    }
}

/// `FieldSetter` with no name: the node itself becomes the new scalar, keeping
/// the old node's quoting.
pub fn replace_scalar(node: &mut Value, text: &str, tag: Tag) -> Result<()> {
    if node.is_object() || node.is_array() {
        return Err(Error::Invalid(format!(
            "wrong node kind: expected scalar but got {}",
            kind_name(node)
        )));
    }
    let was_quoted = matches!(node, Value::String(old) if is_yaml11_non_string(old));
    *node = typed_scalar(text, tag, was_quoted);
    Ok(())
}

/// `FieldSetter` with a name: set a map entry, keeping an existing entry's
/// quoting; a new entry is quoted when needed.
pub fn set_map_entry(node: &mut Value, key: &str, text: &str, tag: Tag) -> Result<()> {
    let Value::Object(map) = node else {
        return Err(Error::Invalid(format!(
            "wrong node kind: expected mapping but got {}",
            kind_name(node)
        )));
    };
    let quoted = match map.get(key) {
        Some(Value::String(old)) => is_yaml11_non_string(old),
        Some(_) => false,
        // A new field: kustomize quotes a string-ish value that would read as
        // another type.
        None => tag != Tag::Typed && is_yaml11_non_string(text),
    };
    map.insert(key.to_string(), typed_scalar(text, tag, quoted));
    Ok(())
}

/// The value a scalar written with this tag and quoting reads back as.
fn typed_scalar(text: &str, tag: Tag, quoted: bool) -> Value {
    if tag == Tag::Str || (quoted && tag == Tag::Untagged) {
        return Value::String(text.to_string());
    }
    plain_scalar(text)
}

/// What a plain (unquoted) scalar reads back as. Text the emitter couldn't
/// write plainly (empty, or parsing as a map/list) is quoted, so it's a string.
pub fn plain_scalar(text: &str) -> Value {
    if text.is_empty() {
        return Value::String(String::new());
    }
    match yaml::from_str::<Value>(text) {
        Ok(v @ (Value::Bool(_) | Value::Number(_) | Value::Null)) => v,
        _ => Value::String(text.to_string()),
    }
}

/// Whether a YAML 1.1 parser reads `value` as something other than a string
/// (kyaml's `IsValueNonString`): booleans like `yes`/`on`, nulls, numbers in
/// 1.1 forms (octal, hex, binary, underscores, sexagesimal), and floats.
pub fn is_yaml11_non_string(value: &str) -> bool {
    if value.is_empty() || value.contains('\n') {
        return false;
    }
    const KEYWORDS: [&str; 38] = [
        "~", "null", "Null", "NULL", "y", "Y", "yes", "Yes", "YES", "true", "True", "TRUE", "on",
        "On", "ON", "n", "N", "no", "No", "NO", "false", "False", "FALSE", "off", "Off", "OFF",
        ".nan", ".NaN", ".NAN", ".inf", ".Inf", ".INF", "+.inf", "+.Inf", "+.INF", "-.inf",
        "-.Inf", "-.INF",
    ];
    if KEYWORDS.contains(&value) {
        return true;
    }
    is_yaml11_int(value) || is_yaml11_float(value)
}

fn strip_sign(s: &str) -> &str {
    s.strip_prefix(['-', '+']).unwrap_or(s)
}

fn all_digits(s: &str, radix: u32) -> bool {
    !s.is_empty() && s.chars().all(|c| c == '_' || c.is_digit(radix)) && s.chars().any(|c| c != '_')
}

fn is_yaml11_int(value: &str) -> bool {
    let s = strip_sign(value);
    if let Some(bin) = s.strip_prefix("0b") {
        return all_digits(bin, 2);
    }
    if let Some(hex) = s.strip_prefix("0x") {
        return all_digits(hex, 16);
    }
    if s.contains(':') {
        // Sexagesimal: 190:20:30
        let mut parts = s.split(':');
        let first = parts.next().unwrap_or_default();
        return all_digits(first, 10)
            && !first.starts_with('0')
            && parts
                .all(|p| !p.is_empty() && p.len() <= 2 && p.chars().all(|c| c.is_ascii_digit()));
    }
    all_digits(s, 10)
}

fn is_yaml11_float(value: &str) -> bool {
    let s = strip_sign(value);
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    if let Some(exp) = exponent {
        let exp = strip_sign(exp);
        if exp.is_empty() || !exp.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    // YAML 1.1 floats need a dot: [-+]?([0-9][0-9_]*)?\.[0-9.]*
    let Some((int, frac)) = mantissa.split_once('.') else {
        return false;
    };
    (int.is_empty() || all_digits(int, 10))
        && frac
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '_')
}

/// kustomize's `hasExistingValue` for a scalar: present and non-empty.
fn has_value(node: &Value) -> bool {
    match node {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Object(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => true,
    }
}

fn kind_name(node: &Value) -> &'static str {
    match node {
        Value::Object(_) => "MappingNode",
        Value::Array(_) => "SequenceNode",
        _ => "ScalarNode",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yaml11_non_strings_match_kyaml() {
        for s in [
            "8080",
            "-1",
            "0644",
            "0x1F",
            "0b101",
            "1_000",
            "190:20:30",
            "1.5",
            ".5",
            "6.8523015e+5",
            "yes",
            "ON",
            "~",
            ".inf",
        ] {
            assert!(
                is_yaml11_non_string(s),
                "{s} should be non-string in YAML 1.1"
            );
        }
        for s in [
            "",
            "foo",
            "1.2.3a",
            "v1",
            "1e3",
            "0x",
            "12:",
            "a:b",
            "multi\nline",
        ] {
            assert!(
                !is_yaml11_non_string(s),
                "{s:?} should be a string in YAML 1.1"
            );
        }
    }

    #[test]
    fn new_entries_are_strings_even_when_they_look_like_numbers() {
        let mut m = json!({});
        set_map_entry(&mut m, "port", "8080", Tag::Untagged).unwrap();
        set_map_entry(&mut m, "on", "yes", Tag::Untagged).unwrap();
        set_map_entry(&mut m, "name", "web", Tag::Untagged).unwrap();
        set_map_entry(&mut m, "replicas", "3", Tag::Typed).unwrap();
        assert_eq!(
            m,
            json!({"port": "8080", "on": "yes", "name": "web", "replicas": 3})
        );
    }

    #[test]
    fn replacing_keeps_the_old_quoting() {
        // Plain number stays plain: the new text gets its YAML type.
        let mut n = json!(3);
        replace_scalar(&mut n, "5", Tag::Untagged).unwrap();
        assert_eq!(n, json!(5));
        // A string that had to be quoted stays quoted.
        let mut n = json!("8080");
        replace_scalar(&mut n, "9090", Tag::Untagged).unwrap();
        assert_eq!(n, json!("9090"));
        // A plain string replaced by number-like text reads back as a number.
        let mut n = json!("foo");
        replace_scalar(&mut n, "123", Tag::Untagged).unwrap();
        assert_eq!(n, json!(123));
        // !!str always wins.
        let mut n = json!("foo");
        replace_scalar(&mut n, "123", Tag::Str).unwrap();
        assert_eq!(n, json!("123"));
        // Created (null) leaves are plain.
        let mut n = Value::Null;
        replace_scalar(&mut n, "e", Tag::Untagged).unwrap();
        assert_eq!(n, json!("e"));
    }

    #[test]
    fn setting_a_scalar_on_a_map_is_an_error() {
        assert!(replace_scalar(&mut json!({"a": 1}), "x", Tag::Untagged).is_err());
        assert!(set_map_entry(&mut json!("s"), "k", "v", Tag::Untagged).is_err());
    }

    #[test]
    fn if_empty_setters_leave_existing_values() {
        let mut m = json!({"a": "keep", "b": ""});
        set_entry_if_empty("a", "new", Tag::Untagged)(&mut m).unwrap();
        set_entry_if_empty("b", "new", Tag::Untagged)(&mut m).unwrap();
        set_entry_if_empty("c", "new", Tag::Untagged)(&mut m).unwrap();
        assert_eq!(m, json!({"a": "keep", "b": "new", "c": "new"}));
        let mut s = json!("x");
        set_scalar_if_empty("y")(&mut s).unwrap();
        assert_eq!(s, json!("x"));
    }
}
