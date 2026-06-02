# RepoTools-for-Rust

A Rust port of [RepoTools](https://github.com/Daniel-Oberlin/RepoTools-for-.NET) — command-line
tools for creating and maintaining repositories of file-based data that can be validated against a
file/hash manifest using cryptographic hashes.

This is a "Rust-native" reimplementation: it uses standard Rust conventions (clap for the CLI,
UNIX-style `--long`/`-short` options, serde/JSON) rather than mirroring the .NET idioms.

## Status

Phase 1 implements **`rt`** (the repository tool). Its manifests are **interoperable with the .NET
implementation** — the .NET `rt` can read and validate manifests produced here and vice-versa (see
[Interoperability](#interoperability)). The `rs` (sync) tool and the repository server are not yet
ported.

The manifest is JSON only; the deprecated .NET binary (`BinaryFormatter`) format is not supported.
The default hash method is MD5 (matching .NET); SHA-256 is also supported.

## Layout

This is a Cargo workspace:

- `manifest/` — the `repotools_manifest` library crate: the manifest data model, .NET-interoperable
  JSON (de)serialization, file hashing, ignore handling, and the scan/diff engine.
- `rt/` — the `rt` binary crate.

## Building and testing

```sh
cargo build --release        # binary at target/release/rt
cargo test --workspace       # unit + integration + cross-validation tests
```

## Using `rt`

```sh
rt create                       # create an empty manifest in the current directory
rt update                       # bring the manifest up to date with the directory
rt status                       # report differences (no manifest changes)
rt validate                     # re-hash every file and verify; exits non-zero on problems
rt status --detail              # list the actual differing files
rt validate --track-moves       # detect renamed/moved files by content hash
rt status --track-duplicates    # detect duplicate files by content hash
rt groom --force                # delete files not listed in the manifest
rt info                         # show manifest metadata
```

Run `rt --help` (or `rt <command> --help`) for the full list of commands and options.

## Interoperability

The manifest format is semantically interoperable with the .NET implementation. The test suite
includes automated cross-validation that runs the .NET `rt` against Rust-produced manifests and
vice-versa. These tests are **skipped unless a .NET `rt` is discoverable**:

- Set `RT_DOTNET` to an executable/wrapper invoked as `rt` (e.g. a shell script running it via Mono), or
- set `RT_DOTNET_MONO` and `RT_DOTNET_EXE` to the Mono binary and `rt.exe` paths, or
- rely on the built-in default Mono + `rt.exe` location if both exist.

```sh
RT_DOTNET=/path/to/dotnet-rt-wrapper cargo test --workspace
```

### Compatibility notes

- Ignore patterns are regular expressions interpreted by the Rust `regex` crate, whose dialect
  differs slightly from .NET's. The default patterns and typical user patterns behave identically.
- Filenames are NFC-normalized (relevant on macOS).

## License

MIT — see [LICENSE](LICENSE).
