//! The manifest data model. Field names and value encodings are chosen to be
//! semantically interoperable with the .NET (Newtonsoft.Json) manifest format:
//! PascalCase keys, Base64 hash bytes, .NET-style UTC dates, hyphenated GUIDs,
//! and directory contents serialized as JSON objects keyed by name.

use std::io;
use std::path::Path;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::hash::{self, HashType};
use crate::ignore;
use crate::serde_fmt::{base64_bytes, dotnet_date, dotnet_min_date};

/// The hash of a file's contents together with the algorithm used.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FileHash {
    pub hash_type: HashType,
    #[serde(with = "base64_bytes")]
    pub hash_data: Vec<u8>,
}

impl FileHash {
    pub fn new(hash_type: HashType, hash_data: Vec<u8>) -> Self {
        FileHash {
            hash_type,
            hash_data,
        }
    }

    /// Computes the hash of a file using the given algorithm.
    pub fn compute(path: &Path, hash_type: HashType) -> io::Result<Self> {
        Ok(FileHash::new(
            hash_type,
            hash::compute_file(path, hash_type)?,
        ))
    }

    /// Hex rendering of the hash bytes (for display).
    pub fn to_hex(&self) -> String {
        hash::to_hex(&self.hash_data)
    }
}

/// Information about a single file in the repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ManifestFileInfo {
    pub file_length: i64,
    #[serde(with = "dotnet_date")]
    pub last_modified_utc: DateTime<Utc>,
    #[serde(with = "dotnet_date")]
    pub registered_utc: DateTime<Utc>,
    pub file_hash: Option<FileHash>,
    pub name: String,
}

/// Information about a directory: its files and subdirectories, each keyed by
/// name (serialized as a JSON object, matching .NET's `Dictionary`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ManifestDirectoryInfo {
    pub files: IndexMap<String, ManifestFileInfo>,
    pub subdirectories: IndexMap<String, ManifestDirectoryInfo>,
    pub name: String,
}

impl ManifestDirectoryInfo {
    pub fn new(name: impl Into<String>) -> Self {
        ManifestDirectoryInfo {
            files: IndexMap::new(),
            subdirectories: IndexMap::new(),
            name: name.into(),
        }
    }

    /// A directory is empty (and thus pruned from the manifest) when it has no
    /// files and no subdirectories.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.subdirectories.is_empty()
    }
}

/// The full contents of a repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Manifest {
    pub guid: Uuid,
    pub root_directory: ManifestDirectoryInfo,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(with = "dotnet_date")]
    pub inception_date_utc: DateTime<Utc>,
    #[serde(with = "dotnet_date")]
    pub last_update_date_utc: DateTime<Utc>,
    #[serde(with = "dotnet_date")]
    pub last_validate_date_utc: DateTime<Utc>,
    #[serde(with = "dotnet_date")]
    pub manifest_info_last_modified_utc: DateTime<Utc>,
    pub ignore_list: Vec<String>,
    pub default_hash_method: Option<HashType>,
}

impl Manifest {
    /// Creates an empty manifest with the default ("clean") settings, matching
    /// `Manifest.MakeCleanManifest`: a fresh GUID, the default ignore list, and
    /// MD5 as the default hash method. `InceptionDateUtc` is set to now; the
    /// other dates use the .NET min-date sentinel until set.
    pub fn new_clean() -> Self {
        Manifest {
            guid: Uuid::new_v4(),
            root_directory: ManifestDirectoryInfo::new("."),
            name: None,
            description: None,
            inception_date_utc: Utc::now(),
            last_update_date_utc: dotnet_min_date(),
            last_validate_date_utc: dotnet_min_date(),
            manifest_info_last_modified_utc: dotnet_min_date(),
            ignore_list: ignore::default_ignore_patterns(),
            default_hash_method: Some(HashType::Md5),
        }
    }

    /// The default hash method, falling back to MD5 when unset.
    pub fn default_hash(&self) -> HashType {
        self.default_hash_method.unwrap_or_default()
    }

    /// Total number of files in the manifest.
    pub fn count_files(&self) -> u64 {
        fn rec(dir: &ManifestDirectoryInfo) -> u64 {
            let mut n = dir.files.len() as u64;
            for sub in dir.subdirectories.values() {
                n += rec(sub);
            }
            n
        }
        rec(&self.root_directory)
    }

    /// Total number of bytes across all files in the manifest.
    pub fn count_bytes(&self) -> i64 {
        fn rec(dir: &ManifestDirectoryInfo) -> i64 {
            let mut n: i64 = dir.files.values().map(|f| f.file_length).sum();
            for sub in dir.subdirectories.values() {
                n += rec(sub);
            }
            n
        }
        rec(&self.root_directory)
    }

    /// Looks up a file by its standard path (e.g. `./dir/file.txt`).
    pub fn get_file(&self, std_path: &str) -> Option<&ManifestFileInfo> {
        let (dirs, name) = split_std_path(std_path)?;
        get_rec(&self.root_directory, &dirs, name)
    }

    /// Mutable lookup of a file by its standard path.
    pub fn get_file_mut(&mut self, std_path: &str) -> Option<&mut ManifestFileInfo> {
        let (dirs, name) = split_std_path(std_path)?;
        get_rec_mut(&mut self.root_directory, &dirs, name)
    }

    /// Inserts (or replaces) a file at the given standard path, creating any
    /// intermediate directories. The file's `name` field is set to match.
    pub fn insert_file(&mut self, std_path: &str, mut info: ManifestFileInfo) {
        let Some((dirs, name)) = split_std_path(std_path) else {
            return;
        };
        info.name = name.to_string();
        insert_rec(&mut self.root_directory, &dirs, name, info);
    }

    /// Removes a file at the given standard path, pruning any directories left
    /// empty. Returns the removed entry, if present.
    pub fn remove_file(&mut self, std_path: &str) -> Option<ManifestFileInfo> {
        let (dirs, name) = split_std_path(std_path)?;
        remove_rec(&mut self.root_directory, &dirs, name)
    }

    /// Copies the changeable manifest metadata from another manifest, matching
    /// `Manifest.CopyManifestInfoFrom` in the .NET implementation.
    pub fn copy_manifest_info_from(&mut self, other: &Manifest) {
        self.name = other.name.clone();
        self.description = other.description.clone();
        self.manifest_info_last_modified_utc = other.manifest_info_last_modified_utc;
        self.ignore_list = other.ignore_list.clone();
        self.default_hash_method = other.default_hash_method;
    }
}

/// Compares two dates stored in (different) manifests, tolerating up to 1 ms of
/// difference, matching `Manifest.CompareManifestDates` (JSON stores millisecond
/// precision). Returns true when they are considered equal.
pub fn compare_manifest_dates(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    match (a - b).num_microseconds() {
        Some(us) => us.abs() <= 1000,
        None => false,
    }
}

/// Splits a standard path (`./a/b/file`) into its directory chain and file name.
/// Returns `None` for an empty or directory-only path.
fn split_std_path(std_path: &str) -> Option<(Vec<&str>, &str)> {
    let trimmed = std_path.strip_prefix("./").unwrap_or(std_path);
    if trimmed.is_empty() {
        return None;
    }
    let mut parts: Vec<&str> = trimmed.split('/').collect();
    let name = parts.pop()?;
    if name.is_empty() {
        return None;
    }
    Some((parts, name))
}

fn get_rec<'a>(
    dir: &'a ManifestDirectoryInfo,
    dirs: &[&str],
    name: &str,
) -> Option<&'a ManifestFileInfo> {
    match dirs.split_first() {
        Some((first, rest)) => get_rec(dir.subdirectories.get(*first)?, rest, name),
        None => dir.files.get(name),
    }
}

fn get_rec_mut<'a>(
    dir: &'a mut ManifestDirectoryInfo,
    dirs: &[&str],
    name: &str,
) -> Option<&'a mut ManifestFileInfo> {
    match dirs.split_first() {
        Some((first, rest)) => get_rec_mut(dir.subdirectories.get_mut(*first)?, rest, name),
        None => dir.files.get_mut(name),
    }
}

fn insert_rec(dir: &mut ManifestDirectoryInfo, dirs: &[&str], name: &str, info: ManifestFileInfo) {
    match dirs.split_first() {
        Some((first, rest)) => {
            let sub = dir
                .subdirectories
                .entry((*first).to_string())
                .or_insert_with(|| ManifestDirectoryInfo::new(*first));
            insert_rec(sub, rest, name, info);
        }
        None => {
            dir.files.insert(name.to_string(), info);
        }
    }
}

fn remove_rec(
    dir: &mut ManifestDirectoryInfo,
    dirs: &[&str],
    name: &str,
) -> Option<ManifestFileInfo> {
    match dirs.split_first() {
        Some((first, rest)) => {
            let sub = dir.subdirectories.get_mut(*first)?;
            let removed = remove_rec(sub, rest, name)?;
            if sub.is_empty() {
                dir.subdirectories.shift_remove(*first);
            }
            Some(removed)
        }
        None => dir.files.shift_remove(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn file(len: i64) -> ManifestFileInfo {
        ManifestFileInfo {
            file_length: len,
            last_modified_utc: dotnet_min_date(),
            registered_utc: dotnet_min_date(),
            file_hash: None,
            name: String::new(),
        }
    }

    #[test]
    fn insert_and_get_nested() {
        let mut m = Manifest::new_clean();
        m.insert_file("./a.txt", file(1));
        m.insert_file("./sub/dir/c.txt", file(3));

        assert_eq!(m.count_files(), 2);
        assert_eq!(m.get_file("./a.txt").unwrap().file_length, 1);
        let nested = m.get_file("./sub/dir/c.txt").unwrap();
        assert_eq!(nested.file_length, 3);
        assert_eq!(nested.name, "c.txt"); // name set on insert
        assert!(m.get_file("./missing.txt").is_none());
    }

    #[test]
    fn remove_prunes_empty_directories() {
        let mut m = Manifest::new_clean();
        m.insert_file("./sub/dir/c.txt", file(3));
        m.insert_file("./sub/keep.txt", file(1));

        // Removing the only file under sub/dir prunes sub/dir but keeps sub.
        let removed = m.remove_file("./sub/dir/c.txt").unwrap();
        assert_eq!(removed.file_length, 3);
        assert!(m.get_file("./sub/dir/c.txt").is_none());
        let sub = m.root_directory.subdirectories.get("sub").unwrap();
        assert!(!sub.subdirectories.contains_key("dir"));
        assert!(sub.files.contains_key("keep.txt"));

        // Removing the last file prunes sub entirely.
        m.remove_file("./sub/keep.txt").unwrap();
        assert!(m.root_directory.subdirectories.is_empty());
        assert_eq!(m.count_files(), 0);
    }

    #[test]
    fn compare_manifest_dates_tolerance() {
        let base = dotnet_min_date();
        assert!(compare_manifest_dates(base, base));
        assert!(compare_manifest_dates(
            base,
            base + Duration::microseconds(500)
        ));
        assert!(!compare_manifest_dates(
            base,
            base + Duration::milliseconds(2)
        ));
    }

    #[test]
    fn copy_manifest_info() {
        let mut src = Manifest::new_clean();
        src.name = Some("Src".into());
        src.description = Some("desc".into());
        src.ignore_list.push(r"\.tmp$".into());

        let mut dst = Manifest::new_clean();
        let dst_guid = dst.guid;
        dst.copy_manifest_info_from(&src);

        assert_eq!(dst.name.as_deref(), Some("Src"));
        assert_eq!(dst.description.as_deref(), Some("desc"));
        assert!(dst.ignore_list.contains(&r"\.tmp$".to_string()));
        // Identity (GUID) and file tree are not part of manifest-info copy.
        assert_eq!(dst.guid, dst_guid);
    }
}
