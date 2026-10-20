//! deckhand-protocol: the wire and on-disk formats shared by the daemon, its workers and clients.
//!
//! Port of types.ts and the message/record shapes built in daemon.ts, sessionWorker.ts, client.ts, storage.ts and
//! uiState.ts. Serialisation reproduces the TS output byte for byte (key order included); see [`order`] for how
//! history-dependent key order is kept.

pub mod data;
pub mod events;
pub mod ndjson;
pub mod order;
pub mod records;
pub mod wire;
pub mod worker_ipc;

/// types.ts PROTOCOL_VERSION: bump whenever the daemon/client request or response shape changes.
pub const PROTOCOL_VERSION: u32 = 48;

pub use events::{ResponseData, ServerMessage, ServerResponse};
pub use order::{KeyOrder, Ordered};
pub use records::{AppConfig, StateFile, UiState, WorktreeRecord};
pub use wire::Request;
pub use worker_ipc::{WorkerCommand, WorkerMessage, WorkspaceWorkerCommand};
