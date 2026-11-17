use std::path::PathBuf;

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything a build can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file or directory could not be read.
    #[error("{}: {message}", path.display())]
    Io {
        /// The path being read.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },

    /// A file is not valid YAML, or not the shape expected.
    #[error("{}: {message}", path.display())]
    Parse {
        /// The file being parsed.
        path: PathBuf,
        /// The parser's message.
        message: String,
    },

    /// The input is well-formed but kustomize would reject it (missing
    /// kustomization file, duplicate resource, path outside the root, ...).
    #[error("{0}")]
    Invalid(String),

    /// The input needs a kustomize feature this crate doesn't implement yet.
    /// The string names the feature, such as "kustomization field `namePrefix`";
    /// the conformance report groups failures by it.
    #[error("not implemented yet: {0}")]
    Unsupported(String),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, err: impl std::fmt::Display) -> Self {
        Self::Io {
            path: path.into(),
            message: err.to_string(),
        }
    }

    pub(crate) fn parse(path: impl Into<PathBuf>, err: impl std::fmt::Display) -> Self {
        Self::Parse {
            path: path.into(),
            message: err.to_string(),
        }
    }
}
