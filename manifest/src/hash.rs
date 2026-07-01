//! Hash methods and streaming file hashing.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::str::FromStr;

use digest::Digest;
use serde::{Deserialize, Serialize};

/// A hash algorithm supported by the manifest. The string forms (`"MD5"`,
/// `"SHA256"`) match the values the .NET tool stores in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HashType {
    #[serde(rename = "MD5")]
    Md5,
    #[serde(rename = "SHA256")]
    Sha256,
}

impl HashType {
    pub fn as_str(self) -> &'static str {
        match self {
            HashType::Md5 => "MD5",
            HashType::Sha256 => "SHA256",
        }
    }
}

impl Default for HashType {
    /// Matches `CryptUtilities.DefaultHashType` in the .NET implementation.
    fn default() -> Self {
        HashType::Md5
    }
}

impl fmt::Display for HashType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HashType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "MD5" => Ok(HashType::Md5),
            "SHA256" => Ok(HashType::Sha256),
            other => Err(format!("Unrecognized hash method: {other}")),
        }
    }
}

/// Computes the hash of a file's contents by streaming it through the chosen
/// algorithm.
pub fn compute_file(path: &Path, hash_type: HashType) -> io::Result<Vec<u8>> {
    compute_file_with_progress(path, hash_type, None)
}

/// Computes a file hash and reports integer percent completion (0..=100)
/// while bytes are read.
pub fn compute_file_with_progress(
    path: &Path,
    hash_type: HashType,
    mut on_progress: Option<&mut dyn FnMut(u8)>,
) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let total_bytes = file.metadata().ok().map(|m| m.len());
    compute_reader_with_progress(&mut file, hash_type, total_bytes, &mut on_progress)
}

/// Computes the hash of an arbitrary reader's contents.
pub fn compute_reader<R: Read>(reader: &mut R, hash_type: HashType) -> io::Result<Vec<u8>> {
    let mut on_progress: Option<&mut dyn FnMut(u8)> = None;
    compute_reader_with_progress(reader, hash_type, None, &mut on_progress)
}

fn compute_reader_with_progress<R: Read>(
    reader: &mut R,
    hash_type: HashType,
    total_bytes: Option<u64>,
    on_progress: &mut Option<&mut dyn FnMut(u8)>,
) -> io::Result<Vec<u8>> {
    match hash_type {
        HashType::Md5 => hash_with::<md5::Md5, _>(reader, total_bytes, on_progress),
        HashType::Sha256 => hash_with::<sha2::Sha256, _>(reader, total_bytes, on_progress),
    }
}

/// Convenience for hashing an in-memory byte slice (used in tests).
pub fn compute_bytes(data: &[u8], hash_type: HashType) -> Vec<u8> {
    let mut cursor = io::Cursor::new(data);
    compute_reader(&mut cursor, hash_type).expect("hashing a slice cannot fail")
}

fn hash_with<D: Digest, R: Read>(
    reader: &mut R,
    total_bytes: Option<u64>,
    on_progress: &mut Option<&mut dyn FnMut(u8)>,
) -> io::Result<Vec<u8>> {
    let mut hasher = D::new();
    let mut buf = [0u8; 64 * 1024];
    let mut processed: u64 = 0;
    let mut last_percent: u8 = 255;

    if let Some(cb) = on_progress.as_deref_mut() {
        cb(0);
        last_percent = 0;
    }

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        processed = processed.saturating_add(n as u64);

        if let (Some(total), Some(cb)) = (total_bytes, on_progress.as_deref_mut()) {
            let percent = if total == 0 {
                100
            } else {
                ((processed.saturating_mul(100) / total).min(100)) as u8
            };
            if percent != last_percent {
                cb(percent);
                last_percent = percent;
            }
        }
    }

    if let Some(cb) = on_progress.as_deref_mut() {
        if last_percent != 100 {
            cb(100);
        }
    }

    Ok(hasher.finalize().to_vec())
}

/// Renders hash bytes as a lowercase hex string (used for console display and
/// duplicate grouping output).
pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
