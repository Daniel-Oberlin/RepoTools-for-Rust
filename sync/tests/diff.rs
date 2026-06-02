//! Unit tests for the pure two-manifest comparison.

use chrono::Duration;

use repotools_manifest::serde_fmt::dotnet_min_date;
use repotools_manifest::{FileHash, HashType, Manifest, ManifestFileInfo};
use repotools_sync::compare_manifests;

fn file(hash_byte: u8) -> ManifestFileInfo {
    let base = dotnet_min_date();
    ManifestFileInfo {
        file_length: 1,
        last_modified_utc: base,
        registered_utc: base,
        file_hash: Some(FileHash::new(HashType::Md5, vec![hash_byte; 16])),
        name: String::new(),
    }
}

fn manifest_with(files: &[(&str, ManifestFileInfo)]) -> Manifest {
    let mut m = Manifest::new_clean();
    for (path, info) in files {
        m.insert_file(path, info.clone());
    }
    m
}

#[test]
fn source_only_and_dest_only() {
    let source = manifest_with(&[("./a.txt", file(1)), ("./b.txt", file(2))]);
    let dest = manifest_with(&[("./a.txt", file(1)), ("./c.txt", file(3))]);

    let diff = compare_manifests(&source, &dest);
    assert_eq!(diff.source_only, vec!["./b.txt".to_string()]);
    assert_eq!(diff.dest_only, vec!["./c.txt".to_string()]);
    assert!(diff.changed.is_empty());
    assert!(diff.last_modified.is_empty());
    assert!(diff.moved.is_empty());
}

#[test]
fn changed_content() {
    let source = manifest_with(&[("./a.txt", file(1))]);
    let dest = manifest_with(&[("./a.txt", file(9))]); // different hash
    let diff = compare_manifests(&source, &dest);
    assert_eq!(diff.changed, vec!["./a.txt".to_string()]);
    assert!(diff.source_only.is_empty());
    assert!(diff.dest_only.is_empty());
}

#[test]
fn last_modified_only() {
    let source = manifest_with(&[("./a.txt", file(1))]);
    let mut later = file(1);
    later.last_modified_utc = dotnet_min_date() + Duration::hours(1);
    let dest = manifest_with(&[("./a.txt", later)]);

    let diff = compare_manifests(&source, &dest);
    assert_eq!(diff.last_modified, vec!["./a.txt".to_string()]);
    assert!(diff.changed.is_empty());
}

#[test]
fn registered_only() {
    let source = manifest_with(&[("./a.txt", file(1))]);
    let mut later = file(1);
    later.registered_utc = dotnet_min_date() + Duration::hours(1);
    let dest = manifest_with(&[("./a.txt", later)]);

    let diff = compare_manifests(&source, &dest);
    assert_eq!(diff.registered, vec!["./a.txt".to_string()]);
    assert!(diff.last_modified.is_empty());
    assert!(diff.changed.is_empty());
}

#[test]
fn detects_move_by_hash() {
    // Same content (hash 7) at different paths on each side.
    let source = manifest_with(&[("./docs/readme.txt", file(7))]);
    let dest = manifest_with(&[("./readme.txt", file(7))]);

    let diff = compare_manifests(&source, &dest);
    assert!(diff.source_only.is_empty());
    assert!(diff.dest_only.is_empty());
    assert_eq!(diff.moved.len(), 1);
    assert_eq!(
        diff.moved[0].source_paths,
        vec!["./docs/readme.txt".to_string()]
    );
    assert_eq!(diff.moved[0].dest_paths, vec!["./readme.txt".to_string()]);
}

#[test]
fn identical_manifests_have_no_diff() {
    let source = manifest_with(&[("./a.txt", file(1)), ("./sub/b.txt", file(2))]);
    let dest = manifest_with(&[("./a.txt", file(1)), ("./sub/b.txt", file(2))]);
    assert!(compare_manifests(&source, &dest).is_empty());
}
