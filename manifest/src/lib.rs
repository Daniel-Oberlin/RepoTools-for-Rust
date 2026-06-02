//! `repotools_manifest` — the core library for RepoTools: the manifest data
//! model, JSON (de)serialization interoperable with the .NET implementation,
//! file hashing, ignore handling, and the repository scan/diff engine.

pub mod hash;
pub mod ignore;
pub mod io;
pub mod model;
pub mod paths;
pub mod scan;
pub mod serde_fmt;

pub use hash::HashType;
pub use model::{FileHash, Manifest, ManifestDirectoryInfo, ManifestFileInfo};

/// The default manifest file name, located in the repository's root directory.
pub const DEFAULT_MANIFEST_FILE_NAME: &str = ".repositoryManifest";
