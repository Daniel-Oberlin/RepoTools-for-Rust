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
use serde_json::{json, Map, Value};

use repotools_manifest::ignore::IgnoreSet;
use repotools_manifest::scan::{scan_with_progress, MovedFileSet, ScanOptions, ScanResult};
use repotools_manifest::{
    io as manifest_io, HashType, Manifest, ScanScope, DEFAULT_MANIFEST_FILE_NAME,
};

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
    /// Number of concurrent hash jobs to use during validate or re-hash.
    #[arg(long, global = true, value_name = "N", default_value_t = 1, value_parser = clap::value_parser!(usize))]
    jobs: usize,
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
    /// Write the result as one JSON object per repository (one per line)
    /// instead of text. Progress (--show-progress) and --time go to stderr.
    /// Prompts are answered "no", so pass --force where a command would ask.
    #[arg(long, global = true)]
    json: bool,
    /// Limit status, update, validate or groom to this directory or file,
    /// relative to the repository root (repeatable). Entries outside it are
    /// left untouched and not reported. A scoped update or validate does not
    /// change the manifest's last-update or last-validation date.
    #[arg(long, global = true, value_name = "PATH")]
    path: Vec<String>,
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

impl Command {
    fn name(self) -> &'static str {
        match self {
            Command::Create => "create",
            Command::Status => "status",
            Command::Validate => "validate",
            Command::Update => "update",
            Command::Edit => "edit",
            Command::Groom => "groom",
            Command::Info => "info",
            Command::Clear => "clear",
        }
    }

    fn scans(self) -> bool {
        matches!(
            self,
            Command::Status | Command::Update | Command::Validate | Command::Groom
        )
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let out = Output {
        silent: cli.silent,
        detail: cli.detail,
        json: cli.json,
        command: cli.command,
    };
    let start = Instant::now();

    let code = run(&cli, &out);

    if cli.time {
        let msg = format!("Duration: {:.3}s", start.elapsed().as_secs_f64());
        if cli.json {
            if !cli.silent {
                eprintln!("{msg}");
            }
        } else {
            out.line(&msg);
        }
    }
    if code == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(code as u8)
    }
}

fn run(cli: &Cli, out: &Output) -> i32 {
    if !cli.path.is_empty() {
        if !cli.command.scans() {
            out.error(
                None,
                "--path only applies to status, update, validate and groom.",
            );
            return 1;
        }
        if cli.recursive {
            out.error(None, "--path can't be combined with --recursive.");
            return 1;
        }
    }

    let manifest_paths = match collect_manifest_paths(cli) {
        Ok(p) => p,
        Err(e) => {
            out.error(None, &format!("Error: {e}"));
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
            out.error(Some(&root), &format!("Could not access directory: {e}"));
            return 1;
        }
    };
    let manifest_abs = root.join(DEFAULT_MANIFEST_FILE_NAME);
    let ctx = Ctx {
        root: &root,
        manifest_path,
        manifest_abs: &manifest_abs,
    };

    match cli.command {
        Command::Create => cmd_create(cli, out, &ctx),
        Command::Info => cmd_info(out, &ctx),
        Command::Clear => cmd_clear(cli, out, &ctx),
        Command::Edit => cmd_edit(cli, out, &ctx),
        Command::Validate => cmd_validate(cli, out, &ctx),
        Command::Status | Command::Update | Command::Groom => cmd_scan(cli, out, &ctx),
    }
}

/// Where the repository being processed lives.
struct Ctx<'a> {
    /// Canonical repository root.
    root: &'a Path,
    /// The manifest file as given (or found).
    manifest_path: &'a Path,
    /// Absolute path of the manifest inside the root (never groomed).
    manifest_abs: &'a Path,
}

/// The base of every JSON result: which command ran on which repository.
fn json_base(out: &Output, ctx: &Ctx) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("ok".into(), json!(true));
    m.insert("command".into(), json!(out.command.name()));
    m.insert("root".into(), json!(ctx.root.display().to_string()));
    m.insert(
        "manifest".into(),
        json!(ctx.manifest_path.display().to_string()),
    );
    m
}

/// Emits the JSON result for commands that only (maybe) write the manifest.
fn finish_write(out: &Output, ctx: &Ctx, outcome: WriteOutcome) -> i32 {
    if out.json {
        if outcome == WriteOutcome::Failed {
            return 1; // already reported by write_manifest
        }
        let mut m = json_base(out, ctx);
        m.insert(
            "manifest_written".into(),
            json!(outcome == WriteOutcome::Written),
        );
        out.emit(m);
    }
    outcome.code()
}

fn cmd_create(cli: &Cli, out: &Output, ctx: &Ctx) -> i32 {
    if !cli.force && ctx.manifest_path.exists() && !confirm(out, "Replace existing manifest file? ")
    {
        return finish_write(out, ctx, WriteOutcome::Declined);
    }
    let mut manifest = Manifest::new_clean();
    if let Err(e) = apply_edits(cli, &mut manifest) {
        out.error(Some(ctx.root), &e);
        return 1;
    }
    let outcome = write_manifest(cli, out, ctx, &manifest);
    finish_write(out, ctx, outcome)
}

fn cmd_edit(cli: &Cli, out: &Output, ctx: &Ctx) -> i32 {
    let mut manifest = match read_manifest(out, ctx) {
        Some(m) => m,
        None => return 1,
    };
    if let Err(e) = apply_edits(cli, &mut manifest) {
        out.error(Some(ctx.root), &e);
        return 1;
    }
    let outcome = write_manifest(cli, out, ctx, &manifest);
    finish_write(out, ctx, outcome)
}

fn cmd_clear(cli: &Cli, out: &Output, ctx: &Ctx) -> i32 {
    let mut manifest = match read_manifest(out, ctx) {
        Some(m) => m,
        None => return 1,
    };
    if !cli.force {
        let prompt = format!("Clear {} files from the manifest? ", manifest.count_files());
        if !confirm(out, &prompt) {
            return finish_write(out, ctx, WriteOutcome::Declined);
        }
    }
    manifest.root_directory.files.clear();
    manifest.root_directory.subdirectories.clear();
    if let Err(e) = apply_edits(cli, &mut manifest) {
        out.error(Some(ctx.root), &e);
        return 1;
    }
    let outcome = write_manifest(cli, out, ctx, &manifest);
    finish_write(out, ctx, outcome)
}

fn cmd_info(out: &Output, ctx: &Ctx) -> i32 {
    let manifest = match read_manifest(out, ctx) {
        Some(m) => m,
        None => return 1,
    };
    if out.json {
        let mut m = json_base(out, ctx);
        m.insert("name".into(), json!(manifest.name));
        m.insert("guid".into(), json!(manifest.guid.to_string()));
        m.insert(
            "default_hash_method".into(),
            json!(manifest.default_hash_method.map(|h| h.as_str())),
        );
        m.insert(
            "inception_date_utc".into(),
            json!(manifest.inception_date_utc.to_rfc3339()),
        );
        m.insert(
            "last_update_date_utc".into(),
            json!(manifest.last_update_date_utc.to_rfc3339()),
        );
        m.insert(
            "manifest_info_last_modified_utc".into(),
            json!(manifest.manifest_info_last_modified_utc.to_rfc3339()),
        );
        m.insert(
            "last_validate_date_utc".into(),
            json!(manifest.last_validate_date_utc.to_rfc3339()),
        );
        m.insert("file_count".into(), json!(manifest.count_files()));
        m.insert("byte_count".into(), json!(manifest.count_bytes()));
        m.insert("ignore_list".into(), json!(manifest.ignore_list));
        m.insert("description".into(), json!(manifest.description));
        out.emit(m);
        return 0;
    }
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
fn cmd_scan(cli: &Cli, out: &Output, ctx: &Ctx) -> i32 {
    let mut manifest = match read_manifest(out, ctx) {
        Some(m) => m,
        None => return 1,
    };
    let Some((ignore, opts)) = prepare_scan(cli, out, ctx, &manifest) else {
        return 1;
    };
    let mut in_dynamic_progress = false;
    let mut progress = |line: &str| out.progress(line, &mut in_dynamic_progress);
    let result = scan_with_progress(
        ctx.root,
        &mut manifest,
        &opts,
        &ignore,
        ctx.manifest_abs,
        Some(&mut progress),
    );
    out.finish_dynamic_progress(&mut in_dynamic_progress);
    let different = report(out, cli, &result, false);
    let mut json = out
        .json
        .then(|| scan_json(out, ctx, &opts, &result, different));

    let code = match cli.command {
        Command::Update => {
            if let Err(e) = apply_edits(cli, &mut manifest) {
                out.error(Some(ctx.root), &e);
                return 1;
            }
            let outcome = write_manifest(cli, out, ctx, &manifest);
            if outcome == WriteOutcome::Failed {
                return 1;
            }
            if let Some(m) = json.as_mut() {
                m.insert(
                    "manifest_written".into(),
                    json!(outcome == WriteOutcome::Written),
                );
            }
            outcome.code()
        }
        Command::Groom => groom(cli, out, &result, json.as_mut()),
        _ => 0, // status writes nothing
    };
    if let Some(m) = json {
        out.emit(m);
    }
    code
}

/// Implements `validate`: scan a copy for reporting, then bump only the
/// validation date on the original manifest (unless --no-touch, or the scan
/// was limited with --path).
fn cmd_validate(cli: &Cli, out: &Output, ctx: &Ctx) -> i32 {
    let mut original = match read_manifest(out, ctx) {
        Some(m) => m,
        None => return 1,
    };
    let Some((ignore, opts)) = prepare_scan(cli, out, ctx, &original) else {
        return 1;
    };
    let mut scanned = original.clone();
    let mut in_dynamic_progress = false;
    let mut progress = |line: &str| out.progress(line, &mut in_dynamic_progress);
    let result = scan_with_progress(
        ctx.root,
        &mut scanned,
        &opts,
        &ignore,
        ctx.manifest_abs,
        Some(&mut progress),
    );
    out.finish_dynamic_progress(&mut in_dynamic_progress);
    let different = report(out, cli, &result, false);

    if different {
        out.line("Problems found.");
    } else {
        out.line("No problems.");
    }

    let exit_code = if different { 1 } else { 0 };
    let mut date_written = false;

    if !cli.no_touch && opts.scope.is_none() {
        original.last_validate_date_utc = Utc::now();
        match write_manifest(cli, out, ctx, &original) {
            WriteOutcome::Failed => return 1,
            WriteOutcome::Written => date_written = true,
            WriteOutcome::Declined => {}
        }
    }

    if out.json {
        let mut m = scan_json(out, ctx, &opts, &result, different);
        m.insert("problems".into(), json!(different));
        m.insert("validation_date_written".into(), json!(date_written));
        out.emit(m);
    }
    exit_code
}

/// Compiles the ignore list and builds the scan options, including the
/// --path scope. Reports and returns `None` on an error.
fn prepare_scan(
    cli: &Cli,
    out: &Output,
    ctx: &Ctx,
    manifest: &Manifest,
) -> Option<(IgnoreSet, ScanOptions)> {
    let ignore = match IgnoreSet::compile(&manifest.ignore_list) {
        Ok(i) => i,
        Err(e) => {
            out.error(Some(ctx.root), &format!("Invalid ignore pattern: {e}"));
            return None;
        }
    };
    let mut opts = scan_options(cli);
    if !cli.path.is_empty() {
        let mut scope = ScanScope::new();
        for p in &cli.path {
            if let Err(e) = scope.add_user_path(ctx.root, manifest, p) {
                out.error(Some(ctx.root), &e);
                return None;
            }
        }
        opts.scope = Some(scope);
    }
    Some((ignore, opts))
}

fn groom(
    cli: &Cli,
    out: &Output,
    result: &ScanResult,
    json: Option<&mut Map<String, Value>>,
) -> i32 {
    let mut deleted: Vec<String> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    let mut declined = false;

    let mut delete_all = |paths: &[PathBuf], prompt: String, declined: &mut bool| {
        if paths.is_empty() {
            return;
        }
        if !(cli.force || confirm(out, &prompt)) {
            *declined = true;
            return;
        }
        for path in paths {
            match std::fs::remove_file(path) {
                Ok(()) => deleted.push(path.display().to_string()),
                Err(e) => {
                    out.line(&format!("Could not delete {}: {e}", path.display()));
                    failed
                        .push(json!({"path": path.display().to_string(), "error": e.to_string()}));
                }
            }
        }
    };

    delete_all(
        &result.new_files_for_groom,
        format!("Delete {} new files? ", result.new_files_for_groom.len()),
        &mut declined,
    );
    if cli.all {
        delete_all(
            &result.ignored_files_for_groom,
            format!(
                "Delete {} ignored files? ",
                result.ignored_files_for_groom.len()
            ),
            &mut declined,
        );
    }

    if let Some(m) = json {
        m.insert("deleted".into(), json!(deleted));
        m.insert("delete_errors".into(), json!(failed));
        m.insert("declined".into(), json!(declined));
    }
    0
}

/// The machine-readable form of a scan result.
fn scan_json(
    out: &Output,
    ctx: &Ctx,
    opts: &ScanOptions,
    r: &ScanResult,
    different: bool,
) -> Map<String, Value> {
    let mut m = json_base(out, ctx);
    m.insert(
        "scope".into(),
        match &opts.scope {
            Some(s) => json!({"dirs": s.dirs(), "files": s.files()}),
            None => Value::Null,
        },
    );
    m.insert("different".into(), json!(different));
    m.insert("files_checked".into(), json!(r.files_checked));
    m.insert("missing".into(), json!(r.missing_files));
    m.insert("changed".into(), json!(r.changed_files));
    m.insert("new".into(), json!(r.new_files));
    m.insert("last_modified".into(), json!(r.last_modified_files));
    m.insert("errors".into(), json!(r.error_files));
    m.insert(
        "moved".into(),
        Value::Array(
            r.moved_files
                .iter()
                .map(|s| {
                    json!({
                        "hash_type": s.hash.hash_type.as_str(),
                        "hash": s.hash.to_hex(),
                        "old_paths": s.old_paths,
                        "new_paths": s.new_paths,
                    })
                })
                .collect(),
        ),
    );
    m.insert(
        "duplicates".into(),
        Value::Array(
            r.duplicate_files
                .iter()
                .map(|s| {
                    json!({
                        "hash_type": s.hash.hash_type.as_str(),
                        "hash": s.hash.to_hex(),
                        "paths": s.paths,
                    })
                })
                .collect(),
        ),
    );
    m.insert("newly_ignored".into(), json!(r.newly_ignored_files));
    m.insert("ignored".into(), json!(r.ignored_files));
    m
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
        hash_jobs: if cli.command == Command::Validate || cli.re_hash {
            cli.jobs.max(1)
        } else {
            1
        },
        scope: None, // set from --path by prepare_scan
    }
}

/// Prints the scan result and returns whether a significant difference was found
/// (used to set the validate exit code).
fn report(out: &Output, cli: &Cli, r: &ScanResult, emit_buffered_progress: bool) -> bool {
    if cli.show_progress && emit_buffered_progress {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteOutcome {
    Written,
    /// The user (or --json/--silent, which answer prompts "no") declined.
    Declined,
    /// Already reported.
    Failed,
}

impl WriteOutcome {
    fn code(self) -> i32 {
        if self == WriteOutcome::Failed {
            1
        } else {
            0
        }
    }
}

fn write_manifest(cli: &Cli, out: &Output, ctx: &Ctx, manifest: &Manifest) -> WriteOutcome {
    if cli.confirm_update && !confirm(out, "Update manifest? ") {
        return WriteOutcome::Declined;
    }
    match manifest_io::write_manifest(ctx.manifest_path, manifest) {
        Ok(()) => WriteOutcome::Written,
        Err(e) => {
            out.error(Some(ctx.root), &format!("Could not write manifest: {e}"));
            WriteOutcome::Failed
        }
    }
}

fn read_manifest(out: &Output, ctx: &Ctx) -> Option<Manifest> {
    match manifest_io::read_manifest(ctx.manifest_path) {
        Ok(m) => Some(m),
        Err(e) => {
            out.error(Some(ctx.root), &format!("Could not read manifest: {e}"));
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
    // Prompts would corrupt JSON output, so --json answers "no" like --silent.
    if out.silent || out.json {
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

/// Console output respecting the --silent, --detail and --json flags. With
/// --json, text lines are suppressed, stdout carries only JSON objects (one per
/// line), and progress goes to stderr.
struct Output {
    silent: bool,
    detail: bool,
    json: bool,
    command: Command,
}

impl Output {
    fn line(&self, msg: &str) {
        if !self.silent && !self.json {
            println!("{msg}");
        }
    }

    /// Reports a failure: as text, or with --json as
    /// `{"ok": false, "command": ..., "root": ..., "error": ...}`.
    fn error(&self, root: Option<&Path>, msg: &str) {
        if self.json {
            let mut m = Map::new();
            m.insert("ok".into(), json!(false));
            m.insert("command".into(), json!(self.command.name()));
            m.insert("root".into(), json!(root.map(|r| r.display().to_string())));
            m.insert("error".into(), json!(msg));
            self.emit(m);
        } else {
            self.line(msg);
        }
    }

    /// Writes one JSON object on its own line (--json only).
    fn emit(&self, m: Map<String, Value>) {
        if self.json && !self.silent {
            println!("{}", Value::Object(m));
            let _ = io::stdout().flush();
        }
    }

    /// Progress text goes to stdout normally, and to stderr with --json.
    fn write_progress(&self, msg: &str, newline: bool) {
        if self.json {
            let mut e = io::stderr();
            let _ = if newline {
                writeln!(e, "{msg}")
            } else {
                write!(e, "{msg}")
            };
            let _ = e.flush();
        } else {
            let mut o = io::stdout();
            let _ = if newline {
                writeln!(o, "{msg}")
            } else {
                write!(o, "{msg}")
            };
            let _ = o.flush();
        }
    }

    fn progress(&self, msg: &str, in_dynamic_progress: &mut bool) {
        if self.silent {
            return;
        }
        if msg.starts_with('\r') {
            self.write_progress(msg, false);
            *in_dynamic_progress = !msg.ends_with('\n');
            return;
        }
        if *in_dynamic_progress {
            self.write_progress("", true);
            *in_dynamic_progress = false;
        }
        self.write_progress(msg, true);
    }

    fn finish_dynamic_progress(&self, in_dynamic_progress: &mut bool) {
        if !self.silent && *in_dynamic_progress {
            self.write_progress("", true);
            *in_dynamic_progress = false;
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
