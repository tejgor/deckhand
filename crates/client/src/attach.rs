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
//! the same results. The one length check that counts code units (an OSC prefix cut off at the end of a chunk) counts
//! them explicitly.
//!
//! The proxy loop runs against a [`Console`] (the outer terminal and process title) so it can be driven by a fake in
//! tests; [`attach_session`] uses the real one.

use std::io::{IsTerminal, Write as _};
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::task::Poll;
use std::thread::JoinHandle;

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::termios::{self, ControlFlags, InputFlags, LocalFlags, OutputFlags, SetArg, Termios};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use deckhand_core::error::DhError;
use deckhand_core::jsstr::{js_collapse_whitespace, js_len, js_slice, js_trim};
use deckhand_protocol::Request;
use deckhand_protocol::ServerMessage;
use deckhand_protocol::data::{AttachTarget, SessionRecord, SessionStatus, TerminalModes};
use deckhand_protocol::events::SessionOutput;
use deckhand_protocol::wire::{Attach, Detach, Input, RequestOnly, Resize, Subscribe};

use crate::attach_status::StatusLine;
use crate::client::{MessageReader, frame_message, open_persistent_connection, socket_error};
use crate::term_state::RESET_TERMINAL_STATE_SEQUENCE;

/// TS: attach.ts AttachSessionOptions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttachSessionOptions {
    /// The session title shown in the terminal title (default: the session ID).
    pub title: Option<String>,
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
    match target {
        AttachTarget::Terminal => TargetRequestNames {
            attach: "attach-terminal",
            input: "terminal-input",
            resize: "terminal-resize",
            detach: "terminal-detach",
            output: "terminal-output",
            detached: "terminal-detached",
        },
        AttachTarget::Git => TargetRequestNames {
            attach: "attach-git",
            input: "git-input",
            resize: "git-resize",
            detach: "git-detach",
            output: "git-output",
            detached: "git-detached",
        },
        AttachTarget::Action => TargetRequestNames {
            attach: "attach-action",
            input: "action-input",
            resize: "action-resize",
            detach: "action-detach",
            output: "action-output",
            detached: "action-detached",
        },
        AttachTarget::Dev => TargetRequestNames {
            attach: "attach-dev",
            input: "dev-input",
            resize: "dev-resize",
            detach: "dev-detach",
            output: "dev-output",
            detached: "dev-detached",
        },
        AttachTarget::Agent => TargetRequestNames {
            attach: "attach",
            input: "input",
            resize: "resize",
            detach: "detach",
            output: "output",
            detached: "detached",
        },
    }
}

/// The target's attach request (`names.attach`).
pub fn attach_request(target: AttachTarget, attach: Attach) -> Request {
    match target {
        AttachTarget::Agent => Request::Attach(attach),
        AttachTarget::Terminal => Request::AttachTerminal(attach),
        AttachTarget::Git => Request::AttachGit(attach),
        AttachTarget::Dev => Request::AttachDev(attach),
        AttachTarget::Action => Request::AttachAction(attach),
    }
}

/// The target's input message (`names.input`).
pub fn input_request(target: AttachTarget, input: Input) -> Request {
    match target {
        AttachTarget::Agent => Request::Input(input),
        AttachTarget::Terminal => Request::TerminalInput(input),
        AttachTarget::Git => Request::GitInput(input),
        AttachTarget::Dev => Request::DevInput(input),
        AttachTarget::Action => Request::ActionInput(input),
    }
}

/// The target's resize message (`names.resize`).
pub fn resize_request(target: AttachTarget, resize: Resize) -> Request {
    match target {
        AttachTarget::Agent => Request::Resize(resize),
        AttachTarget::Terminal => Request::TerminalResize(resize),
        AttachTarget::Git => Request::GitResize(resize),
        AttachTarget::Dev => Request::DevResize(resize),
        AttachTarget::Action => Request::ActionResize(resize),
    }
}

/// The target's detach message (`names.detach`).
pub fn detach_request(target: AttachTarget, detach: Detach) -> Request {
    match target {
        AttachTarget::Agent => Request::Detach(detach),
        AttachTarget::Terminal => Request::TerminalDetach(detach),
        AttachTarget::Git => Request::GitDetach(detach),
        AttachTarget::Dev => Request::DevDetach(detach),
        AttachTarget::Action => Request::ActionDetach(detach),
    }
}

/// `message.type === names.output`: the target's output event, if `message` is one.
pub fn target_output(target: AttachTarget, message: &ServerMessage) -> Option<&SessionOutput> {
    match (target, message) {
        (AttachTarget::Agent, ServerMessage::Output(output))
        | (AttachTarget::Terminal, ServerMessage::TerminalOutput(output))
        | (AttachTarget::Git, ServerMessage::GitOutput(output))
        | (AttachTarget::Dev, ServerMessage::DevOutput(output))
        | (AttachTarget::Action, ServerMessage::ActionOutput(output)) => Some(output),
        _ => None,
    }
}

/// `message.type === names.detached`: the detached session's ID, if `message` is the target's detached event.
pub fn target_detached(target: AttachTarget, message: &ServerMessage) -> Option<&str> {
    match (target, message) {
        (AttachTarget::Agent, ServerMessage::Detached(event))
        | (AttachTarget::Terminal, ServerMessage::TerminalDetached(event))
        | (AttachTarget::Git, ServerMessage::GitDetached(event))
        | (AttachTarget::Dev, ServerMessage::DevDetached(event))
        | (AttachTarget::Action, ServerMessage::ActionDetached(event)) => Some(event.session_id.as_str()),
        _ => None,
    }
}

/// TS: attach.ts sanitizeTerminalTitle: C0/DEL/C1 controls become spaces, whitespace runs collapse, then trim.
pub fn sanitize_terminal_title(value: &str) -> String {
    let spaced: String = value
        .chars()
        .map(|ch| {
            if matches!(ch, '\u{0000}'..='\u{001F}' | '\u{007F}'..='\u{009F}') {
                ' '
            } else {
                ch
            }
        })
        .collect();
    js_trim(&js_collapse_whitespace(&spaced)).to_owned()
}

/// TS: attach.ts setTerminalTitle: OSC 0 and OSC 2 with the sanitized title, when stdout is a TTY.
pub fn set_terminal_title(value: &str) {
    set_terminal_title_on(&mut StdConsole::new(), value);
}

/// TS: attach.ts setProcessTitle (best effort; failures ignored).
pub fn set_process_title(value: &str) {
    write_process_title(&sanitize_terminal_title(value));
}

/// The process title to restore after an attach (TS: `process.title || 'deckhand'`).
pub fn current_process_title() -> String {
    let stored = PROCESS_TITLE.lock().unwrap_or_else(PoisonError::into_inner).clone();
    stored
        .or_else(default_process_title)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| String::from("deckhand"))
}

/// The last title set through [`set_process_title`] (Node keeps `process.title` itself; the OS only gets a copy).
static PROCESS_TITLE: Mutex<Option<String>> = Mutex::new(None);

/// Before any title was set: the executable's name, as Node's default `process.title` is.
fn default_process_title() -> Option<String> {
    let program = std::env::args_os().next()?;
    let name = std::path::Path::new(&program).file_name()?;
    Some(name.to_string_lossy().into_owned())
}

/// `process.title = title`: remembered for [`current_process_title`] and handed to the OS as the main thread's name
/// (the part of a process title Rust can set without rewriting argv). Failures are ignored.
fn write_process_title(title: &str) {
    *PROCESS_TITLE.lock().unwrap_or_else(PoisonError::into_inner) = Some(title.to_owned());
    set_os_thread_name(title);
}

#[cfg(target_os = "macos")]
fn set_os_thread_name(title: &str) {
    // pthread_setname_np rejects names over 63 bytes; cut on a char boundary.
    let mut end = title.len().min(63);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    let Ok(name) = std::ffi::CString::new(title.get(..end).unwrap_or_default()) else {
        return;
    };
    // SAFETY: `name` is a valid NUL-terminated string that outlives the call.
    unsafe {
        libc::pthread_setname_np(name.as_ptr());
    }
}

#[cfg(target_os = "linux")]
fn set_os_thread_name(title: &str) {
    let Ok(name) = std::ffi::CString::new(title) else {
        return;
    };
    // SAFETY: PR_SET_NAME reads a NUL-terminated string (truncated by the kernel to 15 bytes) from a valid pointer.
    unsafe {
        libc::prctl(libc::PR_SET_NAME, name.as_ptr());
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn set_os_thread_name(_title: &str) {}

/// TS: attach.ts normalizeTerminalOutput: 8-bit C1 controls (U+008E..U+009F subset) to their 7-bit ESC forms.
pub fn normalize_terminal_output(data: &str) -> String {
    // Some PTY programs emit 8-bit C1 controls (for example CSI as U+009B). Many UTF-8 terminals do not interpret
    // those reliably and the printable tail leaks onto the screen as text like "40;3H" or ";1".
    let mut out = String::with_capacity(data.len());
    for ch in data.chars() {
        match ch {
            '\u{008E}' => out.push_str("\x1bN"),
            '\u{008F}' => out.push_str("\x1bO"),
            '\u{0090}' => out.push_str("\x1bP"),
            '\u{009B}' => out.push_str("\x1b["),
            '\u{009D}' => out.push_str("\x1b]"),
            '\u{009E}' => out.push_str("\x1b^"),
            '\u{009F}' => out.push_str("\x1b_"),
            other => out.push(other),
        }
    }
    out
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
        TerminalTitleOutputFilter { pending: String::new() }
    }

    /// The returned filter function: the chunk's output with title sequences removed.
    pub fn filter(&mut self, data: &str) -> String {
        let mut input = std::mem::take(&mut self.pending);
        input.push_str(data);
        let mut output = String::new();
        let mut index = 0;

        while index < input.len() {
            let Some(start) = find_from(&input, "\x1b]", index) else {
                output.push_str(input.get(index..).unwrap_or_default());
                if output.ends_with('\x1b') {
                    self.pending = String::from("\x1b");
                    output.pop();
                }
                break;
            };

            output.push_str(input.get(index..start).unwrap_or_default());
            let rest = input.get(start..).unwrap_or_default();
            // `input.slice(start, start + 4).length < 4`, in UTF-16 code units.
            if rest.chars().take(4).map(char::len_utf16).sum::<usize>() < 4 {
                self.pending = rest.to_owned();
                break;
            }

            if !(rest.starts_with("\x1b]0;") || rest.starts_with("\x1b]1;") || rest.starts_with("\x1b]2;")) {
                output.push_str("\x1b]");
                index = start + 2;
                continue;
            }

            let content_start = start + 4;
            let bell_end = find_from(&input, "\x07", content_start);
            let st_end = find_from(&input, "\x1b\\", content_start);
            let end = match (bell_end, st_end) {
                (None, st) => st,
                (bell, None) => bell,
                (Some(bell), Some(st)) => Some(bell.min(st)),
            };
            let Some(end) = end else {
                self.pending = rest.to_owned();
                break;
            };

            index = end + if Some(end) == st_end { 2 } else { 1 };
        }

        output
    }
}

/// `text.indexOf(search, from)` as a byte offset (`from` is a char boundary).
fn find_from(text: &str, search: &str, from: usize) -> Option<usize> {
    text.get(from..)?.find(search).map(|offset| from + offset)
}

/// TS: attach.ts normalizeScrollSensitivity: missing or non-finite is 0.12; otherwise clamped to 0..=1.
pub fn normalize_scroll_sensitivity(value: Option<f64>) -> f64 {
    match value {
        Some(value) if value.is_finite() => value.clamp(0.0, 1.0),
        _ => 0.12,
    }
}

/// TS: attach.ts createAttachInputNormalizer: throttles SGR vertical wheel events (`ESC [ < 64|65 ; x ; y M|m`) by
/// accumulating `scroll_sensitivity` per event and forwarding one per whole unit; horizontal wheel events (66/67) pass
/// through. Every agent's fullscreen TUI scrolls its own view on the wheel (TS turned it into PageUp/PageDown for Pi,
/// which predates Pi's fullscreen mode).
#[derive(Clone, Debug, PartialEq)]
pub struct AttachInputNormalizer {
    scroll_sensitivity: f64,
    vertical_wheel_accumulator: f64,
}

impl AttachInputNormalizer {
    /// TS: createAttachInputNormalizer.
    pub fn new(scroll_sensitivity: f64) -> Self {
        AttachInputNormalizer {
            scroll_sensitivity,
            vertical_wheel_accumulator: 0.0,
        }
    }

    /// The returned normalizer function.
    pub fn normalize(&mut self, data: &str) -> String {
        // `data.replace(/\x1b\[<(6[4-7]);\d+;\d+(?:;\d+;\d+)?[mM]/g, ...)`: every match starts at an ESC.
        let mut out = String::with_capacity(data.len());
        let mut copied = 0;
        let mut search = 0;
        while let Some(at) = find_from(data, "\x1b", search) {
            let Some((end, code)) = match_wheel_event(data, at) else {
                search = at + 1;
                continue;
            };
            out.push_str(data.get(copied..at).unwrap_or_default());
            let matched = data.get(at..end).unwrap_or_default();
            out.push_str(self.replace_wheel_event(matched, code));
            copied = end;
            search = end;
        }
        out.push_str(data.get(copied..).unwrap_or_default());
        out
    }

    /// The regex callback for one wheel event (`code` 64..=67).
    fn replace_wheel_event<'a>(&mut self, matched: &'a str, code: u8) -> &'a str {
        if code != 64 && code != 65 {
            return matched;
        }
        self.vertical_wheel_accumulator += self.scroll_sensitivity;
        if self.vertical_wheel_accumulator < 1.0 {
            return "";
        }
        self.vertical_wheel_accumulator -= 1.0;
        matched
    }
}

/// Matches `\x1b\[<(6[4-7]);\d+;\d+(?:;\d+;\d+)?[mM]` at byte `at`: the match's end and its button code. The regex
/// has no ambiguous backtracking (digit runs are followed by `;` or `m`/`M`), so a direct scan decides it.
fn match_wheel_event(data: &str, at: usize) -> Option<(usize, u8)> {
    let bytes = data.as_bytes();
    let head = bytes.get(at..at + 5)?;
    let (prefix, digit) = head.split_at(4);
    if prefix != b"\x1b[<6" {
        return None;
    }
    let digit = *digit.first()?;
    if !(b'4'..=b'7').contains(&digit) {
        return None;
    }
    let code = 60 + (digit - b'0');
    let semicolon_digits = |from: usize| -> Option<usize> {
        if bytes.get(from) != Some(&b';') {
            return None;
        }
        let digits = bytes
            .get(from + 1..)?
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        (digits > 0).then_some(from + 1 + digits)
    };
    let is_final = |index: usize| matches!(bytes.get(index), Some(b'm' | b'M'));

    let after_xy = semicolon_digits(semicolon_digits(at + 5)?)?;
    if is_final(after_xy) {
        return Some((after_xy + 1, code));
    }
    let after_extra = semicolon_digits(semicolon_digits(after_xy)?)?;
    is_final(after_extra).then_some((after_extra + 1, code))
}

/// TS: attach.ts attachTargetTitleLabel (`term`, `git`, `dev`, `action`, `agent`).
pub fn attach_target_title_label(target: AttachTarget) -> &'static str {
    match target {
        AttachTarget::Terminal => "term",
        AttachTarget::Git => "git",
        AttachTarget::Dev => "dev",
        AttachTarget::Action => "action",
        AttachTarget::Agent => "agent",
    }
}

/// TS: attach.ts compactTerminalTitle: sanitized, cut to `max_length` UTF-16 units (default 32) with a trailing `…`.
pub fn compact_terminal_title(value: &str, max_length: Option<usize>) -> String {
    let max_length = max_length.unwrap_or(32);
    let title = sanitize_terminal_title(value);
    if js_len(&title) <= max_length {
        return title;
    }
    let max = i64::try_from(max_length).unwrap_or(i64::MAX);
    if max_length <= 1 {
        js_slice(&title, 0, Some(max))
    } else {
        format!("{}…", js_slice(&title, 0, Some(max - 1)))
    }
}

/// TS: attach.ts attachedTerminalTitle: `dh/<label> <compact title or session ID>`.
pub fn attached_terminal_title(session_id: &str, target: AttachTarget, options: &AttachSessionOptions) -> String {
    let title = compact_terminal_title(options.title.as_deref().unwrap_or(session_id), None);
    let label = attach_target_title_label(target);
    format!("dh/{label} {title}")
}

/// TS: attach.ts isDetachInput: the chunk holds NUL or 0x1d, or Ctrl+Space as `ESC [32;5u` / `ESC [27;5;32~`.
pub fn is_detach_input(chunk: &[u8]) -> bool {
    if chunk.contains(&0x00) || chunk.contains(&0x1d) {
        return true;
    }
    // Some terminals encode Ctrl+Space as an enhanced-keyboard escape sequence when an attached TUI enables CSI-u /
    // modifyOtherKeys mode. TS decodes the chunk as UTF-8 first; lossy decoding never consumes an ASCII byte into a
    // replacement character, so searching the bytes finds the same sequences.
    contains_bytes(chunk, b"\x1b[32;5u") || contains_bytes(chunk, b"\x1b[27;5;32~")
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// TS: attach.ts attachResponseData: the attach response's `terminalModes`, if its data is an object that has them.
pub fn attach_response_terminal_modes(data: Option<&Value>) -> Option<TerminalModes> {
    // `!value || typeof value !== 'object'` keeps objects (and arrays, which have no `terminalModes`).
    let modes = data?.as_object()?.get("terminalModes")?;
    if modes.is_null() {
        return None;
    }
    // `terminalModes?.bracketedPaste` is read for its truthiness only.
    let bracketed_paste = modes.get("bracketedPaste").is_some_and(js_truthy);
    Some(TerminalModes { bracketed_paste })
}

/// JS truthiness of a JSON value.
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// TS: attach.ts `useAttachScreen`: an agent attach or Git gets deckhand's alternate screen (Claude, Pi and Codex all
/// draw fullscreen; TS kept the normal screen for Pi and Codex); Terminal, Dev and action panes keep the normal screen.
pub fn use_attach_screen(target: AttachTarget) -> bool {
    !matches!(
        target,
        AttachTarget::Terminal | AttachTarget::Dev | AttachTarget::Action
    )
}

/// TS: attach.ts attachRows: the outer terminal's rows (24 when unknown), at least 1.
pub fn attach_rows() -> i64 {
    attach_rows_on(&StdConsole::new())
}

/// The outer terminal's columns (TS: `process.stdout.columns || 80`).
pub fn attach_cols() -> i64 {
    attach_cols_on(&StdConsole::new())
}

/// TS: attach.ts enterAttachScreen: reset modes, then alternate screen, clear, home (TTY only).
pub fn enter_attach_screen() {
    enter_attach_screen_on(&mut StdConsole::new());
}

/// TS: attach.ts enableAttachMouseReporting: basic + SGR mouse reporting (TTY only), so the wheel arrives as mouse
/// events the input normalizer can throttle instead of arrow keys.
pub fn enable_attach_mouse_reporting() {
    enable_attach_mouse_reporting_on(&mut StdConsole::new());
}

/// TS: attach.ts enableAttachBracketedPaste (TTY only): the agent's own request went to the PTY before we attached.
pub fn enable_attach_bracketed_paste() {
    enable_attach_bracketed_paste_on(&mut StdConsole::new());
}

/// TS: attach.ts leaveAttachScreen: reset modes, then leave the alternate screen (TTY only).
pub fn leave_attach_screen() {
    leave_attach_screen_on(&mut StdConsole::new());
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
    let socket = open_persistent_connection().await?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut console = StdConsole::new();
    run_attach(
        &mut console,
        socket,
        session_id,
        target.unwrap_or(AttachTarget::Agent),
        &options,
        request_id,
    )
    .await
}

// ---------------------------------------------------------------------------------------------------------------
// The outer terminal

/// Something the outer terminal reports while attached.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConsoleEvent {
    /// A stdin read (`process.stdin.on('data')`).
    Data(Vec<u8>),
    /// The terminal was resized (`process.stdout.on('resize')`).
    Resize,
}

/// The outer terminal and process an attach drives (TS `process.stdout`, `process.stdin`, `process.title`).
pub(crate) trait Console {
    /// `process.stdout.isTTY`.
    fn stdout_is_tty(&self) -> bool;
    /// `process.stdin.isTTY`.
    fn stdin_is_tty(&self) -> bool;
    /// `process.stdout.write(data)` (best effort).
    fn write(&mut self, data: &str);
    /// `process.stdout.columns` (`None` off a TTY).
    fn columns(&self) -> Option<i64>;
    /// `process.stdout.rows` (`None` off a TTY).
    fn rows(&self) -> Option<i64>;
    /// `process.stdin.setRawMode(enabled)`.
    fn set_raw_mode(&mut self, enabled: bool);
    /// `process.title || 'deckhand'`.
    fn process_title(&self) -> String;
    /// `process.title = title` (best effort).
    fn set_process_title(&mut self, title: &str);
    /// `process.stdin.resume()` + `on('data')` + `process.stdout.on('resize')`: events go to `events` until
    /// [`Console::stop_input`].
    fn start_input(&mut self, events: mpsc::UnboundedSender<ConsoleEvent>);
    /// Removes the listeners [`Console::start_input`] added (stdin is not read any more).
    fn stop_input(&mut self);
}

fn reset_terminal_state_on(console: &mut impl Console) {
    if console.stdout_is_tty() {
        console.write(RESET_TERMINAL_STATE_SEQUENCE);
    }
}

fn set_terminal_title_on(console: &mut impl Console, value: &str) {
    if !console.stdout_is_tty() {
        return;
    }
    let title = sanitize_terminal_title(value);
    console.write(&format!("\x1b]0;{title}\x07\x1b]2;{title}\x07"));
}

fn enter_attach_screen_on(console: &mut impl Console) {
    if !console.stdout_is_tty() {
        return;
    }
    reset_terminal_state_on(console);
    console.write("\x1b[?1049h\x1b[2J\x1b[H");
}

fn enable_attach_mouse_reporting_on(console: &mut impl Console) {
    if !console.stdout_is_tty() {
        return;
    }
    // Deckhand's attach view owns the outer terminal while proxying input to the child PTY. The agent enabled mouse
    // reporting in its PTY before we attached, so the outer terminal never saw that request; in an alternate screen
    // most terminals then turn the wheel into Up/Down arrow keys. Enabling basic SGR mouse reporting here keeps wheel
    // input as mouse events so the attach input normalizer can throttle and forward them to the agent.
    console.write("\x1b[?1000h\x1b[?1006h");
}

fn enable_attach_bracketed_paste_on(console: &mut impl Console) {
    if !console.stdout_is_tty() {
        return;
    }
    // When attaching to an already-running agent TUI, its earlier request to enable bracketed paste was written to
    // the PTY while Deckhand was not yet connected to the real terminal. Re-enable it on the outer terminal so a
    // multi-line paste is forwarded as one bracketed paste instead of a series of Enter key presses that the agent
    // may submit as separate messages.
    console.write("\x1b[?2004h");
}

fn leave_attach_screen_on(console: &mut impl Console) {
    if !console.stdout_is_tty() {
        return;
    }
    reset_terminal_state_on(console);
    console.write("\x1b[?1049l");
}

fn attach_rows_on(console: &impl Console) -> i64 {
    console.rows().filter(|rows| *rows != 0).unwrap_or(24).max(1)
}

fn attach_cols_on(console: &impl Console) -> i64 {
    console.columns().filter(|cols| *cols != 0).unwrap_or(80)
}

/// The real terminal: fds 0 and 1 of this process.
#[derive(Default)]
struct StdConsole {
    /// The termios to restore when raw mode ends (saved when it began).
    saved_termios: Option<Termios>,
    reader: Option<StdinReader>,
    resize_task: Option<tokio::task::JoinHandle<()>>,
}

impl StdConsole {
    fn new() -> Self {
        StdConsole::default()
    }

    /// The stdout window size (cols, rows), when stdout is a terminal.
    fn window_size() -> Option<(i64, i64)> {
        let stdout = std::io::stdout();
        if !stdout.is_terminal() {
            return None;
        }
        let mut size = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: TIOCGWINSZ writes one `winsize` through a pointer to a live, writable `winsize`.
        let result = unsafe { libc::ioctl(stdout.as_raw_fd(), libc::TIOCGWINSZ, &mut size) };
        if result != 0 {
            // libuv leaves the size at 0 when the ioctl fails, which the `|| 80` / `|| 24` fallbacks then replace.
            return Some((0, 0));
        }
        Some((i64::from(size.ws_col), i64::from(size.ws_row)))
    }
}

impl Drop for StdConsole {
    fn drop(&mut self) {
        self.stop_input();
        self.set_raw_mode(false);
    }
}

impl Console for StdConsole {
    fn stdout_is_tty(&self) -> bool {
        std::io::stdout().is_terminal()
    }

    fn stdin_is_tty(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn write(&mut self, data: &str) {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(data.as_bytes());
        let _ = stdout.flush();
    }

    fn columns(&self) -> Option<i64> {
        StdConsole::window_size().map(|(cols, _)| cols)
    }

    fn rows(&self) -> Option<i64> {
        StdConsole::window_size().map(|(_, rows)| rows)
    }

    fn set_raw_mode(&mut self, enabled: bool) {
        let stdin = std::io::stdin();
        if !enabled {
            if let Some(saved) = self.saved_termios.take() {
                let _ = termios::tcsetattr(stdin.as_fd(), SetArg::TCSADRAIN, &saved);
            }
            return;
        }
        if self.saved_termios.is_some() {
            return;
        }
        let Ok(saved) = termios::tcgetattr(stdin.as_fd()) else {
            return;
        };
        // libuv's UV_TTY_MODE_RAW: unlike cfmakeraw it keeps output post-processing (OPOST/ONLCR).
        let mut raw = saved.clone();
        raw.input_flags &=
            !(InputFlags::BRKINT | InputFlags::ICRNL | InputFlags::INPCK | InputFlags::ISTRIP | InputFlags::IXON);
        raw.output_flags |= OutputFlags::ONLCR;
        raw.control_flags |= ControlFlags::CS8;
        raw.local_flags &= !(LocalFlags::ECHO | LocalFlags::ICANON | LocalFlags::IEXTEN | LocalFlags::ISIG);
        raw.control_chars[termios::SpecialCharacterIndices::VMIN as usize] = 1;
        raw.control_chars[termios::SpecialCharacterIndices::VTIME as usize] = 0;
        if termios::tcsetattr(stdin.as_fd(), SetArg::TCSADRAIN, &raw).is_ok() {
            self.saved_termios = Some(saved);
        }
    }

    fn process_title(&self) -> String {
        current_process_title()
    }

    fn set_process_title(&mut self, title: &str) {
        write_process_title(title);
    }

    fn start_input(&mut self, events: mpsc::UnboundedSender<ConsoleEvent>) {
        if self.reader.is_none() {
            self.reader = StdinReader::spawn(std::io::stdin().as_raw_fd(), events.clone()).ok();
        }
        // Node emits 'resize' only for a TTY stdout.
        if self.resize_task.is_none() && self.stdout_is_tty() {
            use tokio::signal::unix::{SignalKind, signal};
            if let Ok(mut signals) = signal(SignalKind::window_change()) {
                self.resize_task = Some(tokio::spawn(async move {
                    while signals.recv().await.is_some() {
                        if events.send(ConsoleEvent::Resize).is_err() {
                            return;
                        }
                    }
                }));
            }
        }
    }

    fn stop_input(&mut self) {
        if let Some(mut reader) = self.reader.take() {
            reader.stop();
        }
        if let Some(task) = self.resize_task.take() {
            task.abort();
        }
    }
}

/// Reads stdin on a thread of its own, blocking in poll(2) (kqueue does not watch ttys on macOS). Stopping it wakes
/// and joins the thread, so no read is left pending (and no byte consumed) once the UI takes the terminal back.
struct StdinReader {
    stop: Option<OwnedFd>,
    thread: Option<JoinHandle<()>>,
}

impl StdinReader {
    fn spawn(fd: RawFd, events: mpsc::UnboundedSender<ConsoleEvent>) -> std::io::Result<StdinReader> {
        let (stop_read, stop_write) = nix::unistd::pipe().map_err(std::io::Error::from)?;
        let thread = std::thread::Builder::new()
            .name(String::from("deckhand-attach-stdin"))
            .spawn(move || stdin_read_loop(fd, &stop_read, &events))?;
        Ok(StdinReader {
            stop: Some(stop_write),
            thread: Some(thread),
        })
    }

    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = nix::unistd::write(&stop, &[1]);
            drop(stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for StdinReader {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reads `fd` until it ends (stdin `end` has no listener in TS: input simply stops) or `stop` becomes readable.
fn stdin_read_loop(fd: RawFd, stop: &OwnedFd, events: &mpsc::UnboundedSender<ConsoleEvent>) {
    let mut buffer = vec![0u8; 65_536];
    let stdin = std::io::stdin();
    loop {
        let mut fds = [
            PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
            PollFd::new(stop.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut fds, PollTimeout::NONE) {
            Err(Errno::EINTR) => continue,
            Err(_) => return,
            Ok(_) => {}
        }
        let ready = |fd: &PollFd<'_>| {
            fd.revents().is_some_and(|flags| {
                !(flags & (PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR)).is_empty()
            })
        };
        let [input, stop_fd] = &fds;
        if ready(stop_fd) {
            return;
        }
        if !ready(input) {
            continue;
        }
        match nix::unistd::read(fd, &mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                let chunk = buffer.get(..count).unwrap_or_default().to_vec();
                if events.send(ConsoleEvent::Data(chunk)).is_err() {
                    return;
                }
            }
            Err(Errno::EINTR | Errno::EAGAIN) => {}
            Err(_) => return,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The proxy

/// The socket's pending writes. TS `socket.write` never blocks: Node buffers what the kernel will not take yet. The
/// proxy queues its messages here and the loop drains them while the socket is writable, so stdin handling (and the
/// detach key) never waits on a daemon that has stopped reading.
#[derive(Default)]
struct WriteQueue {
    buf: Vec<u8>,
    /// Bytes of `buf` already written.
    sent: usize,
}

impl WriteQueue {
    /// writeMessage: wire-logs and queues one request line.
    fn push(&mut self, message: &Request) -> Result<(), DhError> {
        let line = frame_message(message)?;
        self.buf.extend_from_slice(&line);
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.sent >= self.buf.len()
    }

    fn pending(&self) -> &[u8] {
        self.buf.get(self.sent..).unwrap_or_default()
    }

    /// `written` more bytes reached the socket.
    fn advance(&mut self, written: usize) {
        self.sent = self.sent.saturating_add(written).min(self.buf.len());
        if self.is_empty() {
            self.buf.clear();
            self.sent = 0;
        } else if self.sent >= self.buf.len() / 2 {
            self.buf.drain(..self.sent);
            self.sent = 0;
        }
    }

    /// Writes what the socket takes right now, without waiting.
    async fn flush_now<W: AsyncWrite + Unpin>(&mut self, writer: &mut W) {
        std::future::poll_fn(|cx| {
            while !self.is_empty() {
                match Pin::new(&mut *writer).poll_write(cx, self.pending()) {
                    Poll::Ready(Ok(written)) if written > 0 => self.advance(written),
                    _ => break,
                }
            }
            Poll::Ready(())
        })
        .await;
    }
}

/// The TS closure state of one attachSession call.
struct Proxy<'a, C: Console> {
    console: &'a mut C,
    outgoing: WriteQueue,
    session_id: &'a str,
    target: AttachTarget,
    options: &'a AttachSessionOptions,
    request_id: String,
    normalize_attach_input: AttachInputNormalizer,
    filter_terminal_title_output: TerminalTitleOutputFilter,
    use_attach_screen: bool,
    original_process_title: String,
    attached: bool,
    attach_screen_entered: bool,
    title_set: bool,
    events: mpsc::UnboundedSender<ConsoleEvent>,
    input_listening: bool,
    /// Deckhand's status line on the top row (agent attaches on a terminal only).
    status: Option<StatusLine>,
    /// The `list` and `subscribe` requests that feed the status line.
    status_list_id: String,
    status_subscribe_id: String,
}

/// TS attachSession after the connection is open: runs the attach on `socket` against `console`.
pub(crate) async fn run_attach<C: Console, S: AsyncRead + AsyncWrite + Unpin>(
    console: &mut C,
    socket: S,
    session_id: &str,
    target: AttachTarget,
    options: &AttachSessionOptions,
    request_id: String,
) -> Result<(), DhError> {
    let (read_half, mut writer) = tokio::io::split(socket);
    let mut reader = MessageReader::new(read_half);
    let (events, mut input) = mpsc::unbounded_channel();
    let original_process_title = console.process_title();
    let status = (target == AttachTarget::Agent && console.stdout_is_tty())
        .then(|| {
            let title = options.title.as_deref().unwrap_or(session_id);
            StatusLine::new(session_id, title, attach_rows_on(console), attach_cols_on(console))
        })
        .flatten();
    let mut proxy = Proxy {
        console,
        outgoing: WriteQueue::default(),
        session_id,
        target,
        options,
        request_id,
        normalize_attach_input: AttachInputNormalizer::new(normalize_scroll_sensitivity(options.scroll_sensitivity)),
        filter_terminal_title_output: TerminalTitleOutputFilter::new(),
        use_attach_screen: use_attach_screen(target),
        original_process_title,
        attached: false,
        attach_screen_entered: false,
        title_set: false,
        events,
        input_listening: false,
        status,
        status_list_id: uuid::Uuid::new_v4().to_string(),
        status_subscribe_id: uuid::Uuid::new_v4().to_string(),
    };

    let outcome = proxy.run(&mut reader, &mut writer, &mut input).await;
    proxy.cleanup();
    // socket.end(): Node flushes the queued writes, then half-closes, without holding up the settle. Here whatever the
    // socket takes at once is written and the rest dropped with the socket when the attach returns (the daemon detaches
    // a client whose connection closes).
    proxy.outgoing.flush_now(&mut writer).await;
    let _ = std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut writer).poll_shutdown(cx))).await;
    outcome
}

impl<C: Console> Proxy<'_, C> {
    /// Until `finish`: the settled result.
    async fn run<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &mut self,
        reader: &mut MessageReader<R>,
        writer: &mut W,
        input: &mut mpsc::UnboundedReceiver<ConsoleEvent>,
    ) -> Result<(), DhError> {
        let attach = attach_request(
            self.target,
            Attach {
                request_id: self.request_id.clone(),
                session_id: self.session_id.to_owned(),
                cols: Some(attach_cols_on(self.console)),
                rows: Some(self.pty_rows()),
            },
        );
        self.outgoing.push(&attach)?;

        loop {
            tokio::select! {
                // Daemon messages first when both are ready (Node delivers events in arrival order).
                biased;
                message = reader.next() => match message {
                    Ok(Some(message)) => {
                        if let Some(done) = self.on_message(message) {
                            return done;
                        }
                    }
                    Ok(None) => {
                        if self.attached {
                            return Err(DhError::msg("daemon connection closed during attach"));
                        }
                        // TS: a close before the attach response settles nothing, so attachSession never returns.
                        // Nothing else can happen on this connection, and stdin is not being read yet.
                        std::future::pending::<()>().await;
                    }
                    // A socket error. (A line that is not JSON threw out of the TS data listener; it ends the attach
                    // with that error here.)
                    Err(error) => return Err(error),
                },
                // The queued writes, while the socket takes them. A failed write is the socket's 'error' event, which
                // finishes with it.
                written = writer.write(self.outgoing.pending()), if !self.outgoing.is_empty() => match written {
                    Ok(0) => return Err(socket_error(&std::io::ErrorKind::WriteZero.into(), "write")),
                    Ok(written) => self.outgoing.advance(written),
                    Err(error) => return Err(socket_error(&error, "write")),
                },
                event = input.recv(), if self.input_listening => match event {
                    Some(ConsoleEvent::Data(chunk)) => {
                        if let Some(done) = self.on_input(&chunk) {
                            return done;
                        }
                    }
                    Some(ConsoleEvent::Resize) => self.on_resize()?,
                    None => self.input_listening = false,
                },
            }
        }
    }

    /// The attachJsonParser callback: `Some` when the message finishes the attach.
    fn on_message(&mut self, message: ServerMessage) -> Option<Result<(), DhError>> {
        if let Some(done) = self.on_status_message(&message) {
            return done;
        }
        if let ServerMessage::Response(response) = &message
            && response.request_id == self.request_id
        {
            if !response.ok {
                let error = response
                    .error
                    .as_deref()
                    .filter(|error| !error.is_empty())
                    .unwrap_or("attach failed");
                return Some(Err(DhError::msg(error)));
            }
            self.attached = true;
            let terminal_modes = attach_response_terminal_modes(response.data.as_ref());
            // The Ink dashboard enables terminal mouse reporting. When attaching without Deckhand's alternate-screen
            // setup, leaving that mode enabled causes trackpad scrolls to be delivered as input instead of scrolling
            // the IDE terminal's normal scrollback. Always reset modes at attach handoff.
            reset_terminal_state_on(self.console);
            if self.use_attach_screen {
                enter_attach_screen_on(self.console);
                self.attach_screen_entered = true;
            }
            if self.target == AttachTarget::Agent {
                enable_attach_mouse_reporting_on(self.console);
            }
            if self.target == AttachTarget::Agent || terminal_modes.is_some_and(|modes| modes.bracketed_paste) {
                enable_attach_bracketed_paste_on(self.console);
            }
            let next_title = attached_terminal_title(self.session_id, self.target, self.options);
            set_terminal_title_on(self.console, &next_title);
            self.console.set_process_title(&sanitize_terminal_title(&next_title));
            self.title_set = true;
            if let Some(status) = &self.status {
                self.console.write(&status.setup());
                let list = Request::List(RequestOnly {
                    request_id: self.status_list_id.clone(),
                });
                if let Err(error) = self.outgoing.push(&list) {
                    return Some(Err(error));
                }
            }
            if self.console.stdin_is_tty() {
                self.console.set_raw_mode(true);
            }
            if !self.input_listening {
                self.console.start_input(self.events.clone());
                self.input_listening = true;
            }
            return self.on_resize().err().map(Err);
        }

        if let Some(output) = target_output(self.target, &message)
            && output.session_id == self.session_id
        {
            let mut output = self
                .filter_terminal_title_output
                .filter(&normalize_terminal_output(&output.data));
            if let Some(status) = &mut self.status {
                output = status.filter_output(&output);
            }
            self.console.write(&output);
            return None;
        }

        if let ServerMessage::SessionUpdated(update) = &message
            && update.session.id == self.session_id
        {
            // Terminal, Git and Dev belong to the workspace and outlive the agent; their end arrives as
            // <pane>-detached.
            if update.session.status == SessionStatus::Exited && self.target == AttachTarget::Agent {
                self.console
                    .write(&format!("\n\r[session exited: {}]\n", update.session.title));
                return Some(Ok(()));
            }
            return None;
        }

        if target_detached(self.target, &message) == Some(self.session_id) {
            return Some(Ok(()));
        }
        None
    }

    /// The stdin 'data' listener: `Some` when the input detaches (or a write fails).
    fn on_input(&mut self, chunk: &[u8]) -> Option<Result<(), DhError>> {
        // Ctrl+Space is usually NUL (0x00), but attached TUIs can enable enhanced keyboard protocols that encode it
        // as an escape sequence. Ctrl+] (0x1d) remains a secondary attach escape hatch.
        if is_detach_input(chunk) {
            let detach = detach_request(
                self.target,
                Detach {
                    session_id: self.session_id.to_owned(),
                },
            );
            // finish() follows the write at once; its listeners are gone before a write error could be reported.
            let _ = self.outgoing.push(&detach);
            return Some(Ok(()));
        }
        let mut normalized = self.normalize_attach_input.normalize(&String::from_utf8_lossy(chunk));
        if let Some(status) = &mut self.status {
            normalized = status.filter_input(&normalized);
        }
        if normalized.is_empty() {
            return None;
        }
        let message = input_request(
            self.target,
            Input {
                session_id: self.session_id.to_owned(),
                data: normalized.into(),
            },
        );
        self.outgoing.push(&message).err().map(Err)
    }

    /// The stdout 'resize' listener (also called once on attach).
    fn on_resize(&mut self) -> Result<(), DhError> {
        if let Some(status) = &mut self.status {
            let setup = status.resize(attach_rows_on(self.console), attach_cols_on(self.console));
            self.console.write(&setup);
        }
        let message = resize_request(
            self.target,
            Resize {
                session_id: self.session_id.to_owned(),
                cols: Some(attach_cols_on(self.console)),
                rows: Some(self.pty_rows()),
            },
        );
        self.outgoing.push(&message)
    }

    /// The PTY's rows: the outer terminal's, less the status line's row.
    fn pty_rows(&self) -> i64 {
        match &self.status {
            Some(status) => status.agent_rows(),
            None => attach_rows_on(self.console),
        }
    }

    /// The status line's own traffic: the `list` answer (to learn the repository), the `subscribe` answer, and other
    /// sessions' updates. `Some` when the message was one of its answers (the attach goes on either way).
    fn on_status_message(&mut self, message: &ServerMessage) -> Option<Option<Result<(), DhError>>> {
        let status = self.status.as_mut()?;
        match message {
            ServerMessage::Response(response) if response.request_id == self.status_list_id => {
                let sessions: Vec<SessionRecord> = response
                    .data
                    .clone()
                    .and_then(|data| serde_json::from_value(data).ok())
                    .unwrap_or_default();
                let Some(repo_root) = sessions
                    .iter()
                    .find(|session| session.id == self.session_id)
                    .map(|session| session.repo_root.clone())
                else {
                    return Some(None);
                };
                let subscribe = Request::Subscribe(Subscribe {
                    request_id: self.status_subscribe_id.clone(),
                    repo_root: repo_root.into(),
                });
                Some(self.outgoing.push(&subscribe).err().map(Err))
            }
            ServerMessage::Response(response) if response.request_id == self.status_subscribe_id => {
                let sessions: Vec<SessionRecord> = response
                    .data
                    .clone()
                    .and_then(|data| serde_json::from_value(data).ok())
                    .unwrap_or_default();
                let drawn = status.set_sessions(&sessions);
                self.console.write(&drawn);
                Some(None)
            }
            ServerMessage::SessionUpdated(update) => {
                let drawn = status.session_updated(&update.session);
                self.console.write(&drawn);
                None
            }
            ServerMessage::SessionRemoved(removed) => {
                let drawn = status.session_removed(&removed.session_id);
                self.console.write(&drawn);
                None
            }
            _ => None,
        }
    }

    /// TS cleanup (runs once, before the promise settles); `socket.end()` is in [`run_attach`].
    fn cleanup(&mut self) {
        if let Some(status) = &self.status
            && self.attached
        {
            self.console.write(&status.teardown());
        }
        if self.console.stdin_is_tty() {
            self.console.set_raw_mode(false);
        }
        if self.attach_screen_entered {
            leave_attach_screen_on(self.console);
        } else {
            reset_terminal_state_on(self.console);
        }
        if self.title_set {
            set_terminal_title_on(self.console, "deckhand");
            let original = sanitize_terminal_title(&self.original_process_title);
            self.console.set_process_title(&original);
        }
        self.console.stop_input();
        self.input_listening = false;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    use super::*;

    // ---- pure helpers -------------------------------------------------------------------------------------------

    #[test]
    fn target_names_match_the_ts_table() {
        let agent = target_request_names(AttachTarget::Agent);
        assert_eq!(
            (
                agent.attach,
                agent.input,
                agent.resize,
                agent.detach,
                agent.output,
                agent.detached
            ),
            ("attach", "input", "resize", "detach", "output", "detached")
        );
        for (target, prefix) in [
            (AttachTarget::Terminal, "terminal"),
            (AttachTarget::Git, "git"),
            (AttachTarget::Dev, "dev"),
            (AttachTarget::Action, "action"),
        ] {
            let names = target_request_names(target);
            assert_eq!(names.attach, format!("attach-{prefix}"));
            assert_eq!(names.input, format!("{prefix}-input"));
            assert_eq!(names.resize, format!("{prefix}-resize"));
            assert_eq!(names.detach, format!("{prefix}-detach"));
            assert_eq!(names.output, format!("{prefix}-output"));
            assert_eq!(names.detached, format!("{prefix}-detached"));
        }
    }

    #[test]
    fn request_builders_use_the_target_names() {
        for target in AttachTarget::ALL.iter().copied() {
            let names = target_request_names(target);
            let attach = attach_request(
                target,
                Attach {
                    request_id: "r".into(),
                    session_id: "s".into(),
                    cols: Some(80),
                    rows: Some(24),
                },
            );
            assert_eq!(
                serde_json::to_value(&attach).unwrap(),
                json!({"type": names.attach, "requestId": "r", "sessionId": "s", "cols": 80, "rows": 24})
            );
            let input = input_request(
                target,
                Input {
                    session_id: "s".into(),
                    data: "x".into(),
                },
            );
            assert_eq!(serde_json::to_value(&input).unwrap()["type"], names.input);
            let resize = resize_request(
                target,
                Resize {
                    session_id: "s".into(),
                    cols: Some(1),
                    rows: Some(2),
                },
            );
            assert_eq!(serde_json::to_value(&resize).unwrap()["type"], names.resize);
            let detach = detach_request(target, Detach { session_id: "s".into() });
            assert_eq!(
                serde_json::to_value(&detach).unwrap(),
                json!({"type": names.detach, "sessionId": "s"})
            );

            let output: ServerMessage =
                serde_json::from_value(json!({"type": names.output, "sessionId": "s", "data": "d"})).unwrap();
            assert_eq!(target_output(target, &output).map(|o| o.data.as_str()), Some("d"));
            let detached: ServerMessage =
                serde_json::from_value(json!({"type": names.detached, "sessionId": "s"})).unwrap();
            assert_eq!(target_detached(target, &detached), Some("s"));
            for other in AttachTarget::ALL.iter().copied().filter(|other| *other != target) {
                assert_eq!(target_output(other, &output), None);
                assert_eq!(target_detached(other, &detached), None);
            }
        }
    }

    #[test]
    fn sanitize_replaces_controls_and_collapses_whitespace() {
        assert_eq!(
            sanitize_terminal_title("  a\u{0}b\u{1b}c\u{7f}d\u{9f}e\u{a0}\u{a0}f\n "),
            "a b c d e f"
        );
        assert_eq!(sanitize_terminal_title("\u{feff}x\u{85}y"), "x y");
        assert_eq!(sanitize_terminal_title("\t\r\n"), "");
    }

    #[test]
    fn normalize_output_converts_c1_controls() {
        assert_eq!(
            normalize_terminal_output("a\u{9b}40;3Hb\u{9d}0;t\u{9c}\u{8e}\u{8f}\u{90}\u{9e}\u{9f}\u{91}"),
            "a\x1b[40;3Hb\x1b]0;t\u{9c}\x1bN\x1bO\x1bP\x1b^\x1b_\u{91}"
        );
    }

    #[test]
    fn title_filter_drops_osc_titles_across_chunks() {
        let mut filter = TerminalTitleOutputFilter::new();
        assert_eq!(filter.filter("a\x1b]0;title\x07b"), "ab");
        assert_eq!(filter.filter("c\x1b]2;x\x1b\\d"), "cd");
        assert_eq!(filter.filter("e\x1b]8;;url\x07f"), "e\x1b]8;;url\x07f");
        // Incomplete prefix, then incomplete body, then the end.
        assert_eq!(filter.filter("g\x1b]"), "g");
        assert_eq!(filter.filter("1;par"), "");
        assert_eq!(filter.filter("tial\x07h"), "h");
        // A trailing ESC waits for the next chunk.
        assert_eq!(filter.filter("i\x1b"), "i");
        assert_eq!(filter.filter("[0m"), "\x1b[0m");
        assert_eq!(filter.filter("\x1b"), "");
        assert_eq!(filter.filter("]0;t\x07j"), "j");
        // The nearer terminator wins.
        assert_eq!(filter.filter("\x1b]0;a\x1b\\b\x07c"), "b\x07c");
    }

    #[test]
    fn title_filter_counts_the_prefix_in_utf16_units() {
        let mut filter = TerminalTitleOutputFilter::new();
        // `\x1b]é` is 3 UTF-16 units (4 bytes): still an incomplete prefix in TS.
        assert_eq!(filter.filter("\x1b]é"), "");
        assert_eq!(filter.filter("x"), "\x1b]éx");
        // A surrogate pair counts as two units.
        assert_eq!(filter.filter("\x1b]😀"), "\x1b]😀");
    }

    #[test]
    fn scroll_sensitivity_defaults_and_clamps() {
        assert_eq!(normalize_scroll_sensitivity(None), 0.12);
        assert_eq!(normalize_scroll_sensitivity(Some(f64::NAN)), 0.12);
        assert_eq!(normalize_scroll_sensitivity(Some(f64::INFINITY)), 0.12);
        assert_eq!(normalize_scroll_sensitivity(Some(2.0)), 1.0);
        assert_eq!(normalize_scroll_sensitivity(Some(-1.0)), 0.0);
        assert_eq!(normalize_scroll_sensitivity(Some(0.5)), 0.5);
    }

    #[test]
    fn wheel_events_are_throttled() {
        let mut normalizer = AttachInputNormalizer::new(0.5);
        assert_eq!(normalizer.normalize("a\x1b[<64;10;5Mb"), "ab");
        assert_eq!(normalizer.normalize("\x1b[<65;10;5m"), "\x1b[<65;10;5m");
        // Horizontal wheel and ordinary mouse events pass through untouched.
        assert_eq!(
            normalizer.normalize("\x1b[<66;1;1M\x1b[<0;1;1M"),
            "\x1b[<66;1;1M\x1b[<0;1;1M"
        );
        // The optional extra pair is part of the match.
        assert_eq!(
            normalizer.normalize("\x1b[<64;1;2;3;4M\x1b[<64;1;2;3;4M"),
            "\x1b[<64;1;2;3;4M"
        );
        // Not a complete event: left alone and the accumulator untouched.
        assert_eq!(
            normalizer.normalize("\x1b[<64;1;2;3M\x1b[<64;1M\x1b[<68;1;1M"),
            "\x1b[<64;1;2;3M\x1b[<64;1M\x1b[<68;1;1M"
        );
        assert_eq!(normalizer.normalize("\x1b\x1b[<64;1;1M"), "\x1b");
        assert_eq!(normalizer.normalize("\x1b[<64;1;1M"), "\x1b[<64;1;1M");
    }

    #[test]
    fn default_sensitivity_forwards_one_in_nine() {
        let mut normalizer = AttachInputNormalizer::new(normalize_scroll_sensitivity(None));
        let forwarded: Vec<usize> = (0..20)
            .filter(|_| !normalizer.normalize("\x1b[<65;3;4M").is_empty())
            .collect();
        // The same IEEE-754 accumulation as JS: past 1 on the 9th event, then every 8 or 9 events.
        let mut acc = 0.0_f64;
        let mut expected = Vec::new();
        for index in 0..20 {
            acc += 0.12;
            if acc >= 1.0 {
                acc -= 1.0;
                expected.push(index);
            }
        }
        assert_eq!(forwarded.first(), Some(&8));
        assert_eq!(forwarded, expected);
    }

    #[test]
    fn forwarded_wheel_events_stay_mouse_events() {
        let mut normalizer = AttachInputNormalizer::new(1.0);
        assert_eq!(
            normalizer.normalize("\x1b[<64;1;1M\x1b[<65;1;1m"),
            "\x1b[<64;1;1M\x1b[<65;1;1m"
        );
        assert_eq!(normalizer.normalize("\x1b[<67;1;1M"), "\x1b[<67;1;1M");
    }

    #[test]
    fn titles_are_compacted() {
        assert_eq!(attach_target_title_label(AttachTarget::Terminal), "term");
        assert_eq!(attach_target_title_label(AttachTarget::Agent), "agent");
        assert_eq!(compact_terminal_title("short", None), "short");
        let long = "a".repeat(40);
        assert_eq!(compact_terminal_title(&long, None), format!("{}…", "a".repeat(31)));
        assert_eq!(compact_terminal_title(&long, Some(1)), "a");
        assert_eq!(compact_terminal_title(&long, Some(0)), "");
        assert_eq!(compact_terminal_title("abc", Some(3)), "abc");
        assert_eq!(
            attached_terminal_title("s1", AttachTarget::Git, &AttachSessionOptions::default()),
            "dh/git s1"
        );
        let options = AttachSessionOptions {
            title: Some(String::from("My\ntitle")),
            ..AttachSessionOptions::default()
        };
        assert_eq!(
            attached_terminal_title("s1", AttachTarget::Agent, &options),
            "dh/agent My title"
        );
        let empty = AttachSessionOptions {
            title: Some(String::new()),
            ..AttachSessionOptions::default()
        };
        assert_eq!(attached_terminal_title("s1", AttachTarget::Dev, &empty), "dh/dev ");
    }

    #[test]
    fn detach_input_variants() {
        assert!(is_detach_input(b"\x00"));
        assert!(is_detach_input(b"abc\x1d"));
        assert!(is_detach_input(b"\x1b[32;5u"));
        assert!(is_detach_input(b"x\xff\x1b[27;5;32~"));
        assert!(!is_detach_input(b"\x1b[32;5"));
        assert!(!is_detach_input(b"hello"));
        assert!(!is_detach_input(b""));
    }

    #[test]
    fn terminal_modes_from_response_data() {
        let modes = |value: Value| attach_response_terminal_modes(Some(&value));
        assert_eq!(
            modes(json!({"terminalModes": {"bracketedPaste": true}})),
            Some(TerminalModes { bracketed_paste: true })
        );
        assert_eq!(
            modes(json!({"terminalModes": {"bracketedPaste": 0}})),
            Some(TerminalModes { bracketed_paste: false })
        );
        assert_eq!(
            modes(json!({"terminalModes": {}})),
            Some(TerminalModes { bracketed_paste: false })
        );
        assert_eq!(
            modes(json!({"terminalModes": "x"})),
            Some(TerminalModes { bracketed_paste: false })
        );
        assert_eq!(modes(json!({"terminalModes": null})), None);
        assert_eq!(modes(json!({})), None);
        assert_eq!(modes(json!([1])), None);
        assert_eq!(modes(json!("s")), None);
        assert_eq!(attach_response_terminal_modes(None), None);
    }

    #[test]
    fn attach_screen_choice() {
        assert!(use_attach_screen(AttachTarget::Agent));
        assert!(use_attach_screen(AttachTarget::Git));
        assert!(!use_attach_screen(AttachTarget::Terminal));
        assert!(!use_attach_screen(AttachTarget::Dev));
        assert!(!use_attach_screen(AttachTarget::Action));
    }

    #[test]
    fn process_title_round_trips() {
        set_process_title("dh/agent\u{1b} x ");
        assert_eq!(current_process_title(), "dh/agent x");
        set_process_title("deckhand");
        assert_eq!(current_process_title(), "deckhand");
    }

    // ---- the proxy ---------------------------------------------------------------------------------------------

    #[derive(Default)]
    struct FakeState {
        out: String,
        raw: Vec<bool>,
        titles: Vec<String>,
        events: Option<mpsc::UnboundedSender<ConsoleEvent>>,
        stopped: bool,
    }

    struct FakeConsole {
        tty: bool,
        size: Option<(i64, i64)>,
        state: Arc<Mutex<FakeState>>,
    }

    impl Console for FakeConsole {
        fn stdout_is_tty(&self) -> bool {
            self.tty
        }
        fn stdin_is_tty(&self) -> bool {
            self.tty
        }
        fn write(&mut self, data: &str) {
            self.state.lock().unwrap().out.push_str(data);
        }
        fn columns(&self) -> Option<i64> {
            self.size.map(|(cols, _)| cols)
        }
        fn rows(&self) -> Option<i64> {
            self.size.map(|(_, rows)| rows)
        }
        fn set_raw_mode(&mut self, enabled: bool) {
            self.state.lock().unwrap().raw.push(enabled);
        }
        fn process_title(&self) -> String {
            String::from("deckhand")
        }
        fn set_process_title(&mut self, title: &str) {
            self.state.lock().unwrap().titles.push(title.to_owned());
        }
        fn start_input(&mut self, events: mpsc::UnboundedSender<ConsoleEvent>) {
            self.state.lock().unwrap().events = Some(events);
        }
        fn stop_input(&mut self) {
            let mut state = self.state.lock().unwrap();
            state.events = None;
            state.stopped = true;
        }
    }

    /// The daemon side of the socket pair.
    struct FakeDaemon {
        lines: tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        writer: tokio::net::unix::OwnedWriteHalf,
    }

    impl FakeDaemon {
        async fn recv(&mut self) -> Value {
            let line = self.lines.next_line().await.unwrap().expect("a request line");
            serde_json::from_str(&line).unwrap()
        }

        async fn send(&mut self, value: Value) {
            let mut line = serde_json::to_string(&value).unwrap();
            line.push('\n');
            self.writer.write_all(line.as_bytes()).await.unwrap();
        }
    }

    fn setup(tty: bool, size: Option<(i64, i64)>) -> (FakeConsole, Arc<Mutex<FakeState>>, UnixStream, FakeDaemon) {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let console = FakeConsole {
            tty,
            size,
            state: state.clone(),
        };
        let (client, daemon) = UnixStream::pair().unwrap();
        let (read, writer) = daemon.into_split();
        let daemon = FakeDaemon {
            lines: BufReader::new(read).lines(),
            writer,
        };
        (console, state, client, daemon)
    }

    fn send_input(state: &Arc<Mutex<FakeState>>, event: ConsoleEvent) {
        state
            .lock()
            .unwrap()
            .events
            .as_ref()
            .expect("listening")
            .send(event)
            .unwrap();
    }

    fn session_json(id: &str, status: &str) -> Value {
        json!({
            "id": id, "title": "Fix bug", "program": "claude", "command": "claude", "cwd": "/r", "repoRoot": "/r",
            "status": status, "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z"
        })
    }

    #[tokio::test(flavor = "current_thread")]
    async fn agent_attach_proxies_until_detach_key() {
        let (mut console, state, client, mut daemon) = setup(true, Some((120, 40)));
        let options = AttachSessionOptions {
            title: Some(String::from("Fix bug")),
            scroll_sensitivity: Some(1.0),
        };
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Agent, &options, "req".into());
        let driver = async {
            // The status line and its rule take the top two rows: the agent gets two less.
            assert_eq!(
                daemon.recv().await,
                json!({"type": "attach", "requestId": "req", "sessionId": "s1", "cols": 120, "rows": 38})
            );
            // Unrelated responses and other sessions' output are ignored.
            daemon
                .send(json!({"type": "response", "requestId": "other", "ok": false, "error": "x"}))
                .await;
            daemon
                .send(json!({"type": "output", "sessionId": "s2", "data": "nope"}))
                .await;
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": true, "data": {"content": ""}}))
                .await;
            // The status line lists the sessions to learn the repository, then subscribes to it.
            let list = daemon.recv().await;
            assert_eq!(list["type"], "list");
            assert_eq!(
                daemon.recv().await,
                json!({"type": "resize", "sessionId": "s1", "cols": 120, "rows": 38})
            );
            {
                let state = state.lock().unwrap();
                // Alternate screen, mouse reporting, bracketed paste (agent), the title, then the line's scroll region
                // (every row below the line and the rule) and the line.
                let attached = format!(
                    "{RESET_TERMINAL_STATE_SEQUENCE}{RESET_TERMINAL_STATE_SEQUENCE}\x1b[?1049h\x1b[2J\x1b[H\
                     \x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b]0;dh/agent Fix bug\x07\x1b]2;dh/agent Fix bug\x07\x1b[3;40r"
                );
                assert!(state.out.starts_with(&attached), "{:?}", state.out);
                assert!(state.out.contains("Fix bug") && state.out.contains("Ctrl+] detach"));
                assert_eq!(state.raw, vec![true]);
                assert_eq!(state.titles, vec![String::from("dh/agent Fix bug")]);
            }
            state.lock().unwrap().out.clear();
            let mut waiting = session_json("s2", "running");
            waiting["attention"] = json!({"state": "needs-input", "event": "Notification", "at": "x"});
            let sessions = json!([session_json("s1", "running"), waiting]);
            daemon
                .send(json!({"type": "response", "requestId": list["requestId"], "ok": true, "data": sessions}))
                .await;
            let subscribe = daemon.recv().await;
            assert_eq!(subscribe["type"], "subscribe");
            assert_eq!(subscribe["repoRoot"], "/r");
            daemon
                .send(json!({"type": "response", "requestId": subscribe["requestId"], "ok": true, "data": sessions}))
                .await;
            daemon
                .send(json!({"type": "output", "sessionId": "s1", "data": "a\u{9b}1mb\u{1b}]0;child\u{7}c"}))
                .await;
            // The wheel reaches the agent two rows up (and not at all on the line or the rule).
            send_input(
                &state,
                ConsoleEvent::Data(b"hi\x1b[<64;1;1M\x1b[<64;1;2M\x1b[<64;1;5M".to_vec()),
            );
            assert_eq!(
                daemon.recv().await,
                json!({"type": "input", "sessionId": "s1", "data": "hi\x1b[<64;1;3M"})
            );
            send_input(&state, ConsoleEvent::Resize);
            assert_eq!(daemon.recv().await["type"], "resize");
            send_input(&state, ConsoleEvent::Data(vec![0]));
            assert_eq!(daemon.recv().await, json!({"type": "detach", "sessionId": "s1"}));
            // socket.end()
            assert!(daemon.lines.next_line().await.unwrap().is_none());
        };
        let (result, ()) = tokio::join!(attach, driver);
        result.unwrap();
        let state = state.lock().unwrap();
        assert!(state.out.contains("a\x1b[1mbc"), "{:?}", state.out);
        assert!(state.out.contains("1 other session waiting"), "{:?}", state.out);
        // The line and the rule are cleared before the terminal is handed back.
        assert!(
            state.out.ends_with(&format!(
                "\x1b[r\x1b[0m\x1b[1;1H\x1b[2K\x1b[2;1H\x1b[2K\x1b[1;1H{RESET_TERMINAL_STATE_SEQUENCE}\x1b[?1049l\x1b]0;deckhand\x07\x1b]2;deckhand\x07"
            )),
            "{:?}",
            state.out
        );
        assert_eq!(state.raw, vec![true, false]);
        assert_eq!(state.titles.last().map(String::as_str), Some("deckhand"));
        assert!(state.stopped);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn claude_attach_uses_the_alternate_screen_and_ends_on_exit() {
        let (mut console, state, client, mut daemon) = setup(true, None);
        let options = AttachSessionOptions::default();
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Agent, &options, "req".into());
        let driver = async {
            assert_eq!(
                daemon.recv().await,
                json!({"type": "attach", "requestId": "req", "sessionId": "s1", "cols": 80, "rows": 22})
            );
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": true}))
                .await;
            assert_eq!(daemon.recv().await["type"], "list");
            daemon.recv().await;
            assert!(state.lock().unwrap().out.starts_with(&format!(
                "{RESET_TERMINAL_STATE_SEQUENCE}{RESET_TERMINAL_STATE_SEQUENCE}\x1b[?1049h\x1b[2J\x1b[H\
                 \x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b]0;dh/agent s1\x07\x1b]2;dh/agent s1\x07"
            )));
            state.lock().unwrap().out.clear();
            // Another session exiting does nothing; this one ends the attach.
            daemon
                .send(json!({"type": "session-updated", "session": session_json("s2", "exited")}))
                .await;
            daemon
                .send(json!({"type": "session-updated", "session": session_json("s1", "running")}))
                .await;
            daemon
                .send(json!({"type": "session-updated", "session": session_json("s1", "exited")}))
                .await;
        };
        let (result, ()) = tokio::join!(attach, driver);
        result.unwrap();
        // The session updates redraw the status line; the exit notice, then the line and the rule cleared.
        assert!(state.lock().unwrap().out.ends_with(&format!(
            "\n\r[session exited: Fix bug]\n\x1b[r\x1b[0m\x1b[1;1H\x1b[2K\x1b[2;1H\x1b[2K\x1b[1;1H{RESET_TERMINAL_STATE_SEQUENCE}\x1b[?1049l\
             \x1b]0;deckhand\x07\x1b]2;deckhand\x07"
        )));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn terminal_attach_ends_on_its_detached_event_and_ignores_exit() {
        let (mut console, state, client, mut daemon) = setup(false, None);
        let options = AttachSessionOptions::default();
        let attach = run_attach(
            &mut console,
            client,
            "s1",
            AttachTarget::Terminal,
            &options,
            "req".into(),
        );
        let driver = async {
            assert_eq!(daemon.recv().await["type"], "attach-terminal");
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": true,
                    "data": {"terminalModes": {"bracketedPaste": true}}}))
                .await;
            assert_eq!(
                daemon.recv().await,
                json!({"type": "terminal-resize", "sessionId": "s1", "cols": 80, "rows": 24})
            );
            daemon
                .send(json!({"type": "session-updated", "session": session_json("s1", "exited")}))
                .await;
            daemon.send(json!({"type": "detached", "sessionId": "s1"})).await;
            daemon
                .send(json!({"type": "terminal-output", "sessionId": "s1", "data": "x"}))
                .await;
            daemon
                .send(json!({"type": "terminal-detached", "sessionId": "s1"}))
                .await;
        };
        let (result, ()) = tokio::join!(attach, driver);
        result.unwrap();
        let state = state.lock().unwrap();
        // Not a TTY: no escape sequences, no raw mode, but the process title still changes.
        assert_eq!(state.out, "x");
        assert!(state.raw.is_empty());
        assert_eq!(state.titles, vec![String::from("dh/term s1"), String::from("deckhand")]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn attach_failures() {
        let (mut console, state, client, mut daemon) = setup(true, None);
        let options = AttachSessionOptions::default();
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Git, &options, "req".into());
        let driver = async {
            daemon.recv().await;
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": false, "error": ""}))
                .await;
        };
        let (result, ()) = tokio::join!(attach, driver);
        assert_eq!(result.unwrap_err().to_string(), "attach failed");
        // Never attached: modes reset, title untouched.
        assert_eq!(state.lock().unwrap().out, RESET_TERMINAL_STATE_SEQUENCE);
        assert!(state.lock().unwrap().titles.is_empty());

        let (mut console, _state, client, mut daemon) = setup(false, None);
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Git, &options, "req".into());
        let driver = async {
            daemon.recv().await;
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": false, "error": "unknown session: s1"}))
                .await;
        };
        let (result, ()) = tokio::join!(attach, driver);
        assert_eq!(result.unwrap_err().to_string(), "unknown session: s1");

        let (mut console, state, client, mut daemon) = setup(true, None);
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Git, &options, "req".into());
        let driver = async {
            daemon.recv().await;
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": true}))
                .await;
            daemon.recv().await;
            drop(daemon);
        };
        let (result, ()) = tokio::join!(attach, driver);
        assert_eq!(
            result.unwrap_err().to_string(),
            "daemon connection closed during attach"
        );
        // Git uses the alternate screen and leaves it again.
        assert!(
            state
                .lock()
                .unwrap()
                .out
                .ends_with("\x1b[?1049l\x1b]0;deckhand\x07\x1b]2;deckhand\x07")
        );
    }

    /// TS writeMessage buffers in memory (socket.write never blocks), so Ctrl+Space detaches even when the daemon
    /// has stopped reading and the socket's send buffer is full.
    #[tokio::test(flavor = "current_thread")]
    async fn detach_works_when_the_daemon_stops_reading() {
        let (mut console, state, client, mut daemon) = setup(true, Some((80, 24)));
        let options = AttachSessionOptions::default();
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Agent, &options, "req".into());
        let attach = async { tokio::time::timeout(std::time::Duration::from_secs(3), attach).await };
        let driver = async {
            daemon.recv().await;
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": true}))
                .await;
            assert_eq!(daemon.recv().await["type"], "list");
            assert_eq!(daemon.recv().await["type"], "resize");
            // The daemon reads nothing more. A large paste arrives as 64 KiB stdin reads, then Ctrl+Space.
            for _ in 0..4 {
                send_input(&state, ConsoleEvent::Data(vec![b'p'; 65536]));
            }
            send_input(&state, ConsoleEvent::Data(vec![0]));
        };
        let (result, ()) = tokio::join!(attach, driver);
        result
            .expect("Ctrl+Space detaches without waiting on the socket")
            .unwrap();
        let state = state.lock().unwrap();
        assert_eq!(state.raw, vec![true, false]);
        assert!(state.stopped);
        drop(daemon);
    }

    /// Queued writes still reach the daemon in order once it reads them, and the detach goes last.
    #[tokio::test(flavor = "current_thread")]
    async fn queued_input_is_delivered_in_order() {
        let (mut console, state, client, mut daemon) = setup(false, None);
        let options = AttachSessionOptions::default();
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Agent, &options, "req".into());
        let driver = async {
            daemon.recv().await;
            daemon
                .send(json!({"type": "response", "requestId": "req", "ok": true}))
                .await;
            assert_eq!(daemon.recv().await["type"], "resize");
            for index in 0..3u8 {
                send_input(&state, ConsoleEvent::Data(vec![b'a' + index; 40000]));
            }
            for index in 0..3u8 {
                let message = daemon.recv().await;
                assert_eq!(message["type"], "input");
                assert_eq!(message["data"], String::from(char::from(b'a' + index)).repeat(40000));
            }
            send_input(&state, ConsoleEvent::Data(vec![0x1d]));
            assert_eq!(daemon.recv().await, json!({"type": "detach", "sessionId": "s1"}));
            assert!(daemon.lines.next_line().await.unwrap().is_none());
        };
        let (result, ()) = tokio::join!(attach, driver);
        result.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn close_before_the_response_never_settles() {
        let (mut console, _state, client, mut daemon) = setup(false, None);
        let options = AttachSessionOptions::default();
        let attach = run_attach(&mut console, client, "s1", AttachTarget::Agent, &options, "req".into());
        let driver = async {
            daemon.recv().await;
            drop(daemon);
        };
        let attach = async { tokio::time::timeout(std::time::Duration::from_millis(100), attach).await };
        let (result, ()) = tokio::join!(attach, driver);
        assert!(result.is_err(), "attachSession stays pending, as in TS");
    }
}
