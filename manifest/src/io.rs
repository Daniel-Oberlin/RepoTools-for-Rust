//! Reading and writing the on-disk `.repositoryManifest` JSON file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::model::Manifest;

/// Reads and parses a manifest JSON file.
pub fn read_manifest(path: &Path) -> io::Result<Manifest> {
    let data = fs::read_to_string(path)?;
    serde_json::from_str(&data).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Writes a manifest to disk, mirroring the .NET writer's crash-safety: the
/// existing file is first renamed to `<path>.bak`; on success the backup is
/// removed, on failure it is restored.
pub fn write_manifest(path: &Path, manifest: &Manifest) -> io::Result<()> {
    let backup = backup_path(path);

    if path.exists() {
        // Best-effort move of the existing manifest out of the way.
        let _ = fs::rename(path, &backup);
    }

    let json = serde_json::to_string(manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    match fs::write(path, json) {
        Ok(()) => {
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        Err(e) => {
            if backup.exists() {
                let _ = fs::rename(&backup, path);
            }
            Err(e)
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut os = path.to_path_buf().into_os_string();
    os.push(".bak");
    PathBuf::from(os)
}
