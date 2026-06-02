//! Reading and writing the on-disk `.repositoryManifest` JSON file.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::model::Manifest;

/// Reads and parses a manifest JSON file.
///
/// Only the JSON format is supported. The deprecated .NET `BinaryFormatter`
/// format (which `.NET rs seed` still produces) is detected and reported
/// clearly rather than surfacing a raw UTF-8 decode error.
pub fn read_manifest(path: &Path) -> io::Result<Manifest> {
    let bytes = fs::read(path)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "manifest is not valid UTF-8 JSON \
             (the deprecated .NET binary manifest format is not supported)",
        )
    })?;
    serde_json::from_str(text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
