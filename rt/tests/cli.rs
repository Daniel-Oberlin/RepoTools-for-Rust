//! End-to-end tests of the `rt` binary, scripting the use cases the tool is
//! built for: create, update, validate, rename detection, content/length
//! changes, removals, duplicates, ignoring, and grooming.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn rt(dir: &Path) -> Command {
    let mut c = Command::cargo_bin("rt").unwrap();
    c.current_dir(dir);
    c
}

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// create → the manifest file exists and a clean status reports nothing.
#[test]
fn create_then_clean_status() {
    let dir = TempDir::new().unwrap();
    rt(dir.path()).arg("create").assert().success();
    assert!(dir.path().join(".repositoryManifest").is_file());

    rt(dir.path()).arg("status").assert().success();
}

#[test]
fn update_reports_new_files_and_validate_is_clean() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "hello world\n");
    write(dir.path(), "b.txt", "second\n");
    write(dir.path(), "docs/readme.txt", "doc\n");

    rt(dir.path()).arg("create").assert().success();
    rt(dir.path())
        .arg("update")
        .assert()
        .success()
        .stdout(predicate::str::contains("3 files are new."));

    rt(dir.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems."));
}

#[test]
fn validate_detects_rename_with_track_moves() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "movable content\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    fs::rename(dir.path().join("a.txt"), dir.path().join("renamed.txt")).unwrap();

    // validate returns a non-zero exit code when problems are found.
    rt(dir.path())
        .args(["validate", "--track-moves", "--detail"])
        .assert()
        .failure()
        .stdout(
            predicate::str::contains("1 files were moved.")
                .and(predicate::str::contains("./a.txt -> ./renamed.txt")),
        );
}

#[test]
fn status_detects_changed_and_missing() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "keep.txt", "stable\n");
    write(dir.path(), "edit.txt", "before-edit\n");
    write(dir.path(), "gone.txt", "temporary\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    // A length change is detected even without re-hashing.
    write(dir.path(), "edit.txt", "after a much longer edit\n");
    fs::remove_file(dir.path().join("gone.txt")).unwrap();

    rt(dir.path()).arg("status").assert().success().stdout(
        predicate::str::contains("1 files have changed content.")
            .and(predicate::str::contains("1 files are missing.")),
    );
}

#[test]
fn track_duplicates_reports_matches() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "identical bytes\n");
    write(dir.path(), "copy.txt", "identical bytes\n");
    write(dir.path(), "unique.txt", "different\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    rt(dir.path())
        .args(["status", "--track-duplicates"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 file hashes were duplicates."));
}

#[test]
fn groom_deletes_new_files() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "tracked.txt", "in the manifest\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    // A file not in the manifest.
    write(dir.path(), "stray.txt", "not tracked\n");
    rt(dir.path()).args(["groom", "--force"]).assert().success();

    assert!(!dir.path().join("stray.txt").exists());
    assert!(dir.path().join("tracked.txt").exists());
}

#[test]
fn ignore_pattern_excludes_files() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "keep.txt", "keep\n");
    write(dir.path(), "scratch.tmp", "junk\n");
    rt(dir.path()).arg("create").assert().success();

    // Add an ignore pattern via edit, then update.
    rt(dir.path())
        .args(["edit", "--ignore", r"\.tmp$"])
        .assert()
        .success();
    rt(dir.path())
        .arg("update")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 files are new.")); // only keep.txt

    // The ignored file is not part of the manifest, so validate stays clean.
    rt(dir.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems."));
}

#[test]
fn info_shows_guid_and_counts() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "hello\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    rt(dir.path()).arg("info").assert().success().stdout(
        predicate::str::contains("GUID:")
            .and(predicate::str::contains("Total number of files:         1")),
    );
}

#[test]
fn name_and_hash_method_round_trip_through_info() {
    let dir = TempDir::new().unwrap();
    rt(dir.path())
        .args(["create", "--name", "MyRepo", "--hash-method", "SHA256"])
        .assert()
        .success();

    rt(dir.path()).arg("info").assert().success().stdout(
        predicate::str::contains("Name:                          MyRepo").and(
            predicate::str::contains("Default hash method:           SHA256"),
        ),
    );
}

#[test]
fn rejects_invalid_hash_method() {
    let dir = TempDir::new().unwrap();
    rt(dir.path())
        .args(["create", "--hash-method", "SHA1"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Unrecognized hash method"));
}

/// Creates a repository (manifest + files) rooted at `dir`.
fn make_repo(dir: &Path, files: &[(&str, &str)]) {
    fs::create_dir_all(dir).unwrap();
    for (rel, content) in files {
        write(dir, rel, content);
    }
    rt(dir).arg("create").assert().success();
    rt(dir).arg("update").assert().success();
}

#[test]
fn recursive_processes_every_manifest() {
    // Two independent repositories at different depths under a parent that is
    // not itself a repository.
    let parent = TempDir::new().unwrap();
    make_repo(&parent.path().join("alpha"), &[("a.txt", "A\n")]);
    make_repo(&parent.path().join("nested/beta"), &[("b.txt", "B\n")]);

    // `--recursive` from the parent finds and validates both manifests.
    rt(parent.path())
        .args(["validate", "--recursive"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems.").count(2));
}

#[test]
fn recursive_without_recursive_flag_uses_cwd_only() {
    // The parent has no manifest of its own; a plain status there fails to read.
    let parent = TempDir::new().unwrap();
    make_repo(&parent.path().join("alpha"), &[("a.txt", "A\n")]);

    rt(parent.path())
        .arg("status")
        .assert()
        .failure()
        .stdout(predicate::str::contains("Could not read manifest"));
}

#[test]
fn cascade_descends_into_nested_repositories() {
    // An outer repository that itself contains an inner repository.
    let parent = TempDir::new().unwrap();
    let outer = parent.path().join("outer");
    make_repo(&outer, &[("top.txt", "T\n")]);
    make_repo(&outer.join("inner"), &[("deep.txt", "D\n")]);

    // Each processed manifest prints a "files were checked." summary. Without
    // --cascade, recursion stops at the outer manifest (one repo processed).
    rt(parent.path())
        .args(["status", "--recursive"])
        .assert()
        .stdout(predicate::str::contains("files were checked.").count(1));

    // With --cascade, it descends into the nested repository as well (two).
    rt(parent.path())
        .args(["status", "--recursive", "--cascade"])
        .assert()
        .stdout(predicate::str::contains("files were checked.").count(2));
}
