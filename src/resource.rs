//! Resources, their identity, and the ordered set a build produces.

use std::fmt;

use serde_json::{Map, Value};

use crate::{Error, Result, yaml};

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
    /// Split an `apiVersion` (`apps/v1`, `v1`) and a kind.
    pub fn from_api_version(api_version: &str, kind: &str) -> Self {
        let (group, version) = api_version.rsplit_once('/').unwrap_or(("", api_version));
        Self {
            group: group.to_string(),
            version: version.to_string(),
            kind: kind.to_string(),
        }
    }
}

/// A resource's identity: kind plus namespaced name. Two resources with the
/// same id cannot both be in a [`ResMap`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ResId {
    /// Group, version and kind.
    pub gvk: Gvk,
    /// `metadata.namespace`, `""` when unset.
    pub namespace: String,
    /// `metadata.name`.
    pub name: String,
}

impl fmt::Display for ResId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Gvk {
            group,
            version,
            kind,
        } = &self.gvk;
        let ns = if self.namespace.is_empty() {
            "[noNs]"
        } else {
            &self.namespace
        };
        write!(f, "{group}_{version}_{kind}|{ns}|{}", self.name)
    }
}

/// One Kubernetes object.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    object: Map<String, Value>,
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
        Ok(Self { object })
    }

    /// The object's fields.
    pub fn object(&self) -> &Map<String, Value> {
        &self.object
    }

    /// The object as a JSON value.
    pub fn to_value(&self) -> Value {
        Value::Object(self.object.clone())
    }

    fn str_at(&self, path: &[&str]) -> &str {
        let mut cur = self.object.get(path[0]);
        for key in &path[1..] {
            cur = cur.and_then(|v| v.get(key));
        }
        cur.and_then(Value::as_str).unwrap_or_default()
    }

    /// The resource's identity, read from its fields.
    pub fn id(&self) -> ResId {
        ResId {
            gvk: Gvk::from_api_version(self.str_at(&["apiVersion"]), self.str_at(&["kind"])),
            namespace: self.str_at(&["metadata", "namespace"]).to_string(),
            name: self.str_at(&["metadata", "name"]).to_string(),
        }
    }
}

/// The ordered resources of a build. Order is the order they were added.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResMap {
    resources: Vec<Resource>,
}

impl ResMap {
    /// Add a resource, refusing an id that is already present.
    pub fn append(&mut self, resource: Resource) -> Result<()> {
        let id = resource.id();
        if self.resources.iter().any(|r| r.id() == id) {
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

    /// Iterate in order.
    pub fn iter(&self) -> impl Iterator<Item = &Resource> {
        self.resources.iter()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cm(name: &str) -> Resource {
        Resource::from_value(
            json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": name}}),
        )
        .unwrap()
    }

    #[test]
    fn id_splits_group_and_version() {
        let r = Resource::from_value(json!({
            "apiVersion": "apps/v1", "kind": "Deployment",
            "metadata": {"name": "web", "namespace": "prod"}
        }))
        .unwrap();
        let id = r.id();
        assert_eq!(id.gvk.group, "apps");
        assert_eq!(id.gvk.version, "v1");
        assert_eq!(id.to_string(), "apps_v1_Deployment|prod|web");
    }

    #[test]
    fn append_refuses_duplicate_ids() {
        let mut m = ResMap::default();
        m.append(cm("a")).unwrap();
        m.append(cm("b")).unwrap();
        assert!(m.append(cm("a")).is_err());
        assert_eq!(m.len(), 2);
    }
}
