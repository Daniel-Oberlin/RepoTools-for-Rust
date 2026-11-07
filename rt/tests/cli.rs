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

// --- --json and --path --------------------------------------------------------

/// Runs rt with --json and parses each stdout line as a JSON object.
fn rt_json(dir: &Path, args: &[&str]) -> (i32, Vec<serde_json::Value>) {
    let out = rt(dir).args(args).arg("--json").output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let objs = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect();
    (out.status.code().unwrap(), objs)
}

fn strs(v: &serde_json::Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn json_status_reports_categories() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "keep.txt", "keep\n");
    write(dir.path(), "gone.txt", "gone\n");
    write(dir.path(), "edit.txt", "before\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();

    fs::remove_file(dir.path().join("gone.txt")).unwrap();
    write(dir.path(), "edit.txt", "after, and longer\n");
    write(dir.path(), "sub/new.txt", "new\n");

    let (code, objs) = rt_json(dir.path(), &["status"]);
    assert_eq!(code, 0);
    assert_eq!(objs.len(), 1);
    let o = &objs[0];
    assert_eq!(o["ok"], true);
    assert_eq!(o["command"], "status");
    assert_eq!(o["different"], true);
    assert!(o["scope"].is_null());
    assert_eq!(strs(&o["missing"]), vec!["./gone.txt"]);
    assert_eq!(strs(&o["changed"]), vec!["./edit.txt"]);
    assert_eq!(strs(&o["new"]), vec!["./sub/new.txt"]);
    assert_eq!(o["files_checked"], 3);
    assert!(o["root"].as_str().unwrap().len() > 1);
}

#[test]
fn json_update_reports_manifest_written_and_moves() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a/one.txt", "one\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();
    fs::create_dir_all(dir.path().join("b")).unwrap();
    fs::rename(dir.path().join("a/one.txt"), dir.path().join("b/one.txt")).unwrap();

    let (code, objs) = rt_json(dir.path(), &["update", "--track-moves"]);
    assert_eq!(code, 0);
    let o = &objs[0];
    assert_eq!(o["manifest_written"], true);
    let moved = o["moved"].as_array().unwrap();
    assert_eq!(moved.len(), 1);
    assert_eq!(strs(&moved[0]["old_paths"]), vec!["./a/one.txt"]);
    assert_eq!(strs(&moved[0]["new_paths"]), vec!["./b/one.txt"]);
    assert_eq!(moved[0]["hash_type"], "MD5");
    assert_eq!(moved[0]["hash"].as_str().unwrap().len(), 32);
    assert!(manifest(dir.path()).get_file("./b/one.txt").is_some());
}

#[test]
fn json_errors_are_json() {
    let dir = TempDir::new().unwrap(); // no manifest
    let (code, objs) = rt_json(dir.path(), &["status"]);
    assert_ne!(code, 0);
    assert_eq!(objs[0]["ok"], false);
    assert!(objs[0]["error"]
        .as_str()
        .unwrap()
        .contains("Could not read manifest"));
}

#[test]
fn json_info() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a\n");
    rt(dir.path())
        .args(["create", "--name", "Repo"])
        .assert()
        .success();
    rt(dir.path()).arg("update").assert().success();
    let (code, objs) = rt_json(dir.path(), &["info"]);
    assert_eq!(code, 0);
    let o = &objs[0];
    assert_eq!(o["name"], "Repo");
    assert_eq!(o["file_count"], 1);
    assert_eq!(o["default_hash_method"], "MD5");
    assert_eq!(o["guid"], manifest(dir.path()).guid.to_string());
}

/// With --json, prompts are answered "no": groom deletes nothing without
/// --force, and says so.
#[test]
fn json_groom_needs_force() {
    let dir = TempDir::new().unwrap();
    rt(dir.path()).arg("create").assert().success();
    write(dir.path(), "stray.txt", "x\n");

    let (_, objs) = rt_json(dir.path(), &["groom"]);
    assert_eq!(objs[0]["declined"], true);
    assert!(objs[0]["deleted"].as_array().unwrap().is_empty());
    assert!(dir.path().join("stray.txt").exists());

    let (_, objs) = rt_json(dir.path(), &["groom", "--force"]);
    assert_eq!(objs[0]["declined"], false);
    assert_eq!(objs[0]["deleted"].as_array().unwrap().len(), 1);
    assert!(!dir.path().join("stray.txt").exists());
}

/// Progress goes to stderr with --json, so stdout stays parseable.
#[test]
fn json_progress_goes_to_stderr() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a\n");
    rt(dir.path()).arg("create").assert().success();
    let out = rt(dir.path())
        .args(["update", "--json", "--show-progress", "--time"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(stdout.lines().count(), 1);
    serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap();
    assert!(stderr.contains("./a.txt"));
    assert!(stderr.contains("Duration:"));
}

/// --recursive with --json writes one object per repository.
#[test]
fn json_recursive_one_line_per_repository() {
    let dir = TempDir::new().unwrap();
    for sub in ["r1", "r2"] {
        let d = dir.path().join(sub);
        write(&d, "f.txt", sub);
        rt(&d).arg("create").assert().success();
        rt(&d).arg("update").assert().success();
    }
    let (code, objs) = rt_json(dir.path(), &["status", "--recursive"]);
    assert_eq!(code, 0);
    assert_eq!(objs.len(), 2);
    assert!(objs
        .iter()
        .all(|o| o["ok"] == true && o["different"] == false));
}

#[test]
fn path_limits_update_to_one_subtree() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Jazz/Miles/a.flac", "a\n");
    write(dir.path(), "Pop/Yes/c.flac", "c\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();
    let update_date = manifest(dir.path()).last_update_date_utc;

    write(dir.path(), "Jazz/Miles/b.flac", "b\n");
    write(dir.path(), "Pop/Yes/d.flac", "d\n");

    let (code, objs) = rt_json(dir.path(), &["update", "--path", "Jazz/Miles"]);
    assert_eq!(code, 0);
    let o = &objs[0];
    assert_eq!(strs(&o["new"]), vec!["./Jazz/Miles/b.flac"]);
    assert_eq!(strs(&o["scope"]["dirs"]), vec!["./Jazz/Miles/"]);
    assert_eq!(o["manifest_written"], true);

    let m = manifest(dir.path());
    assert!(m.get_file("./Jazz/Miles/b.flac").is_some());
    assert!(m.get_file("./Pop/Yes/d.flac").is_none());
    assert_eq!(m.last_update_date_utc, update_date);

    // The rest is still pending for a full status.
    rt(dir.path())
        .arg("status")
        .assert()
        .stdout(predicate::str::contains("1 files are new."));
}

#[test]
fn path_validate_does_not_touch_validation_date() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "Jazz/a.flac", "a\n");
    rt(dir.path()).arg("create").assert().success();
    rt(dir.path()).arg("update").assert().success();
    let before = manifest(dir.path()).last_validate_date_utc;

    let (code, objs) = rt_json(dir.path(), &["validate", "--path", "Jazz"]);
    assert_eq!(code, 0);
    assert_eq!(objs[0]["problems"], false);
    assert_eq!(objs[0]["validation_date_written"], false);
    assert_eq!(manifest(dir.path()).last_validate_date_utc, before);

    let (_, objs) = rt_json(dir.path(), &["validate"]);
    assert_eq!(objs[0]["validation_date_written"], true);
    assert!(manifest(dir.path()).last_validate_date_utc > before);
}

#[test]
fn path_errors() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "a.txt", "a\n");
    rt(dir.path()).arg("create").assert().success();

    rt(dir.path())
        .args(["status", "--path", "nope"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not in the repository"));
    rt(dir.path())
        .args(["status", "--path", "../elsewhere"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("outside the repository"));
    rt(dir.path())
        .args(["info", "--path", "a.txt"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("--path only applies"));
    rt(dir.path())
        .args(["status", "--recursive", "--path", "a.txt"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("--recursive"));
}
