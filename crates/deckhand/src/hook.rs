//! Port of cli.ts (`deckhand hook`).
//!
//! The agent hook bridge. Hooks are advisory, bounded, and never approve or block agent actions: whatever happens
//! (no daemon, bad input, timeout) the command answers `{}` on stdout and exits 0. Claude runs hooks async, so they
//! can arrive out of order; each carries `sentAt` and the daemon drops a signal older than the last one. Large
//! PostToolUse payloads are read (bounded) so their event isn't lost; only the signal fields are sent
//! (`deckhand_core::agent_signals::hook_payload_fields`).

use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use deckhand_client::client::request;
use deckhand_core::agent_signals::hook_payload_fields;
use deckhand_core::error::DhError;
use deckhand_core::jsstr::date::now_ms;
use deckhand_protocol::wire::{AgentHook, Request};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::sync::mpsc;

/// TS: cli.ts MAX_HOOK_INPUT_BYTES (stdin larger than this is dropped: `Hook input too large`).
pub const MAX_HOOK_INPUT_BYTES: usize = 8 * 1024 * 1024;
/// TS: cli.ts the stdin deadline (`Hook input timed out`).
pub const HOOK_INPUT_TIMEOUT: Duration = Duration::from_millis(500);
/// TS: cli.ts the `agent-hook` request timeout (ms).
pub const HOOK_REQUEST_TIMEOUT_MS: u64 = 1200;
/// TS: cli.ts what a hook always prints.
pub const HOOK_OUTPUT: &str = "{}\n";

/// Size of one read from stdin.
const READ_CHUNK: usize = 64 * 1024;

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
        let var = |name: &str| {
            std::env::var_os(name)
                .map(|value| value.to_string_lossy().into_owned())
                .filter(|value| !value.is_empty())
        };
        Some(HookEnv {
            session_id: var("DECKHAND_SESSION_ID")?,
            launch_id: var("DECKHAND_LAUNCH_ID")?,
            token: var("DECKHAND_HOOK_TOKEN")?,
        })
    }
}

/// TS: cli.ts the stdin loop: reads `reader` to the end, decoding UTF-8 across chunks (lossy, as `StringDecoder`).
/// Errors: `Hook input too large` past `max_bytes`, `Hook input timed out` when the whole read takes longer than
/// `timeout`.
pub async fn read_hook_input<R: AsyncRead + Unpin>(
    mut reader: R,
    max_bytes: usize,
    timeout: Duration,
) -> Result<String, DhError> {
    let read = async {
        let mut input = Vec::new();
        let mut chunk = vec![0u8; READ_CHUNK];
        loop {
            let read = reader
                .read(&mut chunk)
                .await
                .map_err(|error| DhError::msg(error.to_string()))?;
            if read == 0 {
                break;
            }
            if input.len().saturating_add(read) > max_bytes {
                return Err(DhError::msg("Hook input too large"));
            }
            input.extend_from_slice(chunk.get(..read).unwrap_or_default());
        }
        // Decoding the whole input at once equals StringDecoder's chunk-by-chunk decoding plus `end()`.
        Ok(String::from_utf8_lossy(&input).into_owned())
    };
    tokio::time::timeout(timeout, read)
        .await
        .unwrap_or_else(|_| Err(DhError::msg("Hook input timed out")))
}

/// TS: cli.ts `JSON.parse(raw)` of the hook input (V8's message, on the original text, on failure). V8 accepts a
/// lone UTF-16 surrogate escape (`"\ud83d"`, which Node's JSON.stringify writes for a string cut inside a surrogate
/// pair) and serde_json does not: when the text fails to parse, such escapes become `�` (the U+FFFD a Rust
/// string holds for a lone surrogate, see `deckhand_core::jsstr`) and it is parsed again.
pub fn hook_input(raw: &str) -> Result<Value, DhError> {
    serde_json::from_str(raw)
        .or_else(|_| serde_json::from_str(&replace_lone_surrogate_escapes(raw)))
        .map_err(|_| DhError::json_parse(raw))
}

/// The UTF-16 code unit of the four hex digits at `at`, if they are four hex digits.
fn hex_unit(bytes: &[u8], at: usize) -> Option<u16> {
    let digits = bytes.get(at..at.checked_add(4)?)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

/// `text` with every `\uXXXX` escape of a lone surrogate (a high half not followed by a low-half escape, or a low
/// half not preceded by a high one) replaced by `�`. Escaped backslashes are skipped, so `\\ud83d` is untouched.
fn replace_lone_surrogate_escapes(text: &str) -> String {
    const HIGH: std::ops::Range<u16> = 0xD800..0xDC00;
    const LOW: std::ops::Range<u16> = 0xDC00..0xE000;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes.get(at) != Some(&b'\\') {
            at += 1;
            continue;
        }
        let unit = if bytes.get(at + 1) == Some(&b'u') {
            hex_unit(bytes, at + 2)
        } else {
            None
        };
        let Some(unit) = unit else {
            // Any other escape (or a malformed one): skip the backslash and the character it escapes.
            at += 2;
            continue;
        };
        let paired = HIGH.contains(&unit)
            && bytes.get(at + 6) == Some(&b'\\')
            && bytes.get(at + 7) == Some(&b'u')
            && hex_unit(bytes, at + 8).is_some_and(|next| LOW.contains(&next));
        if paired {
            at += 12;
            continue;
        }
        if HIGH.contains(&unit) || LOW.contains(&unit) {
            // Escapes are ASCII, so `at` and `at + 6` are char boundaries.
            out.push_str(text.get(copied..at).unwrap_or_default());
            out.push_str("\\ufffd");
            copied = at + 6;
        }
        at += 6;
    }
    out.push_str(text.get(copied..).unwrap_or_default());
    out
}

/// TS: cli.ts the `agent-hook` request: `JSON.parse(raw)` (V8's message on failure), reduced with
/// `hook_payload_fields`, with the launch identity, `request_id` and `sent_at` (ms since the epoch, taken before
/// reading stdin).
pub fn hook_request(env: &HookEnv, raw: &str, request_id: String, sent_at: i64) -> Result<Request, DhError> {
    let input = hook_input(raw)?;
    let payload = hook_payload_fields(&input)?;
    Ok(Request::AgentHook(AgentHook {
        request_id,
        session_id: env.session_id.clone(),
        launch_id: env.launch_id.clone(),
        token: env.token.clone(),
        payload: Value::Object(payload),
        sent_at: Some(sent_at),
    }))
}

/// TS: cli.ts the `hook` branch: reads stdin (bounded), sends the signal when the launch identity is set, swallows
/// every error, then writes [`HOOK_OUTPUT`] to stdout.
pub async fn run_hook() {
    run_hook_with(stdin_reader(), HookEnv::from_env(), &mut std::io::stdout()).await;
}

/// [`run_hook`] on a given input, launch identity and output.
pub(crate) async fn run_hook_with<R: AsyncRead + Unpin, W: Write>(reader: R, env: Option<HookEnv>, out: &mut W) {
    let sent_at = now_ms();
    if let Ok(raw) = read_hook_input(reader, MAX_HOOK_INPUT_BYTES, HOOK_INPUT_TIMEOUT).await
        && let Some(env) = env
    {
        // Disconnected/unsupported callbacks must not affect permissions: not even a bug in building the signal.
        let built = catch_unwind(AssertUnwindSafe(|| {
            hook_request(&env, &raw, crate::cli::request_id(), sent_at)
        }));
        if let Ok(Ok(message)) = built {
            let _ = request::<Value>(&message, Some(HOOK_REQUEST_TIMEOUT_MS)).await;
        }
    }
    let _ = out.write_all(HOOK_OUTPUT.as_bytes());
    let _ = out.flush();
}

/// Process stdin as an [`AsyncRead`]. A plain thread does the blocking reads: when the input never ends (the agent
/// keeps the pipe open) the hook still answers after its deadline, and the process exits without waiting for that
/// read, as Node does once `process.stdin` is destroyed. (A runtime blocking task would hold up runtime shutdown.)
fn stdin_reader() -> ChannelReader {
    let (sender, receiver) = mpsc::channel(4);
    let spawned = std::thread::Builder::new()
        .name("deckhand-hook-stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut chunk = vec![0u8; READ_CHUNK];
            loop {
                let message = match stdin.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => Ok(chunk.get(..read).unwrap_or_default().to_vec()),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => Err(error),
                };
                let failed = message.is_err();
                if sender.blocking_send(message).is_err() || failed {
                    break;
                }
            }
        });
    if let Err(error) = spawned {
        // The thread owned the only sender: a failed spawn reads as an error, not as empty input.
        let (sender, receiver) = mpsc::channel(1);
        let _ = sender.try_send(Err(error));
        return ChannelReader::new(receiver);
    }
    ChannelReader::new(receiver)
}

/// An [`AsyncRead`] over chunks sent by a reader thread; the channel closing is the end of the input.
struct ChannelReader {
    receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    pending: Vec<u8>,
    offset: usize,
}

impl ChannelReader {
    fn new(receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>) -> Self {
        ChannelReader {
            receiver,
            pending: Vec::new(),
            offset: 0,
        }
    }
}

impl AsyncRead for ChannelReader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        if this.offset >= this.pending.len() {
            match this.receiver.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(Err(error))) => return Poll::Ready(Err(error)),
                Poll::Ready(Some(Ok(chunk))) => {
                    this.pending = chunk;
                    this.offset = 0;
                }
            }
        }
        let available = this.pending.get(this.offset..).unwrap_or_default();
        let take = available.len().min(buf.remaining());
        buf.put_slice(available.get(..take).unwrap_or_default());
        this.offset += take;
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn env() -> HookEnv {
        HookEnv {
            session_id: "missing".into(),
            launch_id: "missing".into(),
            token: "missing".into(),
        }
    }

    // covers: B-B1-01 hook bridge is bounded, always neutral, and never starts a daemon
    #[tokio::test(flavor = "current_thread")]
    async fn hook_bridge_is_bounded_always_neutral_and_never_starts_a_daemon() {
        let home = match tempfile::Builder::new().prefix("deckhand-hook-bridge-").tempdir() {
            Ok(home) => home,
            Err(error) => panic!("tempdir: {error}"),
        };
        // SAFETY: tests run one per process under nextest; nothing else in this test reads the environment
        // concurrently.
        unsafe {
            std::env::set_var("DECKHAND_HOME", home.path());
            std::env::set_var("DECKHAND_DEV", "0");
        }
        let large = "x".repeat(100_000);
        let permission = serde_json::json!({"hook_event_name": "PermissionRequest"}).to_string();
        for input in [Some("{"), Some(large.as_str()), Some(permission.as_str()), None] {
            let (mut writer, reader) = tokio::io::duplex(256 * 1024);
            if let Some(input) = input {
                assert!(writer.write_all(input.as_bytes()).await.is_ok());
                drop(writer);
                let mut output = Vec::new();
                run_hook_with(reader, Some(env()), &mut output).await;
                assert_eq!(String::from_utf8_lossy(&output), "{}\n");
            } else {
                // Intentionally never closes stdin.
                let started = std::time::Instant::now();
                let mut output = Vec::new();
                run_hook_with(reader, Some(env()), &mut output).await;
                assert_eq!(String::from_utf8_lossy(&output), "{}\n");
                assert!(started.elapsed() < Duration::from_secs(3));
                drop(writer);
            }
        }
        let entries = std::fs::read_dir(home.path()).map(|dir| dir.count()).ok();
        assert_eq!(entries, Some(0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_hook_input_is_bounded_in_size_and_time() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        assert!(writer.write_all(b"12345").await.is_ok());
        drop(writer);
        let result = read_hook_input(reader, 4, Duration::from_secs(5)).await;
        assert_eq!(
            result.map_err(|error| error.to_string()),
            Err("Hook input too large".to_owned())
        );

        let (mut writer, reader) = tokio::io::duplex(1024);
        assert!(writer.write_all(b"1234").await.is_ok());
        drop(writer);
        let result = read_hook_input(reader, 4, Duration::from_secs(5)).await;
        assert_eq!(result.map_err(|error| error.to_string()), Ok("1234".to_owned()));

        let (_writer, reader) = tokio::io::duplex(1024);
        let result = read_hook_input(reader, 4, Duration::from_millis(20)).await;
        assert_eq!(
            result.map_err(|error| error.to_string()),
            Err("Hook input timed out".to_owned())
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn read_hook_input_decodes_utf8_across_chunks_lossily() {
        let (mut writer, reader) = tokio::io::duplex(1);
        let task = tokio::spawn(async move {
            let _ = writer.write_all("é€".as_bytes()).await;
            let _ = writer.write_all(&[0xff, b'a']).await;
        });
        let result = read_hook_input(reader, 100, Duration::from_secs(5)).await;
        assert!(task.await.is_ok());
        assert_eq!(result.map_err(|error| error.to_string()), Ok("é€\u{fffd}a".to_owned()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn channel_reader_splits_chunks_to_the_buffer() {
        let (sender, receiver) = mpsc::channel(4);
        assert!(sender.send(Ok(b"hello world".to_vec())).await.is_ok());
        drop(sender);
        let mut reader = ChannelReader::new(receiver);
        let mut small = [0u8; 4];
        let mut collected = Vec::new();
        loop {
            match reader.read(&mut small).await {
                Ok(0) | Err(_) => break,
                Ok(read) => collected.extend_from_slice(small.get(..read).unwrap_or_default()),
            }
        }
        assert_eq!(collected, b"hello world");
    }

    // JSON.parse accepts lone UTF-16 surrogate escapes (Node's JSON.stringify writes them for a string cut inside a
    // surrogate pair), so the signal is still built and sent (hook_request's first step is hook_input).
    #[test]
    fn hook_input_accepts_lone_surrogate_escapes_as_json_parse_does() {
        let raw = r#"{"hook_event_name":"PostToolUse","tool_response":"abc\ud83d"}"#;
        assert!(
            serde_json::from_str::<Value>(raw).is_err(),
            "serde_json rejects the lone half itself"
        );
        assert_eq!(
            hook_input(raw).map_err(|error| error.to_string()),
            Ok(serde_json::json!({"hook_event_name": "PostToolUse", "tool_response": "abc\u{fffd}"}))
        );

        // Pairs stay pairs; lone high or low halves (either hex case) become U+FFFD; `\\` is not an escape start.
        let raw = r#"{"x":"😀|\uDE00|\ud83d😀|\\ud83d|é|\ud83d"}"#;
        assert_eq!(
            hook_input(raw).map_err(|error| error.to_string()),
            Ok(serde_json::json!({"x": "\u{1f600}|\u{fffd}|\u{fffd}\u{1f600}|\\ud83d|\u{e9}|\u{fffd}"}))
        );
        // Text that is invalid for another reason stays invalid, reported on the original text.
        for raw in [r#"{"a":"\ud83d"#, r#"{"a":"\ud83d"} \ud83d"#, r#"{"a":"\u12"}"#] {
            assert_eq!(
                hook_input(raw).map_err(|error| error.to_string()),
                Err(DhError::json_parse(raw).to_string()),
                "{raw}"
            );
        }
    }

    #[test]
    fn hook_request_reports_json_parse_errors_verbatim() {
        let result = hook_request(&env(), "{", "id".into(), 1);
        assert_eq!(
            result.map_err(|error| error.to_string()),
            Err(DhError::json_parse("{").to_string())
        );
    }
}
