//! A Rust implementation of the kustomize build engine, scoped to what Flux
//! uses.
//!
//! `build` takes a directory holding a `kustomization.yaml` and returns the
//! resources it produces, the same output as `kustomize build`. See `SPEC.md`
//! for the target scope and the order features land in. Anything the engine
//! doesn't implement yet fails with [`Error::Unsupported`] rather than producing
//! partial output.
//!
//! ```
//! use kustomize::{build, BuildOptions, fs::MemoryFs};
//!
//! let mut fs = MemoryFs::default();
//! fs.insert("app/kustomization.yaml", "resources:\n- cm.yaml\n");
//! fs.insert("app/cm.yaml", "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: settings\n");
//!
//! let resources = build(&fs, "app", &BuildOptions::flux()).unwrap();
//! assert_eq!(resources.len(), 1);
//! assert_eq!(resources.get(0).unwrap().id().name, "settings");
//! ```

mod build;
pub mod config;
mod error;
pub mod fieldspec;
pub mod fs;
pub mod kustomization;
pub mod openapi;
pub mod resource;
mod schema;
pub mod setters;
pub mod smp;
mod transformers;
pub mod yaml;

pub use build::{BuildOptions, LoadRestrictions, Reorder, build};
pub use error::{Error, Result};
pub use resource::{GenerationBehavior, Gvk, ResId, ResMap, Resource};
