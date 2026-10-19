//! Port of cli.ts (`deckhand hook`).
//!
//! The agent hook bridge. Hooks are advisory, bounded, and never approve or block agent actions: whatever happens
//! (no daemon, bad input, timeout) the command answers `{}` on stdout and exits 0. Claude runs hooks async, so they
//! can arrive out of order; each carries `sentAt` and the daemon drops a signal older than the last one. Large
//! PostToolUse payloads are read (bounded) so their event isn't lost; only the signal fields are sent
//! (`deckhand_core::agent_signals::hook_payload_fields`).

// Wave 0 skeleton: the binary does not reach these items until their unit wires them; drop this then.
#![allow(dead_code)]

use std::time::Duration;

use deckhand_core::error::DhError;
use deckhand_protocol::wire::Request;
use tokio::io::AsyncRead;

/// TS: cli.ts MAX_HOOK_INPUT_BYTES (stdin larger than this is dropped: `Hook input too large`).
pub const MAX_HOOK_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// TS: cli.ts the stdin deadline (`Hook input timed out`).
pub const HOOK_INPUT_TIMEOUT: Duration = Duration::from_millis(500);
/// TS: cli.ts the `agent-hook` request timeout (ms).
pub const HOOK_REQUEST_TIMEOUT_MS: u64 = 1200;
/// TS: cli.ts what a hook always prints.
pub const HOOK_OUTPUT: &str = "{}\n";

/// The launch identity a session's agent inherits (TS: `DECKHAND_SESSION_ID`, `DECKHAND_LAUNCH_ID`,
/// `DECKHAND_HOOK_TOKEN`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEnv {
    pub session_id: String,
    pub launch_id: String,
    pub token: String,
}

impl HookEnv {
    /// TS: cli.ts `if (sessionId && launchId && token)`: all three set and non-empty, else `None` (nothing is sent).
    pub fn from_env() -> Option<HookEnv> {
        todo!("B1")
    }
}

/// TS: cli.ts the stdin loop: reads `reader` to the end, decoding UTF-8 across chunks (lossy, as `StringDecoder`).
/// Errors: `Hook input too large` past `max_bytes`, `Hook input timed out` when the whole read takes longer than
/// `timeout`.
pub async fn read_hook_input<R: AsyncRead + Unpin>(
    reader: R,
    max_bytes: usize,
    timeout: Duration,
) -> Result<String, DhError> {
    let _ = (reader, max_bytes, timeout);
    todo!("B1")
}

/// TS: cli.ts the `agent-hook` request: `JSON.parse(raw)` (V8's message on failure), reduced with
/// `hook_payload_fields`, with the launch identity, `request_id` and `sent_at` (ms since the epoch, taken before
/// reading stdin).
pub fn hook_request(env: &HookEnv, raw: &str, request_id: String, sent_at: i64) -> Result<Request, DhError> {
    let _ = (env, raw, request_id, sent_at);
    todo!("B1")
}

/// TS: cli.ts the `hook` branch: reads stdin (bounded), sends the signal when the launch identity is set, swallows
/// every error, then writes [`HOOK_OUTPUT`] to stdout.
pub async fn run_hook() {
    todo!("B1")
}
