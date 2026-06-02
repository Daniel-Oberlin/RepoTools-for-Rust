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
}
