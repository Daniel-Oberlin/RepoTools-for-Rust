//! End-to-end tests of the `rt` binary, scripting the use cases the tool is
//! built for: create, update, validate, rename detection, content/length
//! changes, removals, duplicates, ignoring, and grooming.

use assert_cmd::Command;
use predicates::prelude::*;
use repotools_manifest::serde_fmt::dotnet_min_date;
use repotools_manifest::{io as manifest_io, HashType, Manifest, DEFAULT_MANIFEST_FILE_NAME};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};
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

fn manifest(dir: &Path) -> Manifest {
    manifest_io::read_manifest(&dir.join(DEFAULT_MANIFEST_FILE_NAME)).unwrap()
}

/// Sets a file's last-modified time well into the past (beyond the 2 s tolerance).
fn set_old_mtime(path: &Path) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    let old = SystemTime::now() - Duration::from_secs(3600);
    f.set_times(fs::FileTimes::new().set_modified(old)).unwrap();
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
fn validate_with_jobs_reports_clean_repo() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "hello world\n");
    write(dir.path(), "b.txt", "second\n");

    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    rt(dir.path())
        .args(["validate", "--jobs", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems."));
}

#[test]
fn validate_with_jobs_detects_rename_with_track_moves() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "movable content\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    fs::rename(dir.path().join("a.txt"), dir.path().join("renamed.txt")).unwrap();

    rt(dir.path())
        .args(["validate", "--jobs", "2", "--track-moves", "--detail"])
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

// --- coverage for the remaining options ------------------------------------

#[test]
fn silent_suppresses_output() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "A\n")]);
    rt(dir.path())
        .args(["status", "--silent"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty());
}

#[test]
fn time_reports_duration() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "A\n")]);
    rt(dir.path())
        .args(["status", "--time"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Duration:"));
}

#[test]
fn show_progress_lists_each_file() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "A\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path())
        .args(["update", "--show-progress"])
        .assert()
        .success()
        .stdout(predicate::str::contains("./a.txt [NEW]"));
}

#[test]
fn description_round_trips_through_info() {
    let dir = TempDir::new().unwrap();
    rt(dir.path())
        .args(["create", "--description", "My repository"])
        .assert()
        .success();
    rt(dir.path())
        .arg("info")
        .assert()
        .success()
        .stdout(predicate::str::contains("My repository"));
}

#[test]
fn all_lists_ignored_files_in_detail() {
    let dir = TempDir::new().unwrap();
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path())
        .args(["edit", "--ignore", r"\.log$"])
        .assert()
        .success();
    write(dir.path(), "keep.txt", "keep\n");
    write(dir.path(), "noise.log", "ignore me\n");
    rt(dir.path()).arg("update").assert().success();

    rt(dir.path())
        .args(["status", "--detail", "--all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("./noise.log"));
}

#[test]
fn ignore_new_clears_validate_exit_code() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "A\n")]);
    write(dir.path(), "fresh.txt", "new\n");

    // A new file makes validate fail...
    rt(dir.path()).arg("validate").assert().failure();
    // ...unless --ignore-new is given.
    rt(dir.path())
        .args(["validate", "--ignore-new"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems."));
}

#[test]
fn ignore_date_clears_validate_exit_code() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "A\n")]);
    set_old_mtime(&dir.path().join("a.txt"));

    // A date-only change makes validate fail...
    rt(dir.path()).arg("validate").assert().failure();
    // ...unless --ignore-date is given (and --no-touch so the date isn't reset).
    rt(dir.path())
        .args(["validate", "--ignore-date", "--no-touch"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems."));
}

#[test]
fn back_date_resets_file_date() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "A\n")]);
    set_old_mtime(&dir.path().join("a.txt"));

    // First validate reports the date difference and back-dates the file.
    rt(dir.path())
        .args(["validate", "--back-date"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("last-modified"));
    // The file date now matches the manifest, so a fresh validate is clean.
    rt(dir.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems."));
}

#[test]
fn no_touch_leaves_validation_date_unset() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "A\n")]);

    rt(dir.path())
        .args(["validate", "--no-touch"])
        .assert()
        .success();
    assert_eq!(
        manifest(dir.path()).last_validate_date_utc,
        dotnet_min_date()
    );

    // A plain validate does record the validation date.
    rt(dir.path()).arg("validate").assert().success();
    assert_ne!(
        manifest(dir.path()).last_validate_date_utc,
        dotnet_min_date()
    );
}

#[test]
fn dont_ignore_removes_a_pattern() {
    let dir = TempDir::new().unwrap();
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path())
        .args(["edit", "--ignore", r"\.tmp$"])
        .assert()
        .success();
    assert!(manifest(dir.path())
        .ignore_list
        .contains(&r"\.tmp$".to_string()));

    rt(dir.path())
        .args(["edit", "--dont-ignore", r"\.tmp$"])
        .assert()
        .success();
    assert!(!manifest(dir.path())
        .ignore_list
        .contains(&r"\.tmp$".to_string()));
}

#[test]
fn ignore_default_resets_the_ignore_list() {
    let dir = TempDir::new().unwrap();
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path())
        .args(["edit", "--ignore", r"\.tmp$"])
        .assert()
        .success();

    rt(dir.path())
        .args(["edit", "--ignore-default"])
        .assert()
        .success();
    let list = manifest(dir.path()).ignore_list;
    assert!(!list.contains(&r"\.tmp$".to_string()));
    assert!(list.contains(&r"^\./\.repositoryManifest$".to_string()));
}

#[test]
fn new_hash_recomputes_with_default_method() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "A\n")]); // MD5 by default
    assert_eq!(
        manifest(dir.path())
            .get_file("./a.txt")
            .unwrap()
            .file_hash
            .as_ref()
            .unwrap()
            .hash_type,
        HashType::Md5
    );

    // Change the default method (no rescan), then recompute all hashes.
    rt(dir.path())
        .args(["edit", "--hash-method", "SHA256"])
        .assert()
        .success();
    rt(dir.path())
        .args(["update", "--new-hash"])
        .assert()
        .success();

    assert_eq!(
        manifest(dir.path())
            .get_file("./a.txt")
            .unwrap()
            .file_hash
            .as_ref()
            .unwrap()
            .hash_type,
        HashType::Sha256
    );
    rt(dir.path())
        .arg("validate")
        .assert()
        .success()
        .stdout(predicate::str::contains("No problems."));
}

#[test]
fn re_hash_detects_change_a_plain_status_would_skip() {
    let dir = TempDir::new().unwrap();
    make_repo(dir.path(), &[("a.txt", "aaaa")]);

    // Same length, same (within-tolerance) mtime, different content: a plain
    // status skips re-hashing and sees nothing...
    write(dir.path(), "a.txt", "bbbb");
    rt(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("changed content").not());
    // ...but --re-hash forces the hash check and detects it.
    rt(dir.path())
        .args(["status", "--re-hash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 files have changed content."));
}

#[test]
fn manifest_file_writes_to_custom_path() {
    let dir = TempDir::new().unwrap();
    rt(dir.path())
        .args(["create", "--manifest-file", "custom.manifest"])
        .assert()
        .success();

    assert!(dir.path().join("custom.manifest").is_file());
    assert!(!dir.path().join(DEFAULT_MANIFEST_FILE_NAME).exists());

    rt(dir.path())
        .args(["info", "--manifest-file", "custom.manifest"])
        .assert()
        .success()
        .stdout(predicate::str::contains("GUID:"));
}

#[test]
fn confirm_update_can_decline_the_write() {
    let dir = TempDir::new().unwrap();
    rt(dir.path()).arg("create").assert().success();

    // Declining the prompt leaves the manifest unchanged.
    rt(dir.path())
        .args(["edit", "--name", "Renamed", "--confirm-update"])
        .write_stdin("n\n")
        .assert()
        .success();
    assert_eq!(manifest(dir.path()).name, None);

    // Confirming applies the edit.
    rt(dir.path())
        .args(["edit", "--name", "Renamed", "--confirm-update"])
        .write_stdin("y\n")
        .assert()
        .success();
    assert_eq!(manifest(dir.path()).name.as_deref(), Some("Renamed"));
}
