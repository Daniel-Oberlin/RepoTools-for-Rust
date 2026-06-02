//! The ignore list: regular expressions matched against standard paths
//! (`./dir/file`). Patterns are stored in the manifest as strings.
//!
//! Compatibility note: these are interpreted with the Rust `regex` crate, whose
//! dialect differs subtly from .NET's. The default patterns and typical
//! user-supplied patterns are simple and behave identically across both.

use regex::Regex;

/// A compiled set of ignore patterns.
pub struct IgnoreSet {
    regexes: Vec<Regex>,
}

impl IgnoreSet {
    /// Compiles a list of pattern strings. Returns the first compilation error.
    pub fn compile(patterns: &[String]) -> Result<Self, regex::Error> {
        let regexes = patterns
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(IgnoreSet { regexes })
    }

    /// Returns true if the standard path matches any ignore pattern.
    pub fn is_ignored(&self, standard_path: &str) -> bool {
        self.regexes.iter().any(|r| r.is_match(standard_path))
    }
}

/// The default ignore patterns for a freshly created ("clean") manifest,
/// matching `Manifest.MakeCleanManifest` in the .NET implementation: the
/// manifest file itself and the RepoSync temp directory.
pub fn default_ignore_patterns() -> Vec<String> {
    vec![
        r"^\./\.repositoryManifest$".to_string(),
        r"^\./temp-repository/".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_patterns_match_expected_paths() {
        let set = IgnoreSet::compile(&default_ignore_patterns()).unwrap();

        // The manifest file at the root is ignored.
        assert!(set.is_ignored("./.repositoryManifest"));
        // But not a similarly named file in a subdirectory (anchored at root).
        assert!(!set.is_ignored("./sub/.repositoryManifest"));
        // The temp directory contents are ignored.
        assert!(set.is_ignored("./temp-repository/anything"));
        // Ordinary files are not.
        assert!(!set.is_ignored("./a.txt"));
        assert!(!set.is_ignored("./sub/file.txt"));
    }

    #[test]
    fn user_pattern_matches_extension() {
        let set = IgnoreSet::compile(&[r"/\.DS_Store$".to_string()]).unwrap();
        assert!(set.is_ignored("./sub/.DS_Store"));
        assert!(!set.is_ignored("./sub/keep.txt"));
    }
}
