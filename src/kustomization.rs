//! The `kustomization.yaml` file.
//!
//! Fields the engine implements are typed. Every other field lands in `rest`,
//! and [`Kustomization::unsupported_fields`] names them so a build fails with
//! [`Error::Unsupported`](crate::Error::Unsupported) instead of quietly ignoring
//! them. Implementing a field means moving it from `rest` to a typed field.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::fs::FileSystem;
use crate::{Error, Result, yaml};

/// File names kustomize recognises, in the order it looks for them.
pub const FILE_NAMES: [&str; 3] = ["kustomization.yaml", "kustomization.yml", "Kustomization"];

/// `kind` of a regular kustomization.
pub const KIND_KUSTOMIZATION: &str = "Kustomization";
/// `kind` of a component.
pub const KIND_COMPONENT: &str = "Component";

/// Fields that never change the output.
const IGNORED_FIELDS: [&str; 1] = ["metadata"];

/// A parsed kustomization file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Kustomization {
    /// `apiVersion`, optional in kustomization files.
    #[serde(default)]
    pub api_version: Option<String>,
    /// `Kustomization` (the default) or `Component`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Files, directories and (later) URLs to include.
    #[serde(default)]
    pub resources: Vec<String>,
    /// Every field the engine doesn't implement yet.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Kustomization {
    /// Find and parse the kustomization file in `dir`. Returns the file's path
    /// too, for error messages.
    pub fn load(fs: &dyn FileSystem, dir: &Path) -> Result<(Self, PathBuf)> {
        let found: Vec<PathBuf> = FILE_NAMES
            .iter()
            .map(|name| dir.join(name))
            .filter(|p| fs.is_file(p))
            .collect();
        let path = match found.as_slice() {
            [one] => one.clone(),
            [] => {
                return Err(Error::Invalid(format!(
                    "unable to find one of {} in directory '{}'",
                    FILE_NAMES.map(|n| format!("'{n}'")).join(", "),
                    dir.display()
                )));
            }
            _ => {
                return Err(Error::Invalid(format!(
                    "found multiple kustomization files under: {}",
                    dir.display()
                )));
            }
        };
        let bytes = fs.read(&path)?;
        let text = String::from_utf8(bytes).map_err(|e| Error::parse(&path, e))?;
        // Through a Value first, so a repeated key resolves last-wins (as in
        // kustomize) before serde's struct handling, which rejects duplicates.
        let value: Value = yaml::from_str(&text).map_err(|e| Error::parse(&path, e))?;
        if value.is_null() {
            return Ok((Self::default(), path));
        }
        let kustomization = serde_json::from_value(value).map_err(|e| Error::parse(&path, e))?;
        Ok((kustomization, path))
    }

    /// Whether this file is a `Component`.
    pub fn is_component(&self) -> bool {
        self.kind.as_deref() == Some(KIND_COMPONENT)
    }

    /// Fields that are set but not implemented yet.
    pub fn unsupported_fields(&self) -> impl Iterator<Item = &str> {
        self.rest
            .iter()
            .filter(|(key, value)| !IGNORED_FIELDS.contains(&key.as_str()) && !is_empty(value))
            .map(|(key, _)| key.as_str())
    }
}

/// An empty list, map, string or null is the same as the field being absent.
fn is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemoryFs;

    #[test]
    fn unknown_fields_are_reported_as_unsupported() {
        let mut fs = MemoryFs::default();
        fs.insert(
            "kustomization.yaml",
            "resources: [a.yaml]\nnamePrefix: x-\ncommonLabels: {}\nmetadata: {name: k}\n",
        );
        let (k, _) = Kustomization::load(&fs, Path::new("")).unwrap();
        assert_eq!(k.resources, ["a.yaml"]);
        assert_eq!(k.unsupported_fields().collect::<Vec<_>>(), ["namePrefix"]);
    }

    #[test]
    fn missing_and_ambiguous_files_are_errors() {
        let mut fs = MemoryFs::default();
        fs.insert("other.yaml", "");
        assert!(Kustomization::load(&fs, Path::new("")).is_err());
        fs.insert("kustomization.yaml", "");
        assert!(Kustomization::load(&fs, Path::new("")).is_ok());
        fs.insert("kustomization.yml", "");
        assert!(Kustomization::load(&fs, Path::new("")).is_err());
    }
}
