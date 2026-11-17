// Portions of this file are ported from kustomize kyaml/openapi/openapi.go,
// Copyright The Kubernetes Authors, licensed under Apache-2.0.
// Modified: translated from Go to Rust and adapted to this crate.

//! The Kubernetes schema as strategic merge patch needs it: which fields a kind
//! has, which lists merge by key, and with what strategy.
//!
//! Data: `src/schema/smp-schema.json`, generated from the schema kustomize
//! embeds (Kubernetes v1.21.2) by `scripts/import-upstream.sh`. Each node is a
//! `$ref` name, `""` for a known field with no further structure, or an object
//! with `t` (type), `i` (array items), `a` (map values), `p` (properties),
//! `s` (patch strategy) and `k` (merge keys).
//!
//! A schema that exists but says nothing still matters: kustomize treats a
//! list with a schema and no `merge` strategy as replace-only, and only infers
//! anything when there's no schema at all.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Node {
    /// `""` (plain field) or the name of a definition.
    Ref(String),
    Inline(Box<Inline>),
}

#[derive(Debug, Default, Deserialize)]
struct Inline {
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    i: Option<Node>,
    #[serde(default)]
    a: Option<Node>,
    #[serde(default)]
    p: HashMap<String, Node>,
    #[serde(default)]
    s: Option<String>,
    #[serde(default)]
    k: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Table {
    kinds: HashMap<String, String>,
    definitions: HashMap<String, Inline>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        // Embedded and checked by a unit test, so a parse failure can't ship.
        serde_json::from_str(include_str!("schema/smp-schema.json")).unwrap_or_default()
    })
}

/// The schema of one node: a kind, a field, or a list's elements.
#[derive(Debug, Clone, Copy)]
pub struct Schema {
    /// `None` for a known field without structure (a scalar, or an opaque
    /// object such as `Quantity`).
    inline: Option<&'static Inline>,
}

impl Schema {
    fn resolve(node: &'static Node) -> Option<Schema> {
        match node {
            Node::Ref(name) if name.is_empty() => Some(Schema { inline: None }),
            Node::Ref(name) => table().definitions.get(name).map(|inline| Schema {
                inline: Some(inline),
            }),
            Node::Inline(inline) => Some(Schema {
                inline: Some(inline),
            }),
        }
    }

    /// The schema of a built-in kind (`SchemaForResourceType`), if known.
    pub fn for_kind(api_version: &str, kind: &str) -> Option<Schema> {
        let name = table().kinds.get(&format!("{api_version}|{kind}"))?;
        table().definitions.get(name).map(|inline| Schema {
            inline: Some(inline),
        })
    }

    /// The schema of a field: a declared property, or the value schema of a
    /// map (`additionalProperties`). `None` when the schema doesn't know it.
    pub fn field(&self, name: &str) -> Option<Schema> {
        let inline = self.inline?;
        match inline.p.get(name) {
            Some(node) => Schema::resolve(node),
            None => inline.a.as_ref().and_then(Schema::resolve),
        }
    }

    /// The schema of a list's elements, if this is a list with an item schema.
    pub fn elements(&self) -> Option<Schema> {
        let inline = self.inline?;
        if inline.t.as_deref() != Some("array") {
            return None;
        }
        inline.i.as_ref().and_then(Schema::resolve)
    }

    /// `x-kubernetes-patch-strategy`, such as `merge` or `merge,retainKeys`;
    /// `""` when unset.
    pub fn patch_strategy(&self) -> &'static str {
        self.inline.and_then(|i| i.s.as_deref()).unwrap_or_default()
    }

    /// Whether the patch strategy includes `strategy`.
    pub fn has_strategy(&self, strategy: &str) -> bool {
        self.patch_strategy().split(',').any(|s| s == strategy)
    }

    /// Merge keys (`x-kubernetes-list-map-keys`, else
    /// `x-kubernetes-patch-merge-key`); empty for a list of primitives.
    pub fn merge_keys(&self) -> &'static [String] {
        self.inline.map(|i| i.k.as_slice()).unwrap_or_default()
    }

    /// Whether a list with this schema merges element by element (its patch
    /// strategy includes `merge`), as opposed to being replaced.
    pub fn is_associative(&self) -> bool {
        self.has_strategy("merge")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deployment() -> Schema {
        Schema::for_kind("apps/v1", "Deployment").unwrap()
    }

    #[test]
    fn embedded_table_parses() {
        assert!(table().kinds.len() > 200);
        assert!(table().definitions.len() > 500);
    }

    #[test]
    fn containers_merge_by_name() {
        let containers = deployment()
            .field("spec")
            .and_then(|s| s.field("template"))
            .and_then(|s| s.field("spec"))
            .and_then(|s| s.field("containers"))
            .unwrap();
        assert!(containers.is_associative());
        assert_eq!(containers.merge_keys(), ["name"]);
        let container = containers.elements().unwrap();
        let ports = container.field("ports").unwrap();
        assert_eq!(ports.merge_keys(), ["containerPort", "protocol"]);
        // A plain list: known to the schema, so it is replaced, never inferred.
        let args = container.field("args").unwrap();
        assert!(!args.is_associative());
    }

    #[test]
    fn volumes_also_retain_keys() {
        let pod = Schema::for_kind("v1", "Pod").unwrap();
        let volumes = pod.field("spec").and_then(|s| s.field("volumes")).unwrap();
        assert_eq!(volumes.patch_strategy(), "merge,retainKeys");
        assert!(volumes.has_strategy("retainKeys"));
    }

    #[test]
    fn maps_use_their_value_schema_and_primitive_lists_have_no_keys() {
        let meta = deployment().field("metadata").unwrap();
        let label = meta.field("labels").and_then(|l| l.field("any-key"));
        assert!(label.is_some());
        let finalizers = meta.field("finalizers").unwrap();
        assert!(finalizers.is_associative());
        assert!(finalizers.merge_keys().is_empty());
    }

    #[test]
    fn unknown_kinds_and_fields_have_no_schema() {
        assert!(Schema::for_kind("example.com/v1", "Widget").is_none());
        assert!(deployment().field("notAField").is_none());
        let replicas = deployment()
            .field("spec")
            .and_then(|s| s.field("replicas"))
            .unwrap();
        assert!(replicas.field("x").is_none());
        assert!(replicas.elements().is_none());
    }
}
