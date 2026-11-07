# RepoTools-for-Rust

A Rust port of [RepoTools](https://github.com/Daniel-Oberlin/RepoTools-for-.NET) — command-line
tools for creating and maintaining repositories of file-based data that can be validated against a
file/hash manifest using cryptographic hashes.

This is a "Rust-native" reimplementation: it uses standard Rust conventions (clap for the CLI,
UNIX-style `--long`/`-short` options, serde/JSON) rather than mirroring the .NET idioms.

## Status

- **`rt`** (the repository tool) — complete.
- **`rs`** (repository sync) — complete for **local↔local** repositories (`diff`, `update`, `sync`,
  `mirror`, `seed`). Remote/HTTP sync (which needs the repository server) and encrypted (crypt)
  repositories are not yet ported.
- The repository **server** is not ported.

Manifests are **interoperable with the .NET implementation** — the .NET tools can read/validate
manifests produced here and vice-versa (see [Interoperability](#interoperability)).

The manifest is JSON only; the deprecated .NET binary (`BinaryFormatter`) format is not supported
(note: `.NET rs seed` writes that format, so seed destinations with `rs`/`rt` here, or `.NET rt`).
The default hash method is MD5 (matching .NET); SHA-256 is also supported.

## Layout

This is a Cargo workspace:

- `manifest/` — the `repotools_manifest` library crate: the manifest data model, .NET-interoperable
  JSON (de)serialization, file hashing, ignore handling, and the scan/diff engine.
- `sync/` — the `repotools_sync` library crate: two-manifest comparison, the `RepositoryProxy`
  abstraction, a local-filesystem proxy, and the sync engine.
- `rt/` — the `rt` binary crate.
- `rs/` — the `rs` binary crate.

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

### Limiting a scan to part of the repository

`--path` restricts `status`, `update`, `validate` and `groom` to a directory or file, relative to
the repository root (repeatable). Manifest entries outside it are left untouched and not reported,
so refreshing one album in a large collection doesn't rescan everything:

```sh
rt update --path "Music Collection/Jazz/Miles Davis"
rt update --track-moves --path Staging/Album --path "Music Collection/Jazz/Album"   # a move
```

A path that no longer exists on disk can still be given if it is in the manifest (its files are
reported missing). A scoped `update` or `validate` does not change the manifest's last-update or
last-validation date, since only part of the repository was checked. `--path` can't be combined
with `--recursive`.

### Machine-readable output

`--json` replaces the text report with one JSON object per repository, one per line on stdout
(one line unless `--recursive`). Progress (`--show-progress`) and `--time` go to stderr. Prompts
are answered "no", so pass `--force` where a command would ask (for example `groom`).

```sh
rt status --json
```

```json
{"ok":true,"command":"status","root":"/data/repo","manifest":"./.repositoryManifest","scope":null,
 "different":true,"files_checked":3,"missing":["./gone.txt"],"changed":["./edit.txt"],
 "new":["./sub/new.txt"],"last_modified":[],"errors":[],"moved":[],"duplicates":[],
 "newly_ignored":[],"ignored":["./.repositoryManifest"]}
```

- Paths are standard manifest paths (`./dir/file`).
- `moved` entries are `{"hash_type","hash","old_paths","new_paths"}`, and `duplicates` entries
  are `{"hash_type","hash","paths"}` (hash in hex).
- `scope` is `{"dirs":[…],"files":[…]}` when `--path` was given.
- `update` adds `manifest_written`; `validate` adds `problems` and `validation_date_written`;
  `groom` adds `deleted`, `delete_errors` and `declined`.
- `info` gives the manifest's metadata (`guid`, `name`, dates in RFC 3339, `file_count`,
  `byte_count`, `ignore_list`, ...).
- On failure the object is `{"ok":false,"command":…,"root":…,"error":"…"}`.

Exit codes are the same as without `--json`.

## Using `rs`

`rs` synchronizes two local repositories that share a manifest GUID (create the destination with
`seed`, which clones the source's GUID):

```sh
rs seed   <source> <dest>     # create an empty dest manifest (clone of source, no files)
rs update <source> <dest>     # copy anything new in source into dest (source unchanged)
rs sync   <source> <dest>     # propagate changes in both directions
rs mirror <source> <dest>     # make dest identical to source (removes extras)
rs diff   <source> <dest>     # report differences; exits non-zero when they differ
rs update <source> <dest> --preview --detail   # show what would change, in full
```

Run `rs --help` for all commands and options.

`rs --json` writes a single JSON object instead of text:

- `diff` gives `different`, `metadata_differences` (names of differing manifest settings),
  `source_only`, `dest_only`, `changed`, `last_modified`, `registered` and `moved`.
- `update`, `sync` and `mirror` (with or without `--preview`) give `preview` and `actions`, one
  per step taken or planned, such as `{"action":"add","side":"dest","path":"./a.txt"}` or
  `{"action":"move","side":"dest","from":"./old/a.txt","to":"./new/a.txt"}`. The other actions
  are `update`, `remove`, `copy`, `update_info`, `update_manifest_info` and `ambiguous_move`.
- On failure the object has `"ok":false` and an `error`. For a sync that failed partway, it also
  includes the actions completed before the failure.

Library users get the same actions as `repotools_sync::SyncEvent` values from
`do_update_events`.

## Interoperability

The manifest format is semantically interoperable with the .NET implementation. The test suite
includes automated cross-validation that runs the .NET tools against Rust-produced repositories and
vice-versa. These tests are **skipped unless the corresponding .NET tool is discoverable**:

- Set `RT_DOTNET` / `RS_DOTNET` to an executable/wrapper invoked as `rt` / `rs` (e.g. a shell script
  running it via Mono), or
- set `RT_DOTNET_MONO`+`RT_DOTNET_EXE` (and `RS_DOTNET_MONO`+`RS_DOTNET_EXE`) to the Mono binary and
  `.exe` paths, or
- rely on the built-in default Mono + `rt.exe`/`rs.exe` location if both exist.

```sh
RT_DOTNET=/path/to/dotnet-rt-wrapper RS_DOTNET=/path/to/dotnet-rs-wrapper cargo test --workspace
```

### Compatibility notes

- Ignore patterns are regular expressions interpreted by the Rust `regex` crate, whose dialect
  differs slightly from .NET's. The default patterns and typical user patterns behave identically.
- Filenames are NFC-normalized (relevant on macOS).

## License

MIT — see [LICENSE](LICENSE).
