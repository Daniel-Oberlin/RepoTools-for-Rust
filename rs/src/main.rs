//! `rs` — RepoTools repository sync. Synchronizes two repositories that share a
//! manifest GUID. Rust port of the .NET `RepositorySync`, using UNIX-style
//! options. Phase 2 supports local↔local repositories; remote and encrypted
//! repositories are deferred to later phases.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};
use serde_json::{json, Map, Value};

use repotools_manifest::model::compare_manifest_dates;
use repotools_manifest::Manifest;
use repotools_sync::diff::SyncDiff;
use repotools_sync::{
    compare_manifests, do_update_events, LocalRepositoryProxy, RepositoryProxy, SyncEvent,
    SyncOptions,
};

#[derive(Parser, Debug)]
#[command(
    name = "rs",
    version,
    about = "RepoTools - synchronize validated repositories",
    long_about = "Synchronizes two repositories that share a manifest GUID. Phase 2 supports \
                  local repositories; remote and encrypted repositories are not yet ported."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Don't output anything (the exit code can still be checked).
    #[arg(long, short = 's', global = true)]
    silent: bool,
    /// List the actual files that differ, not just the counts.
    #[arg(long, short = 'd', global = true)]
    detail: bool,
    /// Show what would happen without changing anything.
    #[arg(long, short = 'p', global = true)]
    preview: bool,
    /// Measure and report the time of the operation.
    #[arg(long, short = 't', global = true)]
    time: bool,
    /// Write the result as a single JSON object instead of text (--time goes
    /// to stderr).
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show differences between two repositories.
    Diff { source: PathBuf, dest: PathBuf },
    /// Update dest with anything new in source (source unchanged).
    Update { source: PathBuf, dest: PathBuf },
    /// Update both source and dest with anything new from the other side.
    Sync { source: PathBuf, dest: PathBuf },
    /// Make dest match source, replacing or removing files as necessary.
    Mirror { source: PathBuf, dest: PathBuf },
    /// Create an empty dest manifest so a later update can populate it.
    Seed { source: PathBuf, dest: PathBuf },
}

impl Command {
    fn name(&self) -> &'static str {
        match self {
            Command::Diff { .. } => "diff",
            Command::Update { .. } => "update",
            Command::Sync { .. } => "sync",
            Command::Mirror { .. } => "mirror",
            Command::Seed { .. } => "seed",
        }
    }

    fn paths(&self) -> (&Path, &Path) {
        match self {
            Command::Diff { source, dest }
            | Command::Update { source, dest }
            | Command::Sync { source, dest }
            | Command::Mirror { source, dest }
            | Command::Seed { source, dest } => (source, dest),
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let start = Instant::now();
    let code = run(&cli);
    if cli.time && !cli.silent {
        let msg = format!("Duration: {:.3}s", start.elapsed().as_secs_f64());
        if cli.json {
            eprintln!("{msg}");
        } else {
            println!("{msg}");
        }
    }
    ExitCode::from(code as u8)
}

/// The base of every JSON result: the command and the two repositories.
fn json_base(cli: &Cli, ok: bool) -> Map<String, Value> {
    let (source, dest) = cli.command.paths();
    let mut m = Map::new();
    m.insert("ok".into(), json!(ok));
    m.insert("command".into(), json!(cli.command.name()));
    m.insert("source".into(), json!(source.display().to_string()));
    m.insert("dest".into(), json!(dest.display().to_string()));
    m
}

fn emit(cli: &Cli, m: Map<String, Value>) {
    if !cli.silent {
        println!("{}", Value::Object(m));
    }
}

/// Reports a failure as text on stderr, or with --json as
/// `{"ok": false, ..., "error": ...}` on stdout.
fn fail(cli: &Cli, msg: &str) {
    if cli.json {
        let mut m = json_base(cli, false);
        m.insert("error".into(), json!(msg));
        emit(cli, m);
    } else if !cli.silent {
        eprintln!("{msg}");
    }
}

fn run(cli: &Cli) -> i32 {
    let result = match &cli.command {
        Command::Diff { source, dest } => cmd_diff(cli, source, dest),
        Command::Update { source, dest } => {
            cmd_sync(cli, source, dest, SyncOptions::default(), false)
        }
        Command::Sync { source, dest } => cmd_sync(
            cli,
            source,
            dest,
            SyncOptions {
                both_ways: true,
                ..Default::default()
            },
            true,
        ),
        Command::Mirror { source, dest } => cmd_sync(
            cli,
            source,
            dest,
            SyncOptions {
                mirror: true,
                ..Default::default()
            },
            false,
        ),
        Command::Seed { source, dest } => cmd_seed(cli, source, dest),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            fail(cli, &e.to_string());
            1
        }
    }
}

fn cmd_diff(cli: &Cli, source: &Path, dest: &Path) -> anyhow::Result<i32> {
    let source_rep = LocalRepositoryProxy::open(source, true)?;
    let dest_rep = LocalRepositoryProxy::open(dest, true)?;

    let diff = compare_manifests(source_rep.manifest(), dest_rep.manifest());
    let different = if cli.json {
        let (different, m) = diff_json(cli, source_rep.manifest(), dest_rep.manifest(), &diff);
        emit(cli, m);
        different
    } else {
        report_diff(cli, source_rep.manifest(), dest_rep.manifest(), &diff)
    };
    Ok(if different { 1 } else { 0 })
}

/// Manifest-metadata differences, as (JSON key, text-report line).
fn metadata_differences(source: &Manifest, dest: &Manifest) -> Vec<(&'static str, &'static str)> {
    let mut d = Vec::new();
    if source.name != dest.name {
        d.push(("name", "Manifest names are different."));
    }
    if source.guid != dest.guid {
        d.push(("guid", "Manifest GUIDs are different."));
    }
    if source.default_hash_method != dest.default_hash_method {
        d.push((
            "default_hash_method",
            "Manifest default hash methods are different.",
        ));
    }
    if source.description != dest.description {
        d.push(("description", "Manifest descriptions are different."));
    }
    if !compare_manifest_dates(source.inception_date_utc, dest.inception_date_utc) {
        d.push(("inception_date", "Manifest creation dates are different."));
    }
    if !compare_manifest_dates(
        source.manifest_info_last_modified_utc,
        dest.manifest_info_last_modified_utc,
    ) {
        d.push((
            "manifest_info_last_modified",
            "Last change to manifest information dates are different.",
        ));
    }
    if source.ignore_list != dest.ignore_list {
        d.push(("ignore_list", "Manifest ignore lists are different."));
    }
    d
}

/// The machine-readable form of a diff; returns whether the repositories differ.
fn diff_json(
    cli: &Cli,
    source: &Manifest,
    dest: &Manifest,
    diff: &SyncDiff,
) -> (bool, Map<String, Value>) {
    let meta: Vec<&str> = metadata_differences(source, dest)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let different = !meta.is_empty() || !diff.is_empty();
    let mut m = json_base(cli, true);
    m.insert("different".into(), json!(different));
    m.insert("metadata_differences".into(), json!(meta));
    m.insert("source_only".into(), json!(diff.source_only));
    m.insert("dest_only".into(), json!(diff.dest_only));
    m.insert("changed".into(), json!(diff.changed));
    m.insert("last_modified".into(), json!(diff.last_modified));
    m.insert("registered".into(), json!(diff.registered));
    m.insert(
        "moved".into(),
        Value::Array(
            diff.moved
                .iter()
                .map(|s| {
                    json!({
                        "hash_type": s.hash.hash_type.as_str(),
                        "hash": s.hash.to_hex(),
                        "source_paths": s.source_paths,
                        "dest_paths": s.dest_paths,
                    })
                })
                .collect(),
        ),
    );
    (different, m)
}

fn cmd_sync(
    cli: &Cli,
    source: &Path,
    dest: &Path,
    opts: SyncOptions,
    source_writable: bool,
) -> anyhow::Result<i32> {
    let mut opts = opts;
    opts.preview = cli.preview;

    let mut source_rep = LocalRepositoryProxy::open(source, !source_writable)?;
    let mut dest_rep = LocalRepositoryProxy::open(dest, false)?;

    if source_rep.manifest().guid != dest_rep.manifest().guid {
        fail(cli, "Source and destination GUIDs are different.");
        return Ok(1);
    }

    let silent = cli.silent;
    let json_mode = cli.json;
    let mut actions: Vec<Value> = Vec::new();
    let mut stdout = std::io::stdout();
    let mut log = |event: &SyncEvent| {
        if json_mode {
            actions.push(serde_json::to_value(event).expect("SyncEvent serializes"));
        } else if !silent {
            let _ = writeln!(stdout, "{event}");
            let _ = stdout.flush();
        }
    };
    let result = do_update_events(&mut source_rep, &mut dest_rep, &opts, &mut log);
    if let Err(e) = result {
        // Report what was done before the failure along with the error.
        if json_mode {
            let mut m = json_base(cli, false);
            m.insert("preview".into(), json!(cli.preview));
            m.insert("actions".into(), Value::Array(actions));
            m.insert("error".into(), json!(e.to_string()));
            emit(cli, m);
            return Ok(1);
        }
        return Err(e.into());
    }

    if !cli.preview {
        if source_writable {
            source_rep.flush()?;
        }
        dest_rep.flush()?;
    }
    if json_mode {
        let mut m = json_base(cli, true);
        m.insert("preview".into(), json!(cli.preview));
        m.insert("actions".into(), Value::Array(actions));
        emit(cli, m);
    }
    Ok(0)
}

fn cmd_seed(cli: &Cli, source: &Path, dest: &Path) -> anyhow::Result<i32> {
    let source_rep = LocalRepositoryProxy::open(source, true)?;
    LocalRepositoryProxy::seed(source_rep.manifest(), dest)?;
    if cli.json {
        emit(cli, json_base(cli, true));
    } else if !cli.silent {
        println!("Seeded destination repository at {}", dest.display());
    }
    Ok(0)
}

/// Reports manifest-metadata and file differences; returns whether the two
/// repositories differ in any way. Ports `RepositorySync.ShowDiff`.
fn report_diff(cli: &Cli, source: &Manifest, dest: &Manifest, diff: &SyncDiff) -> bool {
    let mut different = false;
    let line = |msg: &str| {
        if !cli.silent {
            println!("{msg}");
        }
    };

    for (_, msg) in metadata_differences(source, dest) {
        line(msg);
        different = true;
    }

    different |= report_category(cli, "files are only in source.", &diff.source_only);
    different |= report_category(cli, "files are only in destination.", &diff.dest_only);
    different |= report_category(cli, "files have changed content.", &diff.changed);
    different |= report_category(
        cli,
        "files have last-modified dates which are different.",
        &diff.last_modified,
    );
    different |= report_category(
        cli,
        "files have manifest creation dates which are different.",
        &diff.registered,
    );

    if !diff.moved.is_empty() {
        if !cli.silent {
            println!("{} files were moved.", diff.moved.len());
            if cli.detail {
                for set in &diff.moved {
                    println!(
                        "   {} -> {}",
                        set.source_paths.join(" "),
                        set.dest_paths.join(" ")
                    );
                }
            }
        }
        different = true;
    }

    if !different {
        line("No differences.");
    }
    different
}

fn report_category(cli: &Cli, label: &str, paths: &[String]) -> bool {
    if paths.is_empty() {
        return false;
    }
    if !cli.silent {
        println!("{} {label}", paths.len());
        if cli.detail {
            for p in paths {
                println!("   {p}");
            }
        }
    }
    true
}
