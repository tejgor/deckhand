//! Port of attach.ts.
//!
//! `deckhand attach` (and the UI's handoff to an attached pane) proxies the outer terminal to one PTY in the daemon:
//! stdin goes up as `<pane>-input`, `<pane>-output` comes down to stdout, resizes follow the outer terminal. Ctrl+Space
//! (NUL, or its CSI-u / modifyOtherKeys encodings) or Ctrl+] (0x1d) detaches. Output is cleaned on the way down (8-bit
//! C1 controls become 7-bit ESC sequences; the child's OSC 0/1/2 title changes are dropped so deckhand's own title
//! stays), and input on the way up (vertical wheel events are throttled by the scroll sensitivity; Pi gets PageUp /
//! PageDown instead of wheel events).
//!
//! Offsets: the TS works on UTF-16 strings, but every sequence it looks for is ASCII, so byte offsets on `&str` give
//! the same results.

use deckhand_core::error::DhError;
use deckhand_protocol::Request;
use deckhand_protocol::ServerMessage;
use deckhand_protocol::data::{AttachTarget, ProgramKey, TerminalModes};
use deckhand_protocol::events::SessionOutput;
use deckhand_protocol::wire::{Attach, Detach, Input, Resize};
use serde_json::Value;

/// TS: attach.ts AttachSessionOptions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttachSessionOptions {
    /// The session title shown in the terminal title (default: the session ID).
    pub title: Option<String>,
    /// The agent program (decides alternate screen, mouse reporting and wheel translation).
    pub program: Option<ProgramKey>,
    /// Fraction of vertical wheel events forwarded (0..=1, default 0.12).
    pub scroll_sensitivity: Option<f64>,
}

/// TS: attach.ts targetRequestNames result: the message types of one attach target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetRequestNames {
    pub attach: &'static str,
    pub input: &'static str,
    pub resize: &'static str,
    pub detach: &'static str,
    pub output: &'static str,
    pub detached: &'static str,
}

/// TS: attach.ts targetRequestNames (`attach`/`input`/... for the agent, `attach-terminal`/`terminal-input`/... for
/// the panes).
pub fn target_request_names(target: AttachTarget) -> TargetRequestNames {
    let _ = target;
    todo!("K2")
}

/// The target's attach request (`names.attach`).
pub fn attach_request(target: AttachTarget, attach: Attach) -> Request {
    let _ = (target, attach);
    todo!("K2")
}

/// The target's input message (`names.input`).
pub fn input_request(target: AttachTarget, input: Input) -> Request {
    let _ = (target, input);
    todo!("K2")
}

/// The target's resize message (`names.resize`).
pub fn resize_request(target: AttachTarget, resize: Resize) -> Request {
    let _ = (target, resize);
    todo!("K2")
}

/// The target's detach message (`names.detach`).
pub fn detach_request(target: AttachTarget, detach: Detach) -> Request {
    let _ = (target, detach);
    todo!("K2")
}

/// `message.type === names.output`: the target's output event, if `message` is one.
pub fn target_output(target: AttachTarget, message: &ServerMessage) -> Option<&SessionOutput> {
    let _ = (target, message);
    todo!("K2")
}

/// `message.type === names.detached`: the detached session's ID, if `message` is the target's detached event.
pub fn target_detached(target: AttachTarget, message: &ServerMessage) -> Option<&str> {
    let _ = (target, message);
    todo!("K2")
}

/// TS: attach.ts sanitizeTerminalTitle: C0/DEL/C1 controls become spaces, whitespace runs collapse, then trim.
pub fn sanitize_terminal_title(value: &str) -> String {
    let _ = value;
    todo!("K2")
}

/// TS: attach.ts setTerminalTitle: OSC 0 and OSC 2 with the sanitized title, when stdout is a TTY.
pub fn set_terminal_title(value: &str) {
    let _ = value;
    todo!("K2")
}

/// TS: attach.ts setProcessTitle (best effort; failures ignored).
pub fn set_process_title(value: &str) {
    let _ = value;
    todo!("K2")
}

/// The process title to restore after an attach (TS: `process.title || 'deckhand'`).
pub fn current_process_title() -> String {
    todo!("K2")
}

/// TS: attach.ts normalizeTerminalOutput: 8-bit C1 controls (U+008E..U+009F subset) to their 7-bit ESC forms.
pub fn normalize_terminal_output(data: &str) -> String {
    let _ = data;
    todo!("K2")
}

/// TS: attach.ts createTerminalTitleOutputFilter: drops OSC 0/1/2 (BEL- or ST-terminated) from a stream, holding back
/// an incomplete sequence (or a trailing ESC) until the next chunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalTitleOutputFilter {
    pending: String,
}

impl TerminalTitleOutputFilter {
    /// TS: createTerminalTitleOutputFilter.
    pub fn new() -> Self {
        todo!("K2")
    }

    /// The returned filter function: the chunk's output with title sequences removed.
    pub fn filter(&mut self, data: &str) -> String {
        let _ = (&mut self.pending, data);
        todo!("K2")
    }
}

/// TS: attach.ts normalizeScrollSensitivity: missing or non-finite is 0.12; otherwise clamped to 0..=1.
pub fn normalize_scroll_sensitivity(value: Option<f64>) -> f64 {
    let _ = value;
    todo!("K2")
}

/// TS: attach.ts createAttachInputNormalizer: throttles SGR vertical wheel events (`ESC [ < 64|65 ; x ; y M|m`) by
/// accumulating `scroll_sensitivity` per event and forwarding one per whole unit; horizontal wheel events (66/67) pass
/// through; for Pi a forwarded wheel up/down becomes PageUp (`ESC [5~`) / PageDown (`ESC [6~`).
#[derive(Clone, Debug, PartialEq)]
pub struct AttachInputNormalizer {
    program: Option<ProgramKey>,
    scroll_sensitivity: f64,
    vertical_wheel_accumulator: f64,
}

impl AttachInputNormalizer {
    /// TS: createAttachInputNormalizer.
    pub fn new(program: Option<ProgramKey>, scroll_sensitivity: f64) -> Self {
        let _ = (program, scroll_sensitivity);
        todo!("K2")
    }

    /// The returned normalizer function.
    pub fn normalize(&mut self, data: &str) -> String {
        let _ = (
            self.program,
            self.scroll_sensitivity,
            &mut self.vertical_wheel_accumulator,
            data,
        );
        todo!("K2")
    }
}

/// TS: attach.ts attachTargetTitleLabel (`term`, `git`, `dev`, `action`, `agent`).
pub fn attach_target_title_label(target: AttachTarget) -> &'static str {
    let _ = target;
    todo!("K2")
}

/// TS: attach.ts compactTerminalTitle: sanitized, cut to `max_length` UTF-16 units (default 32) with a trailing `…`.
pub fn compact_terminal_title(value: &str, max_length: Option<usize>) -> String {
    let _ = (value, max_length);
    todo!("K2")
}

/// TS: attach.ts attachedTerminalTitle: `dh/<label> <compact title or session ID>`.
pub fn attached_terminal_title(session_id: &str, target: AttachTarget, options: &AttachSessionOptions) -> String {
    let _ = (session_id, target, options);
    todo!("K2")
}

/// TS: attach.ts isDetachInput: the chunk holds NUL or 0x1d, or Ctrl+Space as `ESC [32;5u` / `ESC [27;5;32~`.
pub fn is_detach_input(chunk: &[u8]) -> bool {
    let _ = chunk;
    todo!("K2")
}

/// TS: attach.ts attachResponseData: the attach response's `terminalModes`, if its data is an object that has them.
pub fn attach_response_terminal_modes(data: Option<&Value>) -> Option<TerminalModes> {
    let _ = data;
    todo!("K2")
}

/// TS: attach.ts `useAttachScreen`: only an agent attach (Claude, or an unknown program) or Git gets deckhand's
/// alternate screen; Terminal, Dev and action panes and Pi/Codex agents keep the normal screen.
pub fn use_attach_screen(target: AttachTarget, program: Option<ProgramKey>) -> bool {
    let _ = (target, program);
    todo!("K2")
}

/// TS: attach.ts attachRows: the outer terminal's rows (24 when unknown), at least 1.
pub fn attach_rows() -> i64 {
    todo!("K2")
}

/// The outer terminal's columns (TS: `process.stdout.columns || 80`).
pub fn attach_cols() -> i64 {
    todo!("K2")
}

/// TS: attach.ts enterAttachScreen: reset modes, then alternate screen, clear, home (TTY only).
pub fn enter_attach_screen() {
    todo!("K2")
}

/// TS: attach.ts enableAttachMouseReporting: basic + SGR mouse reporting (TTY only), so the wheel arrives as mouse
/// events the input normalizer can throttle instead of arrow keys.
pub fn enable_attach_mouse_reporting() {
    todo!("K2")
}

/// TS: attach.ts enableAttachBracketedPaste (TTY only): the agent's own request went to the PTY before we attached.
pub fn enable_attach_bracketed_paste() {
    todo!("K2")
}

/// TS: attach.ts leaveAttachScreen: reset modes, then leave the alternate screen (TTY only).
pub fn leave_attach_screen() {
    todo!("K2")
}

/// TS: attach.ts attachSession (exported). Connects (starting the daemon if needed), attaches `session_id`'s `target`
/// pane (default the agent) at the outer terminal's size, puts stdin in raw mode and proxies until detach, the pane's
/// `*-detached` event, or (agent only) the session exiting (`\n\r[session exited: <title>]\n`). Restores the terminal,
/// title and process title on the way out. Errors: the attach response's `error` (or `attach failed`), a socket
/// error, or `daemon connection closed during attach` after attaching.
pub async fn attach_session(
    session_id: &str,
    target: Option<AttachTarget>,
    options: AttachSessionOptions,
) -> Result<(), DhError> {
    let _ = (session_id, target, options);
    todo!("K2")
}
