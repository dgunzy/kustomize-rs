// Portions of this file are ported from kustomize kyaml/resid/{gvk,resid}.go,
// api/resource/resource.go and api/resmap/reswrangler.go, Copyright The
// Kubernetes Authors, licensed under Apache-2.0.
// Modified: translated from Go to Rust and adapted to this crate.

//! Resources, their identity, and the ordered set a build produces.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::{Error, Result, schema, yaml};

/// The namespace an object without one lives in.
pub const DEFAULT_NAMESPACE: &str = "default";
/// The effective namespace of a cluster-scoped id; matches no real namespace.
pub const TOTALLY_NOT_A_NAMESPACE: &str = "_non_namespaceable_";

/// Group, version and kind. The core group is `""`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Gvk {
    /// API group, `""` for core.
    pub group: String,
    /// API version.
    pub version: String,
    /// Kind.
    pub kind: String,
}

impl Gvk {
    /// Split an `apiVersion` (`apps/v1`, `v1`) at its first `/`, and add a kind.
    pub fn from_api_version(api_version: &str, kind: &str) -> Self {
        let (group, version) = api_version.split_once('/').unwrap_or(("", api_version));
        Self {
            group: group.to_string(),
            version: version.to_string(),
            kind: kind.to_string(),
        }
    }

    /// The `apiVersion` string: `group/version`, or `version` for the core group.
    pub fn api_version(&self) -> String {
        if self.group.is_empty() {
            self.version.clone()
        } else {
            format!("{}/{}", self.group, self.version)
        }
    }

    /// Whether the built-in schema says this kind is cluster-scoped. Unknown
    /// kinds (custom resources) count as namespaced.
    pub fn is_cluster_scoped(&self) -> bool {
        schema::is_cluster_scoped(&self.api_version(), &self.kind)
    }

    /// Whether this matches `selector`, where an empty selector part matches
    /// anything.
    pub fn is_selected(&self, selector: &Gvk) -> bool {
        (selector.group.is_empty() || self.group == selector.group)
            && (selector.version.is_empty() || self.version == selector.version)
            && (selector.kind.is_empty() || self.kind == selector.kind)
    }
}

impl fmt::Display for Gvk {
    /// `Kind.version.group`, with placeholders for empty parts, as kustomize
    /// prints it in errors.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let or = |s: &str, placeholder: &'static str| {
            if s.is_empty() {
                placeholder.to_string()
            } else {
                s.to_string()
            }
        };
        write!(
            f,
            "{}.{}.{}",
            or(&self.kind, "[noKind]"),
            or(&self.version, "[noVer]"),
            or(&self.group, "[noGrp]")
        )
    }
}

/// A resource's identity: kind plus namespaced name.
///
/// Compare ids with [`ResId::equals`], not `==`: kustomize treats a missing
/// namespace and `default` as the same, and ignores the namespace of
/// cluster-scoped kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ResId {
    /// Group, version and kind.
    pub gvk: Gvk,
    /// `metadata.namespace`, `""` when unset.
    pub namespace: String,
    /// `metadata.name`.
    pub name: String,
}

impl ResId {
    /// The namespace this id really lives in: a sentinel for cluster-scoped
    /// kinds, `default` for no namespace, otherwise the namespace.
    pub fn effective_namespace(&self) -> &str {
        if self.gvk.is_cluster_scoped() {
            TOTALLY_NOT_A_NAMESPACE
        } else if self.namespace.is_empty() || self.namespace == DEFAULT_NAMESPACE {
            DEFAULT_NAMESPACE
        } else {
            &self.namespace
        }
    }

    /// Same kind, name and effective namespace.
    pub fn equals(&self, other: &ResId) -> bool {
        self.gvkn_equals(other) && self.effective_namespace() == other.effective_namespace()
    }

    /// Same kind and name, ignoring the namespace.
    pub fn gvkn_equals(&self, other: &ResId) -> bool {
        self.name == other.name && self.gvk == other.gvk
    }
}

impl ResId {
    /// The id of any object, read from `apiVersion`, `kind` and `metadata`.
    pub fn of_object(object: &Map<String, Value>) -> ResId {
        let s = |path: &[&str]| {
            let mut cur = object.get(path[0]);
            for key in &path[1..] {
                cur = cur.and_then(|v| v.get(key));
            }
            cur.and_then(Value::as_str).unwrap_or_default().to_string()
        };
        ResId {
            gvk: Gvk::from_api_version(&s(&["apiVersion"]), &s(&["kind"])),
            namespace: s(&["metadata", "namespace"]),
            name: s(&["metadata", "name"]),
        }
    }
}

impl fmt::Display for ResId {
    /// `Kind.version.group/name.namespace`, as kustomize prints it in errors.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = if self.name.is_empty() {
            "[noName]"
        } else {
            &self.name
        };
        let ns = if self.namespace.is_empty() {
            "[noNs]"
        } else {
            &self.namespace
        };
        write!(f, "{}/{name}.{ns}", self.gvk)
    }
}

/// How a generated resource combines with an existing one of the same id
/// (`behavior:` on configMapGenerator and secretGenerator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GenerationBehavior {
    /// Not generated, or no behavior given (treated as `Create`).
    #[default]
    Unspecified,
    /// Must not exist yet.
    Create,
    /// Replaces the existing resource.
    Replace,
    /// Merges into the existing resource.
    Merge,
}

/// An earlier name of a resource, recorded before a transformer renamed it.
#[derive(Debug, Clone, PartialEq)]
struct PreviousId {
    name: String,
    /// The effective namespace at the time.
    namespace: String,
    kind: String,
}

/// One Kubernetes object, plus the build state kustomize tracks for it.
///
/// Build state (previous ids, generator behavior, hash suffix) lives in fields,
/// never in the object, so it can't leak into the output.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    object: Map<String, Value>,
    previous: Vec<PreviousId>,
    behavior: GenerationBehavior,
    needs_hash_suffix: bool,
}

impl Resource {
    /// Wrap a parsed document. It must be a mapping with a `kind`.
    pub fn from_value(value: Value) -> Result<Self> {
        let Value::Object(object) = value else {
            return Err(Error::Invalid(format!(
                "resource is not a mapping: {value}"
            )));
        };
        if object.get("kind").and_then(Value::as_str).is_none() {
            return Err(Error::Invalid("resource is missing kind".to_string()));
        }
        Ok(Self {
            object,
            previous: Vec::new(),
            behavior: GenerationBehavior::Unspecified,
            needs_hash_suffix: false,
        })
    }

    /// The object's fields.
    pub fn object(&self) -> &Map<String, Value> {
        &self.object
    }

    /// The object's fields, mutably.
    pub fn object_mut(&mut self) -> &mut Map<String, Value> {
        &mut self.object
    }

    /// The object as a JSON value.
    pub fn to_value(&self) -> Value {
        Value::Object(self.object.clone())
    }

    /// The value at a path of map keys, such as `["metadata", "name"]`.
    pub fn get(&self, path: &[&str]) -> Option<&Value> {
        let (first, rest) = path.split_first()?;
        rest.iter()
            .try_fold(self.object.get(*first)?, |v, key| v.get(key))
    }

    fn str_at(&self, path: &[&str]) -> &str {
        self.get(path).and_then(Value::as_str).unwrap_or_default()
    }

    /// The `metadata` map, created (or replaced, if it isn't a map) on demand.
    fn metadata_mut(&mut self) -> Option<&mut Map<String, Value>> {
        let meta = self
            .object
            .entry("metadata")
            .or_insert_with(|| Value::Object(Map::new()));
        if !meta.is_object() {
            *meta = Value::Object(Map::new());
        }
        meta.as_object_mut()
    }

    /// `apiVersion`.
    pub fn api_version(&self) -> &str {
        self.str_at(&["apiVersion"])
    }

    /// `kind`.
    pub fn kind(&self) -> &str {
        self.str_at(&["kind"])
    }

    /// `metadata.name`.
    pub fn name(&self) -> &str {
        self.str_at(&["metadata", "name"])
    }

    /// `metadata.namespace`, `""` when unset.
    pub fn namespace(&self) -> &str {
        self.str_at(&["metadata", "namespace"])
    }

    /// Set `metadata.name`. Call [`Resource::store_previous_id`] first if this
    /// is a rename that references should follow.
    pub fn set_name(&mut self, name: &str) {
        if let Some(meta) = self.metadata_mut() {
            meta.insert("name".to_string(), Value::String(name.to_string()));
        }
    }

    /// Set `metadata.namespace`.
    pub fn set_namespace(&mut self, namespace: &str) {
        if let Some(meta) = self.metadata_mut() {
            meta.insert(
                "namespace".to_string(),
                Value::String(namespace.to_string()),
            );
        }
    }

    fn string_map(&self, field: &str) -> BTreeMap<String, String> {
        let Some(Value::Object(map)) = self.get(&["metadata", field]) else {
            return BTreeMap::new();
        };
        map.iter()
            .map(|(k, v)| (k.clone(), scalar_string(v)))
            .collect()
    }

    /// Replace a metadata map the way kustomize does (`setMapInMetadata`): an
    /// empty map removes the field; otherwise values are strings, keys sorted.
    fn set_string_map(&mut self, field: &str, values: BTreeMap<String, String>) {
        let Some(meta) = self.metadata_mut() else {
            return;
        };
        if values.is_empty() {
            meta.remove(field);
            return;
        }
        let map = values
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect();
        meta.insert(field.to_string(), Value::Object(map));
    }

    /// `metadata.labels`, with non-string values as their YAML text.
    pub fn labels(&self) -> BTreeMap<String, String> {
        self.string_map("labels")
    }

    /// Replace `metadata.labels` (sorted string map; empty removes it).
    pub fn set_labels(&mut self, labels: BTreeMap<String, String>) {
        self.set_string_map("labels", labels);
    }

    /// `metadata.annotations`, with non-string values as their YAML text.
    pub fn annotations(&self) -> BTreeMap<String, String> {
        self.string_map("annotations")
    }

    /// Replace `metadata.annotations` (sorted string map; empty removes it).
    pub fn set_annotations(&mut self, annotations: BTreeMap<String, String>) {
        self.set_string_map("annotations", annotations);
    }

    /// The resource's current identity.
    pub fn id(&self) -> ResId {
        ResId::of_object(&self.object)
    }

    /// Record the current id as a previous one, before a rename (kustomize's
    /// `StorePreviousId`). Name references use the lineage to find referrers.
    pub fn store_previous_id(&mut self) {
        let id = self.id();
        self.previous.push(PreviousId {
            name: id.name.clone(),
            namespace: id.effective_namespace().to_string(),
            kind: id.gvk.kind.clone(),
        });
    }

    /// Earlier ids, oldest first. Group and version come from the current
    /// `apiVersion`, as upstream rebuilds them.
    pub fn prev_ids(&self) -> Vec<ResId> {
        let current = Gvk::from_api_version(self.api_version(), self.kind());
        self.previous
            .iter()
            .map(|p| ResId {
                gvk: Gvk {
                    kind: p.kind.clone(),
                    ..current.clone()
                },
                namespace: p.namespace.clone(),
                name: p.name.clone(),
            })
            .collect()
    }

    /// The id the resource was loaded or generated with.
    pub fn org_id(&self) -> ResId {
        self.prev_ids()
            .into_iter()
            .next()
            .unwrap_or_else(|| self.id())
    }

    /// Generator behavior, for generated resources.
    pub fn behavior(&self) -> GenerationBehavior {
        self.behavior
    }

    /// Set the generator behavior.
    pub fn set_behavior(&mut self, behavior: GenerationBehavior) {
        self.behavior = behavior;
    }

    /// Whether the name gets a content-hash suffix at the end of the build.
    pub fn needs_hash_suffix(&self) -> bool {
        self.needs_hash_suffix
    }

    /// Mark (or unmark) the resource for a content-hash name suffix.
    pub fn set_needs_hash_suffix(&mut self, needs: bool) {
        self.needs_hash_suffix = needs;
    }
}

/// A scalar as kustomize reads it into a string map: strings as-is, numbers
/// and booleans as their text, null as empty.
fn scalar_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The ordered resources of a build. Order is the order they were added.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResMap {
    resources: Vec<Resource>,
}

impl ResMap {
    /// Add a resource, refusing one whose current id equals an existing one.
    pub fn append(&mut self, resource: Resource) -> Result<()> {
        let id = resource.id();
        if !self.positions_by_current_id(&id).is_empty() {
            return Err(Error::Invalid(format!(
                "may not add resource with an already registered id: {id}"
            )));
        }
        self.resources.push(resource);
        Ok(())
    }

    /// Add every resource of `other`, in order.
    pub fn append_all(&mut self, other: ResMap) -> Result<()> {
        other.resources.into_iter().try_for_each(|r| self.append(r))
    }

    /// Number of resources.
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    /// Whether there are no resources.
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    /// The resource at `index`.
    pub fn get(&self, index: usize) -> Option<&Resource> {
        self.resources.get(index)
    }

    /// The resource at `index`, mutably.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Resource> {
        self.resources.get_mut(index)
    }

    /// Iterate in order.
    pub fn iter(&self) -> impl Iterator<Item = &Resource> {
        self.resources.iter()
    }

    /// Iterate mutably, in order.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Resource> {
        self.resources.iter_mut()
    }

    /// Remove and return the resource at `index`.
    pub fn remove(&mut self, index: usize) -> Option<Resource> {
        (index < self.resources.len()).then(|| self.resources.remove(index))
    }

    /// Replace the resource at `index`, keeping its position.
    pub fn replace(&mut self, index: usize, resource: Resource) -> Option<Resource> {
        let slot = self.resources.get_mut(index)?;
        Some(std::mem::replace(slot, resource))
    }

    /// Keep only the resources `keep` accepts.
    pub fn retain(&mut self, keep: impl FnMut(&Resource) -> bool) {
        self.resources.retain(keep);
    }

    /// Positions of resources whose current id equals `id`.
    pub fn positions_by_current_id(&self, id: &ResId) -> Vec<usize> {
        self.positions(|r| r.id().equals(id))
    }

    /// Positions of resources whose current or any previous id equals `id`.
    pub fn positions_by_any_id(&self, id: &ResId) -> Vec<usize> {
        self.positions(|r| r.id().equals(id) || r.prev_ids().iter().any(|p| p.equals(id)))
    }

    fn positions(&self, matches: impl Fn(&Resource) -> bool) -> Vec<usize> {
        self.resources
            .iter()
            .enumerate()
            .filter(|(_, r)| matches(r))
            .map(|(i, _)| i)
            .collect()
    }

    /// Every resource as a JSON value, in order.
    pub fn to_values(&self) -> Vec<Value> {
        self.resources.iter().map(Resource::to_value).collect()
    }

    /// Render as a `---`-separated YAML stream, like `kustomize build`.
    pub fn to_yaml(&self) -> Result<String> {
        let mut out = String::new();
        for (i, r) in self.resources.iter().enumerate() {
            if i > 0 {
                out.push_str("---\n");
            }
            let doc = yaml::to_string(&r.object)
                .map_err(|e| Error::Invalid(format!("rendering {}: {e}", r.id())))?;
            out.push_str(&doc);
        }
        Ok(out)
    }
}

impl IntoIterator for ResMap {
    type Item = Resource;
    type IntoIter = std::vec::IntoIter<Resource>;

    fn into_iter(self) -> Self::IntoIter {
        self.resources.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn res(v: Value) -> Resource {
        Resource::from_value(v).unwrap()
    }

    fn cm(name: &str) -> Resource {
        res(json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": name}}))
    }

    #[test]
    fn id_splits_group_and_version_at_the_first_slash() {
        let r = res(json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": "web", "namespace": "prod"}
        }));
        let id = r.id();
        assert_eq!(id.gvk.group, "apps");
        assert_eq!(id.gvk.version, "v1");
        assert_eq!(id.gvk.api_version(), "apps/v1");
        assert_eq!(Gvk::from_api_version("v1", "Pod").api_version(), "v1");
    }

    #[test]
    fn ids_print_like_kustomize() {
        let r = res(
            json!({"apiVersion": "rbac.authorization.k8s.io/v1", "kind": "RoleBinding", "metadata": {"name": "test"}}),
        );
        assert_eq!(
            r.id().to_string(),
            "RoleBinding.v1.rbac.authorization.k8s.io/test.[noNs]"
        );
        assert_eq!(
            ResId::default().to_string(),
            "[noKind].[noVer].[noGrp]/[noName].[noNs]"
        );
    }

    #[test]
    fn no_namespace_equals_default_and_cluster_scope_ignores_namespace() {
        let id = |ns: Option<&str>, kind: &str| {
            let mut meta = json!({"name": "x"});
            if let Some(ns) = ns {
                meta["namespace"] = json!(ns);
            }
            res(json!({"apiVersion": "v1", "kind": kind, "metadata": meta})).id()
        };
        assert!(id(None, "ConfigMap").equals(&id(Some("default"), "ConfigMap")));
        assert!(!id(None, "ConfigMap").equals(&id(Some("prod"), "ConfigMap")));
        assert!(id(None, "ConfigMap").gvkn_equals(&id(Some("prod"), "ConfigMap")));
        assert!(id(None, "Namespace").equals(&id(Some("prod"), "Namespace")));
        assert_eq!(
            id(None, "Namespace").effective_namespace(),
            TOTALLY_NOT_A_NAMESPACE
        );
    }

    #[test]
    fn gvk_selection_treats_empty_parts_as_wildcards() {
        let deploy = Gvk::from_api_version("apps/v1", "Deployment");
        let sel = |g: &str, v: &str, k: &str| Gvk {
            group: g.into(),
            version: v.into(),
            kind: k.into(),
        };
        assert!(deploy.is_selected(&Gvk::default()));
        assert!(deploy.is_selected(&sel("", "", "Deployment")));
        assert!(deploy.is_selected(&sel("apps", "", "")));
        assert!(!deploy.is_selected(&sel("", "v1beta1", "")));
    }

    #[test]
    fn renames_keep_a_lineage_of_previous_ids() {
        let mut r = cm("app");
        r.store_previous_id();
        r.set_name("p-app");
        r.store_previous_id();
        r.set_name("p-app-s");
        let prev: Vec<_> = r.prev_ids().into_iter().map(|id| id.name).collect();
        assert_eq!(prev, ["app", "p-app"]);
        assert_eq!(r.org_id().name, "app");
        assert_eq!(r.prev_ids()[0].namespace, DEFAULT_NAMESPACE);
        assert_eq!(cm("plain").org_id().name, "plain");
    }

    #[test]
    fn metadata_maps_are_sorted_strings_and_empty_removes() {
        let mut r = res(json!({
            "kind": "Service",
            "metadata": {"name": "s", "annotations": {"port": 8080, "happy": true, "color": "green"}}
        }));
        assert_eq!(r.annotations()["port"], "8080");
        r.set_annotations(r.annotations());
        let annotations = r.get(&["metadata", "annotations"]).unwrap();
        let keys: Vec<_> = annotations.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["color", "happy", "port"]);
        assert_eq!(annotations["happy"], json!("true"));
        r.set_labels(BTreeMap::new());
        assert!(r.get(&["metadata", "labels"]).is_none());
    }

    #[test]
    fn setters_create_metadata() {
        let mut r = res(json!({"kind": "ConfigMap", "metadata": "not a map"}));
        r.set_name("a");
        r.set_namespace("ns");
        assert_eq!((r.name(), r.namespace()), ("a", "ns"));
    }

    #[test]
    fn build_state_is_not_part_of_the_object() {
        let mut r = cm("a");
        r.set_needs_hash_suffix(true);
        r.set_behavior(GenerationBehavior::Merge);
        r.store_previous_id();
        assert_eq!(r.to_value(), cm("a").to_value());
    }

    #[test]
    fn append_refuses_equal_ids() {
        let mut m = ResMap::default();
        m.append(cm("a")).unwrap();
        m.append(cm("b")).unwrap();
        assert!(m.append(cm("a")).is_err());
        let a_in_default = res(
            json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "a", "namespace": "default"}}),
        );
        assert!(m.append(a_in_default).is_err());
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn lookups_by_current_and_previous_id() {
        let mut m = ResMap::default();
        let mut renamed = cm("old");
        renamed.store_previous_id();
        renamed.set_name("new");
        m.append(renamed).unwrap();
        m.append(cm("other")).unwrap();
        let old = cm("old").id();
        assert!(m.positions_by_current_id(&old).is_empty());
        assert_eq!(m.positions_by_any_id(&old), [0]);
        assert_eq!(m.positions_by_current_id(&cm("other").id()), [1]);
    }

    #[test]
    fn remove_replace_and_retain() {
        let mut m = ResMap::default();
        for n in ["a", "b", "c"] {
            m.append(cm(n)).unwrap();
        }
        assert_eq!(m.replace(1, cm("B")).unwrap().name(), "b");
        assert_eq!(m.remove(0).unwrap().name(), "a");
        assert!(m.remove(9).is_none());
        m.retain(|r| r.name() != "c");
        let names: Vec<_> = m.iter().map(|r| r.name().to_string()).collect();
        assert_eq!(names, ["B"]);
    }
}
