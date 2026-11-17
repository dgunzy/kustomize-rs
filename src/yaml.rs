//! YAML parsing and rendering, in one place.
//!
//! Backed by serde-saphyr with `serde_json::Value` as the document type, using
//! the same scalar rules as flux9s: YAML 1.2 booleans (`yes`/`on` stay strings)
//! and kubectl-style octal (`0644` is 420). Like kustomize (go-yaml), a repeated
//! mapping key is not an error: the last one wins.
//!
//! Values don't keep scalar style, so `mode: 0644` renders as `mode: 420`. The
//! output is equal to kustomize's as data, not byte for byte (see SPEC.md,
//! "Output fidelity").

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn options() -> serde_saphyr::Options {
    serde_saphyr::options! {
        strict_booleans: true,
        legacy_octal_numbers: true,
        with_snippet: false,
        duplicate_keys: serde_saphyr::DuplicateKeyPolicy::LastWins,
    }
}

/// Parse one YAML document.
pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T, serde_saphyr::Error> {
    serde_saphyr::from_str_with_options(input, options())
}

/// Parse a `---`-separated stream into its documents, dropping empty ones.
pub fn documents(input: &str) -> Result<Vec<Value>, serde_saphyr::Error> {
    let docs: Vec<Value> = serde_saphyr::from_multiple_with_options(input, options())?;
    Ok(docs.into_iter().filter(|d| !d.is_null()).collect())
}

/// Render one value as a YAML document.
pub fn to_string<T: Serialize>(value: &T) -> Result<String, serde_saphyr::SerializeError> {
    serde_saphyr::to_string(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documents_skips_empty_documents() {
        let docs = documents("---\na: 1\n---\n---\nb: 2\n").unwrap();
        assert_eq!(docs, vec![json!({"a": 1}), json!({"b": 2})]);
    }

    #[test]
    fn duplicate_keys_last_wins_like_kustomize() {
        let v: Value = from_str("kind: A\nkind: B\n").unwrap();
        assert_eq!(v, json!({"kind": "B"}));
    }

    #[test]
    fn scalars_follow_yaml_1_2_and_kubectl_octal() {
        let v: Value = from_str("a: yes\nmode: 0644\n").unwrap();
        assert_eq!(v, json!({"a": "yes", "mode": 420}));
    }
}
