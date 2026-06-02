//! The `RepositoryProxy` abstraction — the Rust analogue of the .NET
//! `IRepositoryProxy`. The sync engine talks only to this trait, so additional
//! backends (remote/HTTP, encrypted) can be added later without changing the
//! engine.

use std::io;
use std::path::PathBuf;

use repotools_manifest::{Manifest, ManifestFileInfo};

pub trait RepositoryProxy {
    /// The manifest for this repository.
    fn manifest(&self) -> &Manifest;

    /// The manifest entry for a file by standard path, if present.
    fn manifest_file(&self, std_path: &str) -> Option<&ManifestFileInfo>;

    /// A filesystem path from which the file's content can be read (used by
    /// another proxy's `put_file`).
    fn get_file_path(&self, std_path: &str) -> io::Result<PathBuf>;

    /// Copies a file from `source` into this repository at `std_path`,
    /// recording the manifest entry from the source.
    fn put_file(&mut self, source: &dyn RepositoryProxy, std_path: &str) -> io::Result<()>;

    /// Removes the file at `std_path` from this repository.
    fn remove_file(&mut self, std_path: &str) -> io::Result<()>;

    /// Copies this repository's file at `existing` to `new_path`, recording the
    /// manifest entry given by `new_info` (which comes from the other side of
    /// the sync). The `existing` file remains in place.
    fn copy_file(
        &mut self,
        existing: &str,
        new_path: &str,
        new_info: &ManifestFileInfo,
    ) -> io::Result<()>;

    /// Moves this repository's file from `existing` to `new_path`, recording the
    /// manifest entry given by `new_info`.
    fn move_file(
        &mut self,
        existing: &str,
        new_path: &str,
        new_info: &ManifestFileInfo,
    ) -> io::Result<()>;

    /// Updates only the dates of the file at `std_path` from `source_file`
    /// (content is unchanged).
    fn copy_file_information(
        &mut self,
        std_path: &str,
        source_file: &ManifestFileInfo,
    ) -> io::Result<()>;

    /// Copies manifest metadata (name, description, ignore list, hash method)
    /// from another manifest.
    fn copy_manifest_information(&mut self, source: &Manifest) -> io::Result<()>;

    /// Persists the manifest if it changed.
    fn flush(&mut self) -> io::Result<()>;
}
