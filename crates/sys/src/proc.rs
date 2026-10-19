//! Port of child process exec with timeout, killpg (Node `child_process.execFile` as the TS uses it).
//!
//! [`exec`] runs a program with arguments (never through a shell), an optional cwd, environment additions and stdin
//! bytes, and captures stdout/stderr up to `max_buffer` bytes each, like `execFile` with `encoding: 'utf8'`. Every
//! child starts in a session of its own (`setsid`, so its process group id equals its pid): a timeout, an overflow or
//! dropping the future signals the whole group, so nothing the child started outlives it (Node only signals the
//! child; Deckhand's git hooks, updaters and `gh` helpers are better stopped as a group).
//!
//! Errors are [`ExecError`]s whose `Display` is Node's message, which TS code shows to users and tests match on:
//! `Command failed: <file> <args joined by spaces>\n<stderr>` for a non-zero exit or a signal, `spawn <file> <CODE>`
//! when the program cannot be started (`ENOENT`: not installed), `stdout maxBuffer length exceeded` on overflow.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use nix::sys::signal::Signal;
use nix::unistd::Pid;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use deckhand_core::error::{DhError, errno_code};

/// Node's default `execFile` maxBuffer (1 MiB per stream).
pub const DEFAULT_MAX_BUFFER: usize = 1024 * 1024;
/// How long a signalled group may take to exit before it is sent SIGKILL.
pub const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(2);

/// How to run a child: [`exec`]'s options (the spawn-related ones also apply to [`command`]).
#[derive(Clone, Debug)]
pub struct ExecOptions {
    /// Working directory (Node `cwd`); the parent's when `None`.
    pub cwd: Option<PathBuf>,
    /// Variables set on top of the inherited environment (TS `env: {...process.env, ...extra}`).
    pub env: Vec<(OsString, OsString)>,
    /// Variables removed from the inherited environment.
    pub env_remove: Vec<OsString>,
    /// Start from an empty environment (TS `env: {...}` without `process.env`); `env` is then the whole of it.
    pub env_clear: bool,
    /// Bytes written to stdin, which is then closed (TS `child.stdin.end(input)`); `None`: stdin is `/dev/null`.
    pub stdin: Option<Vec<u8>>,
    /// Node `timeout`: the group is sent `kill_signal` when it runs longer.
    pub timeout: Option<Duration>,
    /// Node `maxBuffer`: bytes kept per stream; more is an error and the group is sent `kill_signal`.
    pub max_buffer: usize,
    /// Node `killSignal` (SIGTERM).
    pub kill_signal: Signal,
    /// After `kill_signal`, the group is sent SIGKILL when it has not exited within this.
    pub kill_grace: Duration,
    /// Start the child in a new session (`setsid`: pgid == pid, no controlling terminal). Default true.
    pub setsid: bool,
}

impl Default for ExecOptions {
    fn default() -> Self {
        ExecOptions {
            cwd: None,
            env: Vec::new(),
            env_remove: Vec::new(),
            env_clear: false,
            stdin: None,
            timeout: None,
            max_buffer: DEFAULT_MAX_BUFFER,
            kill_signal: Signal::SIGTERM,
            kill_grace: DEFAULT_KILL_GRACE,
            setsid: true,
        }
    }
}

impl ExecOptions {
    /// Sets the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Adds an environment variable.
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env.push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    /// Removes an inherited environment variable.
    pub fn env_remove(mut self, key: impl AsRef<OsStr>) -> Self {
        self.env_remove.push(key.as_ref().to_owned());
        self
    }

    /// Writes `input` to stdin, then closes it.
    pub fn stdin(mut self, input: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(input.into());
        self
    }

    /// Sets the timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the per-stream byte limit.
    pub fn max_buffer(mut self, max_buffer: usize) -> Self {
        self.max_buffer = max_buffer;
        self
    }
}

/// A finished run's output (Node's `{stdout, stderr}` with `encoding: 'utf8'`; invalid UTF-8 becomes U+FFFD).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Which captured stream overflowed `max_buffer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    fn as_str(self) -> &'static str {
        match self {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        }
    }
}

/// Why a run failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecFailure {
    /// The program could not be started (Node's `spawn` error); `code` is the errno name (`ENOENT`, `EACCES`).
    Spawn {
        code: Option<&'static str>,
        message: String,
    },
    /// It exited with a non-zero code.
    Exit { code: i32 },
    /// It was killed by a signal (`SIGTERM`, ...), e.g. after a timeout.
    Signal { signal: String },
    /// A stream exceeded `max_buffer` (Node `ERR_CHILD_PROCESS_STDIO_MAXBUFFER`).
    MaxBuffer { stream: Stream },
}

/// A failed [`exec`], carrying what Node's execFile error carries (`code`, `signal`, `killed`, `cmd`, `stdout`,
/// `stderr`). `Display` is Node's `error.message`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecError {
    /// `<file> <args joined by spaces>` (Node `error.cmd`).
    pub cmd: String,
    pub failure: ExecFailure,
    /// Deckhand signalled the child (timeout or overflow): Node `error.killed`.
    pub killed: bool,
    /// The timeout fired.
    pub timed_out: bool,
    /// What was captured before the failure.
    pub stdout: String,
    pub stderr: String,
}

impl ExecError {
    /// The errno name of a spawn failure (`error.code === 'ENOENT'` in TS: the program is not installed).
    pub fn errno(&self) -> Option<&'static str> {
        match &self.failure {
            ExecFailure::Spawn { code, .. } => *code,
            _ => None,
        }
    }

    /// Whether the program was not found (or the cwd does not exist), Node's `ENOENT`.
    pub fn is_not_found(&self) -> bool {
        self.errno() == Some("ENOENT")
    }

    /// The exit code of a non-zero exit.
    pub fn exit_code(&self) -> Option<i32> {
        match &self.failure {
            ExecFailure::Exit { code } => Some(*code),
            _ => None,
        }
    }

    /// The signal that ended the child (`SIGTERM`), Node `error.signal`.
    pub fn signal(&self) -> Option<&str> {
        match &self.failure {
            ExecFailure::Signal { signal } => Some(signal),
            _ => None,
        }
    }

    /// Node `error.code` as a string: the errno name, the exit code, or `ERR_CHILD_PROCESS_STDIO_MAXBUFFER`.
    pub fn code(&self) -> Option<String> {
        match &self.failure {
            ExecFailure::Spawn { code, .. } => code.map(str::to_owned),
            ExecFailure::Exit { code } => Some(code.to_string()),
            ExecFailure::Signal { .. } => None,
            ExecFailure::MaxBuffer { .. } => Some("ERR_CHILD_PROCESS_STDIO_MAXBUFFER".to_owned()),
        }
    }

    /// Node `error.message`.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.failure {
            ExecFailure::Spawn { message, .. } => f.write_str(message),
            ExecFailure::MaxBuffer { stream } => write!(f, "{} maxBuffer length exceeded", stream.as_str()),
            ExecFailure::Exit { .. } | ExecFailure::Signal { .. } => {
                write!(f, "Command failed: {}\n{}", self.cmd, self.stderr)
            }
        }
    }
}

impl std::error::Error for ExecError {}

impl From<ExecError> for DhError {
    fn from(error: ExecError) -> Self {
        let message = error.to_string();
        match error.failure {
            ExecFailure::Spawn { code: Some(code), .. } => DhError::with_code(code, message),
            ExecFailure::MaxBuffer { .. } => DhError::with_code("ERR_CHILD_PROCESS_STDIO_MAXBUFFER", message),
            _ => DhError::msg(message),
        }
    }
}

/// Node's `error.cmd`: the file and its arguments joined by spaces (no quoting).
pub fn command_line<S: AsRef<OsStr>>(program: &str, args: &[S]) -> String {
    let mut cmd = program.to_owned();
    for arg in args {
        cmd.push(' ');
        cmd.push_str(&arg.as_ref().to_string_lossy());
    }
    cmd
}

/// A `tokio::process::Command` for `program args` with `options`' cwd, environment and session applied (stdio is
/// left to the caller). For runs [`exec`] does not fit (streaming, bounded diffs, PTYs); pair it with
/// [`GroupGuard`] to stop the group when the run is abandoned.
pub fn command<S: AsRef<OsStr>>(program: &str, args: &[S], options: &ExecOptions) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    if let Some(cwd) = &options.cwd {
        command.current_dir(cwd);
    }
    if options.env_clear {
        command.env_clear();
    }
    for key in &options.env_remove {
        command.env_remove(key);
    }
    for (key, value) in &options.env {
        command.env(key, value);
    }
    if options.setsid {
        // SAFETY: setsid is async-signal-safe and touches no memory of the parent.
        unsafe {
            command.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(io::Error::from));
        }
    }
    command
}

/// Sends `signal` to the process group `pgid` (`kill(-pgid, signal)`).
pub fn kill_group(pgid: u32, signal: Signal) -> io::Result<()> {
    let pid = i32::try_from(pgid).map_err(|_| io::Error::from_raw_os_error(libc::ESRCH))?;
    nix::sys::signal::killpg(Pid::from_raw(pid), signal).map_err(io::Error::from)
}

/// Sends `signal` to one process.
pub fn kill_pid(pid: u32, signal: Signal) -> io::Result<()> {
    let pid = i32::try_from(pid).map_err(|_| io::Error::from_raw_os_error(libc::ESRCH))?;
    nix::sys::signal::kill(Pid::from_raw(pid), signal).map_err(io::Error::from)
}

/// Kills a child's process group (or just the child, without `setsid`) with SIGKILL when dropped before
/// [`GroupGuard::disarm`]: an abandoned run (its future dropped) leaves nothing running.
#[derive(Debug)]
pub struct GroupGuard {
    pid: Option<u32>,
    group: bool,
}

impl GroupGuard {
    /// Guards the child `pid`; `group`: it leads its own process group (spawned with `setsid`).
    pub fn new(pid: Option<u32>, group: bool) -> Self {
        GroupGuard { pid, group }
    }

    /// Signals the group (or the child) now; errors (already gone) are ignored.
    pub fn signal(&self, signal: Signal) {
        if let Some(pid) = self.pid {
            let _ = if self.group {
                kill_group(pid, signal)
            } else {
                kill_pid(pid, signal)
            };
        }
    }

    /// The run finished normally: nothing is killed on drop.
    pub fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.signal(Signal::SIGKILL);
    }
}

fn spawn_error(program: &str, error: &io::Error) -> ExecFailure {
    match errno_code(error) {
        Some(code) => ExecFailure::Spawn {
            code: Some(code),
            message: format!("spawn {program} {code}"),
        },
        None => ExecFailure::Spawn {
            code: None,
            message: error.to_string(),
        },
    }
}

fn status_failure(status: ExitStatus) -> Option<ExecFailure> {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        return (code != 0).then_some(ExecFailure::Exit { code });
    }
    let signal = status
        .signal()
        .map(|number| {
            Signal::try_from(number)
                .map(|signal| signal.as_str().to_owned())
                .unwrap_or_else(|_| number.to_string())
        })
        .unwrap_or_default();
    Some(ExecFailure::Signal { signal })
}

/// Collects one stream up to `max` bytes; reports whether it overflowed.
struct Capture {
    bytes: Vec<u8>,
    max: usize,
    overflowed: bool,
}

impl Capture {
    fn new(max: usize) -> Self {
        Capture {
            bytes: Vec::new(),
            max,
            overflowed: false,
        }
    }

    /// Appends a chunk; true when this chunk made the stream overflow (the rest is dropped, as Node keeps the
    /// first `maxBuffer` bytes).
    fn push(&mut self, chunk: &[u8]) -> bool {
        if self.overflowed {
            return false;
        }
        let room = self.max.saturating_sub(self.bytes.len());
        if chunk.len() > room {
            self.bytes.extend_from_slice(&chunk[..room]);
            self.overflowed = true;
            return true;
        }
        self.bytes.extend_from_slice(chunk);
        false
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Node `execFile(program, args, options)` (promisified): runs `program` with `args` and resolves with its output, or
/// fails with an [`ExecError`] for a spawn error, a non-zero exit, a signal or an overflow. A timeout signals the group
/// (`kill_signal`, then SIGKILL after `kill_grace`); like Node, a child that still exits 0 succeeds. Dropping the
/// future kills the group.
pub async fn exec<S: AsRef<OsStr>>(program: &str, args: &[S], options: &ExecOptions) -> Result<ExecOutput, ExecError> {
    let cmd = command_line(program, args);
    let fail = |failure: ExecFailure, killed: bool, timed_out: bool, stdout: String, stderr: String| ExecError {
        cmd: cmd.clone(),
        failure,
        killed,
        timed_out,
        stdout,
        stderr,
    };
    let mut command = command(program, args, options);
    command
        .stdin(if options.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Err(fail(
                spawn_error(program, &error),
                false,
                false,
                String::new(),
                String::new(),
            ));
        }
    };
    let mut guard = GroupGuard::new(child.id(), options.setsid);

    let stdin_pipe = child.stdin.take();
    let input = options.stdin.clone();
    let write_stdin = async move {
        if let (Some(mut pipe), Some(input)) = (stdin_pipe, input) {
            // EPIPE (a child that exits without reading) is not an error of the run.
            let _ = pipe.write_all(&input).await;
            let _ = pipe.shutdown().await;
        }
    };

    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let mut stdout = Capture::new(options.max_buffer);
    let mut stderr = Capture::new(options.max_buffer);
    let mut overflow: Option<Stream> = None;
    let mut killed = false;
    let mut timed_out = false;

    let collect = async {
        let mut out_buf = vec![0u8; 64 * 1024];
        let mut err_buf = vec![0u8; 64 * 1024];
        let mut status: Option<io::Result<ExitStatus>> = None;
        let timeout = tokio::time::sleep(options.timeout.unwrap_or(Duration::MAX));
        tokio::pin!(timeout);
        let mut timeout_armed = options.timeout.is_some();
        let grace = tokio::time::sleep(Duration::MAX);
        tokio::pin!(grace);
        let mut grace_armed = false;
        loop {
            if status.is_some() && stdout_pipe.is_none() && stderr_pipe.is_none() {
                break;
            }
            tokio::select! {
                read = read_some(&mut stdout_pipe, &mut out_buf), if stdout_pipe.is_some() => {
                    match read {
                        Ok(0) | Err(_) => stdout_pipe = None,
                        Ok(n) => {
                            if stdout.push(&out_buf[..n]) && overflow.is_none() {
                                overflow = Some(Stream::Stdout);
                            }
                        }
                    }
                }
                read = read_some(&mut stderr_pipe, &mut err_buf), if stderr_pipe.is_some() => {
                    match read {
                        Ok(0) | Err(_) => stderr_pipe = None,
                        Ok(n) => {
                            if stderr.push(&err_buf[..n]) && overflow.is_none() {
                                overflow = Some(Stream::Stderr);
                            }
                        }
                    }
                }
                result = child.wait(), if status.is_none() => {
                    status = Some(result);
                    if killed && !grace_armed {
                        // The child is gone; give whatever still holds its pipes the grace period, no more.
                        grace.as_mut().reset(tokio::time::Instant::now() + options.kill_grace);
                        grace_armed = true;
                    }
                }
                () = &mut timeout, if timeout_armed => {
                    timeout_armed = false;
                    timed_out = true;
                }
                () = &mut grace, if grace_armed => {
                    grace_armed = false;
                    guard.signal(Signal::SIGKILL);
                    if status.is_some() {
                        // Pipes held open by something outside the group: stop waiting for them.
                        stdout_pipe = None;
                        stderr_pipe = None;
                    }
                }
            }
            if (timed_out || overflow.is_some()) && !killed {
                killed = true;
                guard.signal(options.kill_signal);
                grace.as_mut().reset(tokio::time::Instant::now() + options.kill_grace);
                grace_armed = true;
            }
        }
        status
    };

    let ((), status) = tokio::join!(write_stdin, collect);
    guard.disarm();
    // The collect loop only ends once the child was waited for.
    let status = match status {
        Some(Ok(status)) => status,
        Some(Err(error)) => {
            let failure = spawn_error(program, &error);
            return Err(fail(failure, killed, timed_out, stdout.text(), stderr.text()));
        }
        None => {
            let failure = ExecFailure::Spawn {
                code: None,
                message: format!("{cmd}: exit status unavailable"),
            };
            return Err(fail(failure, killed, timed_out, stdout.text(), stderr.text()));
        }
    };
    if let Some(stream) = overflow {
        return Err(fail(
            ExecFailure::MaxBuffer { stream },
            killed,
            timed_out,
            stdout.text(),
            stderr.text(),
        ));
    }
    match status_failure(status) {
        None => Ok(ExecOutput {
            stdout: stdout.text(),
            stderr: stderr.text(),
        }),
        Some(failure) => Err(fail(failure, killed, timed_out, stdout.text(), stderr.text())),
    }
}

async fn read_some<R: tokio::io::AsyncRead + Unpin>(pipe: &mut Option<R>, buf: &mut [u8]) -> io::Result<usize> {
    match pipe {
        Some(pipe) => pipe.read(buf).await,
        None => Ok(0),
    }
}

/// Node `execFile` for `git -C <cwd> <args>`-style callers that want the promisified `{stdout, stderr}` with the
/// error's output folded in: `[stdout, stderr].filter(Boolean).join('\n').trim() || message` (git.ts `output`).
pub fn failure_output(error: &ExecError) -> String {
    let joined = [error.stdout.as_str(), error.stderr.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        error.to_string()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Vec<String> {
        vec!["-c".to_owned(), script.to_owned()]
    }

    fn alive(pid: i32) -> bool {
        nix::sys::signal::kill(Pid::from_raw(pid), None).is_ok()
    }

    async fn wait_gone(pid: i32) -> bool {
        for _ in 0..100 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    #[tokio::test(flavor = "current_thread")]
    async fn captures_output_with_cwd_env_and_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(dir.path()).unwrap();
        let options = ExecOptions::default()
            .cwd(&real)
            .env("DECKHAND_PROC_TEST", "value")
            .stdin("from stdin\n");
        let output = exec(
            "sh",
            &sh("pwd; echo \"$DECKHAND_PROC_TEST\"; cat; echo err >&2"),
            &options,
        )
        .await
        .unwrap();
        assert_eq!(output.stdout, format!("{}\nvalue\nfrom stdin\n", real.display()));
        assert_eq!(output.stderr, "err\n");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn env_can_be_removed_or_cleared() {
        let options = ExecOptions::default().env("A_VAR", "1").env_remove("HOME");
        let output = exec("sh", &sh("echo \"${A_VAR}:${HOME:-unset}\""), &options)
            .await
            .unwrap();
        assert_eq!(output.stdout, "1:unset\n");
        let options = ExecOptions {
            env_clear: true,
            env: vec![("ONLY".into(), "x".into()), ("PATH".into(), "/usr/bin:/bin".into())],
            ..ExecOptions::default()
        };
        let output = exec("sh", &sh("env | sort | grep -v '^PWD=\\|^SHLVL=\\|^_='"), &options)
            .await
            .unwrap();
        assert_eq!(output.stdout, "ONLY=x\nPATH=/usr/bin:/bin\n");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn non_zero_exit_has_nodes_message() {
        let error = exec(
            "sh",
            &sh("echo out; echo bad thing >&2; exit 3"),
            &ExecOptions::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.exit_code(), Some(3));
        assert_eq!(error.code().as_deref(), Some("3"));
        assert_eq!(error.cmd, "sh -c echo out; echo bad thing >&2; exit 3");
        assert_eq!(
            error.to_string(),
            "Command failed: sh -c echo out; echo bad thing >&2; exit 3\nbad thing\n"
        );
        assert_eq!(error.stdout, "out\n");
        assert!(!error.killed);
        assert_eq!(failure_output(&error), "out\n\nbad thing");
        let as_dh: DhError = error.into();
        assert_eq!(as_dh.code(), None);
        assert!(as_dh.message().starts_with("Command failed: sh -c"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_program_is_enoent() {
        let error = exec("deckhand-no-such-program-xyz", &["a"], &ExecOptions::default())
            .await
            .unwrap_err();
        assert!(error.is_not_found());
        assert_eq!(error.to_string(), "spawn deckhand-no-such-program-xyz ENOENT");
        let as_dh: DhError = error.into();
        assert!(as_dh.is_not_found());
        // A missing cwd is reported the same way, as Node does.
        let options = ExecOptions::default().cwd("/definitely/not/here");
        let error = exec("sh", &sh("true"), &options).await.unwrap_err();
        assert_eq!(error.to_string(), "spawn sh ENOENT");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn child_leads_its_own_process_group() {
        let output = exec(
            "sh",
            &sh("echo $$; ps -o pgid= -p $$; ps -o sess= -p $$"),
            &ExecOptions::default(),
        )
        .await
        .unwrap();
        let mut lines = output.stdout.lines().map(str::trim);
        let pid = lines.next().unwrap().to_owned();
        assert_eq!(lines.next().unwrap(), pid);
        // macOS `ps -o sess` prints a session pointer, Linux the sid: only check the pgid there.
        let _ = lines.next();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn timeout_kills_the_whole_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("grandchild");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
        let options = ExecOptions::default().timeout(Duration::from_millis(300));
        let started = std::time::Instant::now();
        let error = exec("sh", &sh(&script), &options).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(error.timed_out && error.killed);
        assert_eq!(error.signal(), Some("SIGTERM"));
        assert!(error.to_string().starts_with("Command failed: sh -c sleep 30"));
        let grandchild: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        assert!(wait_gone(grandchild).await, "the background sleep survived the timeout");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn timeout_escalates_to_sigkill_after_grace() {
        let options = ExecOptions {
            timeout: Some(Duration::from_millis(200)),
            kill_grace: Duration::from_millis(200),
            ..ExecOptions::default()
        };
        let error = exec("sh", &sh("trap '' TERM; while :; do sleep 1; done"), &options)
            .await
            .unwrap_err();
        assert!(error.timed_out);
        assert_eq!(error.signal(), Some("SIGKILL"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_child_that_exits_zero_after_the_timeout_signal_succeeds() {
        let options = ExecOptions::default().timeout(Duration::from_millis(200));
        let output = exec(
            "sh",
            &sh("trap 'echo bye; exit 0' TERM; while :; do sleep 0.05; done"),
            &options,
        )
        .await
        .unwrap();
        assert_eq!(output.stdout, "bye\n");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn overflow_keeps_max_buffer_bytes_and_stops_the_child() {
        let options = ExecOptions::default().max_buffer(1000);
        let error = exec("sh", &sh("while :; do echo yyyyyyyyyyyyyyyyyyyy; done"), &options)
            .await
            .unwrap_err();
        assert_eq!(error.failure, ExecFailure::MaxBuffer { stream: Stream::Stdout });
        assert_eq!(error.to_string(), "stdout maxBuffer length exceeded");
        assert_eq!(error.stdout.len(), 1000);
        assert!(error.killed);
        let as_dh: DhError = error.into();
        assert_eq!(as_dh.code(), Some("ERR_CHILD_PROCESS_STDIO_MAXBUFFER"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exactly_max_buffer_is_fine() {
        let options = ExecOptions::default().max_buffer(5);
        let output = exec("printf", &["12345"], &options).await.unwrap();
        assert_eq!(output.stdout, "12345");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn killed_by_signal_reports_it() {
        let error = exec("sh", &sh("kill -TERM $$"), &ExecOptions::default())
            .await
            .unwrap_err();
        assert_eq!(error.signal(), Some("SIGTERM"));
        assert!(!error.killed);
        assert_eq!(error.code(), None);
        assert_eq!(error.to_string(), "Command failed: sh -c kill -TERM $$\n");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_the_future_kills_the_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pids");
        let script = format!("sleep 30 & echo $$ $! > '{}'; wait", pidfile.display());
        let args = sh(&script);
        let options = ExecOptions::default();
        let run = exec("sh", &args, &options);
        let waited = tokio::time::timeout(Duration::from_millis(500), run).await;
        assert!(waited.is_err());
        let text = std::fs::read_to_string(&pidfile).unwrap();
        let pids: Vec<i32> = text.split_whitespace().map(|pid| pid.parse().unwrap()).collect();
        for pid in pids {
            assert!(wait_gone(pid).await, "process {pid} survived the drop");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn large_stdin_and_output_do_not_deadlock() {
        let input = vec![b'x'; 512 * 1024];
        let options = ExecOptions::default().stdin(input.clone());
        let output = exec("cat", &[] as &[&str], &options).await.unwrap();
        assert_eq!(output.stdout.len(), input.len());
        // stdin given to a child that never reads it.
        let options = ExecOptions::default().stdin(input);
        let output = exec("true", &[] as &[&str], &options).await.unwrap();
        assert_eq!(output.stdout, "");
    }

    #[test]
    fn command_line_joins_like_node() {
        assert_eq!(command_line("git", &["-C", "/x y", "status"]), "git -C /x y status");
        assert_eq!(command_line::<&str>("git", &[]), "git");
    }
}
