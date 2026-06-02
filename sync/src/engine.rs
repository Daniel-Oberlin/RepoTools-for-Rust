//! The synchronization engine — a port of `RepositorySync.DoUpdate` (and
//! `MoveFileSet`). Operates entirely through the [`RepositoryProxy`] trait.

use std::collections::HashSet;
use std::io;

use chrono::{DateTime, Utc};

use repotools_manifest::model::compare_manifest_dates;

use crate::diff::{compare_manifests, MovedFileSet};
use crate::proxy::RepositoryProxy;

/// Direction/behavior flags for a sync run.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncOptions {
    /// `sync`: propagate changes in both directions.
    pub both_ways: bool,
    /// `mirror`: make the destination identical to the source (removing extras).
    pub mirror: bool,
    /// `--preview`: report intended actions without changing anything.
    pub preview: bool,
}

/// A line-oriented progress logger.
pub type Logger<'a> = dyn FnMut(String) + 'a;

/// Synchronizes `dest` from `source` (and, for `both_ways`, vice-versa)
/// according to `opts`. Returns once all actions are applied (or, in preview
/// mode, reported).
pub fn do_update(
    source: &mut dyn RepositoryProxy,
    dest: &mut dyn RepositoryProxy,
    opts: &SyncOptions,
    log: &mut Logger,
) -> io::Result<()> {
    let diff = compare_manifests(source.manifest(), dest.manifest());
    let preview = opts.preview;

    // Manifest metadata: the destination adopts the source's metadata whenever
    // they differ (matching the .NET behavior).
    let s_info = source.manifest().manifest_info_last_modified_utc;
    let d_info = dest.manifest().manifest_info_last_modified_utc;
    if !compare_manifest_dates(s_info, d_info) {
        log("Updating destination manifest information.".to_string());
        if !preview {
            let source_manifest = source.manifest().clone();
            dest.copy_manifest_information(&source_manifest)?;
        }
    }

    // Files only in the source are added to the destination.
    for path in &diff.source_only {
        log(format!("Adding to dest: {path}"));
        if !preview {
            dest.put_file(&*source, path)?;
        }
    }

    // Files only in the destination.
    if opts.mirror {
        for path in &diff.dest_only {
            log(format!("Removing from dest: {path}"));
            if !preview {
                dest.remove_file(path)?;
            }
        }
    } else if opts.both_ways {
        for path in &diff.dest_only {
            log(format!("Adding to source: {path}"));
            if !preview {
                source.put_file(&*dest, path)?;
            }
        }
    }

    // Files that exist on both sides with differing content: the newer side
    // (by last-modified) wins.
    for path in &diff.changed {
        let s_mtime = source.manifest_file(path).unwrap().last_modified_utc;
        let d_mtime = dest.manifest_file(path).unwrap().last_modified_utc;
        if s_mtime > d_mtime {
            log(format!("Updating dest: {path}"));
            if !preview {
                dest.put_file(&*source, path)?;
            }
        } else if s_mtime < d_mtime {
            if opts.mirror {
                log(format!("Updating dest: {path}"));
                if !preview {
                    dest.put_file(&*source, path)?;
                }
            } else if opts.both_ways {
                log(format!("Updating source: {path}"));
                if !preview {
                    source.put_file(&*dest, path)?;
                }
            }
        }
    }

    // Moved files: act only on the definitive cases.
    for set in &diff.moved {
        let (es, ls) = registered_bounds(source, &set.source_paths);
        let (ed, ld) = registered_bounds(dest, &set.dest_paths);

        if opts.mirror || es > ld {
            // Reorganize the destination to match the source layout.
            move_file_set(
                dest,
                &*source,
                &set.source_paths,
                &set.dest_paths,
                preview,
                log,
            )?;
        } else if ed > ls {
            if opts.both_ways {
                // Reorganize the source to match the destination layout.
                move_file_set(
                    source,
                    &*dest,
                    &set.dest_paths,
                    &set.source_paths,
                    preview,
                    log,
                )?;
            }
        } else {
            report_ambiguous(set, log);
        }
    }

    // Files whose content matches but whose dates differ: copy date info
    // (destination ← source), de-duplicated across the two date categories.
    let mut info_updated: HashSet<&str> = HashSet::new();
    for path in diff.last_modified.iter().chain(diff.registered.iter()) {
        if info_updated.insert(path.as_str()) {
            log(format!("Updating dest information: {path}"));
            if !preview {
                let source_file = source.manifest_file(path).unwrap().clone();
                dest.copy_file_information(path, &source_file)?;
            }
        }
    }

    Ok(())
}

/// Reorganizes `target`'s files (currently at `existing_paths`) to the
/// `new_paths` layout, recording manifest entries from `info_rep`. Existing
/// files are moved where possible; surplus new locations are filled by copying;
/// surplus existing files are removed. Ports `MoveFileSet`.
fn move_file_set(
    target: &mut dyn RepositoryProxy,
    info_rep: &dyn RepositoryProxy,
    new_paths: &[String],
    existing_paths: &[String],
    preview: bool,
    log: &mut Logger,
) -> io::Result<()> {
    let mut si = 0; // index into new_paths
    let mut di = 0; // index into existing_paths

    // Copy to fill any surplus new locations (using the first existing file).
    if new_paths.len() > existing_paths.len() {
        let surplus = new_paths.len() - existing_paths.len();
        while si < surplus {
            let new_path = &new_paths[si];
            log(format!("Copying: {} -> {}", existing_paths[0], new_path));
            if !preview {
                let info = info_rep.manifest_file(new_path).unwrap().clone();
                target.copy_file(&existing_paths[0], new_path, &info)?;
            }
            si += 1;
        }
    }

    // Move existing files into the remaining new locations.
    while si < new_paths.len() && di < existing_paths.len() {
        let new_path = &new_paths[si];
        let existing = &existing_paths[di];
        log(format!("Moving: {existing} -> {new_path}"));
        if !preview {
            let info = info_rep.manifest_file(new_path).unwrap().clone();
            target.move_file(existing, new_path, &info)?;
        }
        si += 1;
        di += 1;
    }

    // Remove any surplus existing files.
    while di < existing_paths.len() {
        log(format!("Removing from dest: {}", existing_paths[di]));
        if !preview {
            target.remove_file(&existing_paths[di])?;
        }
        di += 1;
    }

    Ok(())
}

/// Returns the (earliest, latest) registered dates among the given files.
fn registered_bounds(
    rep: &dyn RepositoryProxy,
    paths: &[String],
) -> (DateTime<Utc>, DateTime<Utc>) {
    let mut earliest = DateTime::<Utc>::MAX_UTC;
    let mut latest = DateTime::<Utc>::MIN_UTC;
    for path in paths {
        if let Some(file) = rep.manifest_file(path) {
            let t = file.registered_utc;
            if t < earliest {
                earliest = t;
            }
            if t > latest {
                latest = t;
            }
        }
    }
    (earliest, latest)
}

fn report_ambiguous(set: &MovedFileSet, log: &mut Logger) {
    log("Ambiguous - files were moved on both sides:".to_string());
    for p in &set.source_paths {
        log(format!("   Source: {p}"));
    }
    for p in &set.dest_paths {
        log(format!("   Dest:   {p}"));
    }
}
