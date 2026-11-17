// Portions of this file are ported from kustomize
// api/internal/plugins/builtinconfig/transformerconfig.go, Copyright The
// Kubernetes Authors, licensed under Apache-2.0.
// Modified: translated from Go to Rust and adapted to this crate.

//! The transformer config: which fields each builtin transformer touches.
//!
//! Starts from kustomize's default tables (`src/builtin_config/*.yaml`), and
//! grows with a kustomization's `configurations:` files and `crds:` (later
//! tasks). A child kustomization's config is merged into its parent's.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::Result;
use crate::fieldspec::{FsSlice, NameBackReferences, merge_field_specs, merge_name_references};

/// Field specs per builtin transformer.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TransformerConfig {
    /// Where `namePrefix` applies.
    pub name_prefix: FsSlice,
    /// Where `nameSuffix` applies.
    pub name_suffix: FsSlice,
    /// Where `namespace` applies.
    pub namespace: FsSlice,
    /// Where `commonLabels` applies (labels and selectors).
    pub common_labels: FsSlice,
    /// Where `labels` applies.
    pub labels: FsSlice,
    /// Template labels, for `labels` with `includeTemplates`.
    pub template_labels: FsSlice,
    /// StatefulSet volume claim template labels.
    pub volume_claim_template_labels: FsSlice,
    /// Where `commonAnnotations` applies.
    pub common_annotations: FsSlice,
    /// Which fields refer to which kinds by name.
    pub name_reference: Vec<NameBackReferences>,
    /// Where `$(VAR)` references may appear.
    pub var_reference: FsSlice,
    /// Where `images` applies.
    pub images: FsSlice,
    /// Where `replicas` applies.
    pub replicas: FsSlice,
}

/// The default tables, one YAML document each, as kustomize ships them.
const BUILTIN_TABLES: [&str; 11] = [
    include_str!("builtin_config/nameprefix.yaml"),
    include_str!("builtin_config/namesuffix.yaml"),
    include_str!("builtin_config/namespace.yaml"),
    include_str!("builtin_config/commonlabels.yaml"),
    include_str!("builtin_config/templatelabels.yaml"),
    include_str!("builtin_config/volumeclaimtemplatelabels.yaml"),
    include_str!("builtin_config/commonannotations.yaml"),
    include_str!("builtin_config/namereference.yaml"),
    include_str!("builtin_config/varreference.yaml"),
    include_str!("builtin_config/images.yaml"),
    include_str!("builtin_config/replicas.yaml"),
];

impl TransformerConfig {
    /// Parse one config file (`configurations:` entries use the same format).
    pub fn from_yaml(text: &str) -> std::result::Result<Self, serde_saphyr::Error> {
        let parsed: Option<Self> = crate::yaml::from_str(text)?;
        Ok(parsed.unwrap_or_default())
    }

    /// kustomize's default config (`MakeDefaultConfig`).
    pub fn builtin() -> Self {
        static BUILTIN: OnceLock<TransformerConfig> = OnceLock::new();
        BUILTIN
            .get_or_init(|| {
                // The tables are embedded and covered by a unit test; a table
                // that fails to parse or merge is skipped rather than panicking.
                BUILTIN_TABLES.iter().fold(Self::default(), |acc, table| {
                    Self::from_yaml(table)
                        .ok()
                        .and_then(|t| acc.merge(&t).ok())
                        .unwrap_or(acc)
                })
            })
            .clone()
    }

    /// Merge `other` into this config (upstream `TransformerConfig.Merge`).
    pub fn merge(&self, other: &Self) -> Result<Self> {
        Ok(Self {
            name_prefix: merge_field_specs(&self.name_prefix, &other.name_prefix)?,
            name_suffix: merge_field_specs(&self.name_suffix, &other.name_suffix)?,
            namespace: merge_field_specs(&self.namespace, &other.namespace)?,
            common_labels: merge_field_specs(&self.common_labels, &other.common_labels)?,
            labels: merge_field_specs(&self.labels, &other.labels)?,
            template_labels: merge_field_specs(&self.template_labels, &other.template_labels)?,
            volume_claim_template_labels: merge_field_specs(
                &self.volume_claim_template_labels,
                &other.volume_claim_template_labels,
            )?,
            common_annotations: merge_field_specs(
                &self.common_annotations,
                &other.common_annotations,
            )?,
            name_reference: merge_name_references(&self.name_reference, &other.name_reference)?,
            var_reference: merge_field_specs(&self.var_reference, &other.var_reference)?,
            images: merge_field_specs(&self.images, &other.images)?,
            replicas: merge_field_specs(&self.replicas, &other.replicas)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_table_parses() {
        for table in BUILTIN_TABLES {
            let parsed = TransformerConfig::from_yaml(table).unwrap();
            assert_ne!(
                parsed,
                TransformerConfig::default(),
                "empty table:\n{table}"
            );
        }
    }

    #[test]
    fn builtin_config_has_every_table() {
        let c = TransformerConfig::builtin();
        for (name, specs) in [
            ("namePrefix", &c.name_prefix),
            ("nameSuffix", &c.name_suffix),
            ("namespace", &c.namespace),
            ("commonLabels", &c.common_labels),
            ("templateLabels", &c.template_labels),
            ("volumeClaimTemplateLabels", &c.volume_claim_template_labels),
            ("commonAnnotations", &c.common_annotations),
            ("varReference", &c.var_reference),
            ("images", &c.images),
            ("replicas", &c.replicas),
        ] {
            assert!(!specs.is_empty(), "{name} is empty");
        }
        assert!(c.name_reference.iter().any(|r| r.kind == "ConfigMap"));
        // The namespace table has the special cases only; the namespace filter
        // sets metadata/namespace itself.
        assert!(
            c.namespace
                .iter()
                .any(|s| s.kind == "Namespace" && s.path == "metadata/name")
        );
    }

    #[test]
    fn merging_a_custom_config_adds_specs() {
        let custom = TransformerConfig::from_yaml(
            "commonLabels:\n- path: spec/selectors\n  kind: MyKind\n  create: true\n",
        )
        .unwrap();
        let merged = TransformerConfig::builtin().merge(&custom).unwrap();
        assert_eq!(
            merged.common_labels.len(),
            TransformerConfig::builtin().common_labels.len() + 1
        );
    }
}
