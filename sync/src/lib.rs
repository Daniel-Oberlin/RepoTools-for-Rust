//! `repotools_sync` — the synchronization library for RepoTools: a comparison
//! of two manifests, the `RepositoryProxy` abstraction, a local-filesystem
//! proxy, and the sync engine. Used by the `rs` binary.

pub mod diff;
pub mod engine;
pub mod local;
pub mod proxy;

pub use diff::{compare_manifests, MovedFileSet, SyncDiff};
pub use engine::{do_update, do_update_events, Side, SyncEvent, SyncOptions};
pub use local::LocalRepositoryProxy;
pub use proxy::RepositoryProxy;
