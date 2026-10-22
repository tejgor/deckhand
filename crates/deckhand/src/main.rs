//! deckhand binary: argv dispatch to UI, daemon, workers, hook, hooks, setup, doctor, restart (port of cli.ts `main`).
//!
//! One binary plays every role the TS split between `node dist/cli.js <mode>` invocations: the Ink UI, the daemon
//! (`--daemon`), the per-session and per-workspace workers (`--session-worker`, `--workspace-worker`), the agent hook
//! bridge (`hook`), the hooks config printer (`hooks [codex]`), `setup`/`doctor`, `restart` (not in TS), the dev
//! channel's `status`/`stop`, `--version` and `help`. Whatever the role, the process runs on one current_thread tokio runtime with a `LocalSet`,
//! so shared state is `Rc<RefCell<..>>` and futures need not be `Send` (docs/conventions.md, Runtime model).

mod cli;
mod doctor;
mod guard;
mod hook;
mod hooks;
#[cfg(target_os = "macos")]
mod notifier;
mod restart;
mod setup;
mod ui_loop;

use std::future::Future;
use std::io::Write;
use std::process::ExitCode;

/// The runtime every Deckhand process runs on: a current_thread tokio runtime (I/O and time drivers on) driving
/// `future` inside a `LocalSet`, so `spawn_local` works anywhere below it. Returns when `future` completes and drops
/// local tasks still pending then (unlike Node, which keeps running until its loop is empty, so a mode's future
/// awaits whatever must finish, such as a queued save). The daemon's future never completes, which is its
/// `await new Promise(() => {})`.
pub(crate) fn block_on_local<F: Future>(future: F) -> std::io::Result<F::Output> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let local = tokio::task::LocalSet::new();
    Ok(local.block_on(&runtime, future))
}

/// `process.argv.slice(2)`. Non-UTF-8 arguments are converted lossily rather than panicking (argv is untrusted).
fn argv() -> Vec<String> {
    std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

/// TS: cli.ts `main().catch(...)`: run the command argv asks for; an error prints its message to stderr and exits 1.
fn main() -> ExitCode {
    let args = argv();
    // The copy inside Deckhand.app only posts notifications (see `notifier`).
    #[cfg(target_os = "macos")]
    if let Some(code) = notifier::run_if_bundled(&args) {
        return code;
    }
    let command = cli::parse_args(&args, deckhand_sys::paths::is_dev_channel());
    // Debug builds refuse ~/.deckhand before anything can read or write it (CONVENTIONS, Isolation).
    if guard::needs_home(&command)
        && let Err(error) = guard::check_home()
    {
        // A hook must never affect the agent: it still answers `{}` and succeeds.
        if command == cli::Command::Hook {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(hook::HOOK_OUTPUT.as_bytes());
            let _ = stdout.flush();
            return ExitCode::SUCCESS;
        }
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    match block_on_local(cli::run(command)) {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("deckhand: could not start the runtime: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_local_runs_non_send_local_tasks() {
        let shared = std::rc::Rc::new(std::cell::Cell::new(0));
        let inner = shared.clone();
        let result = block_on_local(async move {
            let task = tokio::task::spawn_local(async move {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                inner.set(42);
                7
            });
            task.await.unwrap_or_default()
        });
        assert_eq!(result.ok(), Some(7));
        assert_eq!(shared.get(), 42);
    }

    #[test]
    fn guard_skips_commands_that_never_touch_the_state_dir() {
        assert!(!guard::needs_home(&cli::Command::Version));
        assert!(!guard::needs_home(&cli::Command::Help));
        assert!(!guard::needs_home(&cli::Command::Setup(vec!["--check".into()])));
        assert!(!guard::needs_home(&cli::Command::Doctor(Vec::new())));
        assert!(!guard::needs_home(&cli::Command::Hooks(hooks::HooksTarget::Codex)));
        assert!(guard::needs_home(&cli::Command::Ui));
        assert!(guard::needs_home(&cli::Command::Daemon));
        assert!(guard::needs_home(&cli::Command::Restart(Vec::new())));
        assert!(guard::needs_home(&cli::Command::Hook));
        assert!(guard::needs_home(&cli::Command::SessionWorker));
        assert!(guard::needs_home(&cli::Command::WorkspaceWorker));
        assert!(guard::needs_home(&cli::Command::DevDaemon(
            cli::DevDaemonAction::Status
        )));
    }
}
