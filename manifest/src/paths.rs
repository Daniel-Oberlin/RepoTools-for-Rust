//! Path helpers. The manifest uses platform-independent "standard" paths with a
//! `/` delimiter, rooted at `.` (e.g. `./dir/file.txt`). File and directory
//! names are NFC-normalized so that comparisons are stable across platforms
//! (notably macOS, which may present filenames in NFD).

use unicode_normalization::UnicodeNormalization;

/// The delimiter used in standard manifest paths.
pub const STANDARD_DELIMITER: char = '/';

/// Normalizes a name to Unicode NFC, matching the .NET `String.Normalize()`
/// calls in the scanning code.
pub fn normalize_nfc(name: &str) -> String {
    name.nfc().collect()
}

/// Builds the standard path for a directory given its chain of names from the
/// root. The root directory itself (name `.`) yields `./`; a child `sub` yields
/// `./sub/`.
pub fn standard_dir_path(names_from_root: &[&str]) -> String {
    let mut path = String::from(".");
    path.push(STANDARD_DELIMITER);
    for name in names_from_root {
        path.push_str(name);
        path.push(STANDARD_DELIMITER);
    }
    path
}

/// Joins a directory's standard prefix (ending in `/`) with a file name.
pub fn standard_file_path(dir_prefix: &str, file_name: &str) -> String {
    format!("{dir_prefix}{file_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_paths() {
        assert_eq!(standard_dir_path(&[]), "./");
        assert_eq!(standard_dir_path(&["sub"]), "./sub/");
        assert_eq!(standard_dir_path(&["a", "b"]), "./a/b/");
        assert_eq!(standard_file_path("./", "a.txt"), "./a.txt");
        assert_eq!(standard_file_path("./sub/", "c.txt"), "./sub/c.txt");
    }

    #[test]
    fn nfc_normalization() {
        // "é" as NFD (e + combining acute) normalizes to the single NFC code point.
        let nfd = "e\u{0301}.txt";
        let nfc = "\u{00e9}.txt";
        assert_eq!(normalize_nfc(nfd), nfc);
        assert_eq!(normalize_nfc(nfc), nfc);
    }
}
