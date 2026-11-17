// Portions of this file are ported from kustomize api/types/fieldspec.go,
// api/filters/fieldspec/fieldspec.go, api/filters/fsslice/fsslice.go and
// kyaml/utils/pathsplitter.go, Copyright The Kubernetes Authors, licensed under
// Apache-2.0.
// Modified: translated from Go to Rust and adapted to this crate.

//! Field specs: "the field at this path, in resources of this kind".
//!
//! Every builtin transformer is configured by field specs: the namespace
//! transformer sets `metadata/namespace` on all kinds, plus the subjects of
//! RoleBindings, and so on. The default tables live in `src/builtin_config/`.
//!
//! [`apply`] runs a setter ([`crate::setters`]) at every field a spec selects:
//!
//! - The spec's gvk must match the resource (empty parts match anything).
//! - The path is split on `/` (`\/` escapes a slash inside a field name).
//! - A sequence met along the way applies the rest of the path to every
//!   element; `name[]` marks a field that holds a sequence.
//! - With `create`, missing maps along the path are created, and a missing
//!   leaf is created as [`CreateKind`]; a field that is present but null is
//!   turned into that kind too.
//! - The setter also runs on a leaf that exists but is null.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::resource::{Gvk, ResId};
use crate::{Error, Resource, Result};

/// One field spec. An empty group, version or kind matches any resource.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct FieldSpec {
    /// API group the spec applies to (`""` = any).
    pub group: String,
    /// API version the spec applies to (`""` = any).
    pub version: String,
    /// Kind the spec applies to (`""` = any).
    pub kind: String,
    /// `/`-separated path, such as `spec/template/metadata/labels`.
    pub path: String,
    /// Create missing fields along the path.
    #[serde(rename = "create")]
    pub create_if_not_present: bool,
}

impl FieldSpec {
    /// The spec's group, version and kind, as a selector.
    pub fn gvk(&self) -> Gvk {
        Gvk {
            group: self.group.clone(),
            version: self.version.clone(),
            kind: self.kind.clone(),
        }
    }

    /// Same path, and this spec's gvk is selected by `other`'s: the "primary
    /// key" upstream uses to de-duplicate (`fs.IsSelected(&other.Gvk)`).
    fn effectively_equals(&self, other: &FieldSpec) -> bool {
        self.path == other.path && self.gvk().is_selected(&other.gvk())
    }
}

/// An ordered list of field specs.
pub type FsSlice = Vec<FieldSpec>;

/// Merge `incoming` into `existing` (upstream `FsSlice.MergeAll`): specs already
/// present are skipped; one that differs only in `create` is a conflict.
pub fn merge_field_specs(existing: &[FieldSpec], incoming: &[FieldSpec]) -> Result<FsSlice> {
    let mut result = existing.to_vec();
    for spec in incoming {
        match result.iter().find(|s| s.effectively_equals(spec)) {
            Some(found) if found.create_if_not_present != spec.create_if_not_present => {
                return Err(Error::Invalid(format!(
                    "conflicting fieldspecs for path {}",
                    spec.path
                )));
            }
            Some(_) => {}
            None => result.push(spec.clone()),
        }
    }
    Ok(result)
}

/// The fields that hold references to resources of one kind
/// (`nameReference` entries): renaming such a resource updates these fields.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct NameBackReferences {
    /// Group of the referenced kind.
    pub group: String,
    /// Version of the referenced kind.
    pub version: String,
    /// The referenced kind.
    pub kind: String,
    /// Where referrers hold the name (the YAML key is `fieldSpecs`).
    #[serde(rename = "fieldSpecs")]
    pub referrers: FsSlice,
}

impl NameBackReferences {
    /// The referenced kind as a gvk.
    pub fn gvk(&self) -> Gvk {
        Gvk {
            group: self.group.clone(),
            version: self.version.clone(),
            kind: self.kind.clone(),
        }
    }
}

/// Merge name back-references: entries for the same gvk merge their referrers.
pub fn merge_name_references(
    existing: &[NameBackReferences],
    incoming: &[NameBackReferences],
) -> Result<Vec<NameBackReferences>> {
    let mut result = existing.to_vec();
    for other in incoming {
        if let Some(found) = result.iter_mut().find(|r| r.gvk() == other.gvk()) {
            found.referrers = merge_field_specs(&found.referrers, &other.referrers)?;
        } else {
            result.push(other.clone());
        }
    }
    Ok(result)
}

/// What to create for a missing leaf when a spec has `create: true`
/// (upstream's `CreateKind`; `None` means never create).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateKind {
    /// Don't create missing fields, even if the spec asks to.
    None,
    /// An empty scalar, for setters that write a value.
    Scalar,
    /// An empty map, for setters that add entries (labels, annotations).
    Map,
    /// An empty sequence.
    Sequence,
}

impl CreateKind {
    fn empty(self) -> Value {
        match self {
            CreateKind::Map => Value::Object(Map::new()),
            CreateKind::Sequence => Value::Array(Vec::new()),
            CreateKind::Scalar | CreateKind::None => Value::String(String::new()),
        }
    }
}

/// Split a field-spec path on `/`, where `\/` keeps a slash inside a field
/// name and a leading `/` is ignored (kyaml's `PathSplitter`).
pub fn split_path(path: &str) -> Vec<String> {
    let mut parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 1 && parts[0].is_empty() {
        parts.remove(0);
    }
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        match out.last_mut() {
            Some(last) if last.ends_with('\\') => {
                last.pop();
                last.push('/');
                last.push_str(part);
            }
            _ => out.push(part.to_string()),
        }
    }
    out
}

/// Apply `set` at every field `spec` selects in `resource`.
pub fn apply(
    spec: &FieldSpec,
    resource: &mut Resource,
    create: CreateKind,
    set: &mut dyn FnMut(&mut Value) -> Result<()>,
) -> Result<()> {
    apply_to_object(spec, resource.object_mut(), create, set)
}

/// [`apply`] on any object (upstream's filter works on plain YAML nodes).
pub fn apply_to_object(
    spec: &FieldSpec,
    object: &mut Map<String, Value>,
    create: CreateKind,
    set: &mut dyn FnMut(&mut Value) -> Result<()>,
) -> Result<()> {
    let id = ResId::of_object(object);
    if !id.gvk.is_selected(&spec.gvk()) {
        return Ok(());
    }
    let path = split_path(&spec.path);
    let mut root = Value::Object(std::mem::take(object));
    let result = walk(&mut root, &path, spec, create, set);
    if let Value::Object(taken) = root {
        *object = taken;
    }
    result.map_err(|e| {
        Error::Invalid(format!(
            "considering field '{}' of object {id}: {}",
            spec.path,
            strip_invalid(&e)
        ))
    })
}

/// Apply `set` for every spec in order (upstream's `fsslice.Filter`).
pub fn apply_all(
    specs: &[FieldSpec],
    resource: &mut Resource,
    create: CreateKind,
    set: &mut dyn FnMut(&mut Value) -> Result<()>,
) -> Result<()> {
    specs
        .iter()
        .try_for_each(|spec| apply(spec, resource, create, set))
}

fn strip_invalid(e: &Error) -> String {
    match e {
        Error::Invalid(m) => m.clone(),
        other => other.to_string(),
    }
}

fn walk(
    node: &mut Value,
    path: &[String],
    spec: &FieldSpec,
    create: CreateKind,
    set: &mut dyn FnMut(&mut Value) -> Result<()>,
) -> Result<()> {
    let Some(head) = path.first() else {
        return set(node);
    };
    match node {
        Value::Null => Ok(()),
        Value::Array(items) => items
            .iter_mut()
            .try_for_each(|item| walk(item, path, spec, create, set)),
        Value::Object(map) => {
            let (name, is_seq) = match head.strip_suffix("[]") {
                Some(short) => (short, true),
                None => (head.as_str(), false),
            };
            if name.is_empty() {
                return Err(Error::Invalid(
                    "cannot set or create an empty field name".to_string(),
                ));
            }
            let creating = spec.create_if_not_present && create != CreateKind::None && !is_seq;
            // The kind a missing or null field becomes (upstream `kind`).
            let kind = if !creating {
                is_seq.then_some(CreateKind::Sequence)
            } else if path.len() <= 1 {
                Some(create)
            } else {
                Some(CreateKind::Map)
            };
            let field = match map.get_mut(name) {
                Some(field) => field,
                None if creating => map
                    .entry(name.to_string())
                    .or_insert(kind.unwrap_or(create).empty()),
                None => return Ok(()),
            };
            if field.is_null()
                && let Some(kind) = kind
            {
                *field = kind.empty();
            }
            walk(field, &path[1..], spec, create, set)
        }
        _ => Err(Error::Invalid(
            "expected sequence or mapping node".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: &str, path: &str, create: bool) -> FieldSpec {
        FieldSpec {
            kind: kind.into(),
            path: path.into(),
            create_if_not_present: create,
            ..FieldSpec::default()
        }
    }

    #[test]
    fn deserializes_the_upstream_shape() {
        let s: FieldSpec = crate::yaml::from_str(
            "path: spec/selector\ncreate: true\nversion: v1\nkind: Service\n",
        )
        .unwrap();
        assert_eq!(s.gvk().kind, "Service");
        assert!(s.create_if_not_present);
    }

    #[test]
    fn merge_skips_duplicates_and_rejects_conflicting_create() {
        let base = vec![spec("", "metadata/labels", true)];
        let merged = merge_field_specs(
            &base,
            &[
                spec("", "metadata/labels", true),
                spec("Deployment", "spec/x", false),
            ],
        )
        .unwrap();
        assert_eq!(merged.len(), 2);
        assert!(merge_field_specs(&base, &[spec("", "metadata/labels", false)]).is_err());
    }

    #[test]
    fn an_incoming_wildcard_is_absorbed_by_an_existing_specific_spec() {
        // Upstream asks whether the existing spec is selected by the incoming
        // one, so the incoming spec acts as the selector.
        let any = spec("", "metadata/name", false);
        let service = spec("Service", "metadata/name", false);
        assert_eq!(
            merge_field_specs(std::slice::from_ref(&service), std::slice::from_ref(&any))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(merge_field_specs(&[any], &[service]).unwrap().len(), 2);
    }

    #[test]
    fn name_references_merge_referrers_per_kind() {
        let a = NameBackReferences {
            kind: "ConfigMap".into(),
            referrers: vec![spec("Pod", "spec/volumes/configMap/name", false)],
            ..Default::default()
        };
        let b = NameBackReferences {
            kind: "ConfigMap".into(),
            referrers: vec![spec("Job", "spec/volumes/configMap/name", false)],
            ..Default::default()
        };
        let merged = merge_name_references(&[a], &[b]).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].referrers.len(), 2);
    }

    // ---- the engine

    use crate::setters::set_scalar;
    use serde_json::{Value, json};

    #[test]
    fn split_path_handles_escapes_and_a_leading_slash() {
        assert_eq!(split_path("a/b/c"), ["a", "b", "c"]);
        assert_eq!(split_path("/a/b"), ["a", "b"]);
        assert_eq!(
            split_path("metadata/annotations/a.io\\/b"),
            ["metadata", "annotations", "a.io/b"]
        );
        assert_eq!(split_path("a\\/b\\/c/d"), ["a/b/c", "d"]);
        assert_eq!(split_path(""), [""]);
    }

    fn resource(v: Value) -> Resource {
        Resource::from_value(v).unwrap()
    }

    #[test]
    fn sequences_apply_the_rest_of_the_path_to_each_element() {
        let mut r = resource(json!({
            "kind": "Pod",
            "spec": {"containers": [{"name": "a", "image": "x"}, {"name": "b", "image": "y"}]}
        }));
        let s = spec("Pod", "spec/containers/image", false);
        apply(&s, &mut r, CreateKind::None, &mut set_scalar("z")).unwrap();
        assert_eq!(
            r.get(&["spec", "containers"]),
            Some(&json!([{"name": "a", "image": "z"}, {"name": "b", "image": "z"}]))
        );
    }

    #[test]
    fn create_makes_intermediate_maps_and_the_leaf_kind() {
        let mut r = resource(json!({"kind": "Deployment"}));
        let s = spec("", "spec/template/metadata/labels", true);
        apply(
            &s,
            &mut r,
            CreateKind::Map,
            &mut crate::setters::set_entry("app", "web", crate::setters::Tag::Str),
        )
        .unwrap();
        assert_eq!(
            r.get(&["spec", "template", "metadata", "labels"]),
            Some(&json!({"app": "web"}))
        );
    }

    #[test]
    fn without_create_missing_paths_are_left_alone() {
        let mut r = resource(json!({"kind": "Deployment"}));
        let s = spec("", "spec/template/metadata/labels", false);
        apply(&s, &mut r, CreateKind::Map, &mut |_| {
            panic!("setter must not run")
        })
        .unwrap();
        assert_eq!(r.to_value(), json!({"kind": "Deployment"}));
    }

    #[test]
    fn a_null_field_with_a_sequence_hint_becomes_an_empty_sequence() {
        let mut r = resource(json!({"kind": "X", "items": null}));
        let s = spec("", "items[]/name", false);
        apply(&s, &mut r, CreateKind::None, &mut set_scalar("v")).unwrap();
        assert_eq!(r.get(&["items"]), Some(&json!([])));
    }

    #[test]
    fn errors_name_the_field_and_object() {
        let mut r = resource(
            json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "c"}, "data": "flat"}),
        );
        let s = spec("", "data/key", false);
        let err = apply(&s, &mut r, CreateKind::None, &mut set_scalar("v"))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "considering field 'data/key' of object ConfigMap.v1.[noGrp]/c.[noNs]: expected sequence or mapping node"
        );
    }

    /// Upstream's table (api/filters/fieldspec/fieldspec_test.go), extracted to
    /// tests/fixtures/units by the import script. Compared as data.
    #[test]
    fn upstream_fieldspec_filter_table() {
        let doc: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/units/fieldspec_filter.json"
        ))
        .unwrap();
        let mut ran = 0;
        for case in doc["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            if case.get("unsupported").is_some() {
                continue;
            }
            let s: FieldSpec = crate::yaml::from_str(case["fieldSpec"].as_str().unwrap()).unwrap();
            let create = match case.get("createKind").and_then(Value::as_str) {
                Some("scalar") => CreateKind::Scalar,
                Some("mapping") => CreateKind::Map,
                Some("sequence") => CreateKind::Sequence,
                _ => CreateKind::None,
            };
            let docs = crate::yaml::documents(case["input"].as_str().unwrap()).unwrap();
            let mut object = docs[0].as_object().unwrap().clone();
            let text = case["setScalar"].as_str().unwrap();
            let result = apply_to_object(&s, &mut object, create, &mut set_scalar(text));
            match case.get("error").and_then(Value::as_str) {
                Some(want) => assert_eq!(result.unwrap_err().to_string(), want, "case {name}"),
                None => {
                    result.unwrap_or_else(|e| panic!("case {name}: {e}"));
                    let want = crate::yaml::documents(case["expected"].as_str().unwrap()).unwrap();
                    assert_eq!(Value::Object(object), want[0], "case {name}");
                }
            }
            ran += 1;
        }
        assert!(ran >= 20, "only {ran} upstream cases ran");
    }
}
