//! `rt` — the RepoTools repository tool. Maintains a `.repositoryManifest` of
//! files, their sizes, last-modified dates, and content hashes, so a
//! repository's integrity can be validated. This is a Rust port of the .NET
//! `RepositoryTool`, using UNIX-style `--long`/`-short` options.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::Instant;

use chrono::Utc;
use clap::{Parser, Subcommand};

use repotools_manifest::ignore::IgnoreSet;
use repotools_manifest::scan::{scan, MovedFileSet, ScanOptions, ScanResult};
use repotools_manifest::{io as manifest_io, HashType, Manifest, DEFAULT_MANIFEST_FILE_NAME};

#[derive(Parser, Debug)]
#[command(
    name = "rt",
    version,
    about = "RepoTools - repository tool for data validation",
    long_about = "Maintains a manifest of files in a repository along with their hash \
                  values and last-modified dates, so the data integrity of the repository \
                  can be validated."
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
    /// Process ignored files as well (for --detail and groom).
    #[arg(long, short = 'a', global = true)]
    all: bool,
    /// List each file as it is scanned.
    #[arg(long, global = true)]
    show_progress: bool,
    /// Don't treat a last-modified-date-only change as a difference.
    #[arg(long, global = true)]
    ignore_date: bool,
    /// Don't treat the presence of new files alone as a difference.
    #[arg(long, global = true)]
    ignore_new: bool,
    /// Don't stop to prompt before deleting or replacing.
    #[arg(long, short = 'f', global = true)]
    force: bool,
    /// Measure and report the time of the operation.
    #[arg(long, short = 't', global = true)]
    time: bool,
    /// Set the repository name in the manifest.
    #[arg(long, short = 'n', global = true, value_name = "NAME")]
    name: Option<String>,
    /// Set the repository description in the manifest.
    #[arg(long, global = true, value_name = "TEXT")]
    description: Option<String>,
    /// Reset changed last-modified dates back to the manifest's values.
    #[arg(long, global = true)]
    back_date: bool,
    /// Detect renamed/moved files by matching content hashes.
    #[arg(long, short = 'm', global = true)]
    track_moves: bool,
    /// Detect duplicate files by matching content hashes.
    #[arg(long, global = true)]
    track_duplicates: bool,
    /// Add a regular expression matching paths to ignore (repeatable).
    #[arg(long, global = true, value_name = "REGEX")]
    ignore: Vec<String>,
    /// Remove a previously added ignore expression (repeatable).
    #[arg(long, global = true, value_name = "REGEX")]
    dont_ignore: Vec<String>,
    /// Replace the ignore list with the default list.
    #[arg(long, global = true)]
    ignore_default: bool,
    /// Default hash method: MD5 or SHA256.
    #[arg(long, global = true, value_name = "METHOD")]
    hash_method: Option<String>,
    /// Recompute all hashes using the default hash method.
    #[arg(long, global = true)]
    new_hash: bool,
    /// Recompute all hashes during the scan (always check hash).
    #[arg(long, global = true)]
    re_hash: bool,
    /// Path to the manifest file (defaults to ./.repositoryManifest).
    #[arg(long, global = true, value_name = "PATH")]
    manifest_file: Option<PathBuf>,
    /// Recurse from the current directory, handling any manifests found.
    #[arg(long, short = 'r', global = true)]
    recursive: bool,
    /// With --recursive, descend into repositories to find nested ones.
    #[arg(long, global = true)]
    cascade: bool,
    /// After a validate, do not rewrite the manifest's validation date.
    #[arg(long, global = true)]
    no_touch: bool,
    /// After an update, ask before writing the updated manifest.
    #[arg(long, global = true)]
    confirm_update: bool,
}

#[derive(Subcommand, Debug, PartialEq, Eq, Clone, Copy)]
enum Command {
    /// Create an empty repository manifest in the current directory.
    Create,
    /// Check files by length and date; hash modified files; report differences.
    Status,
    /// Check every file against its hash value; report differences.
    Validate,
    /// Bring the manifest up to date with the current state of the repository.
    Update,
    /// Change manifest settings without scanning files.
    Edit,
    /// Remove any files that are not listed in the manifest.
    Groom,
    /// Show manifest information.
    Info,
    /// Clear the manifest: remove all file entries.
    Clear,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let out = Output {
        silent: cli.silent,
        detail: cli.detail,
    };
    let start = Instant::now();

    let code = run(&cli, &out);

    if cli.time {
        out.line(&format!("Duration: {:.3}s", start.elapsed().as_secs_f64()));
    }
    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(code as u8)
    }
}

fn run(cli: &Cli, out: &Output) -> i32 {
    let manifest_paths = match collect_manifest_paths(cli) {
        Ok(p) => p,
        Err(e) => {
            out.line(&format!("Error: {e}"));
            return 1;
        }
    };

    let mut exit_code = 0;
    for manifest_path in manifest_paths {
        if cli.recursive {
            if let Some(parent) = manifest_path.parent() {
                out.line(&format!("{}:", parent.display()));
            }
        }
        let code = process_one(cli, out, &manifest_path);
        if code != 0 {
            exit_code = code;
        }
        if cli.recursive {
            out.line("");
        }
    }
    exit_code
}

fn process_one(cli: &Cli, out: &Output, manifest_path: &Path) -> i32 {
    // The repository root is the manifest's directory when recursing, otherwise
    // the current directory (matching the .NET tool).
    let root = if cli.recursive {
        manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    } else {
        PathBuf::from(".")
    };
    let root = match std::fs::canonicalize(&root) {
        Ok(r) => r,
        Err(e) => {
            out.line(&format!("Could not access directory: {e}"));
            return 1;
        }
    };
    let manifest_abs = root.join(DEFAULT_MANIFEST_FILE_NAME);

    match cli.command {
        Command::Create => cmd_create(cli, out, manifest_path),
        Command::Info => cmd_info(out, manifest_path),
        Command::Clear => cmd_clear(cli, out, manifest_path),
        Command::Edit => cmd_edit(cli, out, manifest_path),
        Command::Validate => cmd_validate(cli, out, manifest_path, &root, &manifest_abs),
        Command::Status | Command::Update | Command::Groom => {
            cmd_scan(cli, out, manifest_path, &root, &manifest_abs)
        }
    }
}

fn cmd_create(cli: &Cli, out: &Output, manifest_path: &Path) -> i32 {
    if !cli.force && manifest_path.exists() && !confirm(out, "Replace existing manifest file? ") {
        return 0;
    }
    let mut manifest = Manifest::new_clean();
    if let Err(e) = apply_edits(cli, &mut manifest) {
        out.line(&e);
        return 1;
    }
    write_manifest(cli, out, manifest_path, &manifest)
}

fn cmd_edit(cli: &Cli, out: &Output, manifest_path: &Path) -> i32 {
    let mut manifest = match read_manifest(out, manifest_path) {
        Some(m) => m,
        None => return 1,
    };
    if let Err(e) = apply_edits(cli, &mut manifest) {
        out.line(&e);
        return 1;
    }
    write_manifest(cli, out, manifest_path, &manifest)
}

fn cmd_clear(cli: &Cli, out: &Output, manifest_path: &Path) -> i32 {
    let mut manifest = match read_manifest(out, manifest_path) {
        Some(m) => m,
        None => return 1,
    };
    if !cli.force {
        let prompt = format!("Clear {} files from the manifest? ", manifest.count_files());
        if !confirm(out, &prompt) {
            return 0;
        }
    }
    manifest.root_directory.files.clear();
    manifest.root_directory.subdirectories.clear();
    if let Err(e) = apply_edits(cli, &mut manifest) {
        out.line(&e);
        return 1;
    }
    write_manifest(cli, out, manifest_path, &manifest)
}

fn cmd_info(out: &Output, manifest_path: &Path) -> i32 {
    let manifest = match read_manifest(out, manifest_path) {
        Some(m) => m,
        None => return 1,
    };
    if let Some(name) = &manifest.name {
        out.line(&format!("Name:                          {name}"));
    }
    out.line(&format!("GUID:                          {}", manifest.guid));
    if let Some(h) = manifest.default_hash_method {
        out.line(&format!("Default hash method:           {h}"));
    }
    out.line(&format!(
        "Date of creation:              {}",
        manifest.inception_date_utc
    ));
    out.line(&format!(
        "Date of last update:           {}",
        manifest.last_update_date_utc
    ));
    out.line(&format!(
        "Last change of manifest info:  {}",
        manifest.manifest_info_last_modified_utc
    ));
    out.line(&format!(
        "Date of last validation:       {}",
        manifest.last_validate_date_utc
    ));
    out.line(&format!(
        "Total number of files:         {}",
        manifest.count_files()
    ));
    out.line(&format!(
        "Total number of bytes:         {}",
        manifest.count_bytes()
    ));
    if !manifest.ignore_list.is_empty() {
        out.line("Ignoring these file patterns:");
        for pat in &manifest.ignore_list {
            out.line(&format!("   {pat}"));
        }
    }
    if let Some(desc) = &manifest.description {
        out.line("");
        out.line("Description: ");
        out.line(desc);
    }
    0
}

/// Implements `status`, `update`, and `groom`, which all scan the repository.
fn cmd_scan(
    cli: &Cli,
    out: &Output,
    manifest_path: &Path,
    root: &Path,
    manifest_abs: &Path,
) -> i32 {
    let mut manifest = match read_manifest(out, manifest_path) {
        Some(m) => m,
        None => return 1,
    };
    let ignore = match IgnoreSet::compile(&manifest.ignore_list) {
        Ok(i) => i,
        Err(e) => {
            out.line(&format!("Invalid ignore pattern: {e}"));
            return 1;
        }
    };
    let opts = scan_options(cli);
    let result = scan(root, &mut manifest, &opts, &ignore, manifest_abs);
    report(out, cli, &result);

    match cli.command {
        Command::Update => {
            if let Err(e) = apply_edits(cli, &mut manifest) {
                out.line(&e);
                return 1;
            }
            write_manifest(cli, out, manifest_path, &manifest)
        }
        Command::Groom => groom(cli, out, &result),
        _ => 0, // status writes nothing
    }
}

/// Implements `validate`: scan a copy for reporting, then bump only the
/// validation date on the original manifest (unless --no-touch).
fn cmd_validate(
    cli: &Cli,
    out: &Output,
    manifest_path: &Path,
    root: &Path,
    manifest_abs: &Path,
) -> i32 {
    let mut original = match read_manifest(out, manifest_path) {
        Some(m) => m,
        None => return 1,
    };
    let ignore = match IgnoreSet::compile(&original.ignore_list) {
        Ok(i) => i,
        Err(e) => {
            out.line(&format!("Invalid ignore pattern: {e}"));
            return 1;
        }
    };
    let mut scanned = original.clone();
    let opts = scan_options(cli);
    let result = scan(root, &mut scanned, &opts, &ignore, manifest_abs);
    let different = report(out, cli, &result);

    if different {
        out.line("Problems found.");
    } else {
        out.line("No problems.");
    }

    let mut exit_code = if different { 1 } else { 0 };

    if !cli.no_touch {
        original.last_validate_date_utc = Utc::now();
        let code = write_manifest(cli, out, manifest_path, &original);
        if code != 0 {
            exit_code = code;
        }
    }
    exit_code
}

fn groom(cli: &Cli, out: &Output, result: &ScanResult) -> i32 {
    if !result.new_files_for_groom.is_empty() {
        let proceed = cli.force
            || confirm(
                out,
                &format!("Delete {} new files? ", result.new_files_for_groom.len()),
            );
        if proceed {
            for path in &result.new_files_for_groom {
                if let Err(e) = std::fs::remove_file(path) {
                    out.line(&format!("Could not delete {}: {e}", path.display()));
                }
            }
        }
    }

    if cli.all && !result.ignored_files_for_groom.is_empty() {
        let proceed = cli.force
            || confirm(
                out,
                &format!(
                    "Delete {} ignored files? ",
                    result.ignored_files_for_groom.len()
                ),
            );
        if proceed {
            for path in &result.ignored_files_for_groom {
                if let Err(e) = std::fs::remove_file(path) {
                    out.line(&format!("Could not delete {}: {e}", path.display()));
                }
            }
        }
    }
    0
}

fn scan_options(cli: &Cli) -> ScanOptions {
    ScanOptions {
        update: cli.command == Command::Update,
        always_check_hash: cli.command == Command::Validate || cli.re_hash,
        make_new_hash: cli.new_hash,
        back_date: cli.back_date,
        track_moves: cli.track_moves,
        track_duplicates: cli.track_duplicates,
        show_progress: cli.show_progress,
    }
}

/// Prints the scan result and returns whether a significant difference was found
/// (used to set the validate exit code).
fn report(out: &Output, cli: &Cli, r: &ScanResult) -> bool {
    if cli.show_progress {
        for line in &r.progress_log {
            out.line(line);
        }
    }

    let mut different = false;

    if !r.missing_files.is_empty() {
        out.line(&format!("{} files are missing.", r.missing_files.len()));
        out.detail_paths(&r.missing_files);
        different = true;
    }
    if !r.changed_files.is_empty() {
        out.line(&format!(
            "{} files have changed content.",
            r.changed_files.len()
        ));
        out.detail_paths(&r.changed_files);
        different = true;
    }
    if !r.new_files.is_empty() {
        out.line(&format!("{} files are new.", r.new_files.len()));
        out.detail_paths(&r.new_files);
        if !cli.ignore_new {
            different = true;
        }
    }
    if !r.last_modified_files.is_empty() {
        out.line(&format!(
            "{} files have last-modified dates which are different.",
            r.last_modified_files.len()
        ));
        out.detail_paths(&r.last_modified_files);
        if !cli.ignore_date {
            different = true;
        }
    }
    if !r.error_files.is_empty() {
        out.line(&format!("{} files have errors.", r.error_files.len()));
        out.detail_paths(&r.error_files);
        different = true;
    }
    if !r.moved_files.is_empty() {
        out.line(&format!("{} files were moved.", r.moved_files.len()));
        out.detail_moves(&r.moved_files);
        different = true;
    }
    if !r.duplicate_files.is_empty() {
        out.line(&format!(
            "{} file hashes were duplicates.",
            r.duplicate_files.len()
        ));
        if out.detail {
            for set in &r.duplicate_files {
                out.line(&format!("   {}", set.hash.to_hex()));
                for p in &set.paths {
                    out.line(&format!("      {p}"));
                }
            }
        }
    }
    if !r.newly_ignored_files.is_empty() {
        out.line(&format!(
            "{} files are newly ignored.",
            r.newly_ignored_files.len()
        ));
        out.detail_paths(&r.newly_ignored_files);
    }
    // Mirror the .NET tool: only report ignored files when more than the
    // manifest file itself was ignored.
    if r.ignored_files.len() > 1 {
        out.line(&format!("{} files were ignored.", r.ignored_files.len()));
        if cli.all {
            out.detail_paths(&r.ignored_files);
        }
    }

    out.line(&format!("{} files were checked.", r.files_checked));
    different
}

/// Applies manifest-info edits (name, description, hash method, ignore list)
/// from the CLI, returning an error string on an invalid hash method.
fn apply_edits(cli: &Cli, manifest: &mut Manifest) -> Result<(), String> {
    let mut changed = false;

    if let Some(name) = &cli.name {
        manifest.name = Some(name.clone());
        changed = true;
    }
    if let Some(desc) = &cli.description {
        manifest.description = Some(desc.clone());
        changed = true;
    }
    if let Some(method) = &cli.hash_method {
        let ht = HashType::from_str(method)?;
        manifest.default_hash_method = Some(ht);
        changed = true;
    }
    for pat in &cli.ignore {
        if !manifest.ignore_list.contains(pat) {
            manifest.ignore_list.push(pat.clone());
        }
        changed = true;
    }
    for pat in &cli.dont_ignore {
        manifest.ignore_list.retain(|p| p != pat);
        changed = true;
    }
    if cli.ignore_default {
        manifest.ignore_list = repotools_manifest::ignore::default_ignore_patterns();
        changed = true;
    }

    if changed {
        manifest.manifest_info_last_modified_utc = Utc::now();
    }
    Ok(())
}

fn write_manifest(cli: &Cli, out: &Output, manifest_path: &Path, manifest: &Manifest) -> i32 {
    if cli.confirm_update && !confirm(out, "Update manifest? ") {
        return 0;
    }
    match manifest_io::write_manifest(manifest_path, manifest) {
        Ok(()) => 0,
        Err(e) => {
            out.line(&format!("Could not write manifest: {e}"));
            1
        }
    }
}

fn read_manifest(out: &Output, manifest_path: &Path) -> Option<Manifest> {
    match manifest_io::read_manifest(manifest_path) {
        Ok(m) => Some(m),
        Err(e) => {
            out.line(&format!("Could not read manifest: {e}"));
            None
        }
    }
}

/// Determines which manifest file(s) to operate on.
fn collect_manifest_paths(cli: &Cli) -> io::Result<Vec<PathBuf>> {
    if cli.recursive {
        let mut paths = Vec::new();
        find_manifests(Path::new("."), cli.cascade, &mut paths)?;
        Ok(paths)
    } else {
        let path = cli
            .manifest_file
            .clone()
            .unwrap_or_else(|| PathBuf::from(".").join(DEFAULT_MANIFEST_FILE_NAME));
        Ok(vec![path])
    }
}

fn find_manifests(dir: &Path, cascade: bool, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let candidate = dir.join(DEFAULT_MANIFEST_FILE_NAME);
    let found = candidate.is_file();
    if found {
        out.push(candidate);
    }
    if cascade || !found {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    find_manifests(&entry.path(), cascade, out)?;
                }
            }
        }
    }
    Ok(())
}

fn confirm(out: &Output, prompt: &str) -> bool {
    if out.silent {
        return false;
    }
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_ok() {
        let t = line.trim_start();
        t.starts_with('y') || t.starts_with('Y')
    } else {
        false
    }
}

/// Console output respecting the --silent and --detail flags.
struct Output {
    silent: bool,
    detail: bool,
}

impl Output {
    fn line(&self, msg: &str) {
        if !self.silent {
            println!("{msg}");
        }
    }

    fn detail_paths(&self, paths: &[String]) {
        if self.detail {
            for p in paths {
                self.line(&format!("   {p}"));
            }
            self.line("");
        }
    }

    fn detail_moves(&self, moves: &[MovedFileSet]) {
        if self.detail {
            for set in moves {
                let old = set.old_paths.join(" ");
                let new = set.new_paths.join(" ");
                self.line(&format!("   {old} -> {new}"));
            }
            self.line("");
        }
    }
}
