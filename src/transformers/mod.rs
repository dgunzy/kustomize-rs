// Original code, not ported from upstream.

//! Builtin transformers: one module per kustomize transformer.
//!
//! # Adding a transformer
//!
//! 1. Create `src/transformers/<name>.rs` with a struct holding its settings
//!    (from the kustomization field) and the field specs it applies to (from
//!    [`TransformerConfig`](crate::config::TransformerConfig)), and implement
//!    [`Transformer`] for it. Port upstream's `api/filters/<name>` logic.
//! 2. Declare the module below.
//! 3. Add the kustomization field as a typed field on
//!    [`Kustomization`](crate::kustomization::Kustomization) (moving it out of
//!    `rest`, which is what stops the build reporting it as unsupported).
//! 4. Construct it in `builtin_transformers` in `src/build.rs`, at its slot:
//!    the order there is upstream's order, and it matters.
//! 5. Unit-test the transformer on a `ResMap` directly, in its own module.
//!
//! Most transformers are "apply a setter at these field specs":
//!
//! ```ignore
//! for r in resources.iter_mut() {
//!     fieldspec::apply_all(&self.specs, r, CreateKind::Scalar, &mut setters::set_scalar(&self.value))?;
//! }
//! ```
//!
//! See [`crate::fieldspec`] for how paths are walked and [`crate::setters`]
//! for how values get their YAML type.
//!
//! A transformer that renames resources calls
//! [`Resource::store_previous_id`](crate::Resource::store_previous_id) before
//! changing the name, so name references can follow the rename.

use crate::{ResMap, Result};

/// Changes resources in place. Transformers run in a fixed order per
/// kustomization, after its resources, generators and components are gathered.
pub(crate) trait Transformer {
    /// Apply to every resource it selects.
    fn transform(&self, resources: &mut ResMap) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Resource;
    use serde_json::json;

    /// A stand-in transformer: renames every resource with a prefix.
    struct Prefix(&'static str);

    impl Transformer for Prefix {
        fn transform(&self, resources: &mut ResMap) -> Result<()> {
            for r in resources.iter_mut() {
                let name = format!("{}{}", self.0, r.name());
                r.store_previous_id();
                r.set_name(&name);
            }
            Ok(())
        }
    }

    #[test]
    fn transformers_run_in_order_and_record_renames() {
        let mut m = ResMap::default();
        m.append(
            Resource::from_value(
                json!({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "a"}}),
            )
            .unwrap(),
        )
        .unwrap();
        let pipeline: Vec<Box<dyn Transformer>> =
            vec![Box::new(Prefix("x-")), Box::new(Prefix("y-"))];
        for t in &pipeline {
            t.transform(&mut m).unwrap();
        }
        let r = m.get(0).unwrap();
        assert_eq!(r.name(), "y-x-a");
        assert_eq!(r.org_id().name, "a");
        assert_eq!(r.prev_ids().len(), 2);
    }
}
