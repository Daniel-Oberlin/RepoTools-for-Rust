//! Cross-validation against the .NET `rt` tool: proves the two implementations
//! produce manifests the other can read and validate.
//!
//! These tests are skipped unless a .NET `rt` is discoverable. Discovery order:
//!   1. `RT_DOTNET` env var — a single executable/wrapper run as `<RT_DOTNET> <args>`.
//!   2. `RT_DOTNET_MONO` + `RT_DOTNET_EXE` env vars — run as `<mono> <exe> <args>`.
//!   3. The default Mono + `rt.exe` location on this machine, if both exist.
//!
//! When none is found, each test prints a notice and passes (does not fail CI).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How to invoke the .NET tool: a program plus any leading args (e.g. the .exe
/// path when invoked through Mono).
struct DotnetRt {
    program: PathBuf,
    prefix_args: Vec<String>,
}

fn discover_dotnet_rt() -> Option<DotnetRt> {
    if let Ok(wrapper) = std::env::var("RT_DOTNET") {
        return Some(DotnetRt {
            program: PathBuf::from(wrapper),
            prefix_args: vec![],
        });
    }
    if let (Ok(mono), Ok(exe)) = (
        std::env::var("RT_DOTNET_MONO"),
        std::env::var("RT_DOTNET_EXE"),
    ) {
        return Some(DotnetRt {
            program: PathBuf::from(mono),
            prefix_args: vec![exe],
        });
    }
    let mono = "/Library/Frameworks/Mono.framework/Versions/Current/Commands/mono";
    let exe = "/Users/Daniel/Google Drive/My Drive/bin/rt.exe";
    if Path::new(mono).exists() && Path::new(exe).exists() {
        return Some(DotnetRt {
            program: PathBuf::from(mono),
            prefix_args: vec![exe.to_string()],
        });
    }
    None
}

impl DotnetRt {
    fn run(&self, dir: &Path, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix_args).args(args).current_dir(dir);
        cmd.output().expect("failed to run .NET rt")
    }
}

fn rust_rt(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rt"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run rust rt")
}

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn populate(dir: &Path) {
    write(dir, "one.txt", "alpha\n");
    write(dir, "two.txt", "beta beta\n");
    write(dir, "nested/three.txt", "gamma gamma gamma\n");
}

/// Rust writes the manifest; the .NET tool must read and validate it cleanly.
#[test]
fn rust_manifest_validates_under_dotnet() {
    let Some(net) = discover_dotnet_rt() else {
        eprintln!("skipping: .NET rt not found (set RT_DOTNET to enable)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path());

    assert!(rust_rt(dir.path(), &["create"]).status.success());
    assert!(rust_rt(dir.path(), &["update"]).status.success());

    let out = net.run(dir.path(), &["validate"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("No problems."),
        ".NET validate of Rust manifest failed:\nstatus={:?}\nstdout={stdout}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// The .NET tool writes the manifest; Rust must read and validate it cleanly.
#[test]
fn dotnet_manifest_validates_under_rust() {
    let Some(net) = discover_dotnet_rt() else {
        eprintln!("skipping: .NET rt not found (set RT_DOTNET to enable)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path());

    assert!(net.run(dir.path(), &["create"]).status.success());
    assert!(net.run(dir.path(), &["update"]).status.success());

    let out = rust_rt(dir.path(), &["validate"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("No problems."),
        "Rust validate of .NET manifest failed:\nstatus={:?}\nstdout={stdout}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// A manifest created by .NET, then updated by Rust after a rename, must still
/// validate under .NET — exercising round-tripping through both writers.
#[test]
fn round_trip_update_across_implementations() {
    let Some(net) = discover_dotnet_rt() else {
        eprintln!("skipping: .NET rt not found (set RT_DOTNET to enable)");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    populate(dir.path());

    // .NET creates and updates.
    assert!(net.run(dir.path(), &["create"]).status.success());
    assert!(net.run(dir.path(), &["update"]).status.success());

    // Rust records a rename via update --track-moves.
    fs::rename(
        dir.path().join("one.txt"),
        dir.path().join("one-renamed.txt"),
    )
    .unwrap();
    assert!(rust_rt(dir.path(), &["update", "--track-moves"])
        .status
        .success());

    // .NET validates the Rust-rewritten manifest cleanly.
    let out = net.run(dir.path(), &["validate"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("No problems."),
        ".NET validate after Rust update failed:\nstatus={:?}\nstdout={stdout}\nstderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr),
    );
}
