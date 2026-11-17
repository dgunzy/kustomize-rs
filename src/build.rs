// Portions of this file are ported from kustomize api/internal/target/kusttarget.go
// and api/krusty/kustomizer.go, Copyright The Kubernetes Authors, licensed under
// Apache-2.0. Modified: translated from Go to Rust and adapted to this crate.

//! The build pipeline, mirroring kustomize's `KustTarget` and `Kustomizer.Run`.
//!
//! Each kustomization directory is *accumulated*:
//!
//! 1. resources (files, and directories accumulated recursively)
//! 2. configurations + crds, merged into the transformer config
//! 3. generators
//! 4. components
//! 5. builtin transformers, in upstream's order (`builtin_transformers`)
//! 6. validators
//! 7. vars (merged)
//!
//! A child directory's resources and transformer config merge into its
//! parent's. Then, for the top-level kustomization only:
//!
//! 8. hash suffixes on generated resources
//! 9. name references (follow renames)
//! 10. vars (resolved)
//! 11. drop `config.kubernetes.io/local-config` resources
//! 12. sort, managed-by label, strip build annotations
//!
//! Every stage is a named function below. A feature fills its stage; anything
//! a kustomization asks for that isn't implemented is reported as
//! [`Error::Unsupported`] before the build starts.

use std::path::{Path, PathBuf};

use crate::config::TransformerConfig;
use crate::fs::{FileSystem, clean};
use crate::kustomization::Kustomization;
use crate::transformers::Transformer;
use crate::{Error, ResMap, Resource, Result, yaml};

/// Where a kustomization may load files from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadRestrictions {
    /// Files must be inside the kustomization's own directory (kustomize's
    /// default). Directories of other kustomizations may be anywhere.
    #[default]
    RootOnly,
    /// Files may be anywhere the filesystem allows (Flux's setting; Flux
    /// confines the filesystem itself instead).
    None,
}

/// How to order the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Reorder {
    /// Keep input order (depth-first through the kustomization tree).
    #[default]
    None,
    /// kustomize's fixed legacy kind order (the CLI's default).
    Legacy,
}

/// Build settings, mirroring kustomize's `krusty.Options`.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Where files may be loaded from.
    pub load_restrictions: LoadRestrictions,
    /// Output order, unless the kustomization sets `sortOptions`.
    pub reorder: Reorder,
    /// Add `app.kubernetes.io/managed-by: kustomize-<version>`.
    pub add_managed_by_label: bool,
}

impl BuildOptions {
    /// The settings Flux's kustomize-controller builds with.
    pub fn flux() -> Self {
        Self {
            load_restrictions: LoadRestrictions::None,
            reorder: Reorder::None,
            add_managed_by_label: false,
        }
    }
}

/// Build the kustomization in `dir`.
pub fn build(fs: &dyn FileSystem, dir: impl AsRef<Path>, options: &BuildOptions) -> Result<ResMap> {
    if options.reorder == Reorder::Legacy {
        return Err(Error::Unsupported("legacy reorder".to_string()));
    }
    if options.add_managed_by_label {
        return Err(Error::Unsupported("managed-by label".to_string()));
    }
    let mut builder = Builder {
        fs,
        options,
        stack: Vec::new(),
    };
    let accumulated = builder.accumulate_target(&clean(dir.as_ref()))?;
    let mut resources = finish(accumulated)?;
    emit(&mut resources)?;
    Ok(resources)
}

/// What accumulating a kustomization produces, and what a child passes up to
/// its parent (upstream's `ResAccumulator`).
struct Accumulated {
    resources: ResMap,
    config: TransformerConfig,
}

impl Accumulated {
    /// Merge a child kustomization's result (`MergeAccumulator`).
    fn absorb(&mut self, child: Accumulated) -> Result<()> {
        self.resources.append_all(child.resources)?;
        self.config = self.config.merge(&child.config)?;
        Ok(())
    }
}

/// Stages 8–11: run once, on the top-level result.
fn finish(accumulated: Accumulated) -> Result<ResMap> {
    let Accumulated {
        resources,
        config: _config,
    } = accumulated;
    // 8. hash suffixes (W-hash-suffix)
    // 9. name references (L-nameref): uses `_config.name_reference`
    // 10. vars
    // 11. drop local-config resources (W-local-config)
    Ok(resources)
}

/// Stage 12: what `Kustomizer.Run` does before returning. Sorting and the
/// managed-by label are refused in `build` until implemented (W-sort-order).
fn emit(_resources: &mut ResMap) -> Result<()> {
    // strip build annotations: kustomize rewrites annotations here, as sorted
    // string maps (W-metadata-strings)
    Ok(())
}

/// Stage 5: builtin transformers, in upstream's order
/// (`configureBuiltinTransformers` in kusttarget_configplugin.go). A
/// transformer task constructs its transformer at its slot and pushes it.
fn builtin_transformers(
    _kustomization: &Kustomization,
    _config: &TransformerConfig,
) -> Result<Vec<Box<dyn Transformer>>> {
    let transformers: Vec<Box<dyn Transformer>> = Vec::new();
    // PatchStrategicMerge  patchesStrategicMerge   (W-patches-smp)
    // Patch                patches                 (W-patches-smp, W-json6902)
    // Namespace            namespace               (W-namespace)
    // Prefix               namePrefix              (W-prefix-suffix)
    // Suffix               nameSuffix              (W-prefix-suffix)
    // Label                commonLabels, labels    (W-labels)
    // Annotations          commonAnnotations       (W-annotations)
    // PatchJson6902        patchesJson6902         (W-json6902)
    // ReplicaCount         replicas                (W-replicas)
    // ImageTag             images                  (W-images)
    // Replacement          replacements            (L-replacements)
    Ok(transformers)
}

struct Builder<'a> {
    fs: &'a dyn FileSystem,
    options: &'a BuildOptions,
    /// Kustomization directories being built, to detect cycles.
    stack: Vec<PathBuf>,
}

impl Builder<'_> {
    /// Accumulate one kustomization directory (stages 1–7).
    fn accumulate_target(&mut self, dir: &Path) -> Result<Accumulated> {
        if self.stack.iter().any(|d| d == dir) {
            return Err(Error::Invalid(format!(
                "cycle detected: {} includes itself",
                dir.display()
            )));
        }
        let (kustomization, file) = Kustomization::load(self.fs, dir)?;
        if kustomization.is_component() {
            return Err(Error::Unsupported("components".to_string()));
        }
        if let Some(field) = kustomization.unsupported_fields().next() {
            return Err(Error::Unsupported(format!("kustomization field `{field}`")));
        }

        self.stack.push(dir.to_path_buf());
        let result = self.accumulate_stages(dir, &file, &kustomization);
        self.stack.pop();
        result
    }

    fn accumulate_stages(
        &mut self,
        dir: &Path,
        file: &Path,
        k: &Kustomization,
    ) -> Result<Accumulated> {
        let mut acc = Accumulated {
            resources: ResMap::default(),
            config: TransformerConfig::builtin(),
        };
        // 1. resources
        self.accumulate_resources(&mut acc, dir, file, k)?;
        // 2. configurations + crds: merged into acc.config (Phase C)
        // 3. generators (W-generators)
        // 4. components (L-components)
        // 5. builtin transformers
        for transformer in builtin_transformers(k, &acc.config)? {
            transformer.transform(&mut acc.resources)?;
        }
        // 6. validators, 7. vars (Phase C)
        Ok(acc)
    }

    /// Stage 1: each entry is a file of resources or a kustomization directory.
    fn accumulate_resources(
        &mut self,
        acc: &mut Accumulated,
        dir: &Path,
        file: &Path,
        k: &Kustomization,
    ) -> Result<()> {
        for entry in &k.resources {
            if is_remote(entry) {
                return Err(Error::Unsupported("remote resources".to_string()));
            }
            let path = clean(&dir.join(entry));
            if self.fs.is_dir(&path) {
                let child = self.accumulate_target(&path)?;
                acc.absorb(child)?;
            } else {
                self.check_file_allowed(dir, &path, file)?;
                for r in self.load_file(&path)? {
                    acc.resources.append(r)?;
                }
            }
        }
        Ok(())
    }

    fn check_file_allowed(&self, root: &Path, path: &Path, file: &Path) -> Result<()> {
        if self.options.load_restrictions == LoadRestrictions::RootOnly && !path.starts_with(root) {
            return Err(Error::Invalid(format!(
                "{}: security; file '{}' is not in or below '{}'",
                file.display(),
                path.display(),
                root.display()
            )));
        }
        Ok(())
    }

    /// Parse a resource file, flattening `List` kinds into their items.
    fn load_file(&self, path: &Path) -> Result<Vec<Resource>> {
        let bytes = self.fs.read(path)?;
        let text = String::from_utf8(bytes).map_err(|e| Error::parse(path, e))?;
        let docs = yaml::documents(&text).map_err(|e| Error::parse(path, e))?;
        let mut out = Vec::new();
        for doc in docs {
            let is_list = doc
                .get("kind")
                .and_then(|k| k.as_str())
                .is_some_and(|k| k.ends_with("List"));
            match doc.get("items").and_then(|i| i.as_array()) {
                Some(items) if is_list => {
                    for item in items {
                        out.push(
                            Resource::from_value(item.clone())
                                .map_err(|e| Error::parse(path, e))?,
                        );
                    }
                }
                _ => out.push(Resource::from_value(doc).map_err(|e| Error::parse(path, e))?),
            }
        }
        Ok(out)
    }
}

/// Whether a `resources` entry points at a remote repository or URL.
fn is_remote(entry: &str) -> bool {
    entry.contains("://") || entry.starts_with("git@") || entry.starts_with("github.com/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::MemoryFs;

    const CM: &str = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: ";

    #[test]
    fn bases_are_built_depth_first_in_order() {
        let mut fs = MemoryFs::default();
        fs.insert("base/kustomization.yaml", "resources: [a.yaml]\n");
        fs.insert("base/a.yaml", format!("{CM}a\n"));
        fs.insert("app/kustomization.yaml", "resources: [../base, b.yaml]\n");
        fs.insert("app/b.yaml", format!("{CM}b\n---\n{CM}c\n"));
        let m = build(&fs, "app", &BuildOptions::default()).unwrap();
        let names: Vec<_> = m.iter().map(|r| r.id().name).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn root_only_refuses_files_outside_the_kustomization() {
        let mut fs = MemoryFs::default();
        fs.insert("shared/a.yaml", format!("{CM}a\n"));
        fs.insert("app/kustomization.yaml", "resources: [../shared/a.yaml]\n");
        assert!(build(&fs, "app", &BuildOptions::default()).is_err());
        assert_eq!(build(&fs, "app", &BuildOptions::flux()).unwrap().len(), 1);
    }

    #[test]
    fn cycles_are_errors() {
        let mut fs = MemoryFs::default();
        fs.insert("a/kustomization.yaml", "resources: [../b]\n");
        fs.insert("b/kustomization.yaml", "resources: [../a]\n");
        assert!(matches!(
            build(&fs, "a", &BuildOptions::default()),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn unimplemented_fields_fail_loudly() {
        let mut fs = MemoryFs::default();
        fs.insert("kustomization.yaml", "namePrefix: x-\n");
        assert!(matches!(
            build(&fs, "", &BuildOptions::default()),
            Err(Error::Unsupported(f)) if f.contains("namePrefix")
        ));
    }
}
