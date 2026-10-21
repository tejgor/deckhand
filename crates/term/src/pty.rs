//! Spawning a program in a PTY.
//!
//! What node-pty's `pty.spawn` gave the workers: a child in its own session with the PTY slave as controlling terminal
//! (so `kill(-pid)` reaches the whole job, pgid == pid), the same termios node-pty sets up, `TERM`/`PWD` in the child's
//! environment, an exit status reported like `onExit({exitCode, signal})` and a non-blocking master. nodePty.ts itself
//! only made node-pty's prebuilt macOS binaries executable and signed; a native spawn has nothing to sign, so it has
//! no counterpart here.
//!
//! Reads return raw bytes; decode them with [`crate::utf8::Utf8Decoder`] to get exactly the strings node-pty emitted.
//! Exit and EOF are independent: node-pty delayed `onExit` until the socket closed (or 200 ms passed), so callers that
//! need that order drain [`Pty::read`] to EOF (bounded by a timer) before acting on [`Pty::wait`]. Keep reading
//! until exit: on macOS a child cannot finish exiting (it stays in state `E`) while output it wrote is unread.

use std::ffi::{CString, OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use nix::errno::Errno;
use nix::sys::signal::{SigSet, SigmaskHow, Signal, kill, killpg, pthread_sigmask};
use nix::sys::termios::{SetArg, Termios, tcsetattr};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, Pid, fork};
use tokio::io::unix::AsyncFd;
use tokio::sync::{Mutex, watch};

/// TS: node-pty terminal.ts DEFAULT_COLS
pub const DEFAULT_COLS: u16 = 80;
/// TS: node-pty terminal.ts DEFAULT_ROWS
pub const DEFAULT_ROWS: u16 = 24;
/// TS: node-pty unixTerminal.ts DEFAULT_NAME (the child's `TERM` when none is given)
pub const DEFAULT_NAME: &str = "xterm";
/// Message node-pty throws for a non-positive resize (`UnixTerminal.resize`).
pub const RESIZE_ERROR: &str = "resizing must be done using positive cols and rows";

/// TS: node-pty unixTerminal.ts _sanitizeEnv — removed when the child inherits this process's environment.
const SANITIZED_ENV: [&str; 8] = [
    "TMUX",
    "TMUX_PANE",
    "STY",
    "WINDOW",
    "WINDOWID",
    "TERMCAP",
    "COLUMNS",
    "LINES",
];

#[cfg(target_os = "macos")]
const DEFAULT_PATH: &str = "/usr/bin:/bin";
#[cfg(not(target_os = "macos"))]
const DEFAULT_PATH: &str = "/bin:/usr/bin";

/// Highest signal number reset to its default disposition in the child (Linux real-time signals end at 64).
const MAX_SIGNAL: libc::c_int = 64;

/// Room for a perror(3) line in the child, which cannot allocate.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const PERROR_BUFFER: usize = 256;

/// Upper bound of the brute-force descriptor sweep, used only when the child cannot list or range-close its fds.
const FD_SWEEP_LIMIT: libc::c_int = 1 << 16;

/// Linux `close_range(2)` (5.9+): the same number on every architecture, so no per-target libc constant is needed.
#[cfg(target_os = "linux")]
const SYS_CLOSE_RANGE: libc::c_long = 436;

/// Serialises PTY creation and fork within this process (see [`Pty::spawn`]).
static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// TS: node-pty IPtyForkOptions (the subset deckhand passes: name, cols, rows, cwd, env)
#[derive(Debug, Clone, Default)]
pub struct PtyOptions {
    /// `TERM` for the child; `None` uses the env's `TERM`, then [`DEFAULT_NAME`].
    pub name: Option<String>,
    /// Columns; 0 means [`DEFAULT_COLS`] (node-pty: `opt.cols || 80`).
    pub cols: u16,
    /// Rows; 0 means [`DEFAULT_ROWS`].
    pub rows: u16,
    /// Working directory; `None` is this process's cwd. Also exported as `PWD`.
    pub cwd: Option<PathBuf>,
    /// The child's whole environment; `None` inherits this process's, minus [`SANITIZED_ENV`].
    pub env: Option<Vec<(OsString, OsString)>>,
}

/// TS: node-pty IPty.onExit event `{exitCode, signal}`. A signal death reports `exit_code: 0`, a normal exit
/// `signal: 0` (node-pty's native ExitEvent defaults; the worker forwards both numbers as-is).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PtyExit {
    /// `WEXITSTATUS`, or 0 when killed by a signal.
    pub exit_code: i32,
    /// `WTERMSIG`, or 0 when it exited normally.
    pub signal: i32,
}

/// TS: node-pty IPty (UnixTerminal) — a child process on a pseudo-terminal.
///
/// Dropping it sends `SIGKILL` to the child's process group unless the child was already reaped; a background thread
/// reaps the child.
#[derive(Debug)]
pub struct Pty {
    master: AsyncFd<OwnedFd>,
    pid: Pid,
    /// cols << 16 | rows
    size: AtomicU32,
    exit: watch::Receiver<Option<PtyExit>>,
    write_lock: Mutex<()>,
    reaped: Reaped,
}

/// Whether the child has been reaped. Signals are sent with the lock held and only while it is `false`: until the
/// reap the kernel cannot reuse the pid (and so the group id), after it an unrelated process may own either.
type Reaped = Arc<std::sync::Mutex<bool>>;

fn lock_reaped(reaped: &Reaped) -> std::sync::MutexGuard<'_, bool> {
    reaped.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Everything the child needs, allocated before `fork` (only async-signal-safe calls happen after it).
struct ExecPlan {
    argv: Vec<CString>,
    envp: Vec<CString>,
    cwd: Option<CString>,
    candidates: Vec<CString>,
}

/// perror(3)'s line, `<prefix>: <strerror(errno)>\n`, built in `buf` without allocating (the forked child may only make
/// async-signal-safe calls) and truncated to fit. Returns its length.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn perror_line(prefix: &[u8], errno: libc::c_int, buf: &mut [u8]) -> usize {
    let parts: [&[u8]; 4] = [prefix, b": ", Errno::from_raw(errno).desc().as_bytes(), b"\n"];
    let mut len = 0;
    for byte in parts.into_iter().flatten() {
        let Some(slot) = buf.get_mut(len) else {
            break;
        };
        *slot = *byte;
        len += 1;
    }
    len
}

/// Reports a failed chdir/exec in the child the way node-pty did: Linux's forked child `perror`s into the PTY, macOS's
/// spawn-helper exits silently.
///
/// # Safety
/// Async-signal-safe; for the forked child.
unsafe fn child_perror(prefix: &[u8], errno: libc::c_int) {
    #[cfg(target_os = "macos")]
    let _ = (prefix, errno);
    #[cfg(not(target_os = "macos"))]
    {
        let mut buf = [0u8; PERROR_BUFFER];
        let len = perror_line(prefix, errno, &mut buf);
        // SAFETY: buf holds len initialised bytes.
        unsafe { libc::write(libc::STDERR_FILENO, buf.as_ptr().cast(), len) };
    }
}

/// What the child needs to close every descriptor above stderr before exec, as node-pty's macOS spawn did with
/// `POSIX_SPAWN_CLOEXEC_DEFAULT` (whatever the worker holds open without close-on-exec must not reach the agent).
/// Prepared before fork, since the child cannot allocate.
struct InheritedFds {
    /// macOS: room for the `PROC_PIDLISTFDS` listing, with slack for descriptors opened before the fork.
    #[cfg(target_os = "macos")]
    listing: Vec<libc::proc_fdinfo>,
}

impl InheritedFds {
    fn prepare() -> Self {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: a size query (null buffer) about our own process.
            let bytes =
                unsafe { libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
            let entries = usize::try_from(bytes).unwrap_or(0) / std::mem::size_of::<libc::proc_fdinfo>();
            let empty = libc::proc_fdinfo {
                proc_fd: -1,
                proc_fdtype: 0,
            };
            Self {
                listing: vec![empty; entries + 256],
            }
        }
        #[cfg(not(target_os = "macos"))]
        Self {}
    }

    /// Closes every descriptor above 2.
    ///
    /// # Safety
    /// Async-signal-safe; for the forked child just before exec.
    unsafe fn close_all(&mut self) {
        // SAFETY (whole body): syscalls on our own descriptor table and a buffer allocated before fork.
        unsafe {
            #[cfg(target_os = "linux")]
            {
                let (first, last, flags): (libc::c_uint, libc::c_uint, libc::c_uint) = (3, libc::c_uint::MAX, 0);
                if libc::syscall(SYS_CLOSE_RANGE, first, last, flags) == 0 {
                    return;
                }
            }
            #[cfg(target_os = "macos")]
            {
                let entry = std::mem::size_of::<libc::proc_fdinfo>();
                let capacity = libc::c_int::try_from(self.listing.len() * entry).unwrap_or(libc::c_int::MAX);
                let bytes = libc::proc_pidinfo(
                    libc::getpid(),
                    libc::PROC_PIDLISTFDS,
                    0,
                    self.listing.as_mut_ptr().cast(),
                    capacity,
                );
                // A full buffer may have been cut short; sweep instead.
                if bytes > 0 && bytes < capacity {
                    let count = usize::try_from(bytes).unwrap_or(0) / entry;
                    for info in self.listing.iter().take(count) {
                        if info.proc_fd > 2 {
                            libc::close(info.proc_fd);
                        }
                    }
                    return;
                }
            }
            for fd in 3..FD_SWEEP_LIMIT {
                libc::close(fd);
            }
        }
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn cstring(bytes: &[u8], what: &str) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| invalid_input(format!("{what} contains a NUL byte")))
}

fn pack_size(cols: u16, rows: u16) -> u32 {
    (u32::from(cols) << 16) | u32::from(rows)
}

/// The termios node-pty's `PtyFork` hands to `forkpty`/the slave.
fn node_pty_termios() -> Termios {
    // SAFETY: termios is a plain C struct; all-zero is a valid (if useless) value we then fill in.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    t.c_iflag = libc::ICRNL | libc::IXON | libc::IXANY | libc::IMAXBEL | libc::BRKINT | libc::IUTF8;
    t.c_oflag = libc::OPOST | libc::ONLCR;
    t.c_cflag = libc::CREAD | libc::CS8 | libc::HUPCL;
    t.c_lflag = libc::ICANON
        | libc::ISIG
        | libc::IEXTEN
        | libc::ECHO
        | libc::ECHOE
        | libc::ECHOK
        | libc::ECHOKE
        | libc::ECHOCTL;
    t.c_cc[libc::VEOF] = 4;
    t.c_cc[libc::VEOL] = 0xff;
    t.c_cc[libc::VEOL2] = 0xff;
    t.c_cc[libc::VERASE] = 0x7f;
    t.c_cc[libc::VWERASE] = 23;
    t.c_cc[libc::VKILL] = 21;
    t.c_cc[libc::VREPRINT] = 18;
    t.c_cc[libc::VINTR] = 3;
    t.c_cc[libc::VQUIT] = 0x1c;
    t.c_cc[libc::VSUSP] = 26;
    t.c_cc[libc::VSTART] = 17;
    t.c_cc[libc::VSTOP] = 19;
    t.c_cc[libc::VLNEXT] = 22;
    t.c_cc[libc::VDISCARD] = 15;
    t.c_cc[libc::VMIN] = 1;
    t.c_cc[libc::VTIME] = 0;
    #[cfg(target_os = "macos")]
    {
        t.c_cc[libc::VDSUSP] = 25;
        t.c_cc[libc::VSTATUS] = 20;
    }
    // SAFETY: valid pointer to an initialised termios.
    unsafe {
        libc::cfsetispeed(&mut t, libc::B38400);
        libc::cfsetospeed(&mut t, libc::B38400);
    }
    Termios::from(t)
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_winsize(fd: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ reads a winsize from a valid pointer.
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// macOS `TIOCPTYGNAME` (`_IOC(IOC_OUT, 't', 83, 128)`): the slave's path, without `ptsname`'s static buffer.
#[cfg(target_os = "macos")]
const TIOCPTYGNAME: libc::c_ulong = 0x4080_7453;

fn owned(fd: libc::c_int) -> io::Result<OwnedFd> {
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a freshly returned descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// The slave device path of `master`.
fn slave_name(master: RawFd) -> io::Result<CString> {
    let mut name = [0 as c_char; 128];
    // SAFETY: TIOCPTYGNAME writes at most 128 bytes (NUL-terminated) into `name`.
    #[cfg(target_os = "macos")]
    check(unsafe { libc::ioctl(master, TIOCPTYGNAME, name.as_mut_ptr()) })?;
    #[cfg(not(target_os = "macos"))]
    {
        // SAFETY: ptsname_r writes at most name.len() bytes into `name`.
        let error = unsafe { libc::ptsname_r(master, name.as_mut_ptr(), name.len()) };
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
    }
    // SAFETY: both calls NUL-terminate on success.
    Ok(unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_owned())
}

/// openpty(3) without its thread-unsafe `ptsname` (concurrent spawns on macOS raced on it and failed or crossed
/// slaves): master and slave close-on-exec, node-pty's termios and the window size set on the slave.
fn open_pty(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    #[cfg(target_os = "macos")]
    let flags = libc::O_RDWR | libc::O_NOCTTY;
    #[cfg(not(target_os = "macos"))]
    let flags = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;
    // SAFETY: plain libc calls; every returned descriptor is wrapped in an OwnedFd at once.
    let master = owned(unsafe { libc::posix_openpt(flags) })?;
    set_cloexec(master.as_raw_fd())?;
    // SAFETY: grantpt/unlockpt on the master we own.
    check(unsafe { libc::grantpt(master.as_raw_fd()) })?;
    check(unsafe { libc::unlockpt(master.as_raw_fd()) })?;
    let name = slave_name(master.as_raw_fd())?;
    // SAFETY: open with a NUL-terminated path; the descriptor is wrapped at once.
    let slave = owned(unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) })?;
    set_cloexec(slave.as_raw_fd())?;
    tcsetattr(&slave, SetArg::TCSANOW, &node_pty_termios()).map_err(io::Error::from)?;
    set_winsize(slave.as_raw_fd(), cols, rows)?;
    Ok((master, slave))
}

/// The environment node-pty builds:the given one (or ours, sanitised), then `PWD` and `TERM`.
fn child_env(options: &PtyOptions, cwd: &OsStr) -> (Vec<(OsString, OsString)>, String) {
    let mut env: Vec<(OsString, OsString)> = match &options.env {
        Some(env) => env.clone(),
        None => std::env::vars_os()
            .filter(|(key, _)| !SANITIZED_ENV.iter().any(|name| key == *name))
            .collect(),
    };
    let name = options
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .or_else(|| {
            env.iter()
                .rev()
                .find(|(key, _)| key == "TERM")
                .map(|(_, value)| value.to_string_lossy().into_owned())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_NAME.to_string());
    for (key, value) in [
        (OsString::from("PWD"), cwd.to_os_string()),
        (OsString::from("TERM"), OsString::from(&name)),
    ] {
        match env.iter_mut().find(|(existing, _)| *existing == key) {
            Some(slot) => slot.1 = value,
            None => env.push((key, value)),
        }
    }
    (env, name)
}

/// `execvp`'s search, done before fork with the child's `PATH` (node-pty runs execvp with `environ = env`).
fn exec_candidates(file: &str, env: &[(OsString, OsString)]) -> io::Result<Vec<CString>> {
    if file.is_empty() {
        return Ok(Vec::new());
    }
    if file.contains('/') {
        return Ok(vec![cstring(file.as_bytes(), "command")?]);
    }
    let path = env
        .iter()
        .rev()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.as_bytes().to_vec())
        .unwrap_or_else(|| DEFAULT_PATH.as_bytes().to_vec());
    path.split(|byte| *byte == b':')
        .map(|dir| {
            if dir.is_empty() {
                cstring(file.as_bytes(), "command")
            } else {
                let mut full = dir.to_vec();
                if !full.ends_with(b"/") {
                    full.push(b'/');
                }
                full.extend_from_slice(file.as_bytes());
                cstring(&full, "command")
            }
        })
        .collect()
}

impl ExecPlan {
    fn new(file: &str, args: &[String], env: &[(OsString, OsString)], cwd: &OsStr) -> io::Result<Self> {
        let mut argv = vec![cstring(file.as_bytes(), "command")?];
        for arg in args {
            argv.push(cstring(arg.as_bytes(), "argument")?);
        }
        let mut envp = Vec::with_capacity(env.len());
        for (key, value) in env {
            if key.is_empty() || key.as_bytes().contains(&b'=') {
                return Err(invalid_input(format!("invalid environment variable name {key:?}")));
            }
            let mut entry = key.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            envp.push(cstring(&entry, "environment")?);
        }
        let cwd = if cwd.is_empty() {
            None
        } else {
            Some(cstring(cwd.as_bytes(), "cwd")?)
        };
        Ok(Self {
            argv,
            envp,
            cwd,
            candidates: exec_candidates(file, env)?,
        })
    }
}

fn null_terminated(strings: &[CString]) -> Vec<*const c_char> {
    strings
        .iter()
        .map(|value| value.as_ptr())
        .chain(std::iter::once(std::ptr::null()))
        .collect()
}

/// Runs in the forked child: become session leader with the slave as controlling tty, then exec.
/// Never returns; exits 1 when the cwd or every exec candidate fails, as node-pty's child does.
///
/// # Safety
/// Must only be called in the child right after `fork`; every pointer array must be null-terminated and outlive it.
#[allow(clippy::too_many_arguments)]
unsafe fn exec_child(
    slave: RawFd,
    master: RawFd,
    inherited: &mut InheritedFds,
    cwd: Option<&CString>,
    argv: &[*const c_char],
    envp: &[*const c_char],
    candidates: &[*const c_char],
    shell_argvs: &[Vec<*const c_char>],
) -> ! {
    // SAFETY (whole body): only async-signal-safe libc calls on data prepared before fork.
    unsafe {
        libc::setsid();
        libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
        for target in 0..3 {
            if slave == target {
                libc::fcntl(slave, libc::F_SETFD, 0);
            } else {
                libc::dup2(slave, target);
            }
        }
        if slave > 2 {
            libc::close(slave);
        }
        libc::close(master);
        inherited.close_all();

        // Inherited ignored dispositions (and tokio's handlers) must not leak into the agent.
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = libc::SIG_DFL;
        libc::sigemptyset(&mut action.sa_mask);
        for signal in 1..=MAX_SIGNAL {
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
        let mut empty: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        libc::pthread_sigmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());

        if let Some(cwd) = cwd
            && libc::chdir(cwd.as_ptr()) == -1
        {
            child_perror(b"chdir(2) failed.", Errno::last_raw());
            libc::_exit(1);
        }
        // glibc execvp's search: an empty name is ENOENT, a missing or unreachable candidate moves on, any other
        // failure ends the search, and an EACCES seen on the way is what gets reported.
        let mut errno = libc::ENOENT;
        let mut denied = false;
        for (index, candidate) in candidates.iter().enumerate() {
            libc::execve(*candidate, argv.as_ptr(), envp.as_ptr());
            errno = Errno::last_raw();
            // Like execvp: a file the kernel cannot run is retried as a shell script.
            if errno == libc::ENOEXEC
                && let Some(shell_argv) = shell_argvs.get(index)
            {
                libc::execve(c"/bin/sh".as_ptr(), shell_argv.as_ptr(), envp.as_ptr());
                errno = Errno::last_raw();
            }
            match errno {
                libc::EACCES => denied = true,
                libc::ENOENT | libc::ESTALE | libc::ENOTDIR | libc::ENODEV | libc::ETIMEDOUT => {}
                _ => {
                    denied = false;
                    break;
                }
            }
        }
        if denied {
            errno = libc::EACCES;
        }
        child_perror(b"execvp(3) failed.", errno);
        libc::_exit(1)
    }
}

fn exit_from_status(status: Result<WaitStatus, Errno>) -> PtyExit {
    match status {
        Ok(WaitStatus::Exited(_, code)) => PtyExit {
            exit_code: code,
            signal: 0,
        },
        Ok(WaitStatus::Signaled(_, signal, _)) => PtyExit {
            exit_code: 0,
            signal: signal as i32,
        },
        _ => PtyExit::default(),
    }
}

fn spawn_reaper(pid: Pid, sender: watch::Sender<Option<PtyExit>>, reaped: Reaped) -> io::Result<()> {
    std::thread::Builder::new()
        .name(format!("deckhand-pty-wait-{pid}"))
        .spawn(move || {
            // Wait for the exit without reaping, then reap under the lock signallers hold. Should waitid fail, reap
            // without the lock rather than block signallers until the child exits.
            let exited = loop {
                // SAFETY: siginfo_t is a plain C struct waitid fills in.
                let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
                // SAFETY: waitid on our own child with a valid siginfo pointer.
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid.as_raw() as libc::id_t,
                        &mut info,
                        libc::WEXITED | libc::WNOWAIT,
                    )
                };
                if result == 0 {
                    break true;
                }
                if Errno::last() != Errno::EINTR {
                    break false;
                }
            };
            let reap = || loop {
                match waitpid(pid, None) {
                    Err(Errno::EINTR) => continue,
                    other => break other,
                }
            };
            let status = if exited {
                let mut reaped = lock_reaped(&reaped);
                let status = reap();
                *reaped = true;
                status
            } else {
                let status = reap();
                *lock_reaped(&reaped) = true;
                status
            };
            let _ = sender.send(Some(exit_from_status(status)));
        })?;
    Ok(())
}

/// `read(2)` that retries EINTR and reports EIO (Linux: the slave side is gone) as EOF.
fn read_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        // SAFETY: buf is valid for buf.len() bytes.
        let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read >= 0 {
            return Ok(read as usize);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EIO) => return Ok(0),
            _ => return Err(error),
        }
    }
}

fn write_fd(fd: RawFd, data: &[u8]) -> io::Result<usize> {
    loop {
        // SAFETY: data is valid for data.len() bytes.
        let written = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if written >= 0 {
            return Ok(written as usize);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINTR) {
            return Err(error);
        }
    }
}

impl Pty {
    /// TS: node-pty spawn(file, args, {name, cols, rows, cwd, env})
    ///
    /// Must run inside a tokio runtime with IO enabled. Fails for setup errors (openpty, fork, invalid strings); a
    /// command that cannot be executed still spawns and exits with code 1, as with node-pty.
    pub fn spawn(file: &str, args: &[String], options: PtyOptions) -> io::Result<Pty> {
        let cols = if options.cols == 0 { DEFAULT_COLS } else { options.cols };
        let rows = if options.rows == 0 { DEFAULT_ROWS } else { options.rows };
        let cwd = match &options.cwd {
            Some(cwd) => cwd.clone(),
            None => std::env::current_dir()?,
        };
        let (env, _name) = child_env(&options, cwd.as_os_str());
        let plan = ExecPlan::new(file, args, &env, cwd.as_os_str())?;
        let argv = null_terminated(&plan.argv);
        let envp = null_terminated(&plan.envp);
        let candidates: Vec<*const c_char> = plan.candidates.iter().map(|value| value.as_ptr()).collect();
        let shell_argvs: Vec<Vec<*const c_char>> = plan
            .candidates
            .iter()
            .map(|candidate| {
                std::iter::once(c"/bin/sh".as_ptr())
                    .chain(std::iter::once(candidate.as_ptr()))
                    .chain(plan.argv.iter().skip(1).map(|arg| arg.as_ptr()))
                    .chain(std::iter::once(std::ptr::null()))
                    .collect()
            })
            .collect();

        // Held until the parent closes its slave: macOS cannot open the master close-on-exec atomically, and a
        // concurrent spawn forking in that window would hand our master/slave to its child (whose output then
        // never reaches EOF).
        let spawning = SPAWN_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let (master, slave) = open_pty(cols, rows)?;
        set_nonblocking(master.as_raw_fd())?;
        let master_raw = master.as_raw_fd();
        let slave_raw = slave.as_raw_fd();
        // SAFETY: the OwnedFd stays open and unchanged for as long as the AsyncFd owns it.
        let master = unsafe { AsyncFd::register(master) }?;
        let mut inherited = InheritedFds::prepare();

        // Block every signal across fork so no handler runs in the child before it resets them (node-pty does too).
        let mut previous = SigSet::empty();
        pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&SigSet::all()), Some(&mut previous)).map_err(io::Error::from)?;
        // SAFETY: the child only makes async-signal-safe calls (exec_child) before exec or _exit.
        let forked = unsafe { fork() };
        if let Ok(ForkResult::Child) = forked {
            // SAFETY: we are the freshly forked child; all pointers point into `plan`, alive until exec.
            unsafe {
                exec_child(
                    slave_raw,
                    master_raw,
                    &mut inherited,
                    plan.cwd.as_ref(),
                    &argv,
                    &envp,
                    &candidates,
                    &shell_argvs,
                )
            }
        }
        let _ = pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None);
        let pid = match forked {
            Ok(ForkResult::Parent { child }) => child,
            Ok(ForkResult::Child) => unreachable!("child never returns from exec_child"),
            Err(errno) => return Err(io::Error::from(errno)),
        };
        // The parent's slave must close, or reads never see EOF once the child is gone.
        drop(slave);
        drop(spawning);

        let (sender, receiver) = watch::channel(None);
        let reaped = Reaped::default();
        if let Err(error) = spawn_reaper(pid, sender, reaped.clone()) {
            let _ = killpg(pid, Signal::SIGKILL);
            let _ = waitpid(pid, None);
            return Err(error);
        }
        Ok(Pty {
            master,
            pid,
            size: AtomicU32::new(pack_size(cols, rows)),
            exit: receiver,
            write_lock: Mutex::new(()),
            reaped,
        })
    }

    /// TS: IPty.pid — also the child's process group and session id.
    pub fn pid(&self) -> i32 {
        self.pid.as_raw()
    }

    /// TS: IPty.cols
    pub fn cols(&self) -> u16 {
        (self.size.load(Ordering::Relaxed) >> 16) as u16
    }

    /// TS: IPty.rows
    pub fn rows(&self) -> u16 {
        (self.size.load(Ordering::Relaxed) & 0xffff) as u16
    }

    /// The master descriptor (non-blocking, close-on-exec).
    pub fn master_fd(&self) -> RawFd {
        self.master.get_ref().as_raw_fd()
    }

    /// TS: IPty.onData — the next chunk of output; `Ok(0)` is EOF (every slave holder is gone; EIO on Linux).
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let mut guard = self.master.readable().await?;
            match guard.try_io(|inner| read_fd(inner.get_ref().as_raw_fd(), buf)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    /// TS: IPty.write — writes all of `data`; concurrent writers are serialised in call order, like node-pty's queue.
    pub async fn write(&self, data: &[u8]) -> io::Result<()> {
        let _turn = self.write_lock.lock().await;
        let mut offset = 0;
        while offset < data.len() {
            let mut guard = self.master.writable().await?;
            match guard.try_io(|inner| write_fd(inner.get_ref().as_raw_fd(), &data[offset..])) {
                Ok(Ok(written)) => offset += written,
                Ok(Err(error)) => return Err(error),
                Err(_would_block) => continue,
            }
        }
        Ok(())
    }

    /// TS: IPty.resize(cols, rows) — sets the window size (the kernel sends SIGWINCH to the foreground group).
    pub fn resize(&self, cols: u16, rows: u16) -> io::Result<()> {
        if cols == 0 || rows == 0 {
            return Err(invalid_input(RESIZE_ERROR));
        }
        set_winsize(self.master_fd(), cols, rows)?;
        self.size.store(pack_size(cols, rows), Ordering::Relaxed);
        Ok(())
    }

    /// TS: IPty.kill(signal = 'SIGHUP') — signals the child process only. ESRCH once the child has been reaped.
    pub fn kill(&self, signal: Option<Signal>) -> io::Result<()> {
        self.signal_unreaped(|pid| kill(pid, signal.unwrap_or(Signal::SIGHUP)))
    }

    /// TS: process.kill(-term.pid, signal) — signals the child's whole process group. ESRCH once the child (the
    /// group leader) has been reaped: from then on that group id may belong to an unrelated process.
    pub fn kill_group(&self, signal: Signal) -> io::Result<()> {
        self.signal_unreaped(|pid| killpg(pid, signal))
    }

    /// Runs `send` with the child's pid while it is unreaped (and so cannot be reused before `send` returns).
    fn signal_unreaped(&self, send: impl FnOnce(Pid) -> nix::Result<()>) -> io::Result<()> {
        let reaped = lock_reaped(&self.reaped);
        if *reaped {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        send(self.pid).map_err(io::Error::from)
    }

    /// The exit status, once the child has been reaped.
    pub fn try_exit(&self) -> Option<PtyExit> {
        *self.exit.borrow()
    }

    /// TS: IPty.onExit — resolves when the child has exited and been reaped.
    pub async fn wait(&self) -> PtyExit {
        let mut exit = self.exit.clone();
        let result = match exit.wait_for(Option::is_some).await {
            Ok(value) => *value,
            Err(_) => None,
        };
        result.unwrap_or_default()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // An exited but unreaped leader still pins its group id, so this also reaches the orphans it left; once the
        // reaper has run, the id is no longer ours to signal.
        let _ = self.signal_unreaped(|pid| killpg(pid, Signal::SIGKILL));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_candidates_follow_execvp_search() {
        let env = vec![(OsString::from("PATH"), OsString::from("/a::/b/"))];
        let found: Vec<_> = exec_candidates("sh", &env)
            .unwrap()
            .into_iter()
            .map(|c| c.into_string().unwrap())
            .collect();
        assert_eq!(found, ["/a/sh", "sh", "/b/sh"]);
        let direct: Vec<_> = exec_candidates("./x", &env)
            .unwrap()
            .into_iter()
            .map(|c| c.into_string().unwrap())
            .collect();
        assert_eq!(direct, ["./x"]);
        assert!(exec_candidates("", &env).unwrap().is_empty());
        let fallback: Vec<_> = exec_candidates("ls", &[])
            .unwrap()
            .into_iter()
            .map(|c| c.into_string().unwrap())
            .collect();
        assert_eq!(fallback.len(), 2);
    }

    #[test]
    fn child_env_sets_term_and_pwd() {
        let options = PtyOptions {
            env: Some(vec![
                (OsString::from("TERM"), OsString::from("dumb")),
                (OsString::from("A"), OsString::from("1")),
            ]),
            ..PtyOptions::default()
        };
        let (env, name) = child_env(&options, OsStr::new("/tmp"));
        assert_eq!(name, "dumb");
        assert_eq!(
            env,
            [
                (OsString::from("TERM"), OsString::from("dumb")),
                (OsString::from("A"), OsString::from("1")),
                (OsString::from("PWD"), OsString::from("/tmp")),
            ]
        );
        let named = PtyOptions {
            name: Some("xterm-256color".into()),
            env: Some(Vec::new()),
            ..PtyOptions::default()
        };
        let (env, name) = child_env(&named, OsStr::new("/"));
        assert_eq!(name, "xterm-256color");
        assert!(env.contains(&(OsString::from("TERM"), OsString::from("xterm-256color"))));
        let (_, default) = child_env(
            &PtyOptions {
                env: Some(Vec::new()),
                ..PtyOptions::default()
            },
            OsStr::new("/"),
        );
        assert_eq!(default, DEFAULT_NAME);
    }

    #[test]
    fn env_with_nul_or_equals_is_rejected() {
        let bad = [(OsString::from("A=B"), OsString::from("1"))];
        assert!(ExecPlan::new("sh", &[], &bad, OsStr::new("/")).is_err());
        assert!(ExecPlan::new("sh", &["a\0b".to_string()], &[], OsStr::new("/")).is_err());
    }

    #[test]
    fn exit_status_mapping_matches_node_pty() {
        let pid = Pid::from_raw(1);
        assert_eq!(
            exit_from_status(Ok(WaitStatus::Exited(pid, 3))),
            PtyExit {
                exit_code: 3,
                signal: 0
            }
        );
        assert_eq!(
            exit_from_status(Ok(WaitStatus::Signaled(pid, Signal::SIGKILL, false))),
            PtyExit {
                exit_code: 0,
                signal: 9
            }
        );
        assert_eq!(exit_from_status(Err(Errno::ECHILD)), PtyExit::default());
    }

    #[test]
    fn child_errors_read_like_perror() {
        let mut buf = [0u8; PERROR_BUFFER];
        let len = perror_line(b"execvp(3) failed.", libc::ENOENT, &mut buf);
        assert_eq!(&buf[..len], b"execvp(3) failed.: No such file or directory\n");
        let len = perror_line(b"chdir(2) failed.", libc::EACCES, &mut buf);
        assert_eq!(&buf[..len], b"chdir(2) failed.: Permission denied\n");
        let mut tiny = [0u8; 8];
        let len = perror_line(b"chdir(2) failed.", libc::ENOENT, &mut tiny);
        assert_eq!(&tiny[..len], b"chdir(2)");
    }
}
