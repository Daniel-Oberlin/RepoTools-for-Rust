//! Cross-validation of `rs` against the .NET `rs`: each implementation must
//! agree that a repository synced by the other has no differences.
//!
//! Skipped unless a .NET `rs` is discoverable (env `RS_DOTNET`, or
//! `RS_DOTNET_MONO`+`RS_DOTNET_EXE`, or the default Mono + `rs.exe` location).
//!
//! Note: destinations are seeded with the Rust `rs` (which writes JSON). The
//! .NET `rs seed` would emit the deprecated binary format, which is out of
//! scope; `.NET rs update` into a JSON destination preserves JSON.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use repotools_manifest::ignore::IgnoreSet;
use repotools_manifest::scan::{scan, ScanOptions};
use repotools_manifest::{io as manifest_io, Manifest, DEFAULT_MANIFEST_FILE_NAME};

struct DotnetRs {
    program: PathBuf,
    prefix_args: Vec<String>,
}

fn discover() -> Option<DotnetRs> {
    if let Ok(wrapper) = std::env::var("RS_DOTNET") {
        return Some(DotnetRs {
            program: PathBuf::from(wrapper),
            prefix_args: vec![],
        });
    }
    if let (Ok(mono), Ok(exe)) = (
        std::env::var("RS_DOTNET_MONO"),
        std::env::var("RS_DOTNET_EXE"),
    ) {
        return Some(DotnetRs {
            program: PathBuf::from(mono),
            prefix_args: vec![exe],
        });
    }
    let mono = "/Library/Frameworks/Mono.framework/Versions/Current/Commands/mono";
    let exe = "/Users/Daniel/Google Drive/My Drive/bin/rs.exe";
    if Path::new(mono).exists() && Path::new(exe).exists() {
        return Some(DotnetRs {
            program: PathBuf::from(mono),
            prefix_args: vec![exe.to_string()],
        });
    }
    None
}

impl DotnetRs {
    fn run(&self, args: &[&Path]) -> std::process::Output {
        Command::new(&self.program)
            .args(&self.prefix_args)
            .args(args)
            .output()
            .expect("run .NET rs")
    }
}

fn rust_rs(args: &[&Path]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rs"))
        .args(args)
        .output()
        .expect("run rust rs")
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn build_repo(root: &Path) {
    write(root, "one.txt", "alpha\n");
    write(root, "two.txt", "beta beta\n");
    write(root, "nested/three.txt", "gamma gamma gamma\n");
    let mut m = Manifest::new_clean();
    let ignore = IgnoreSet::compile(&m.ignore_list).unwrap();
    let abs = root.join(DEFAULT_MANIFEST_FILE_NAME);
    scan(
        root,
        &mut m,
        &ScanOptions {
            update: true,
            ..Default::default()
        },
        &ignore,
        &abs,
    );
    manifest_io::write_manifest(&abs, &m).unwrap();
}

fn assert_no_differences(out: &std::process::Output, who: &str) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("No differences."),
        "{who} diff was not clean:\nstatus={:?}\nstdout={stdout}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Rust syncs the destination; the .NET `rs` must see no differences.
#[test]
fn rust_sync_validates_under_dotnet() {
    let Some(net) = discover() else {
        eprintln!("skipping: .NET rs not found (set RS_DOTNET to enable)");
        return;
    };
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    build_repo(src.path());

    assert!(rust_rs(&[Path::new("seed"), src.path(), dst.path()])
        .status
        .success());
    assert!(rust_rs(&[Path::new("update"), src.path(), dst.path()])
        .status
        .success());

    assert_no_differences(
        &net.run(&[Path::new("diff"), src.path(), dst.path()]),
        ".NET",
    );
}

/// The .NET `rs` syncs a Rust-seeded (JSON) destination; Rust must see no
/// differences.
#[test]
fn dotnet_sync_validates_under_rust() {
    let Some(net) = discover() else {
        eprintln!("skipping: .NET rs not found (set RS_DOTNET to enable)");
        return;
    };
    let src = tempfile::tempdir().unwrap();
    let dst = tempfile::tempdir().unwrap();
    build_repo(src.path());

    assert!(rust_rs(&[Path::new("seed"), src.path(), dst.path()])
        .status
        .success());
    assert!(net
        .run(&[Path::new("update"), src.path(), dst.path()])
        .status
        .success());

    assert_no_differences(
        &rust_rs(&[Path::new("diff"), src.path(), dst.path()]),
        "Rust",
    );
}
