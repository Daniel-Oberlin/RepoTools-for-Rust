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

// --- --json -------------------------------------------------------------------

fn rs_json(args: &[&str], src: &Path, dst: &Path) -> (i32, serde_json::Value) {
    let out = rs()
        .args(args)
        .arg(src)
        .arg(dst)
        .arg("--json")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout.lines().count(),
        1,
        "expected one JSON line: {stdout}"
    );
    (
        out.status.code().unwrap(),
        serde_json::from_str(stdout.trim()).unwrap(),
    )
}

#[test]
fn json_diff_and_preview_then_update() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n"), ("sub/b.txt", "B\n")]);
    rs().arg("seed")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    let (code, d) = rs_json(&["diff"], src.path(), dst.path());
    assert_eq!(code, 1);
    assert_eq!(d["ok"], true);
    assert_eq!(d["different"], true);
    assert_eq!(d["source_only"].as_array().unwrap().len(), 2);
    assert!(d["metadata_differences"].as_array().unwrap().is_empty());

    // A preview lists the actions and changes nothing.
    let (code, p) = rs_json(&["update", "--preview"], src.path(), dst.path());
    assert_eq!(code, 0);
    assert_eq!(p["preview"], true);
    let actions = p["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 2);
    assert!(actions
        .iter()
        .all(|a| a["action"] == "add" && a["side"] == "dest"));
    assert!(!dst.path().join("a.txt").exists());

    let (code, u) = rs_json(&["update"], src.path(), dst.path());
    assert_eq!(code, 0);
    assert_eq!(u["preview"], false);
    assert!(dst.path().join("sub/b.txt").exists());

    let (code, d) = rs_json(&["diff"], src.path(), dst.path());
    assert_eq!(code, 0);
    assert_eq!(d["different"], false);
}

#[test]
fn json_move_actions() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("old/a.txt", "A\n")]);
    rs().arg("seed")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    rs().arg("update")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();

    fs::create_dir_all(src.path().join("new")).unwrap();
    fs::rename(src.path().join("old/a.txt"), src.path().join("new/a.txt")).unwrap();
    rescan(src.path());

    let (_, p) = rs_json(&["update", "--preview"], src.path(), dst.path());
    let actions = p["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["action"], "move");
    assert_eq!(actions[0]["side"], "dest");
    assert_eq!(actions[0]["from"], "./old/a.txt");
    assert_eq!(actions[0]["to"], "./new/a.txt");
}

#[test]
fn json_guid_mismatch_is_an_error_object() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n")]);
    build_repo(dst.path(), &[("a.txt", "A\n")]); // a different GUID
    let (code, o) = rs_json(&["update"], src.path(), dst.path());
    assert_eq!(code, 1);
    assert_eq!(o["ok"], false);
    assert!(o["error"].as_str().unwrap().contains("GUIDs"));
}

/// Source and dest in sync with albums A and B; then both change in source.
fn two_albums_changed() -> (TempDir, TempDir) {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("A/1.txt", "a1\n"), ("B/1.txt", "b1\n")]);
    rs().arg("seed")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    rs().arg("update")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(src.path(), "A/1.txt", "a1 changed\n");
    write(src.path(), "A/2.txt", "a2\n");
    write(src.path(), "B/1.txt", "b1 changed\n");
    write(src.path(), "B/2.txt", "b2\n");
    rescan(src.path());
    (src, dst)
}

#[test]
fn path_limits_diff_and_update_to_the_scope() {
    let (src, dst) = two_albums_changed();

    let (code, d) = rs_json(&["diff", "--path", "A"], src.path(), dst.path());
    assert_eq!(code, 1);
    assert_eq!(d["source_only"], serde_json::json!(["./A/2.txt"]));
    assert_eq!(d["changed"], serde_json::json!(["./A/1.txt"]));
    assert!(d["metadata_differences"].as_array().unwrap().is_empty());

    let (code, u) = rs_json(&["update", "--path", "A"], src.path(), dst.path());
    assert_eq!(code, 0);
    let actions = u["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 2, "{actions:?}");
    assert!(actions
        .iter()
        .all(|a| a["path"].as_str().unwrap().starts_with("./A/")));

    assert_eq!(
        fs::read_to_string(dst.path().join("A/1.txt")).unwrap(),
        "a1 changed\n"
    );
    assert!(dst.path().join("A/2.txt").is_file());
    assert_eq!(
        fs::read_to_string(dst.path().join("B/1.txt")).unwrap(),
        "b1\n"
    );
    assert!(!dst.path().join("B/2.txt").exists());

    let (code, _) = rs_json(&["diff", "--path", "A"], src.path(), dst.path());
    assert_eq!(code, 0);
    let (code, d) = rs_json(&["diff"], src.path(), dst.path());
    assert_eq!(code, 1);
    assert_eq!(d["source_only"], serde_json::json!(["./B/2.txt"]));
}

#[test]
fn path_scoped_mirror_removes_only_inside_the_scope() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(
        src.path(),
        &[
            ("A/1.txt", "a1\n"),
            ("A/x.txt", "x\n"),
            ("B/1.txt", "b1\n"),
            ("B/y.txt", "y\n"),
        ],
    );
    rs().arg("seed")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    rs().arg("update")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    fs::remove_file(src.path().join("A/x.txt")).unwrap();
    fs::remove_file(src.path().join("B/y.txt")).unwrap();
    rescan(src.path());

    let (code, _) = rs_json(&["mirror", "--path", "A"], src.path(), dst.path());
    assert_eq!(code, 0);
    assert!(!dst.path().join("A/x.txt").exists());
    assert!(dst.path().join("B/y.txt").is_file());
}

#[test]
fn path_scope_follows_a_renamed_folder_by_either_name() {
    for given in ["new", "old"] {
        let src = TempDir::new().unwrap();
        let dst = TempDir::new().unwrap();
        build_repo(src.path(), &[("old/a.txt", "A\n"), ("other/b.txt", "B\n")]);
        rs().arg("seed")
            .arg(src.path())
            .arg(dst.path())
            .assert()
            .success();
        rs().arg("update")
            .arg(src.path())
            .arg(dst.path())
            .assert()
            .success();
        fs::rename(src.path().join("old"), src.path().join("new")).unwrap();
        fs::rename(
            src.path().join("other/b.txt"),
            src.path().join("other/c.txt"),
        )
        .unwrap();
        rescan(src.path());

        // "old" is only in the destination (and its manifest) now.
        let (code, u) = rs_json(&["update", "--path", given], src.path(), dst.path());
        assert_eq!(code, 0, "{u}");
        let actions = u["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 1, "{given}: {actions:?}");
        assert_eq!(actions[0]["action"], "move");
        assert_eq!(actions[0]["to"], "./new/a.txt");
        assert!(dst.path().join("other/b.txt").is_file()); // outside: untouched
    }
}

#[test]
fn path_in_neither_repository_is_an_error() {
    let src = TempDir::new().unwrap();
    let dst = TempDir::new().unwrap();
    build_repo(src.path(), &[("a.txt", "A\n")]);
    rs().arg("seed")
        .arg(src.path())
        .arg(dst.path())
        .assert()
        .success();
    let (code, r) = rs_json(&["update", "--path", "nope"], src.path(), dst.path());
    assert_eq!(code, 1);
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("nope"));
}
