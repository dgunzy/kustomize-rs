// Portions of this file are ported from kustomize api/internal/target/kusttarget.go
// and api/krusty/kustomizer.go, Copyright The Kubernetes Authors, licensed under
// Apache-2.0. Modified: translated from Go to Rust and adapted to this crate.

//! The build: load a kustomization, gather its resources, and (as features
//! land) generate and transform them. The pipeline mirrors kustomize's
//! `KustTarget`; SPEC.md lists the stages in order.

use std::path::{Path, PathBuf};

use crate::fs::{FileSystem, clean};
use crate::kustomization::Kustomization;
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
    builder.target(&clean(dir.as_ref()))
}

struct Builder<'a> {
    fs: &'a dyn FileSystem,
    options: &'a BuildOptions,
    /// Kustomization directories being built, to detect cycles.
    stack: Vec<PathBuf>,
}

impl Builder<'_> {
    /// Build one kustomization directory.
    fn target(&mut self, dir: &Path) -> Result<ResMap> {
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
        let result = self.accumulate(dir, &file, &kustomization);
        self.stack.pop();
        result
    }

    fn accumulate(&mut self, dir: &Path, file: &Path, k: &Kustomization) -> Result<ResMap> {
        let mut resources = ResMap::default();
        for entry in &k.resources {
            if is_remote(entry) {
                return Err(Error::Unsupported("remote resources".to_string()));
            }
            let path = clean(&dir.join(entry));
            if self.fs.is_dir(&path) {
                resources.append_all(self.target(&path)?)?;
            } else {
                self.check_file_allowed(dir, &path, file)?;
                for r in self.load_file(&path)? {
                    resources.append(r)?;
                }
            }
        }
        Ok(resources)
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
