//! `rs` — RepoTools repository sync. Synchronizes two repositories that share a
//! manifest GUID. Rust port of the .NET `RepositorySync`, using UNIX-style
//! options. Phase 2 supports local↔local repositories; remote and encrypted
//! repositories are deferred to later phases.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use clap::{Parser, Subcommand};

use repotools_manifest::model::compare_manifest_dates;
use repotools_manifest::Manifest;
use repotools_sync::diff::SyncDiff;
use repotools_sync::{
    compare_manifests, do_update, LocalRepositoryProxy, RepositoryProxy, SyncOptions,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let start = Instant::now();
    let code = run(&cli);
    if cli.time && !cli.silent {
        println!("Duration: {:.3}s", start.elapsed().as_secs_f64());
    }
    ExitCode::from(code as u8)
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
            if !cli.silent {
                eprintln!("{e}");
            }
            1
        }
    }
}

fn cmd_diff(cli: &Cli, source: &Path, dest: &Path) -> anyhow::Result<i32> {
    let source_rep = LocalRepositoryProxy::open(source, true)?;
    let dest_rep = LocalRepositoryProxy::open(dest, true)?;

    let diff = compare_manifests(source_rep.manifest(), dest_rep.manifest());
    let different = report_diff(cli, source_rep.manifest(), dest_rep.manifest(), &diff);
    Ok(if different { 1 } else { 0 })
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
        if !cli.silent {
            eprintln!("Source and destination GUIDs are different.");
        }
        return Ok(1);
    }

    let silent = cli.silent;
    let mut stdout = std::io::stdout();
    let mut log = |msg: String| {
        if !silent {
            let _ = writeln!(stdout, "{msg}");
            let _ = stdout.flush();
        }
    };
    do_update(&mut source_rep, &mut dest_rep, &opts, &mut log)?;

    if !cli.preview {
        if source_writable {
            source_rep.flush()?;
        }
        dest_rep.flush()?;
    }
    Ok(0)
}

fn cmd_seed(cli: &Cli, source: &Path, dest: &Path) -> anyhow::Result<i32> {
    let source_rep = LocalRepositoryProxy::open(source, true)?;
    LocalRepositoryProxy::seed(source_rep.manifest(), dest)?;
    if !cli.silent {
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

    if source.name != dest.name {
        line("Manifest names are different.");
        different = true;
    }
    if source.guid != dest.guid {
        line("Manifest GUIDs are different.");
        different = true;
    }
    if source.default_hash_method != dest.default_hash_method {
        line("Manifest default hash methods are different.");
        different = true;
    }
    if source.description != dest.description {
        line("Manifest descriptions are different.");
        different = true;
    }
    if !compare_manifest_dates(source.inception_date_utc, dest.inception_date_utc) {
        line("Manifest creation dates are different.");
        different = true;
    }
    if !compare_manifest_dates(
        source.manifest_info_last_modified_utc,
        dest.manifest_info_last_modified_utc,
    ) {
        line("Last change to manifest information dates are different.");
        different = true;
    }
    if source.ignore_list != dest.ignore_list {
        line("Manifest ignore lists are different.");
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
