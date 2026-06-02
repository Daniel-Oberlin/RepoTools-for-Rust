//! Pure comparison of two manifests — a port of `RepositorySync.CompareManifests`
//! plus `TrackMoves`. No filesystem access; operates only on the manifest trees.

use std::collections::{HashMap, HashSet};

use repotools_manifest::model::compare_manifest_dates;
use repotools_manifest::{FileHash, Manifest, ManifestDirectoryInfo, ManifestFileInfo};

/// A set of files that appear to have moved between source and destination:
/// the source locations and destination locations sharing one content hash.
#[derive(Debug, Clone)]
pub struct MovedFileSet {
    pub hash: FileHash,
    pub source_paths: Vec<String>,
    pub dest_paths: Vec<String>,
}

/// The categorized comparison of a source and destination manifest. Path fields
/// are standard manifest paths (e.g. `./dir/file`).
#[derive(Debug, Default)]
pub struct SyncDiff {
    /// Files present only in the source.
    pub source_only: Vec<String>,
    /// Files present only in the destination.
    pub dest_only: Vec<String>,
    /// Files at the same path whose content hash differs.
    pub changed: Vec<String>,
    /// Files whose content matches but whose last-modified date differs.
    pub last_modified: Vec<String>,
    /// Files whose content matches but whose registered date differs.
    pub registered: Vec<String>,
    /// Files that moved (matched across source-only/dest-only by hash).
    pub moved: Vec<MovedFileSet>,
}

impl SyncDiff {
    /// True when there is no difference of any kind between the two manifests.
    pub fn is_empty(&self) -> bool {
        self.source_only.is_empty()
            && self.dest_only.is_empty()
            && self.changed.is_empty()
            && self.last_modified.is_empty()
            && self.registered.is_empty()
            && self.moved.is_empty()
    }
}

/// Compares two manifests, returning the categorized differences.
pub fn compare_manifests(source: &Manifest, dest: &Manifest) -> SyncDiff {
    let mut diff = SyncDiff::default();

    let source_files = collect(&source.root_directory);
    let dest_files = collect(&dest.root_directory);

    let source_map: HashMap<&str, &ManifestFileInfo> =
        source_files.iter().map(|(p, f)| (p.as_str(), *f)).collect();
    let dest_map: HashMap<&str, &ManifestFileInfo> =
        dest_files.iter().map(|(p, f)| (p.as_str(), *f)).collect();

    let mut matched_dest: HashSet<&str> = HashSet::new();

    for (path, sfile) in &source_files {
        if let Some(dfile) = dest_map.get(path.as_str()) {
            matched_dest.insert(path.as_str());
            if sfile.file_hash != dfile.file_hash {
                diff.changed.push(path.clone());
            } else {
                if !compare_manifest_dates(sfile.last_modified_utc, dfile.last_modified_utc) {
                    diff.last_modified.push(path.clone());
                }
                if !compare_manifest_dates(sfile.registered_utc, dfile.registered_utc) {
                    diff.registered.push(path.clone());
                }
            }
        } else {
            diff.source_only.push(path.clone());
        }
    }

    for (path, _) in &dest_files {
        if !matched_dest.contains(path.as_str()) {
            diff.dest_only.push(path.clone());
        }
    }

    track_moves(&mut diff, &source_map, &dest_map);
    diff
}

fn track_moves(
    diff: &mut SyncDiff,
    source_map: &HashMap<&str, &ManifestFileInfo>,
    dest_map: &HashMap<&str, &ManifestFileInfo>,
) {
    // Index dest-only files by content hash.
    let mut dest_by_hash: HashMap<FileHash, Vec<String>> = HashMap::new();
    for path in &diff.dest_only {
        if let Some(h) = dest_map[path.as_str()].file_hash.clone() {
            dest_by_hash.entry(h).or_default().push(path.clone());
        }
    }

    let mut moved: Vec<MovedFileSet> = Vec::new();
    let mut moved_index: HashMap<FileHash, usize> = HashMap::new();
    let mut moved_dest: HashSet<String> = HashSet::new();
    let mut remaining_source: Vec<String> = Vec::new();

    for spath in &diff.source_only {
        let shash = source_map[spath.as_str()].file_hash.clone();
        let matched = shash
            .as_ref()
            .and_then(|h| dest_by_hash.get(h).map(|dests| (h.clone(), dests.clone())));

        match matched {
            Some((h, dpaths)) => {
                let idx = match moved_index.get(&h) {
                    Some(&i) => i,
                    None => {
                        let i = moved.len();
                        for dp in &dpaths {
                            moved_dest.insert(dp.clone());
                        }
                        moved.push(MovedFileSet {
                            hash: h.clone(),
                            source_paths: Vec::new(),
                            dest_paths: dpaths,
                        });
                        moved_index.insert(h, i);
                        i
                    }
                };
                moved[idx].source_paths.push(spath.clone());
            }
            None => remaining_source.push(spath.clone()),
        }
    }

    diff.source_only = remaining_source;
    diff.dest_only = diff
        .dest_only
        .iter()
        .filter(|p| !moved_dest.contains(*p))
        .cloned()
        .collect();
    diff.moved = moved;
}

/// Collects `(standard_path, file)` for every file in a directory tree, in
/// depth-first order (files before subdirectories), matching the .NET walk.
fn collect(root: &ManifestDirectoryInfo) -> Vec<(String, &ManifestFileInfo)> {
    fn rec<'a>(
        dir: &'a ManifestDirectoryInfo,
        prefix: &str,
        out: &mut Vec<(String, &'a ManifestFileInfo)>,
    ) {
        for (name, f) in &dir.files {
            out.push((format!("{prefix}{name}"), f));
        }
        for (name, sub) in &dir.subdirectories {
            rec(sub, &format!("{prefix}{name}/"), out);
        }
    }
    let mut out = Vec::new();
    rec(root, "./", &mut out);
    out
}
