//! Engine tests for the scan/diff logic, covering the difference categories the
//! .NET tool detects: new, missing, changed (content and length-only),
//! last-modified, moved, duplicate, ignored, newly-ignored, and empty-directory
//! pruning.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use repotools_manifest::ignore::IgnoreSet;
use repotools_manifest::scan::{scan, ScanOptions, ScanResult};
use repotools_manifest::Manifest;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn manifest_abs(root: &Path) -> PathBuf {
    root.join(repotools_manifest::DEFAULT_MANIFEST_FILE_NAME)
}

fn run(root: &Path, manifest: &mut Manifest, opts: ScanOptions) -> ScanResult {
    let ignore = IgnoreSet::compile(&manifest.ignore_list).unwrap();
    scan(root, manifest, &opts, &ignore, &manifest_abs(root))
}

/// An `update` scan: records new files and builds the directory tree.
fn update(root: &Path, manifest: &mut Manifest) -> ScanResult {
    run(
        root,
        manifest,
        ScanOptions {
            update: true,
            ..Default::default()
        },
    )
}

fn set_mtime_secs_ago(path: &Path, secs: u64) {
    let t = SystemTime::now() - Duration::from_secs(secs);
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_times(fs::FileTimes::new().set_modified(t)).unwrap();
}

#[test]
fn update_adds_new_files_and_builds_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "hello\n");
    write(root, "b.txt", "world\n");
    write(root, "sub/c.txt", "nested\n");

    let mut m = Manifest::new_clean();
    let r = update(root, &mut m);

    assert_eq!(r.new_files.len(), 3);
    assert_eq!(r.files_checked, 3);
    assert_eq!(m.count_files(), 3);
    assert!(m.root_directory.files.contains_key("a.txt"));
    let sub = m.root_directory.subdirectories.get("sub").unwrap();
    assert!(sub.files.contains_key("c.txt"));
    // New files carry computed hashes.
    assert!(m.root_directory.files["a.txt"].file_hash.is_some());
}

#[test]
fn clean_status_after_update_reports_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "hello\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    let r = run(root, &mut m, ScanOptions::default());
    assert!(r.new_files.is_empty());
    assert!(r.missing_files.is_empty());
    assert!(r.changed_files.is_empty());
    assert!(r.last_modified_files.is_empty());
}

#[test]
fn detects_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "hello\n");
    write(root, "b.txt", "world\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    fs::remove_file(root.join("b.txt")).unwrap();
    let r = run(root, &mut m, ScanOptions::default());
    assert_eq!(r.missing_files, vec!["./b.txt".to_string()]);
    assert!(!m.root_directory.files.contains_key("b.txt"));
}

#[test]
fn validate_detects_changed_content_same_length() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "aaaa");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    // Same length, different content. `validate` (always_check_hash) re-hashes
    // every file and so detects this even when the mtime is unchanged — whereas
    // `status` would skip a same-length file whose date is within tolerance.
    write(root, "a.txt", "bbbb");
    let r = run(
        root,
        &mut m,
        ScanOptions {
            always_check_hash: true,
            ..Default::default()
        },
    );
    assert_eq!(r.changed_files, vec!["./a.txt".to_string()]);
}

#[test]
fn status_detects_changed_content_when_date_differs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "aaaa");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    // Same length, different content, with a clearly different mtime: `status`
    // hashes because the date moved beyond tolerance, and finds the change.
    write(root, "a.txt", "bbbb");
    set_mtime_secs_ago(&root.join("a.txt"), 3600);
    let r = run(root, &mut m, ScanOptions::default());
    assert_eq!(r.changed_files, vec!["./a.txt".to_string()]);
}

#[test]
fn detects_length_change_without_hashing_in_status() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "short");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    write(root, "a.txt", "a much longer content");
    let r = run(root, &mut m, ScanOptions::default());
    assert_eq!(r.changed_files, vec!["./a.txt".to_string()]);
}

#[test]
fn detects_last_modified_date_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "hello\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    // Change only the mtime, keep content/length identical.
    set_mtime_secs_ago(&root.join("a.txt"), 3600);
    let r = run(
        root,
        &mut m,
        ScanOptions {
            always_check_hash: true,
            ..Default::default()
        },
    );
    assert_eq!(r.last_modified_files, vec!["./a.txt".to_string()]);
    assert!(r.changed_files.is_empty());
}

#[test]
fn back_date_resets_filesystem_date() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "hello\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    set_mtime_secs_ago(&root.join("a.txt"), 3600);
    let r = run(
        root,
        &mut m,
        ScanOptions {
            always_check_hash: true,
            back_date: true,
            ..Default::default()
        },
    );
    assert_eq!(r.last_modified_files.len(), 1);

    // After back-dating, a fresh validate finds no date difference.
    let r2 = run(
        root,
        &mut m,
        ScanOptions {
            always_check_hash: true,
            ..Default::default()
        },
    );
    assert!(r2.last_modified_files.is_empty());
}

#[test]
fn track_moves_detects_rename() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "movable content\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    fs::rename(root.join("a.txt"), root.join("renamed.txt")).unwrap();
    let r = run(
        root,
        &mut m,
        ScanOptions {
            track_moves: true,
            ..Default::default()
        },
    );
    assert_eq!(r.moved_files.len(), 1);
    assert_eq!(r.moved_files[0].old_paths, vec!["./a.txt".to_string()]);
    assert_eq!(
        r.moved_files[0].new_paths,
        vec!["./renamed.txt".to_string()]
    );
    assert!(r.missing_files.is_empty());
    assert!(r.new_files.is_empty());
}

#[test]
fn without_track_moves_rename_is_missing_plus_new() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "movable content\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);

    fs::rename(root.join("a.txt"), root.join("renamed.txt")).unwrap();
    let r = run(
        root,
        &mut m,
        ScanOptions {
            update: true,
            ..Default::default()
        },
    );
    assert_eq!(r.missing_files, vec!["./a.txt".to_string()]);
    assert_eq!(r.new_files, vec!["./renamed.txt".to_string()]);
}

#[test]
fn track_duplicates_groups_identical_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "same bytes\n");
    write(root, "copy.txt", "same bytes\n");
    write(root, "different.txt", "other\n");

    let mut m = Manifest::new_clean();
    let r = run(
        root,
        &mut m,
        ScanOptions {
            update: true,
            track_duplicates: true,
            ..Default::default()
        },
    );
    assert_eq!(r.duplicate_files.len(), 1);
    let mut paths = r.duplicate_files[0].paths.clone();
    paths.sort();
    assert_eq!(paths, vec!["./a.txt".to_string(), "./copy.txt".to_string()]);
}

#[test]
fn ignores_matching_new_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "keep\n");
    write(root, "scratch.tmp", "junk\n");

    let mut m = Manifest::new_clean();
    m.ignore_list.push(r"\.tmp$".to_string());
    let r = update(root, &mut m);

    assert_eq!(r.new_files, vec!["./a.txt".to_string()]);
    assert_eq!(r.ignored_files, vec!["./scratch.tmp".to_string()]);
    assert!(!m.root_directory.files.contains_key("scratch.tmp"));
}

#[test]
fn detects_newly_ignored_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", "keep\n");
    write(root, "note.tmp", "tracked then ignored\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);
    assert!(m.root_directory.files.contains_key("note.tmp"));

    // Add an ignore pattern and rescan.
    m.ignore_list.push(r"\.tmp$".to_string());
    let r = update(root, &mut m);
    assert_eq!(r.newly_ignored_files, vec!["./note.tmp".to_string()]);
    assert!(!m.root_directory.files.contains_key("note.tmp"));
}

#[test]
fn prunes_empty_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "sub/only.txt", "lonely\n");

    let mut m = Manifest::new_clean();
    update(root, &mut m);
    assert!(m.root_directory.subdirectories.contains_key("sub"));

    fs::remove_file(root.join("sub/only.txt")).unwrap();
    let r = update(root, &mut m);
    assert_eq!(r.missing_files, vec!["./sub/only.txt".to_string()]);
    assert!(m.root_directory.subdirectories.is_empty());
}

// --- scoped scans (ScanOptions::scope) ----------------------------------------

fn scoped(dirs: &[&str], files: &[&str]) -> Option<repotools_manifest::ScanScope> {
    let mut s = repotools_manifest::ScanScope::new();
    for d in dirs {
        s.add_dir(d);
    }
    for f in files {
        s.add_file(f);
    }
    Some(s)
}

/// Changes outside the scope are neither reported nor recorded; changes inside
/// it are, exactly as an unscoped scan would.
#[test]
fn scoped_update_only_touches_the_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Jazz/Miles/a.flac", "a\n");
    write(root, "Jazz/Miles/b.flac", "b\n");
    write(root, "Pop/Yes/c.flac", "c\n");
    let mut m = Manifest::new_clean();
    update(root, &mut m);
    let before_update_date = m.last_update_date_utc;

    // Change things both inside and outside the scope.
    fs::remove_file(root.join("Jazz/Miles/a.flac")).unwrap();
    write(root, "Jazz/Miles/new.flac", "new\n");
    fs::remove_file(root.join("Pop/Yes/c.flac")).unwrap();
    write(root, "Pop/Yes/d.flac", "d\n");
    write(root, "top.txt", "t\n");

    let r = run(
        root,
        &mut m,
        ScanOptions {
            update: true,
            scope: scoped(&["./Jazz/Miles/"], &[]),
            ..Default::default()
        },
    );

    assert_eq!(r.missing_files, vec!["./Jazz/Miles/a.flac"]);
    assert_eq!(r.new_files, vec!["./Jazz/Miles/new.flac"]);
    assert_eq!(r.files_checked, 2); // b.flac and new.flac
                                    // Inside the scope the manifest follows the filesystem...
    assert!(m.get_file("./Jazz/Miles/a.flac").is_none());
    assert!(m.get_file("./Jazz/Miles/new.flac").is_some());
    // ...outside it, nothing changed even though the files did.
    assert!(m.get_file("./Pop/Yes/c.flac").is_some());
    assert!(m.get_file("./Pop/Yes/d.flac").is_none());
    assert!(m.get_file("./top.txt").is_none());
    // A partial update doesn't claim the whole repository is up to date.
    assert_eq!(m.last_update_date_utc, before_update_date);

    // An unscoped update then picks up the rest.
    let r = update(root, &mut m);
    assert_eq!(r.missing_files, vec!["./Pop/Yes/c.flac"]);
    assert_eq!(r.new_files.len(), 2);
}

/// A scope can name single files, and a directory that no longer exists on
/// disk (its files are reported missing).
#[test]
fn scoped_files_and_vanished_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Pop/Yes/c.flac", "c\n");
    write(root, "Pop/Yes/keep.flac", "k\n");
    write(root, "Old/Album/x.flac", "x\n");
    let mut m = Manifest::new_clean();
    update(root, &mut m);

    write(root, "Pop/Yes/c.flac", "changed, longer\n");
    write(root, "Pop/Yes/keep.flac", "also changed\n");
    fs::remove_dir_all(root.join("Old")).unwrap();

    let r = run(
        root,
        &mut m,
        ScanOptions {
            scope: scoped(&["./Old/"], &["./Pop/Yes/c.flac"]),
            ..Default::default()
        },
    );
    assert_eq!(r.changed_files, vec!["./Pop/Yes/c.flac"]);
    assert_eq!(r.missing_files, vec!["./Old/Album/x.flac"]);
    assert!(r.new_files.is_empty());
}

/// An album moved from one place to another is seen as a move when both the
/// old and new locations are in the scope.
#[test]
fn scoped_move_between_two_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "Staging/Album/01.flac", "one\n");
    write(root, "Other/z.flac", "z\n");
    let mut m = Manifest::new_clean();
    update(root, &mut m);

    fs::create_dir_all(root.join("Jazz")).unwrap();
    fs::rename(root.join("Staging/Album"), root.join("Jazz/Album")).unwrap();

    let r = run(
        root,
        &mut m,
        ScanOptions {
            update: true,
            track_moves: true,
            scope: scoped(&["./Staging/Album/", "./Jazz/Album/"], &[]),
            ..Default::default()
        },
    );
    assert_eq!(r.moved_files.len(), 1);
    assert_eq!(r.moved_files[0].old_paths, vec!["./Staging/Album/01.flac"]);
    assert_eq!(r.moved_files[0].new_paths, vec!["./Jazz/Album/01.flac"]);
    assert!(r.missing_files.is_empty() && r.new_files.is_empty());
    assert!(m.get_file("./Jazz/Album/01.flac").is_some());
    assert!(m.get_file("./Other/z.flac").is_some());
}
