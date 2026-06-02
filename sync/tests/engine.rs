//! Engine tests driving `do_update` through real `LocalRepositoryProxy`
//! instances over temp-directory repositories.

use std::fs;
use std::path::Path;

use repotools_manifest::ignore::IgnoreSet;
use repotools_manifest::scan::{scan, ScanOptions};
use repotools_manifest::{io as manifest_io, Manifest, DEFAULT_MANIFEST_FILE_NAME};
use repotools_sync::{
    compare_manifests, do_update, LocalRepositoryProxy, RepositoryProxy, SyncOptions,
};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// Builds a real repository: writes the files, scans them into a manifest, and
/// writes the manifest to disk. Returns the in-memory manifest.
fn build_repo(root: &Path, files: &[(&str, &str)]) -> Manifest {
    for (rel, content) in files {
        write(root, rel, content);
    }
    let mut m = Manifest::new_clean();
    let ignore = IgnoreSet::compile(&m.ignore_list).unwrap();
    let abs = root.join(DEFAULT_MANIFEST_FILE_NAME);
    scan(
        root,
        &mut m,
        &ScanOptions {
            update: true,
            ..Default::default()
        },
        &ignore,
        &abs,
    );
    manifest_io::write_manifest(&abs, &m).unwrap();
    m
}

fn read_manifest(root: &Path) -> Manifest {
    manifest_io::read_manifest(&root.join(DEFAULT_MANIFEST_FILE_NAME)).unwrap()
}

fn no_log(_: String) {}

#[test]
fn update_adds_source_only_to_dest() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let source_manifest = build_repo(src.path(), &[("a.txt", "A\n"), ("sub/b.txt", "B\n")]);
    LocalRepositoryProxy::seed(&source_manifest, dst.path()).unwrap();

    let mut source = LocalRepositoryProxy::open(src.path(), true).unwrap();
    let mut dest = LocalRepositoryProxy::open(dst.path(), false).unwrap();
    do_update(&mut source, &mut dest, &SyncOptions::default(), &mut no_log).unwrap();
    dest.flush().unwrap();

    assert!(dst.path().join("a.txt").is_file());
    assert!(dst.path().join("sub/b.txt").is_file());
    assert!(compare_manifests(&source_manifest, &read_manifest(dst.path())).is_empty());
}

#[test]
fn preview_changes_nothing() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    let source_manifest = build_repo(src.path(), &[("a.txt", "A\n"), ("b.txt", "B\n")]);
    LocalRepositoryProxy::seed(&source_manifest, dst.path()).unwrap();

    let mut source = LocalRepositoryProxy::open(src.path(), true).unwrap();
    let mut dest = LocalRepositoryProxy::open(dst.path(), false).unwrap();
    do_update(
        &mut source,
        &mut dest,
        &SyncOptions {
            preview: true,
            ..Default::default()
        },
        &mut no_log,
    )
    .unwrap();
    dest.flush().unwrap();

    assert!(!dst.path().join("a.txt").exists());
    assert_eq!(read_manifest(dst.path()).count_files(), 0);
}

#[test]
fn mirror_makes_dest_identical() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    // dest built first so its registered dates are older than source's.
    build_repo(
        dst.path(),
        &[("a.txt", "A\n"), ("stale.txt", "remove me\n")],
    );
    let source_manifest = build_repo(src.path(), &[("a.txt", "A\n"), ("b.txt", "B\n")]);

    let mut source = LocalRepositoryProxy::open(src.path(), true).unwrap();
    let mut dest = LocalRepositoryProxy::open(dst.path(), false).unwrap();
    do_update(
        &mut source,
        &mut dest,
        &SyncOptions {
            mirror: true,
            ..Default::default()
        },
        &mut no_log,
    )
    .unwrap();
    dest.flush().unwrap();

    assert!(dst.path().join("b.txt").is_file());
    assert!(!dst.path().join("stale.txt").exists()); // extra removed
    assert!(compare_manifests(&source_manifest, &read_manifest(dst.path())).is_empty());
}

#[test]
fn sync_propagates_both_directions() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n"), ("only-src.txt", "S\n")]);
    build_repo(dst.path(), &[("a.txt", "A\n"), ("only-dst.txt", "D\n")]);

    let mut source = LocalRepositoryProxy::open(src.path(), false).unwrap();
    let mut dest = LocalRepositoryProxy::open(dst.path(), false).unwrap();
    do_update(
        &mut source,
        &mut dest,
        &SyncOptions {
            both_ways: true,
            ..Default::default()
        },
        &mut no_log,
    )
    .unwrap();
    source.flush().unwrap();
    dest.flush().unwrap();

    // Both sides end up with all three files.
    for repo in [src.path(), dst.path()] {
        assert!(repo.join("a.txt").is_file());
        assert!(repo.join("only-src.txt").is_file());
        assert!(repo.join("only-dst.txt").is_file());
    }
    assert!(compare_manifests(&read_manifest(src.path()), &read_manifest(dst.path())).is_empty());
}

#[test]
fn mirror_applies_a_move() {
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    // Same content at different paths: a rename from dest's view.
    build_repo(dst.path(), &[("readme.txt", "same content\n")]);
    let source_manifest = build_repo(src.path(), &[("docs/readme.txt", "same content\n")]);

    let mut source = LocalRepositoryProxy::open(src.path(), true).unwrap();
    let mut dest = LocalRepositoryProxy::open(dst.path(), false).unwrap();
    do_update(
        &mut source,
        &mut dest,
        &SyncOptions {
            mirror: true,
            ..Default::default()
        },
        &mut no_log,
    )
    .unwrap();
    dest.flush().unwrap();

    assert!(dst.path().join("docs/readme.txt").is_file());
    assert!(!dst.path().join("readme.txt").exists());
    assert!(compare_manifests(&source_manifest, &read_manifest(dst.path())).is_empty());
}
