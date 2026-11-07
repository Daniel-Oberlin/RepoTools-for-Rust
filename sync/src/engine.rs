//! The synchronization engine — a port of `RepositorySync.DoUpdate` (and
//! `MoveFileSet`). Operates entirely through the [`RepositoryProxy`] trait.

use std::collections::HashSet;
use std::fmt;
use std::io;

use chrono::{DateTime, Utc};
use serde::Serialize;

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

/// A receiver of structured sync actions.
pub type EventSink<'a> = dyn FnMut(&SyncEvent) + 'a;

/// Which repository an action applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Source,
    Dest,
}

impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Side::Source => "source",
            Side::Dest => "dest",
        }
    }
}

/// One action taken (or, with `preview`, that would be taken) by a sync. The
/// [`Display`](fmt::Display) form is the line `rs` has always printed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SyncEvent {
    /// The destination adopts the source's manifest metadata.
    UpdateManifestInfo,
    /// A file only on the other side is copied to `side`.
    Add { side: Side, path: String },
    /// A file whose newer version is on the other side is replaced on `side`.
    Update { side: Side, path: String },
    /// A file is deleted from `side`.
    Remove { side: Side, path: String },
    /// A moved file is copied to an extra location on `side`.
    Copy {
        side: Side,
        from: String,
        to: String,
    },
    /// A file is moved on `side` to follow the other side's layout.
    Move {
        side: Side,
        from: String,
        to: String,
    },
    /// Matching content: `side`'s manifest dates are copied from the other side.
    UpdateInfo { side: Side, path: String },
    /// Files moved differently on both sides; nothing is done.
    AmbiguousMove {
        source_paths: Vec<String>,
        dest_paths: Vec<String>,
    },
}

impl fmt::Display for SyncEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncEvent::UpdateManifestInfo => {
                write!(f, "Updating destination manifest information.")
            }
            SyncEvent::Add { side, path } => write!(f, "Adding to {}: {path}", side.as_str()),
            SyncEvent::Update { side, path } => write!(f, "Updating {}: {path}", side.as_str()),
            SyncEvent::Remove { side, path } => {
                write!(f, "Removing from {}: {path}", side.as_str())
            }
            SyncEvent::Copy { from, to, .. } => write!(f, "Copying: {from} -> {to}"),
            SyncEvent::Move { from, to, .. } => write!(f, "Moving: {from} -> {to}"),
            SyncEvent::UpdateInfo { side, path } => {
                write!(f, "Updating {} information: {path}", side.as_str())
            }
            SyncEvent::AmbiguousMove {
                source_paths,
                dest_paths,
            } => {
                write!(f, "Ambiguous - files were moved on both sides:")?;
                for p in source_paths {
                    write!(f, "\n   Source: {p}")?;
                }
                for p in dest_paths {
                    write!(f, "\n   Dest:   {p}")?;
                }
                Ok(())
            }
        }
    }
}

/// Synchronizes `dest` from `source` (and, for `both_ways`, vice-versa)
/// according to `opts`, logging each action as a line of text. Returns once
/// all actions are applied (or, in preview mode, reported).
pub fn do_update(
    source: &mut dyn RepositoryProxy,
    dest: &mut dyn RepositoryProxy,
    opts: &SyncOptions,
    log: &mut Logger,
) -> io::Result<()> {
    do_update_events(source, dest, opts, &mut |e: &SyncEvent| log(e.to_string()))
}

/// Like [`do_update`], but reports each action as a structured [`SyncEvent`].
pub fn do_update_events(
    source: &mut dyn RepositoryProxy,
    dest: &mut dyn RepositoryProxy,
    opts: &SyncOptions,
    log: &mut EventSink,
) -> io::Result<()> {
    let diff = compare_manifests(source.manifest(), dest.manifest());
    let preview = opts.preview;

    // Manifest metadata: the destination adopts the source's metadata whenever
    // they differ (matching the .NET behavior).
    let s_info = source.manifest().manifest_info_last_modified_utc;
    let d_info = dest.manifest().manifest_info_last_modified_utc;
    if !compare_manifest_dates(s_info, d_info) {
        log(&SyncEvent::UpdateManifestInfo);
        if !preview {
            let source_manifest = source.manifest().clone();
            dest.copy_manifest_information(&source_manifest)?;
        }
    }

    // Files only in the source are added to the destination.
    for path in &diff.source_only {
        log(&SyncEvent::Add {
            side: Side::Dest,
            path: path.clone(),
        });
        if !preview {
            dest.put_file(&*source, path)?;
        }
    }

    // Files only in the destination.
    if opts.mirror {
        for path in &diff.dest_only {
            log(&SyncEvent::Remove {
                side: Side::Dest,
                path: path.clone(),
            });
            if !preview {
                dest.remove_file(path)?;
            }
        }
    } else if opts.both_ways {
        for path in &diff.dest_only {
            log(&SyncEvent::Add {
                side: Side::Source,
                path: path.clone(),
            });
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
            log(&SyncEvent::Update {
                side: Side::Dest,
                path: path.clone(),
            });
            if !preview {
                dest.put_file(&*source, path)?;
            }
        } else if s_mtime < d_mtime {
            if opts.mirror {
                log(&SyncEvent::Update {
                    side: Side::Dest,
                    path: path.clone(),
                });
                if !preview {
                    dest.put_file(&*source, path)?;
                }
            } else if opts.both_ways {
                log(&SyncEvent::Update {
                    side: Side::Source,
                    path: path.clone(),
                });
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
                Side::Dest,
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
                    Side::Source,
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
            log(&SyncEvent::UpdateInfo {
                side: Side::Dest,
                path: path.clone(),
            });
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
#[allow(clippy::too_many_arguments)]
fn move_file_set(
    target: &mut dyn RepositoryProxy,
    side: Side,
    info_rep: &dyn RepositoryProxy,
    new_paths: &[String],
    existing_paths: &[String],
    preview: bool,
    log: &mut EventSink,
) -> io::Result<()> {
    let mut si = 0; // index into new_paths
    let mut di = 0; // index into existing_paths

    // Copy to fill any surplus new locations (using the first existing file).
    if new_paths.len() > existing_paths.len() {
        let surplus = new_paths.len() - existing_paths.len();
        while si < surplus {
            let new_path = &new_paths[si];
            log(&SyncEvent::Copy {
                side,
                from: existing_paths[0].clone(),
                to: new_path.clone(),
            });
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
        log(&SyncEvent::Move {
            side,
            from: existing.clone(),
            to: new_path.clone(),
        });
        if !preview {
            let info = info_rep.manifest_file(new_path).unwrap().clone();
            target.move_file(existing, new_path, &info)?;
        }
        si += 1;
        di += 1;
    }

    // Remove any surplus existing files.
    while di < existing_paths.len() {
        log(&SyncEvent::Remove {
            side,
            path: existing_paths[di].clone(),
        });
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

fn report_ambiguous(set: &MovedFileSet, log: &mut EventSink) {
    log(&SyncEvent::AmbiguousMove {
        source_paths: set.source_paths.clone(),
        dest_paths: set.dest_paths.clone(),
    });
}
