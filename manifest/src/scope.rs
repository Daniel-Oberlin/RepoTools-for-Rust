//! Limiting a scan to part of a repository.
//!
//! A [`ScanScope`] is a set of directories and/or files, given as standard
//! manifest paths (`./dir/sub/` for a directory, `./dir/file` for a file). A
//! scoped scan only looks at, reports, and changes manifest entries inside the
//! scope; everything outside it is left exactly as it is in the manifest (not
//! reported as missing, not re-checked).
//!
//! The usual way to build one is [`ScanScope::add_user_path`], which accepts
//! the forms a user would type (relative to the repository root, or absolute
//! under it) and works out whether the path names a directory or a file, using
//! the filesystem first and the manifest for paths that no longer exist.

use std::path::{Component, Path};

use crate::model::{Manifest, ManifestDirectoryInfo};
use crate::paths::normalize_nfc;

/// A set of directories and files a scan is limited to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanScope {
    /// Directory prefixes, each in standard form ending with `/` (`./` is the
    /// whole repository).
    dirs: Vec<String>,
    /// Individual files, in standard form (`./dir/file`).
    files: Vec<String>,
}

impl ScanScope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a directory given as a standard prefix (`./a/b/`; a missing
    /// trailing `/` is added).
    pub fn add_dir(&mut self, std_dir: &str) {
        let mut d = std_dir.to_string();
        if !d.ends_with('/') {
            d.push('/');
        }
        if !self.dirs.contains(&d) {
            self.dirs.push(d);
        }
    }

    /// Adds a single file given as a standard path (`./a/b/file`).
    pub fn add_file(&mut self, std_path: &str) {
        let f = std_path.to_string();
        if !self.files.contains(&f) {
            self.files.push(f);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.files.is_empty()
    }

    /// The scoped directories (standard prefixes ending with `/`).
    pub fn dirs(&self) -> &[String] {
        &self.dirs
    }

    /// The scoped individual files (standard paths).
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// True when the file at `std_path` is inside the scope.
    pub fn contains_file(&self, std_path: &str) -> bool {
        self.files.iter().any(|f| f == std_path)
            || self.dirs.iter().any(|d| std_path.starts_with(d.as_str()))
    }

    /// True when the whole directory `dir_prefix` (ending with `/`) is inside
    /// the scope.
    pub fn contains_dir(&self, dir_prefix: &str) -> bool {
        self.dirs.iter().any(|d| dir_prefix.starts_with(d.as_str()))
    }

    /// True when a scan must enter `dir_prefix`: it is inside the scope, or it
    /// is an ancestor of something in the scope.
    pub fn should_visit(&self, dir_prefix: &str) -> bool {
        self.contains_dir(dir_prefix)
            || self.dirs.iter().any(|d| d.starts_with(dir_prefix))
            || self.files.iter().any(|f| f.starts_with(dir_prefix))
    }

    /// Adds a path as a user would give it: relative to the repository `root`
    /// (`Music/Jazz`, `./Music/Jazz/`, `Music/Jazz/track.flac`), or absolute
    /// and under `root`. An empty path or `.` means the whole repository.
    ///
    /// Whether it names a directory or a file is decided by the filesystem
    /// when the path exists, otherwise by the manifest (so a scope can cover
    /// something that was deleted or moved away). A path that is in neither is
    /// an error, as is one that leaves the repository (`..`).
    pub fn add_user_path(
        &mut self,
        root: &Path,
        manifest: &Manifest,
        input: &str,
    ) -> Result<(), String> {
        let rel = relative_components(root, input)?;
        if rel.is_empty() {
            self.add_dir("./");
            return Ok(());
        }

        let fs_path = rel.iter().fold(root.to_path_buf(), |p, c| p.join(c));
        let std = format!("./{}", rel.join("/"));

        if fs_path.is_dir() {
            self.add_dir(&std);
        } else if fs_path.is_file() {
            self.add_file(&std);
        } else if manifest_dir(&manifest.root_directory, &rel).is_some() {
            self.add_dir(&std);
        } else if manifest.get_file(&std).is_some() {
            self.add_file(&std);
        } else {
            return Err(format!(
                "Path is not in the repository or its manifest: {input}"
            ));
        }
        Ok(())
    }
}

/// Splits a user-supplied path into NFC-normalized components relative to
/// `root`, rejecting paths outside the repository.
fn relative_components(root: &Path, input: &str) -> Result<Vec<String>, String> {
    let given = Path::new(input);
    let rel_path = if given.is_absolute() {
        // Compare against the canonical root; canonicalize the input too when
        // it exists, so symlinked spellings of the root still match.
        let canon = std::fs::canonicalize(given).unwrap_or_else(|_| given.to_path_buf());
        match canon
            .strip_prefix(root)
            .or_else(|_| given.strip_prefix(root))
        {
            Ok(r) => r.to_path_buf(),
            Err(_) => return Err(format!("Path is outside the repository: {input}")),
        }
    } else {
        given.to_path_buf()
    };

    let mut parts = Vec::new();
    for c in rel_path.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(s) => parts.push(normalize_nfc(&s.to_string_lossy())),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("Path is outside the repository: {input}"))
            }
        }
    }
    Ok(parts)
}

fn manifest_dir<'a>(
    dir: &'a ManifestDirectoryInfo,
    parts: &[String],
) -> Option<&'a ManifestDirectoryInfo> {
    match parts.split_first() {
        Some((first, rest)) => manifest_dir(dir.subdirectories.get(first)?, rest),
        None => Some(dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership() {
        let mut s = ScanScope::new();
        s.add_dir("./Music/Jazz");
        s.add_file("./Music/Pop/hit.flac");

        assert!(s.contains_file("./Music/Jazz/a/b.flac"));
        assert!(s.contains_file("./Music/Pop/hit.flac"));
        assert!(!s.contains_file("./Music/Pop/other.flac"));
        assert!(!s.contains_file("./Music/Jazzy/x.flac")); // prefix needs the '/'

        assert!(s.contains_dir("./Music/Jazz/"));
        assert!(s.contains_dir("./Music/Jazz/sub/"));
        assert!(!s.contains_dir("./Music/"));

        assert!(s.should_visit("./"));
        assert!(s.should_visit("./Music/"));
        assert!(s.should_visit("./Music/Pop/")); // leads to the scoped file
        assert!(s.should_visit("./Music/Jazz/sub/"));
        assert!(!s.should_visit("./Video/"));
        assert!(!s.should_visit("./Music/Rock/"));
    }

    #[test]
    fn whole_repository() {
        let mut s = ScanScope::new();
        s.add_dir("./");
        assert!(s.contains_dir("./anything/"));
        assert!(s.contains_file("./a.txt"));
    }

    #[test]
    fn user_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), "x").unwrap();
        let mut m = Manifest::new_clean();
        m.insert_file(
            "./gone/old.txt",
            crate::model::ManifestFileInfo {
                file_length: 1,
                last_modified_utc: crate::serde_fmt::dotnet_min_date(),
                registered_utc: crate::serde_fmt::dotnet_min_date(),
                file_hash: None,
                name: String::new(),
            },
        );

        let mut s = ScanScope::new();
        s.add_user_path(&root, &m, "a/b/").unwrap();
        s.add_user_path(&root, &m, "./a/b/f.txt").unwrap();
        s.add_user_path(&root, &m, root.join("a").to_str().unwrap())
            .unwrap();
        s.add_user_path(&root, &m, "gone").unwrap(); // only in the manifest
        s.add_user_path(&root, &m, "gone/old.txt").unwrap();
        assert_eq!(s.dirs(), &["./a/b/", "./a/", "./gone/"]);
        assert_eq!(s.files(), &["./a/b/f.txt", "./gone/old.txt"]);

        assert!(s.add_user_path(&root, &m, "nope").is_err());
        assert!(s.add_user_path(&root, &m, "../x").is_err());
        assert!(s.add_user_path(&root, &m, "/etc").is_err());

        let mut whole = ScanScope::new();
        whole.add_user_path(&root, &m, ".").unwrap();
        assert_eq!(whole.dirs(), &["./"]);
    }
}
