//! End-to-end tests of the `rs` binary over local repositories.

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use repotools_manifest::ignore::IgnoreSet;
use repotools_manifest::scan::{scan, ScanOptions};
use repotools_manifest::{io as manifest_io, Manifest, DEFAULT_MANIFEST_FILE_NAME};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// Builds a real JSON repository (files + manifest) using the manifest library.
fn build_repo(root: &Path, files: &[(&str, &str)]) {
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
}

/// Re-scans an existing repository in place (preserving its GUID), picking up
/// any added/removed files — the library equivalent of `rt update`.
fn rescan(root: &Path) {
    let abs = root.join(DEFAULT_MANIFEST_FILE_NAME);
    let mut m = manifest_io::read_manifest(&abs).unwrap();
    let ignore = IgnoreSet::compile(&m.ignore_list).unwrap();
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
}

fn rs() -> Command {
    Command::cargo_bin("rs").unwrap()
}

#[test]
fn seed_then_update_then_diff() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n"), ("sub/b.txt", "B\n")]);

    // dst exists but is empty; seed needs no pre-existing manifest there.
    rs().args(["seed"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    assert!(dst.path().join(".repositoryManifest").is_file());

    rs().args(["update"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Adding to dest: ./a.txt"));

    rs().args(["diff"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No differences."));

    assert!(dst.path().join("a.txt").is_file());
    assert!(dst.path().join("sub/b.txt").is_file());
}

#[test]
fn diff_reports_and_exits_nonzero_when_different() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n"), ("b.txt", "B\n")]);
    rs().args(["seed"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    // dst is empty: every source file is "only in source".
    rs().args(["diff"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("2 files are only in source."));
}

#[test]
fn preview_does_not_modify_destination() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n")]);
    rs().args(["seed"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    rs().args(["update", "--preview"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Adding to dest: ./a.txt"));

    assert!(!dst.path().join("a.txt").exists());
}

#[test]
fn guid_mismatch_is_rejected() {
    // Two independently-created repos have different GUIDs.
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n")]);
    build_repo(dst.path(), &[("b.txt", "B\n")]);

    rs().args(["update"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Source and destination GUIDs are different.",
        ));
}

#[test]
fn mirror_removes_extra_destination_files() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n"), ("b.txt", "B\n")]);

    // Seed and populate the destination so it shares the source GUID.
    rs().args(["seed"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    rs().args(["update"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    // Introduce a file in the destination that the source does not have.
    write(dst.path(), "extra.txt", "remove me\n");
    rescan(dst.path()); // record extra.txt in the dest manifest (GUID preserved)

    rs().args(["mirror"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    assert!(!dst.path().join("extra.txt").exists());
    assert!(dst.path().join("a.txt").is_file());
    rs().args(["diff"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("No differences."));
}

#[test]
fn detail_lists_differing_files() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n"), ("b.txt", "B\n")]);
    rs().args(["seed"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    rs().args(["diff", "--detail"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains("./a.txt").and(predicate::str::contains("./b.txt")));
}

#[test]
fn silent_suppresses_output_but_keeps_exit_code() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n")]);
    rs().args(["seed"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    // Differences exist (dst is empty): exit code is non-zero but nothing prints.
    rs().args(["diff", "--silent"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty());
}

#[test]
fn time_reports_duration() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n")]);
    rs().args(["seed"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    rs().args(["update", "--time"])
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Duration:"));
}
