//! Port of helpers.ts `terminalUi`: runs a program in a real PTY and reads it through a `vt100` screen model.
//!
//! `screen()` matches the rendered screen, not the raw stream: a UI may redraw only the cells that changed, so text on
//! screen need not appear contiguously in the output (and text drawn then erased is gone).

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::harness::{Sandbox, wait_exit};

pub const UI_COLS: u16 = 130;
pub const UI_ROWS: u16 = 36;
/// Ceiling for a real-PTY UI wait. Waits are condition-based, so this only matters under load.
pub const UI_WAIT: Duration = Duration::from_secs(30);

struct State {
    parser: vt100::Parser,
    chunks: u64,
    chunks_since_press: u64,
    last_chunk_at: Instant,
}

pub struct Ui {
    state: Arc<Mutex<State>>,
    writer: File,
    child: Child,
}

/// What to run. `dev` adds the isolated dev launcher environment (`DECKHAND_DEV_HOME`, no stable `DECKHAND_HOME`).
pub struct UiOptions<'a> {
    pub program: &'a Path,
    pub args: &'a [&'a str],
    pub cwd: &'a Path,
    pub env: &'a [(&'a str, &'a str)],
    pub dev: bool,
}

impl Ui {
    /// Starts `options.program` in a 130x36 PTY (TERM=xterm-256color, SHELL=/bin/sh, private HOME, no agent-release
    /// lookups). The caller keeps `sandbox` alive longer than the `Ui`; the sandbox stops the dev daemon on drop.
    pub fn spawn(sandbox: &Sandbox, options: UiOptions) -> Ui {
        let mut command = sandbox.command(options.program);
        command
            .args(options.args)
            .current_dir(options.cwd)
            .env("TERM", "xterm-256color")
            .env("SHELL", "/bin/sh");
        if options.dev {
            sandbox.dev_env(&mut command);
        }
        for (key, value) in options.env {
            command.env(key, value);
        }
        for key in ["DECKHAND_SESSION_ID", "DECKHAND_LAUNCH_ID", "DECKHAND_HOOK_TOKEN"] {
            command.env_remove(key);
        }
        let size = nix::pty::Winsize {
            ws_row: UI_ROWS,
            ws_col: UI_COLS,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pty = nix::pty::openpty(Some(&size), None).expect("openpty");
        let slave = |fd: &std::os::fd::OwnedFd| Stdio::from(fd.try_clone().expect("dup pty slave"));
        command
            .stdin(slave(&pty.slave))
            .stdout(slave(&pty.slave))
            .stderr(slave(&pty.slave));
        // SAFETY: only async-signal-safe calls (setsid, ioctl) between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().expect("spawn program in pty");
        drop(command);
        drop(pty.slave);
        let master = File::from(pty.master);
        let mut reader = master.try_clone().expect("dup pty master");
        let state = Arc::new(Mutex::new(State {
            parser: vt100::Parser::new(UI_ROWS, UI_COLS, 0),
            chunks: 0,
            chunks_since_press: 0,
            last_chunk_at: Instant::now(),
        }));
        let thread_state = Arc::clone(&state);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            // EIO once the child side closes.
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let mut state = thread_state.lock().unwrap_or_else(|e| e.into_inner());
                state.chunks += 1;
                state.chunks_since_press += 1;
                state.last_chunk_at = Instant::now();
                state.parser.process(&buffer[..n]);
            }
        });
        Ui {
            state,
            writer: master,
            child,
        }
    }

    /// The rendered screen now: every row, right-trimmed, joined with `\n`. Reads once output has paused briefly
    /// (bounded, as a spinner never stops), so a half-drawn frame is not taken for the screen.
    pub fn screen_text(&self) -> String {
        let deadline = Instant::now() + Duration::from_millis(300);
        loop {
            let quiet = {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                state.last_chunk_at.elapsed() >= Duration::from_millis(50)
            };
            if quiet || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .parser
            .screen()
            .rows(0, UI_COLS)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn since_press(&self) -> u64 {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).chunks_since_press
    }

    /// Waits until output arrived after the last `press()` and the rendered screen contains `text`; returns the screen.
    pub fn screen(&self, text: &str) -> String {
        self.screen_within(text, UI_WAIT)
    }

    pub fn screen_within(&self, text: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            if self.since_press() > 0 {
                let screen = self.screen_text();
                if self.since_press() > 0 && screen.contains(text) {
                    return screen;
                }
            }
            if Instant::now() >= deadline {
                let note = if self.since_press() > 0 {
                    ""
                } else {
                    " (no output since the last key)"
                };
                panic!("Screen never showed {text:?}{note}:\n{}", self.screen_text());
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    /// Resetting first means the next `screen()` only matches once output produced after this key arrived.
    pub fn press(&mut self, key: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).chunks_since_press = 0;
        let _ = self.writer.write_all(key.as_bytes());
    }

    pub fn write(&mut self, data: &str) {
        let _ = self.writer.write_all(data.as_bytes());
    }

    /// The exit status once the program has ended (waits up to `timeout`).
    pub fn ended(&mut self, timeout: Duration) -> Option<ExitStatus> {
        wait_exit(&mut self.child, timeout)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            // setsid made the child a group leader (pgid == pid).
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(self.child.id() as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
            let _ = self.child.wait();
        }
    }
}

/// Convenience for the commonest case: the isolated dev launcher in a PTY (`scripts/deckhand-dev.sh <args>`).
pub fn spawn_launcher(sandbox: &Sandbox, args: &[&str], cwd: &Path) -> Ui {
    spawn_launcher_env(sandbox, args, cwd, &[])
}

/// [`spawn_launcher`] with extra environment variables. The launcher is executed directly (it is executable).
pub fn spawn_launcher_env(sandbox: &Sandbox, args: &[&str], cwd: &Path, env: &[(&str, &str)]) -> Ui {
    let launcher = crate::harness::launcher();
    Ui::spawn(
        sandbox,
        UiOptions {
            program: &launcher,
            args,
            cwd,
            env,
            dev: true,
        },
    )
}

/// The disposable dev sandbox UI (`scripts/deckhand-dev.sh --sandbox`) started in the dev home, with the fake agents in
/// `bin` first on PATH. `env` adds variables (a `PATH` entry here replaces the one built from `bin`).
pub fn launch_sandbox_ui(sandbox: &Sandbox, bin: &Path, env: &[(&str, &str)]) -> Ui {
    let path = crate::daemon::path_with(bin);
    let mut vars: Vec<(&str, &str)> = vec![("PATH", path.as_str())];
    vars.extend_from_slice(env);
    spawn_launcher_env(sandbox, &["--sandbox"], &sandbox.dev_home(), &vars)
}
