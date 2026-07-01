//! The repository scan/diff engine — a port of `RepositoryTool.DoUpdate` /
//! `UpdateRecursive` (plus move and duplicate tracking) from the .NET
//! implementation.
//!
//! [`scan`] walks the filesystem under a repository root, compares it against
//! the manifest, mutates the manifest in place to reflect the filesystem, and
//! returns a [`ScanResult`] categorizing every difference. Callers decide
//! whether to persist the mutated manifest (e.g. `update` does, `status` does
//! not) and what to report.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;

use crate::hash::HashType;
use crate::ignore::IgnoreSet;
use crate::model::{FileHash, Manifest, ManifestDirectoryInfo, ManifestFileInfo};
use crate::paths::{normalize_nfc, standard_file_path};
use crate::serde_fmt::dotnet_min_date;

/// Allowance for difference between a date stored in the manifest and the
/// filesystem's last-modified time (matches `FilesystemDateTimeTolerance` = 2s).
const FILESYSTEM_DATE_TOLERANCE_MS: i64 = 2000;

/// Behavior flags controlling a scan, mirroring the toggles on the .NET
/// `RepositoryTool`.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Bringing the manifest up to date (compute hashes for new/changed files).
    pub update: bool,
    /// Always recompute and check every file's hash (used by `validate`).
    pub always_check_hash: bool,
    /// Recompute all hashes using the default hash method.
    pub make_new_hash: bool,
    /// Reset filesystem dates back to the manifest's recorded dates when only
    /// the date differs.
    pub back_date: bool,
    /// Detect renamed/moved files by matching content hashes.
    pub track_moves: bool,
    /// Detect duplicate files by matching content hashes.
    pub track_duplicates: bool,
    /// Record a per-file progress line in [`ScanResult::progress_log`].
    pub show_progress: bool,
}

/// A set of files that appear to have moved: the old (missing) locations and
/// the new locations sharing the same content hash.
#[derive(Debug, Clone)]
pub struct MovedFileSet {
    pub hash: FileHash,
    pub old_paths: Vec<String>,
    pub new_paths: Vec<String>,
}

/// A set of files sharing the same content hash.
#[derive(Debug, Clone)]
pub struct DuplicateSet {
    pub hash: FileHash,
    pub paths: Vec<String>,
}

/// The categorized outcome of a scan. Path fields are standard manifest paths
/// (e.g. `./dir/file`).
#[derive(Debug, Default)]
pub struct ScanResult {
    pub new_files: Vec<String>,
    pub missing_files: Vec<String>,
    pub changed_files: Vec<String>,
    pub last_modified_files: Vec<String>,
    pub error_files: Vec<String>,
    pub ignored_files: Vec<String>,
    pub newly_ignored_files: Vec<String>,
    pub moved_files: Vec<MovedFileSet>,
    pub duplicate_files: Vec<DuplicateSet>,
    pub files_checked: u64,
    /// Native paths of new files, for grooming.
    pub new_files_for_groom: Vec<PathBuf>,
    /// Native paths of ignored files, for grooming with `--all`.
    pub ignored_files_for_groom: Vec<PathBuf>,
    /// Per-file progress lines, populated only when `show_progress` is set.
    pub progress_log: Vec<String>,
}

/// Scans `root` against `manifest`, mutating the manifest to reflect the
/// filesystem and returning the categorized differences.
///
/// `manifest_abs_path` is the absolute path of the manifest file itself, so it
/// is never offered up for grooming. `ignore` must be compiled from
/// `manifest.ignore_list` by the caller (so regex-compilation errors surface
/// before scanning).
pub fn scan(
    root: &Path,
    manifest: &mut Manifest,
    opts: &ScanOptions,
    ignore: &IgnoreSet,
    manifest_abs_path: &Path,
) -> ScanResult {
    scan_with_progress(root, manifest, opts, ignore, manifest_abs_path, None)
}

/// Like [`scan`], but optionally emits each progress line as soon as it is
/// produced.
pub fn scan_with_progress(
    root: &Path,
    manifest: &mut Manifest,
    opts: &ScanOptions,
    ignore: &IgnoreSet,
    manifest_abs_path: &Path,
    mut on_progress: Option<&mut dyn FnMut(&str)>,
) -> ScanResult {
    let default_hash = manifest.default_hash();
    let mut acc = Accum::default();

    update_recursive(
        Some(root),
        &mut manifest.root_directory,
        "./",
        opts,
        default_hash,
        ignore,
        manifest_abs_path,
        &mut on_progress,
        &mut acc,
    );

    if opts.track_moves {
        track_moves(&mut acc);
    } else {
        acc.result.missing_files = acc.missing_entries.iter().map(|m| m.path.clone()).collect();
        acc.result.new_files = acc.new_entries.iter().map(|e| e.path.clone()).collect();
        acc.result.new_files_for_groom = acc.new_entries.iter().map(|e| e.native.clone()).collect();
    }

    if opts.track_duplicates {
        track_duplicates(manifest, &mut acc.result);
    }

    manifest.last_update_date_utc = Utc::now();
    acc.result
}

// --- internals ---------------------------------------------------------------

#[derive(Default)]
struct Accum {
    result: ScanResult,
    new_entries: Vec<NewEntry>,
    missing_entries: Vec<MissingEntry>,
}

struct NewEntry {
    path: String,
    hash: Option<FileHash>,
    native: PathBuf,
}

#[derive(Clone)]
struct MissingEntry {
    path: String,
    hash: Option<FileHash>,
}

#[allow(clippy::too_many_arguments)]
fn update_recursive(
    fs_dir: Option<&Path>,
    man_dir: &mut ManifestDirectoryInfo,
    dir_prefix: &str,
    opts: &ScanOptions,
    default_hash: HashType,
    ignore: &IgnoreSet,
    manifest_abs_path: &Path,
    on_progress: &mut Option<&mut dyn FnMut(&str)>,
    acc: &mut Accum,
) {
    // Enumerate the filesystem directory (NFC-normalized names).
    let mut fs_files: HashMap<String, PathBuf> = HashMap::new();
    let mut fs_dirs: HashMap<String, PathBuf> = HashMap::new();
    if let Some(d) = fs_dir {
        match fs::read_dir(d) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    let Ok(ft) = entry.file_type() else { continue };
                    let name = normalize_nfc(&entry.file_name().to_string_lossy());
                    if ft.is_dir() {
                        fs_dirs.insert(name, entry.path());
                    } else if ft.is_file() {
                        fs_files.insert(name, entry.path());
                    }
                    // symlinks and other entry types are skipped
                }
            }
            Err(_) => {
                // Directory exists in the manifest but its contents are not
                // accessible. Unless it is ignored, note it and leave the
                // manifest entries untouched (matching the .NET behavior).
                if !ignore.is_ignored(dir_prefix) {
                    acc.result.error_files.push(dir_prefix.to_string());
                }
                return;
            }
        }
    }

    // Process existing manifest file entries.
    let existing_names: Vec<String> = man_dir.files.keys().cloned().collect();
    for name in &existing_names {
        let std_path = standard_file_path(dir_prefix, name);

        let Some(fs_path) = fs_files.get(name) else {
            // Missing from the filesystem.
            let hash = man_dir.files.get(name).and_then(|mf| mf.file_hash.clone());
            man_dir.files.shift_remove(name);
            progress(acc, opts, &std_path, "MISSING", on_progress);
            acc.missing_entries.push(MissingEntry {
                path: std_path,
                hash,
            });
            continue;
        };

        acc.result.files_checked += 1;

        let Some((fs_len, fs_mtime)) = read_len_mtime(fs_path) else {
            progress(acc, opts, &std_path, "ERROR", on_progress);
            acc.result.error_files.push(std_path);
            continue;
        };

        if ignore.is_ignored(&std_path) {
            man_dir.files.shift_remove(name);
            progress(acc, opts, &std_path, "NEWLY IGNORED", on_progress);
            acc.result.newly_ignored_files.push(std_path);
            continue;
        }

        let (man_len, man_mtime, man_hash) = {
            let mf = man_dir.files.get(name).unwrap();
            (mf.file_length, mf.last_modified_utc, mf.file_hash.clone())
        };

        if fs_len != man_len && !opts.update && !opts.always_check_hash {
            // Length differs but we are not updating: report without hashing.
            progress(acc, opts, &std_path, "DIFFERENT", on_progress);
            acc.result.changed_files.push(std_path);
            continue;
        }

        let must_hash = opts.always_check_hash
            || opts.make_new_hash
            || man_hash.is_none()
            || !compare_fs_date(fs_mtime, man_mtime)
            || fs_len != man_len;

        if !must_hash {
            progress(acc, opts, &std_path, "SKIPPED", on_progress);
            continue;
        }

        let hash_type = man_hash
            .as_ref()
            .map(|h| h.hash_type)
            .unwrap_or(default_hash);
        let mut effective_mtime = fs_mtime;

        let mut on_hash_progress = |percent: u8| {
            emit_hash_progress(opts, &std_path, percent, on_progress);
        };

        match FileHash::compute_with_progress(fs_path, hash_type, Some(&mut on_hash_progress)) {
            Ok(check_hash) => {
                match &man_hash {
                    None => {
                        finish_hash_progress(acc, opts, &std_path, "DIFFERENT", on_progress);
                        acc.result.changed_files.push(std_path.clone());
                    }
                    Some(existing) if check_hash != *existing => {
                        finish_hash_progress(acc, opts, &std_path, "DIFFERENT", on_progress);
                        acc.result.changed_files.push(std_path.clone());
                    }
                    Some(_) => {
                        finish_hash_progress(acc, opts, &std_path, "VALIDATED", on_progress);
                        if !compare_fs_date(fs_mtime, man_mtime) {
                            acc.result.last_modified_files.push(std_path.clone());
                            if opts.back_date && set_file_mtime(fs_path, man_mtime).is_ok() {
                                effective_mtime = man_mtime;
                            }
                        }
                    }
                }

                let new_hash = if opts.make_new_hash {
                    match FileHash::compute(fs_path, default_hash) {
                        Ok(h) => Some(h),
                        Err(_) => {
                            acc.result.error_files.push(std_path.clone());
                            None
                        }
                    }
                } else {
                    Some(check_hash)
                };

                let mf = man_dir.files.get_mut(name).unwrap();
                mf.file_hash = new_hash;
                mf.last_modified_utc = effective_mtime;
                mf.file_length = fs_len;
            }
            Err(_) => {
                finish_hash_progress(acc, opts, &std_path, "ERROR", on_progress);
                acc.result.error_files.push(std_path);
                let mf = man_dir.files.get_mut(name).unwrap();
                mf.file_hash = None;
                mf.last_modified_utc = fs_mtime;
                mf.file_length = fs_len;
            }
        }
    }

    // Recurse into existing manifest subdirectories, pruning empties.
    let sub_names: Vec<String> = man_dir.subdirectories.keys().cloned().collect();
    for sub_name in &sub_names {
        let fs_sub = fs_dirs.get(sub_name).map(|p| p.as_path());
        let child_prefix = format!("{dir_prefix}{sub_name}/");
        {
            let sub = man_dir.subdirectories.get_mut(sub_name).unwrap();
            update_recursive(
                fs_sub,
                sub,
                &child_prefix,
                opts,
                default_hash,
                ignore,
                manifest_abs_path,
                on_progress,
                acc,
            );
        }
        if man_dir.subdirectories[sub_name].is_empty() {
            man_dir.subdirectories.shift_remove(sub_name);
        }
    }

    // New files (filesystem files not in the manifest), in stable order.
    let mut new_file_names: Vec<String> = fs_files
        .keys()
        .filter(|n| !man_dir.files.contains_key(n.as_str()))
        .cloned()
        .collect();
    new_file_names.sort();
    for name in &new_file_names {
        let fs_path = fs_files.get(name).unwrap().clone();
        let std_path = standard_file_path(dir_prefix, name);

        if ignore.is_ignored(&std_path) {
            progress(acc, opts, &std_path, "IGNORED", on_progress);
            acc.result.ignored_files.push(std_path);
            if fs_path != manifest_abs_path {
                acc.result.ignored_files_for_groom.push(fs_path);
            }
            continue;
        }

        acc.result.files_checked += 1;

        let Some((fs_len, fs_mtime)) = read_len_mtime(&fs_path) else {
            progress(acc, opts, &std_path, "ERROR", on_progress);
            acc.result.error_files.push(std_path);
            continue;
        };

        let check_hash = opts.update || opts.always_check_hash || opts.track_moves;
        let mut file_hash = None;
        let mut errored = false;
        if check_hash {
            let mut on_hash_progress = |percent: u8| {
                emit_hash_progress(opts, &std_path, percent, on_progress);
            };
            match FileHash::compute_with_progress(&fs_path, default_hash, Some(&mut on_hash_progress)) {
                Ok(h) => file_hash = Some(h),
                Err(_) => errored = true,
            }
        }

        if errored {
            finish_hash_progress(acc, opts, &std_path, "ERROR", on_progress);
            acc.result.error_files.push(std_path.clone());
        } else {
            if check_hash {
                finish_hash_progress(acc, opts, &std_path, "NEW", on_progress);
            } else {
                progress(acc, opts, &std_path, "NEW", on_progress);
            }
            acc.new_entries.push(NewEntry {
                path: std_path.clone(),
                hash: file_hash.clone(),
                native: fs_path.clone(),
            });
        }

        // The entry is added to the manifest regardless (matching .NET); on a
        // hash error it simply carries a null hash.
        man_dir.files.insert(
            name.clone(),
            ManifestFileInfo {
                file_length: fs_len,
                last_modified_utc: fs_mtime,
                registered_utc: Utc::now(),
                file_hash,
                name: name.clone(),
            },
        );
    }

    // New subdirectories (filesystem dirs not in the manifest), pruning empties.
    let mut new_dir_names: Vec<String> = fs_dirs
        .keys()
        .filter(|n| !man_dir.subdirectories.contains_key(n.as_str()))
        .cloned()
        .collect();
    new_dir_names.sort();
    for sub_name in &new_dir_names {
        let fs_sub = fs_dirs.get(sub_name).unwrap().clone();
        let child_prefix = format!("{dir_prefix}{sub_name}/");
        let mut new_sub = ManifestDirectoryInfo::new(sub_name.clone());
        update_recursive(
            Some(&fs_sub),
            &mut new_sub,
            &child_prefix,
            opts,
            default_hash,
            ignore,
            manifest_abs_path,
            on_progress,
            acc,
        );
        if !new_sub.is_empty() {
            man_dir.subdirectories.insert(sub_name.clone(), new_sub);
        }
    }
}

fn track_moves(acc: &mut Accum) {
    // Index new files by hash.
    let mut new_by_hash: HashMap<FileHash, Vec<usize>> = HashMap::new();
    for (i, e) in acc.new_entries.iter().enumerate() {
        if let Some(h) = &e.hash {
            new_by_hash.entry(h.clone()).or_default().push(i);
        }
    }

    let mut moved: Vec<MovedFileSet> = Vec::new();
    let mut moved_index: HashMap<FileHash, usize> = HashMap::new();
    let mut moved_new: HashSet<usize> = HashSet::new();
    let mut remaining_missing: Vec<String> = Vec::new();

    for m in &acc.missing_entries {
        let matched = m
            .hash
            .as_ref()
            .and_then(|h| new_by_hash.get(h).map(|v| (h, v)));
        match matched {
            Some((h, new_idxs)) => {
                let idx = match moved_index.get(h) {
                    Some(&i) => i,
                    None => {
                        let i = moved.len();
                        moved.push(MovedFileSet {
                            hash: h.clone(),
                            old_paths: Vec::new(),
                            new_paths: Vec::new(),
                        });
                        moved_index.insert(h.clone(), i);
                        // First time: record all matching new files.
                        for &ni in new_idxs {
                            moved[i].new_paths.push(acc.new_entries[ni].path.clone());
                            moved_new.insert(ni);
                        }
                        i
                    }
                };
                moved[idx].old_paths.push(m.path.clone());
            }
            None => remaining_missing.push(m.path.clone()),
        }
    }

    let mut new_files = Vec::new();
    let mut new_groom = Vec::new();
    for (i, e) in acc.new_entries.iter().enumerate() {
        if !moved_new.contains(&i) {
            new_files.push(e.path.clone());
            new_groom.push(e.native.clone());
        }
    }

    acc.result.moved_files = moved;
    acc.result.missing_files = remaining_missing;
    acc.result.new_files = new_files;
    acc.result.new_files_for_groom = new_groom;
}

fn track_duplicates(manifest: &Manifest, result: &mut ScanResult) {
    fn rec(
        dir: &ManifestDirectoryInfo,
        prefix: &str,
        by_hash: &mut IndexMap<FileHash, Vec<String>>,
    ) {
        for (name, f) in &dir.files {
            if let Some(h) = &f.file_hash {
                by_hash
                    .entry(h.clone())
                    .or_default()
                    .push(standard_file_path(prefix, name));
            }
        }
        for (name, sub) in &dir.subdirectories {
            let child = format!("{prefix}{name}/");
            rec(sub, &child, by_hash);
        }
    }

    let mut by_hash: IndexMap<FileHash, Vec<String>> = IndexMap::new();
    rec(&manifest.root_directory, "./", &mut by_hash);

    for (hash, paths) in by_hash {
        if paths.len() > 1 {
            result.duplicate_files.push(DuplicateSet { hash, paths });
        }
    }
}

fn progress(
    acc: &mut Accum,
    opts: &ScanOptions,
    std_path: &str,
    tag: &str,
    on_progress: &mut Option<&mut dyn FnMut(&str)>,
) {
    if opts.show_progress {
        let line = format!("{std_path} [{tag}]");
        if let Some(cb) = on_progress.as_deref_mut() {
            cb(&line);
        }
        acc.result.progress_log.push(line);
    }
}

fn emit_hash_progress(
    opts: &ScanOptions,
    std_path: &str,
    percent: u8,
    on_progress: &mut Option<&mut dyn FnMut(&str)>,
) {
    if !opts.show_progress {
        return;
    }

    if let Some(cb) = on_progress.as_deref_mut() {
        let line = format!("\r{std_path} [HASHING {percent}%]      ");
        cb(&line);
    }
}

fn finish_hash_progress(
    acc: &mut Accum,
    opts: &ScanOptions,
    std_path: &str,
    tag: &str,
    on_progress: &mut Option<&mut dyn FnMut(&str)>,
) {
    if opts.show_progress {
        if let Some(cb) = on_progress.as_deref_mut() {
            let dynamic = format!("\r{std_path} [{tag}]      \n");
            cb(&dynamic);
        }
        acc.result.progress_log.push(format!("{std_path} [{tag}]"));
    }
}

fn read_len_mtime(path: &Path) -> Option<(i64, DateTime<Utc>)> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(dotnet_min_date);
    Some((meta.len() as i64, mtime))
}

fn set_file_mtime(path: &Path, mtime: DateTime<Utc>) -> std::io::Result<()> {
    let f = fs::OpenOptions::new().write(true).open(path)?;
    let times = fs::FileTimes::new().set_modified(SystemTime::from(mtime));
    f.set_times(times)
}

fn compare_fs_date(a: DateTime<Utc>, b: DateTime<Utc>) -> bool {
    (a - b).num_milliseconds().abs() <= FILESYSTEM_DATE_TOLERANCE_MS
}
