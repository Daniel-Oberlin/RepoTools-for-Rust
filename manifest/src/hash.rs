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
    let mut file = File::open(path)?;
    compute_reader(&mut file, hash_type)
}

/// Computes the hash of an arbitrary reader's contents.
pub fn compute_reader<R: Read>(reader: &mut R, hash_type: HashType) -> io::Result<Vec<u8>> {
    match hash_type {
        HashType::Md5 => hash_with::<md5::Md5, _>(reader),
        HashType::Sha256 => hash_with::<sha2::Sha256, _>(reader),
    }
}

/// Convenience for hashing an in-memory byte slice (used in tests).
pub fn compute_bytes(data: &[u8], hash_type: HashType) -> Vec<u8> {
    let mut cursor = io::Cursor::new(data);
    compute_reader(&mut cursor, hash_type).expect("hashing a slice cannot fail")
}

fn hash_with<D: Digest, R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut hasher = D::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
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
