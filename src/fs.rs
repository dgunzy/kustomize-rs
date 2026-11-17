//! The filesystem a build reads from.
//!
//! The engine only reads, through [`FileSystem`]. [`MemoryFs`] backs the tests
//! and lets callers overlay generated files (such as Flux's generated
//! `kustomization.yaml`) without touching disk. [`DiskFs`] reads the real
//! filesystem, optionally confined to a root the way Flux's secure filesystem is.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::{Error, Result};

/// Read access to the files a build may load.
pub trait FileSystem {
    /// Read a whole file.
    fn read(&self, path: &Path) -> Result<Vec<u8>>;

    /// Whether `path` is a directory.
    fn is_dir(&self, path: &Path) -> bool;

    /// Whether `path` is an existing file.
    fn is_file(&self, path: &Path) -> bool;
}

/// Resolve `.` and `..` lexically, without touching the filesystem.
///
/// `..` at the top of a relative path is kept, so a path that climbs above its
/// starting point stays visibly outside it.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }
    out
}

/// An in-memory tree of files, keyed by path relative to its root.
///
/// Leading `/` and `./` are ignored, so `/app/k.yaml`, `./app/k.yaml` and
/// `app/k.yaml` are the same file, and the root is `""`, `.` or `/`.
#[derive(Debug, Clone, Default)]
pub struct MemoryFs {
    files: BTreeMap<PathBuf, Vec<u8>>,
}

impl MemoryFs {
    /// Add or replace a file.
    pub fn insert(&mut self, path: impl AsRef<Path>, contents: impl Into<Vec<u8>>) {
        self.files.insert(Self::key(path.as_ref()), contents.into());
    }

    /// Copy every file under `dir` on disk into a new in-memory tree.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        let mut fs = Self::default();
        fs.load_dir(dir, dir)?;
        Ok(fs)
    }

    fn load_dir(&mut self, root: &Path, dir: &Path) -> Result<()> {
        let entries = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?;
        for entry in entries {
            let path = entry.map_err(|e| Error::io(dir, e))?.path();
            if path.is_dir() {
                self.load_dir(root, &path)?;
            } else {
                let contents = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
                let relative = path.strip_prefix(root).unwrap_or(&path);
                self.insert(relative, contents);
            }
        }
        Ok(())
    }

    fn key(path: &Path) -> PathBuf {
        let cleaned = clean(path);
        cleaned
            .strip_prefix("/")
            .map(Path::to_path_buf)
            .unwrap_or(cleaned)
    }
}

impl FileSystem for MemoryFs {
    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        self.files
            .get(&Self::key(path))
            .cloned()
            .ok_or_else(|| Error::io(path, "no such file"))
    }

    fn is_dir(&self, path: &Path) -> bool {
        let dir = Self::key(path);
        self.files.keys().any(|f| f != &dir && f.starts_with(&dir))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(&Self::key(path))
    }
}

/// The real filesystem, optionally confined to a root directory.
///
/// Confined, any path that resolves (after symlinks) outside the root is
/// refused, matching Flux's `MakeFsOnDiskSecure`.
#[derive(Debug, Clone, Default)]
pub struct DiskFs {
    root: Option<PathBuf>,
}

impl DiskFs {
    /// Read anywhere, like the kustomize CLI.
    pub fn unrestricted() -> Self {
        Self { root: None }
    }

    /// Read only inside `root`.
    pub fn confined(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let root = root.canonicalize().map_err(|e| Error::io(root, e))?;
        Ok(Self { root: Some(root) })
    }

    fn check(&self, path: &Path) -> Result<PathBuf> {
        let Some(root) = &self.root else {
            return Ok(path.to_path_buf());
        };
        let resolved = path.canonicalize().map_err(|e| Error::io(path, e))?;
        if resolved.starts_with(root) {
            Ok(resolved)
        } else {
            Err(Error::Invalid(format!(
                "{} is outside the root {}",
                path.display(),
                root.display()
            )))
        }
    }
}

impl FileSystem for DiskFs {
    fn read(&self, path: &Path) -> Result<Vec<u8>> {
        let path = self.check(path)?;
        std::fs::read(&path).map_err(|e| Error::io(&path, e))
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.check(path).is_ok_and(|p| p.is_dir())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.check(path).is_ok_and(|p| p.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_resolves_dots_lexically() {
        assert_eq!(clean(Path::new("a/./b/../c")), Path::new("a/c"));
        assert_eq!(clean(Path::new("a/../../b")), Path::new("../b"));
        assert_eq!(clean(Path::new("/../a")), Path::new("/a"));
    }

    #[test]
    fn memory_fs_ignores_leading_slash_and_dot() {
        let mut fs = MemoryFs::default();
        fs.insert("/app/k.yaml", "x");
        assert!(fs.is_file(Path::new("app/k.yaml")));
        assert!(fs.is_file(Path::new("./app/k.yaml")));
        assert!(fs.is_dir(Path::new("app")));
        assert!(fs.is_dir(Path::new("/")));
        assert!(!fs.is_dir(Path::new("app/k.yaml")));
        assert!(!fs.is_dir(Path::new("ap")));
    }
}
