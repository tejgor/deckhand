//! Port of the Ink runtime the app runs in (ink render + components/App.js stdin handling + React state updates).
//!
//! One loop on the process's `current_thread` runtime merges:
//! - stdin bytes, read in raw mode by a poll(2) thread ([`StdinReader`]) and parsed with [`crate::input`] (never
//!   crossterm's event parser): each read goes to [`UiModel::on_stdin_text`] (a raw `stdin.on('data')` listener, as
//!   the preview's mouse-wheel handler is) and then key by key to [`UiModel::on_key`]. A trailing lone `ESC` is
//!   flushed as Escape once no more input is immediately queued (Ink's `setImmediate`);
//! - terminal resizes (SIGWINCH, [`spawn_resize_watcher`]): the screen is cleared and fully repainted, as app.tsx's
//!   resize handler does;
//! - model updates ([`Dispatch`]): what React's `setState` from async code is: daemon events from the
//!   `LiveClient` handlers, replies of requests, timers and intervals all send a closure that runs on the model.
//!
//! After each batch the model's effects run ([`UiModel::after_update`], React's `useEffect` phase) and the frame is
//! drawn with ratatui (which writes only the cells that changed), at most once per [`RENDER_THROTTLE`] as Ink
//! throttles its renders, each frame wrapped in synchronized output ([`InkBackend`]). [`UiModel::exit`] ends the loop
//! with the result Ink's `waitUntilExit` resolved to.

use std::cell::RefCell;
use std::future::Future;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::Duration;

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::termios::{self, ControlFlags, InputFlags, LocalFlags, OutputFlags, SetArg, Termios};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use tokio::sync::mpsc;
use tokio::time::Instant;

use deckhand_sys::async_util::{Interval, Task, Timer, spawn};

use crate::input::{KeyEvent, StdinParser};

/// A state change to run on the model (TS `setState(updater)` called from async code).
pub type Update<M> = Box<dyn FnOnce(&mut M)>;

/// Sends updates to the model from timers, replies and daemon events. Updates sent after the loop ended are
/// dropped, as React ignores `setState` on an unmounted component.
pub struct Dispatch<M> {
    tx: mpsc::UnboundedSender<Update<M>>,
}

impl<M> Clone for Dispatch<M> {
    fn clone(&self) -> Self {
        Dispatch { tx: self.tx.clone() }
    }
}

impl<M> std::fmt::Debug for Dispatch<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatch").finish_non_exhaustive()
    }
}

impl<M: 'static> Dispatch<M> {
    /// A dispatcher and the receiving end the loop reads.
    pub fn channel() -> (Dispatch<M>, mpsc::UnboundedReceiver<Update<M>>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Dispatch { tx }, rx)
    }

    /// Runs `update` on the model in the loop.
    pub fn update(&self, update: impl FnOnce(&mut M) + 'static) {
        let _ = self.tx.send(Box::new(update));
    }

    /// `void promise.then(value => then(model, value))`: runs `future` on the local set and `then` with its output
    /// on the model. Aborting (or dropping the loop) leaves `then` unrun, like a `cancelled` flag in TS.
    pub fn spawn<T: 'static>(
        &self,
        future: impl Future<Output = T> + 'static,
        then: impl FnOnce(&mut M, T) + 'static,
    ) -> Task<()> {
        let dispatch = self.clone();
        spawn(async move {
            let value = future.await;
            dispatch.update(move |model| then(model, value));
        })
    }

    /// TS `setTimeout(() => update(model), delay)` (cancelled when the returned timer is dropped or cleared).
    pub fn after(&self, delay: Duration, update: impl FnOnce(&mut M) + 'static) -> Timer {
        let dispatch = self.clone();
        Timer::after(delay, move || dispatch.update(update))
    }

    /// TS `setInterval(() => tick(model), period)` (stopped when the returned interval is dropped or cleared).
    pub fn every(&self, period: Duration, tick: impl FnMut(&mut M) + 'static) -> Interval {
        let dispatch = self.clone();
        let tick = Rc::new(RefCell::new(tick));
        Interval::every(period, move || {
            let tick = Rc::clone(&tick);
            dispatch.update(move |model| (tick.borrow_mut())(model));
        })
    }
}

/// What the loop drives: the app (or a test double).
pub trait UiModel: Sized + 'static {
    /// What the UI ends with (TS `exit(result)` → `waitUntilExit()`).
    type Exit;

    /// TS `process.stdin.on('data')`: the decoded text of one read, before its keys are handled.
    fn on_stdin_text(&mut self, text: &str) {
        let _ = text;
    }

    /// One Ink input event (TS useInput / useTerminalInput handlers).
    fn on_key(&mut self, event: &KeyEvent);

    /// The terminal was resized to `cols` × `rows` (TS `process.stdout.on('resize')`).
    fn on_resize(&mut self, cols: u16, rows: u16);

    /// React's effect phase: runs after every batch of events and updates, before the frame is drawn.
    fn after_update(&mut self) {}

    /// Draws the whole UI (TS the App component's render).
    fn render(&mut self, frame: &mut Frame<'_>);

    /// The exit result once the UI asked to exit (taken by the loop).
    fn exit(&mut self) -> Option<Self::Exit>;
}

/// An input to the loop from the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    /// Bytes read from stdin.
    Data(Vec<u8>),
    /// The terminal's new size.
    Resize { cols: u16, rows: u16 },
    /// stdin reached end of file or failed.
    Closed,
}

/// How the loop ended.
#[derive(Debug, PartialEq, Eq)]
pub enum LoopEnd<E> {
    /// The model asked to exit.
    Exit(E),
    /// stdin closed (or every input source went away) before that.
    InputClosed,
}

/// Feeds one batch of stdin bytes to the model; returns whether the model asked to exit.
fn feed<M: UiModel>(model: &mut M, parser: &mut StdinParser, bytes: &[u8], exit: &mut Option<M::Exit>) {
    let chunk = parser.push(bytes);
    if !chunk.text.is_empty() {
        model.on_stdin_text(&chunk.text);
    }
    for event in &chunk.events {
        if exit.is_some() {
            return;
        }
        model.on_key(event);
        *exit = model.exit();
    }
}

/// Handles one terminal event (and, for data, everything queued right behind it); returns false once stdin closed.
async fn handle_term_event<M: UiModel, B: Backend>(
    model: &mut M,
    terminal: &mut Terminal<B>,
    parser: &mut StdinParser,
    events: &mut mpsc::UnboundedReceiver<TermEvent>,
    first: TermEvent,
    exit: &mut Option<M::Exit>,
    resized: &mut bool,
) -> io::Result<bool> {
    let mut next = Some(first);
    while let Some(event) = next.take() {
        match event {
            TermEvent::Data(bytes) => {
                feed(model, parser, &bytes, exit);
                if exit.is_some() {
                    return Ok(true);
                }
                next = events.try_recv().ok();
                if next.is_none() && parser.has_pending_escape() {
                    // Ink's setImmediate: a held ESC is a key of its own unless more input is already queued.
                    tokio::task::yield_now().await;
                    next = events.try_recv().ok();
                    if !matches!(next, Some(TermEvent::Data(_)))
                        && let Some(escape) = parser.flush_pending_escape()
                    {
                        model.on_key(&escape);
                        *exit = model.exit();
                        if exit.is_some() {
                            return Ok(true);
                        }
                    }
                }
            }
            TermEvent::Resize { cols, rows } => {
                // TS: app.tsx onResize clears the screen before the next render.
                terminal.clear()?;
                *resized = true;
                model.on_resize(cols, rows);
                *exit = model.exit();
                next = events.try_recv().ok();
            }
            TermEvent::Closed => {
                if let Some(escape) = parser.flush_pending_escape() {
                    model.on_key(&escape);
                    *exit = model.exit();
                }
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// TS: Ink's render throttle: `maxFps` 30 → `throttle(onRender, Math.ceil(1000 / 30))`, leading and trailing.
pub const RENDER_THROTTLE: Duration = Duration::from_millis(34);

/// Runs the UI until the model exits or stdin closes. The first frame is drawn before any input is read; later frames
/// are throttled to [`RENDER_THROTTLE`].
pub async fn run<M: UiModel, B: Backend>(
    model: &mut M,
    terminal: &mut Terminal<B>,
    events: &mut mpsc::UnboundedReceiver<TermEvent>,
    updates: &mut mpsc::UnboundedReceiver<Update<M>>,
) -> io::Result<LoopEnd<M::Exit>> {
    run_throttled(model, terminal, events, updates, RENDER_THROTTLE).await
}

/// [`run`] with frames at most once per `throttle`, as Ink's throttled `onRender` (lodash-style leading and trailing
/// edges): a frame is drawn at once when none was drawn for `throttle`, else once the window ends, with everything
/// that changed meanwhile. The model's effects still run after every batch (React commits every update; only Ink's
/// output is throttled). A resize draws at once, as Ink's `resized` calls `onRender` directly.
pub async fn run_throttled<M: UiModel, B: Backend>(
    model: &mut M,
    terminal: &mut Terminal<B>,
    events: &mut mpsc::UnboundedReceiver<TermEvent>,
    updates: &mut mpsc::UnboundedReceiver<Update<M>>,
    throttle: Duration,
) -> io::Result<LoopEnd<M::Exit>> {
    let mut parser = StdinParser::new();
    let mut exit = None;
    let mut input_open = true;
    model.after_update();
    if let Some(result) = model.exit() {
        return Ok(LoopEnd::Exit(result));
    }
    terminal.draw(|frame| model.render(frame))?;
    let mut last_draw = Instant::now();
    let mut draw_pending = false;
    loop {
        let mut resized = false;
        tokio::select! {
            biased;
            event = events.recv(), if input_open => match event {
                Some(event) => {
                    input_open =
                        handle_term_event(model, terminal, &mut parser, events, event, &mut exit, &mut resized).await?;
                }
                None => input_open = false,
            },
            update = updates.recv() => match update {
                Some(update) => {
                    update(model);
                    exit = exit.or_else(|| model.exit());
                    while exit.is_none() && let Ok(update) = updates.try_recv() {
                        update(model);
                        exit = model.exit();
                    }
                }
                None if !input_open => return Ok(LoopEnd::InputClosed),
                None => {}
            },
            () = tokio::time::sleep_until(last_draw + throttle), if draw_pending => {
                // The trailing edge: the effects of what it shows have run already.
                terminal.draw(|frame| model.render(frame))?;
                last_draw = Instant::now();
                draw_pending = false;
                continue;
            }
        }
        if let Some(result) = exit.take() {
            return Ok(LoopEnd::Exit(result));
        }
        if !input_open {
            return Ok(LoopEnd::InputClosed);
        }
        model.after_update();
        if let Some(result) = model.exit() {
            return Ok(LoopEnd::Exit(result));
        }
        if resized || last_draw.elapsed() >= throttle {
            terminal.draw(|frame| model.render(frame))?;
            last_draw = Instant::now();
            draw_pending = false;
        } else {
            draw_pending = true;
        }
    }
}

/// TS: Ink's write-synchronized.js `bsu` (begin synchronized update, DEC private mode 2026).
pub const BEGIN_SYNCHRONIZED_UPDATE: &str = "\x1b[?2026h";
/// TS: Ink's write-synchronized.js `esu` (end synchronized update).
pub const END_SYNCHRONIZED_UPDATE: &str = "\x1b[?2026l";

/// TS: is-in-ci (`CI` or `CONTINUOUS_INTEGRATION` set to anything but `0` or `false`).
pub fn is_in_ci() -> bool {
    is_in_ci_with(|key| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned()))
}

fn is_in_ci_with(env: impl Fn(&str) -> Option<String>) -> bool {
    let check = |key: &str| env(key).is_some_and(|value| value != "0" && value != "false");
    check("CI") || check("CONTINUOUS_INTEGRATION")
}

/// TS: Ink's shouldSynchronize(stdout): a TTY stdout outside CI.
pub fn should_synchronize() -> bool {
    use std::io::IsTerminal;
    io::stdout().is_terminal() && !is_in_ci()
}

/// The crossterm backend the UI draws with, writing frames as Ink's throttled log does: a frame that changes no cell
/// writes no cells (Ink skips output equal to the last), and when `synchronize` (see [`should_synchronize`]) a frame
/// that does is wrapped in [`BEGIN_SYNCHRONIZED_UPDATE`] … [`END_SYNCHRONIZED_UPDATE`], so the terminal shows it
/// whole. The frame ends at ratatui's final flush, after the cursor update.
#[derive(Debug)]
pub struct InkBackend<W: io::Write> {
    inner: CrosstermBackend<W>,
    synchronize: bool,
    in_frame: bool,
}

impl<W: io::Write> InkBackend<W> {
    /// A backend writing to `writer`.
    pub fn new(writer: W, synchronize: bool) -> InkBackend<W> {
        InkBackend {
            inner: CrosstermBackend::new(writer),
            synchronize,
            in_frame: false,
        }
    }
}

impl<W: io::Write> Backend for InkBackend<W> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut content = content.peekable();
        if content.peek().is_none() {
            return Ok(());
        }
        if self.synchronize && !self.in_frame {
            io::Write::write_all(&mut self.inner, BEGIN_SYNCHRONIZED_UPDATE.as_bytes())?;
            self.in_frame = true;
        }
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.in_frame {
            self.in_frame = false;
            io::Write::write_all(&mut self.inner, END_SYNCHRONIZED_UPDATE.as_bytes())?;
        }
        self.inner.flush()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The real terminal

/// Reads stdin on a thread of its own, blocking in poll(2) (kqueue does not watch ttys on macOS) and sending every
/// read as [`TermEvent::Data`]. Dropping it wakes and joins the thread, so no read is left pending when the terminal
/// is handed to an attached session or `$EDITOR`.
#[derive(Debug)]
pub struct StdinReader {
    stop: Option<OwnedFd>,
    thread: Option<JoinHandle<()>>,
}

/// Reads `fd` until it closes or `stop` becomes readable.
fn read_loop(fd: RawFd, stop: OwnedFd, tx: mpsc::UnboundedSender<TermEvent>) {
    let mut buffer = vec![0u8; 4096];
    loop {
        // SAFETY: `fd` stays open for the reader's lifetime (the caller owns stdin; dropping the reader joins this
        // thread first).
        let input = unsafe { BorrowedFd::borrow_raw(fd) };
        let mut fds = [
            PollFd::new(input, PollFlags::POLLIN),
            PollFd::new(stop.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut fds, PollTimeout::NONE) {
            Err(Errno::EINTR) => continue,
            Err(_) => {
                let _ = tx.send(TermEvent::Closed);
                return;
            }
            Ok(_) => {}
        }
        let ready = |index: usize| {
            fds[index].revents().is_some_and(|flags| {
                !(flags & (PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR)).is_empty()
            })
        };
        if ready(1) {
            return;
        }
        if !ready(0) {
            continue;
        }
        match nix::unistd::read(fd, &mut buffer) {
            Ok(0) => {
                let _ = tx.send(TermEvent::Closed);
                return;
            }
            Ok(count) => {
                if tx.send(TermEvent::Data(buffer[..count].to_vec())).is_err() {
                    return;
                }
            }
            Err(Errno::EINTR | Errno::EAGAIN) => {}
            Err(_) => {
                let _ = tx.send(TermEvent::Closed);
                return;
            }
        }
    }
}

impl StdinReader {
    /// Starts reading `fd` (stdin) into `tx`.
    pub fn spawn(fd: RawFd, tx: mpsc::UnboundedSender<TermEvent>) -> io::Result<StdinReader> {
        let (stop_read, stop_write) = nix::unistd::pipe().map_err(io::Error::from)?;
        let thread = std::thread::Builder::new()
            .name("deckhand-stdin".to_string())
            .spawn(move || read_loop(fd, stop_read, tx))?;
        Ok(StdinReader {
            stop: Some(stop_write),
            thread: Some(thread),
        })
    }

    /// Stops reading and waits for the thread.
    pub fn stop(&mut self) {
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

/// Sends [`TermEvent::Resize`] with the new size on every SIGWINCH (TS `process.stdout.on('resize')`).
pub fn spawn_resize_watcher(tx: mpsc::UnboundedSender<TermEvent>) -> io::Result<Task<()>> {
    let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
    Ok(spawn(async move {
        while signals.recv().await.is_some() {
            let (cols, rows) = terminal_size();
            if tx.send(TermEvent::Resize { cols, rows }).is_err() {
                return;
            }
        }
    }))
}

/// TS: Ink's `signalExit(this.unmount)`. Registers SIGINT, SIGTERM and SIGHUP; the future resolves with the number
/// of the first one delivered, so the caller can unwind the terminal state before dying of it.
pub fn termination_signal() -> io::Result<impl std::future::Future<Output = i32>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut set = Vec::new();
    for kind in [SignalKind::interrupt(), SignalKind::terminate(), SignalKind::hangup()] {
        set.push((kind.as_raw_value(), signal(kind)?));
    }
    Ok(async move {
        let waits = set.iter_mut().map(|(number, stream)| {
            let number = *number;
            Box::pin(async move {
                stream.recv().await;
                number
            })
        });
        futures::future::select_all(waits).await.0
    })
}

/// Dies of `signal` with its default action (what signal-exit does after the cleanups ran), after Node's ResetStdio
/// put back the startup termios.
pub fn die_of_signal(signal: i32) {
    use nix::sys::signal::{SigHandler, Signal, raise, signal as install};
    let Ok(signal) = Signal::try_from(signal) else { return };
    restore_startup_termios();
    // SAFETY: restoring the default disposition installs no Rust handler.
    let _ = unsafe { install(signal, SigHandler::SigDfl) };
    let _ = raise(signal);
}

/// A terminal's termios as Node saves stdio's at startup (`PlatformInit`) and puts back at exit (`ResetStdio`, also
/// run before dying of a signal). Raw-mode entries restore what they found instead ([`RawMode`]), so only this undoes a
/// tty an `$EDITOR` left raw.
#[derive(Clone, Copy)]
pub struct SavedTermios {
    fd: RawFd,
    termios: libc::termios,
}

impl std::fmt::Debug for SavedTermios {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedTermios")
            .field("fd", &self.fd)
            .finish_non_exhaustive()
    }
}

impl SavedTermios {
    /// `fd`'s current termios; `None` when it is not a terminal.
    pub fn capture(fd: RawFd) -> Option<SavedTermios> {
        // SAFETY: an all-zero termios is a valid value for tcgetattr to overwrite.
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: tcgetattr writes one termios through a pointer to a live one; a bad fd only fails.
        (unsafe { libc::tcgetattr(fd, &mut termios) } == 0).then_some(SavedTermios { fd, termios })
    }

    /// Puts the saved termios back (best effort). As Node's ResetStdio: at once (`TCSANOW`), retried on `EINTR`, with
    /// SIGTTOU blocked so a background process is not stopped for it. Only plain libc calls: the panic hook runs this.
    pub fn restore(&self) {
        // SAFETY: the sigset is initialised by sigemptyset before use; the masks only touch this thread's.
        unsafe {
            let mut block: libc::sigset_t = std::mem::zeroed();
            let mut previous: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut block);
            libc::sigaddset(&mut block, libc::SIGTTOU);
            let masked = libc::pthread_sigmask(libc::SIG_BLOCK, &block, &mut previous) == 0;
            while libc::tcsetattr(self.fd, libc::TCSANOW, &self.termios) == -1
                && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
            {}
            if masked {
                libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
            }
        }
    }
}

/// The process's startup termios (stdin's), captured once.
static STARTUP_TERMIOS: std::sync::OnceLock<Option<SavedTermios>> = std::sync::OnceLock::new();

/// Saves `fd`'s termios as the one to restore at exit, unless one was saved already (the first capture wins, so call
/// it at startup). Returns what is saved.
pub fn capture_startup_termios(fd: RawFd) -> Option<SavedTermios> {
    *STARTUP_TERMIOS.get_or_init(|| SavedTermios::capture(fd))
}

/// Node's ResetStdio: restores the termios [`capture_startup_termios`] saved (nothing when none was).
pub fn restore_startup_termios() {
    if let Some(Some(saved)) = STARTUP_TERMIOS.get() {
        saved.restore();
    }
}

/// TS app.tsx getTerminalSize: `process.stdout.columns || 80`, `rows || 24`.
pub fn terminal_size() -> (u16, u16) {
    match crossterm::terminal::size() {
        Ok((cols, rows)) => (if cols == 0 { 80 } else { cols }, if rows == 0 { 24 } else { rows }),
        Err(_) => (80, 24),
    }
}

/// Node's `setRawMode(true)` (libuv `UV_TTY_MODE_RAW`) on `fd`, restored when dropped. Unlike cfmakeraw it keeps
/// output post-processing (`OPOST`/`ONLCR`).
pub struct RawMode {
    fd: RawFd,
    saved: Termios,
}

impl std::fmt::Debug for RawMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawMode").field("fd", &self.fd).finish_non_exhaustive()
    }
}

impl RawMode {
    /// Puts the terminal on `fd` in raw mode.
    pub fn enable(fd: RawFd) -> io::Result<RawMode> {
        // SAFETY: the caller passes an open terminal descriptor that outlives this guard.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let saved = termios::tcgetattr(borrowed).map_err(io::Error::from)?;
        let mut raw = saved.clone();
        raw.input_flags &=
            !(InputFlags::BRKINT | InputFlags::ICRNL | InputFlags::INPCK | InputFlags::ISTRIP | InputFlags::IXON);
        raw.output_flags |= OutputFlags::ONLCR;
        raw.control_flags |= ControlFlags::CS8;
        raw.local_flags &= !(LocalFlags::ECHO | LocalFlags::ICANON | LocalFlags::IEXTEN | LocalFlags::ISIG);
        raw.control_chars[termios::SpecialCharacterIndices::VMIN as usize] = 1;
        raw.control_chars[termios::SpecialCharacterIndices::VTIME as usize] = 0;
        termios::tcsetattr(borrowed, SetArg::TCSADRAIN, &raw).map_err(io::Error::from)?;
        Ok(RawMode { fd, saved })
    }

    /// The descriptor in raw mode.
    pub fn fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: as in `enable`.
        let borrowed = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let _ = termios::tcsetattr(borrowed, SetArg::TCSADRAIN, &self.saved);
    }
}

/// TS: Ink 6 components/App.js handleSetRawMode's error when `process.stdin` is not a TTY (`isRawModeSupported` is
/// `stdin.isTTY`).
pub const RAW_MODE_UNSUPPORTED: &str = "Raw mode is not supported on the current process.stdin, which Ink uses as input \
stream by default.\nRead about how to prevent this error on https://github.com/vadimdemedes/ink/#israwmodesupported";

/// TS: Ink's `setRawMode(true)` from useInput: [`RAW_MODE_UNSUPPORTED`] unless `fd` is a TTY, else [`RawMode::enable`].
/// The error reaches cli.ts `main().catch`, which prints its message and exits 1.
pub fn enable_ink_raw_mode(fd: RawFd) -> io::Result<RawMode> {
    // SAFETY: isatty only inspects the descriptor (a closed one is simply not a TTY).
    if unsafe { libc::isatty(fd) } != 1 {
        return Err(io::Error::other(RAW_MODE_UNSUPPORTED));
    }
    RawMode::enable(fd)
}

/// stdin's descriptor.
pub fn stdin_fd() -> RawFd {
    io::stdin().as_raw_fd()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[tokio::test]
    async fn termination_signals_are_caught_instead_of_killing_the_process() {
        for signal in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
            let wait = termination_signal().unwrap_or_else(|error| panic!("{error}"));
            // SAFETY: raises a signal the registered handler now owns.
            assert_eq!(unsafe { libc::raise(signal) }, 0);
            let got = tokio::time::timeout(std::time::Duration::from_secs(5), wait).await;
            assert_eq!(got.ok(), Some(signal));
        }
    }
    use ratatui::text::Line;

    /// Records what the loop delivered and renders a counter.
    #[derive(Default)]
    struct Recorder {
        keys: Vec<String>,
        texts: Vec<String>,
        sizes: Vec<(u16, u16)>,
        effects: usize,
        renders: usize,
        quit: Option<&'static str>,
        counter: i32,
    }

    impl UiModel for Recorder {
        type Exit = &'static str;

        fn on_stdin_text(&mut self, text: &str) {
            self.texts.push(text.to_string());
        }

        fn on_key(&mut self, event: &KeyEvent) {
            let input = &event.input;
            let name = if input.key.escape {
                "<esc>".to_string()
            } else if input.key.up_arrow {
                "<up>".to_string()
            } else {
                input.input.clone()
            };
            if name == "q" {
                self.quit = Some("quit");
            }
            self.keys.push(name);
        }

        fn on_resize(&mut self, cols: u16, rows: u16) {
            self.sizes.push((cols, rows));
        }

        fn after_update(&mut self) {
            self.effects += 1;
        }

        fn render(&mut self, frame: &mut Frame<'_>) {
            self.renders += 1;
            frame.render_widget(Line::from(format!("count {}", self.counter)), frame.area());
        }

        fn exit(&mut self) -> Option<&'static str> {
            self.quit.take()
        }
    }

    fn rows(terminal: &Terminal<TestBackend>) -> Vec<String> {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    async fn drive(
        model: &mut Recorder,
        terminal: &mut Terminal<TestBackend>,
        script: Vec<TermEvent>,
        updates: Vec<Update<Recorder>>,
        close: bool,
    ) -> LoopEnd<&'static str> {
        let (tx, mut events) = mpsc::unbounded_channel();
        let (dispatch, mut update_rx) = Dispatch::<Recorder>::channel();
        for update in updates {
            dispatch.tx.send(update).expect("update queued");
        }
        for event in script {
            tx.send(event).expect("event queued");
        }
        if close {
            drop(tx);
        } else {
            std::mem::forget(tx);
        }
        let _keep = dispatch;
        run(model, terminal, &mut events, &mut update_rx)
            .await
            .expect("loop runs")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delivers_text_then_keys_and_exits_on_request() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut terminal = Terminal::new(TestBackend::new(12, 2)).expect("terminal");
                let mut model = Recorder::default();
                let end = drive(
                    &mut model,
                    &mut terminal,
                    vec![
                        TermEvent::Data(b"ab\x1b[A".to_vec()),
                        TermEvent::Data(b"q".to_vec()),
                        TermEvent::Data(b"z".to_vec()),
                    ],
                    Vec::new(),
                    false,
                )
                .await;
                assert_eq!(end, LoopEnd::Exit("quit"));
                assert_eq!(model.keys, ["ab", "<up>", "q"]);
                assert_eq!(model.texts, ["ab\u{1b}[A", "q"]);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_lone_escape_waits_for_queued_input_then_flushes() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut terminal = Terminal::new(TestBackend::new(12, 2)).expect("terminal");
                let mut model = Recorder::default();
                // ESC followed at once by "[A" is one key (up); a final lone ESC is Escape.
                let end = drive(
                    &mut model,
                    &mut terminal,
                    vec![
                        TermEvent::Data(b"\x1b".to_vec()),
                        TermEvent::Data(b"[A".to_vec()),
                        TermEvent::Data(b"\x1b".to_vec()),
                    ],
                    Vec::new(),
                    true,
                )
                .await;
                assert_eq!(end, LoopEnd::InputClosed);
                assert_eq!(model.keys, ["<up>", "<esc>"]);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn updates_resizes_and_renders() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut terminal = Terminal::new(TestBackend::new(12, 2)).expect("terminal");
                let mut model = Recorder::default();
                let updates: Vec<Update<Recorder>> = vec![
                    Box::new(|model: &mut Recorder| model.counter += 1),
                    Box::new(|model: &mut Recorder| model.counter += 10),
                    Box::new(|model: &mut Recorder| model.quit = Some("done")),
                ];
                let end = drive(
                    &mut model,
                    &mut terminal,
                    vec![TermEvent::Resize { cols: 12, rows: 2 }],
                    updates,
                    false,
                )
                .await;
                assert_eq!(end, LoopEnd::Exit("done"));
                assert_eq!(model.sizes, [(12, 2)]);
                assert_eq!(model.counter, 11);
                // The first frame, then one after the resize; the update batch ended in exit before drawing.
                assert_eq!(model.renders, 2);
                assert!(model.effects >= 2);
                assert_eq!(rows(&terminal)[0], "count 0     ");
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_spawns_and_times_updates() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut terminal = Terminal::new(TestBackend::new(12, 2)).expect("terminal");
                let mut model = Recorder::default();
                let (tx, mut events) = mpsc::unbounded_channel::<TermEvent>();
                let (dispatch, mut update_rx) = Dispatch::<Recorder>::channel();
                let _task = dispatch.spawn(async { 41 }, |model, value| model.counter = value);
                // Later than the render throttle, so the trailing frame shows the spawned value first.
                let _timer = dispatch.after(RENDER_THROTTLE * 3, |model| {
                    model.counter += 1;
                    model.quit = Some("timer");
                });
                let end = run(&mut model, &mut terminal, &mut events, &mut update_rx)
                    .await
                    .expect("loop runs");
                drop(tx);
                assert_eq!(end, LoopEnd::Exit("timer"));
                assert_eq!(model.counter, 42);
                assert_eq!(rows(&terminal)[0], "count 41    ");
            })
            .await;
    }

    /// Whether the terminal on `fd` is in canonical (cooked) mode.
    fn is_cooked(fd: RawFd) -> bool {
        // SAFETY: the test keeps `fd` open.
        let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
        let attrs = termios::tcgetattr(borrowed).expect("tcgetattr");
        attrs.local_flags.contains(LocalFlags::ICANON | LocalFlags::ECHO)
    }

    #[test]
    fn exit_restores_the_startup_termios_not_the_latest_raw_entry() {
        let pty = nix::pty::openpty(None, None).expect("openpty");
        let fd = pty.slave.as_raw_fd();
        assert!(is_cooked(fd));
        let startup = SavedTermios::capture(fd).expect("a tty");
        // Each raw-mode entry restores what it found (libuv's per-handle orig_termios).
        drop(RawMode::enable(fd).expect("raw"));
        assert!(is_cooked(fd));
        // $EDITOR quits leaving the tty raw; the UI comes back and later quits.
        std::mem::forget(RawMode::enable(fd).expect("raw"));
        assert!(!is_cooked(fd));
        drop(RawMode::enable(fd).expect("raw"));
        assert!(!is_cooked(fd));
        // Node's ResetStdio at exit: the termios saved at startup.
        startup.restore();
        assert!(is_cooked(fd));
        // The process-wide copy is captured once; later captures keep the first.
        assert_eq!(capture_startup_termios(fd).map(|saved| saved.fd), Some(fd));
        std::mem::forget(RawMode::enable(fd).expect("raw"));
        let (read, _write) = nix::unistd::pipe().expect("pipe");
        assert_eq!(
            capture_startup_termios(read.as_raw_fd()).map(|saved| saved.fd),
            Some(fd)
        );
        restore_startup_termios();
        assert!(is_cooked(fd));
        assert!(SavedTermios::capture(read.as_raw_fd()).is_none());
    }

    #[test]
    fn raw_mode_on_a_non_tty_fails_with_inks_message() {
        let (read, _write) = nix::unistd::pipe().expect("pipe");
        let error = enable_ink_raw_mode(read.as_raw_fd())
            .err()
            .map(|error| error.to_string());
        assert_eq!(
            error.as_deref(),
            Some(
                "Raw mode is not supported on the current process.stdin, which Ink uses as input stream by default.\n\
                 Read about how to prevent this error on https://github.com/vadimdemedes/ink/#israwmodesupported"
            )
        );
        // A closed stdin is not a TTY either.
        assert_eq!(
            enable_ink_raw_mode(-1).err().map(|error| error.to_string()),
            Some(RAW_MODE_UNSUPPORTED.to_owned())
        );
    }

    #[test]
    fn renders_are_throttled_like_ink_max_fps_30() {
        // TS: Ink's maxFps 30 → throttle(onRender, ceil(1000 / 30), {leading: true, trailing: true}).
        assert_eq!(RENDER_THROTTLE, Duration::from_millis(34));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn updates_within_the_throttle_window_share_one_trailing_frame() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let mut terminal = Terminal::new(TestBackend::new(12, 2)).expect("terminal");
                let mut model = Recorder::default();
                let (_tx, mut events) = mpsc::unbounded_channel::<TermEvent>();
                let (dispatch, mut update_rx) = Dispatch::<Recorder>::channel();
                // A wide window so the timers below land inside it however loaded the machine is.
                let window = Duration::from_millis(400);
                let _timers: Vec<Timer> = (1..=3)
                    .map(|step| dispatch.after(Duration::from_millis(5 * step), |model| model.counter += 1))
                    .collect();
                let _quit = dispatch.after(window * 2, |model| model.quit = Some("done"));
                let end = run_throttled(&mut model, &mut terminal, &mut events, &mut update_rx, window)
                    .await
                    .expect("loop runs");
                assert_eq!(end, LoopEnd::Exit("done"));
                // The leading frame, then one trailing frame with all three updates; every batch ran its effects.
                assert_eq!(model.renders, 2);
                assert!(model.effects >= 4);
                assert_eq!(rows(&terminal)[0], "count 3     ");
            })
            .await;
    }

    #[test]
    fn frames_are_wrapped_in_synchronized_output_when_they_write() {
        use ratatui::layout::Rect;
        use ratatui::{TerminalOptions, Viewport};
        let options = || TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 8, 1)),
        };
        /// What the backend wrote, shared with the test.
        #[derive(Clone, Default)]
        struct Out(Rc<RefCell<Vec<u8>>>);
        impl io::Write for Out {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let written = |out: &Out| String::from_utf8(std::mem::take(&mut *out.0.borrow_mut())).expect("utf-8");
        let out = Out::default();
        let mut terminal = Terminal::with_options(InkBackend::new(out.clone(), true), options()).expect("terminal");
        terminal
            .draw(|frame| frame.render_widget(Line::from("hi"), frame.area()))
            .expect("draw");
        let first = written(&out);
        assert!(first.starts_with("\x1b[?2026h"), "{first:?}");
        assert!(first.ends_with("\x1b[?2026l"), "{first:?}");
        assert!(first.contains("hi"));
        // An unchanged frame writes no cells, so it is not wrapped (Ink skips output equal to the last).
        terminal
            .draw(|frame| frame.render_widget(Line::from("hi"), frame.area()))
            .expect("draw");
        let second = written(&out);
        assert!(!second.contains("2026") && !second.contains("hi"), "{second:?}");
        // Off a TTY (or in CI) Ink does not synchronize.
        let plain_out = Out::default();
        let mut plain = Terminal::with_options(InkBackend::new(plain_out.clone(), false), options()).expect("terminal");
        plain
            .draw(|frame| frame.render_widget(Line::from("hi"), frame.area()))
            .expect("draw");
        let plain_text = written(&plain_out);
        assert!(
            plain_text.contains("hi") && !plain_text.contains("2026"),
            "{plain_text:?}"
        );
    }

    #[test]
    fn ci_detection_matches_is_in_ci() {
        let env = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
            move |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        };
        assert!(!is_in_ci_with(env(&[])));
        assert!(is_in_ci_with(env(&[("CI", "")])));
        assert!(is_in_ci_with(env(&[("CI", "1")])));
        assert!(!is_in_ci_with(env(&[("CI", "0")])));
        assert!(!is_in_ci_with(env(&[("CI", "false")])));
        assert!(is_in_ci_with(env(&[
            ("CI", "false"),
            ("CONTINUOUS_INTEGRATION", "true")
        ])));
    }

    #[test]
    fn stdin_reader_reads_a_pipe_and_stops() {
        let (read, write) = nix::unistd::pipe().expect("pipe");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut reader = StdinReader::spawn(read.as_raw_fd(), tx).expect("reader");
        nix::unistd::write(&write, b"hi").expect("write");
        let mut got = Vec::new();
        while got.len() < 2 {
            match rx.blocking_recv() {
                Some(TermEvent::Data(bytes)) => got.extend(bytes),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(got, b"hi");
        reader.stop();
        drop(write);
        drop(read);
    }

    #[test]
    fn stdin_reader_reports_end_of_file() {
        let (read, write) = nix::unistd::pipe().expect("pipe");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _reader = StdinReader::spawn(read.as_raw_fd(), tx).expect("reader");
        drop(write);
        assert_eq!(rx.blocking_recv(), Some(TermEvent::Closed));
    }
}
