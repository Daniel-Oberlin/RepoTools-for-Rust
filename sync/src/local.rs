//! `LocalRepositoryProxy` — a [`RepositoryProxy`] backed by the local
//! filesystem. Ports `LocalRepositoryProxy` / `LocalRepositoryState`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use repotools_manifest::serde_fmt::dotnet_min_date;
use repotools_manifest::{
    io as manifest_io, Manifest, ManifestFileInfo, DEFAULT_MANIFEST_FILE_NAME,
};

use crate::proxy::RepositoryProxy;

/// A repository stored on the local filesystem.
pub struct LocalRepositoryProxy {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest: Manifest,
    read_only: bool,
    changed: bool,
}

impl LocalRepositoryProxy {
    /// Opens the repository rooted at `root` by loading its manifest. When
    /// `read_only` is set, the manifest is never written back.
    pub fn open(root: &Path, read_only: bool) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        let manifest_path = root.join(DEFAULT_MANIFEST_FILE_NAME);
        let manifest = manifest_io::read_manifest(&manifest_path)?;
        Ok(LocalRepositoryProxy {
            root,
            manifest_path,
            manifest,
            read_only,
            changed: false,
        })
    }

    /// Creates a seed (empty) repository at `dest_dir` cloned from `source`:
    /// same GUID and metadata, empty file tree, reset update date. Errors if a
    /// manifest already exists there. Mirrors `SeedLocalRepository`.
    pub fn seed(source: &Manifest, dest_dir: &Path) -> io::Result<()> {
        let manifest_path = dest_dir.join(DEFAULT_MANIFEST_FILE_NAME);
        if manifest_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Destination manifest already exists.",
            ));
        }
        fs::create_dir_all(dest_dir)?;

        let mut seed = source.clone();
        seed.root_directory.files.clear();
        seed.root_directory.subdirectories.clear();
        seed.last_update_date_utc = dotnet_min_date();

        manifest_io::write_manifest(&manifest_path, &seed)
    }

    /// Converts a standard path (`./a/b/c`) to a native filesystem path under
    /// the repository root.
    fn native_path(&self, std_path: &str) -> PathBuf {
        let rel = std_path.strip_prefix("./").unwrap_or(std_path);
        let mut path = self.root.clone();
        for part in rel.split('/') {
            if !part.is_empty() {
                path.push(part);
            }
        }
        path
    }

    fn set_mtime(path: &Path, mtime: DateTime<Utc>) -> io::Result<()> {
        let f = fs::OpenOptions::new().write(true).open(path)?;
        f.set_times(fs::FileTimes::new().set_modified(SystemTime::from(mtime)))
    }

    fn ensure_parent(path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Removes now-empty parent directories of `file_path`, stopping at the
    /// repository root.
    fn remove_empty_parents(&self, file_path: &Path) {
        let mut current = file_path.parent().map(Path::to_path_buf);
        while let Some(dir) = current {
            if dir == self.root {
                break;
            }
            let is_empty = match fs::read_dir(&dir) {
                Ok(mut entries) => entries.next().is_none(),
                Err(_) => false,
            };
            if !is_empty {
                break; // non-empty or unreadable
            }
            if fs::remove_dir(&dir).is_err() {
                break;
            }
            current = dir.parent().map(Path::to_path_buf);
        }
    }
}

impl RepositoryProxy for LocalRepositoryProxy {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn manifest_file(&self, std_path: &str) -> Option<&ManifestFileInfo> {
        self.manifest.get_file(std_path)
    }

    fn get_file_path(&self, std_path: &str) -> io::Result<PathBuf> {
        Ok(self.native_path(std_path))
    }

    fn put_file(&mut self, source: &dyn RepositoryProxy, std_path: &str) -> io::Result<()> {
        let src_path = source.get_file_path(std_path)?;
        let info = source.manifest_file(std_path).cloned().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "source file missing from manifest")
        })?;

        let dest_path = self.native_path(std_path);
        Self::ensure_parent(&dest_path)?;
        fs::copy(&src_path, &dest_path)?;

        let mtime = info.last_modified_utc;
        self.manifest.insert_file(std_path, info);
        Self::set_mtime(&dest_path, mtime)?;
        self.changed = true;
        Ok(())
    }

    fn remove_file(&mut self, std_path: &str) -> io::Result<()> {
        let path = self.native_path(std_path);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        self.remove_empty_parents(&path);
        self.manifest.remove_file(std_path);
        self.changed = true;
        Ok(())
    }

    fn copy_file(
        &mut self,
        existing: &str,
        new_path: &str,
        new_info: &ManifestFileInfo,
    ) -> io::Result<()> {
        let from = self.native_path(existing);
        let to = self.native_path(new_path);
        Self::ensure_parent(&to)?;
        fs::copy(&from, &to)?;

        self.manifest.insert_file(new_path, new_info.clone());
        Self::set_mtime(&to, new_info.last_modified_utc)?;
        self.changed = true;
        Ok(())
    }

    fn move_file(
        &mut self,
        existing: &str,
        new_path: &str,
        new_info: &ManifestFileInfo,
    ) -> io::Result<()> {
        let from = self.native_path(existing);
        let to = self.native_path(new_path);
        Self::ensure_parent(&to)?;
        fs::rename(&from, &to)?;
        self.remove_empty_parents(&from);

        self.manifest.remove_file(existing);
        self.manifest.insert_file(new_path, new_info.clone());
        Self::set_mtime(&to, new_info.last_modified_utc)?;
        self.changed = true;
        Ok(())
    }

    fn copy_file_information(
        &mut self,
        std_path: &str,
        source_file: &ManifestFileInfo,
    ) -> io::Result<()> {
        if let Some(file) = self.manifest.get_file_mut(std_path) {
            file.last_modified_utc = source_file.last_modified_utc;
            file.registered_utc = source_file.registered_utc;
            let mtime = file.last_modified_utc;
            let path = self.native_path(std_path);
            Self::set_mtime(&path, mtime)?;
            self.changed = true;
        }
        Ok(())
    }

    fn copy_manifest_information(&mut self, source: &Manifest) -> io::Result<()> {
        self.manifest.copy_manifest_info_from(source);
        self.changed = true;
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.changed && !self.read_only {
            self.manifest.last_update_date_utc = Utc::now();
            manifest_io::write_manifest(&self.manifest_path, &self.manifest)?;
            self.changed = false;
        }
        Ok(())
    }
}
