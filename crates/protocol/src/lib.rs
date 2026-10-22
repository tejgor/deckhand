//! deckhand-protocol: the wire and on-disk formats shared by the daemon, its workers and clients.
//!
//! Serialisation keeps the existing formats byte for byte, key order included; see [`order`] for how history-dependent
//! key order is kept.

pub mod data;
pub mod events;
pub mod ndjson;
pub mod order;
pub mod records;
pub mod wire;
pub mod worker_ipc;

/// types.ts PROTOCOL_VERSION: bump whenever the daemon/client request or response shape changes.
pub const PROTOCOL_VERSION: u32 = 49;

/// This build's version (the workspace version). The daemon reports it in `ping` so a client can tell that another
/// build of the same protocol is still running; compatibility is decided by [`PROTOCOL_VERSION`] alone.
pub const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use events::{ResponseData, ServerMessage, ServerResponse};
pub use order::{KeyOrder, Ordered};
pub use records::{AppConfig, StateFile, UiState, WorktreeRecord};
pub use wire::Request;
pub use worker_ipc::{WorkerCommand, WorkerMessage, WorkspaceWorkerCommand};
