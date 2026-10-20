//! Port of setup.ts.
//!
//! `deckhand setup [--check] [--yes|-y]`: reports which agents (and the optional lazygit) are on the user's PATH as
//! their interactive shell sees it (`$SHELL -ic 'command -v <tool>'`), and offers to run each missing agent's
//! official installer. `--check` only reports. Tests run it with a temp `HOME`, so the shell reads scratch rc files.

// B1 wires `deckhand setup`/`doctor` into the binary; until then only the tests reach these items.
#![allow(dead_code)]

use std::io::Write;

use deckhand_core::error::DhError;
use deckhand_core::jsstr;
use deckhand_sys::proc::{self, ExecOptions};

/// TS: setup.ts ToolInstall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolInstall {
    pub key: &'static str,
    pub label: &'static str,
    pub command: &'static str,
    /// Agents only; optional tools are never installed by setup.
    pub install_command: Option<&'static str>,
    pub description: &'static str,
    pub optional: bool,
}

/// TS: setup.ts ToolStatus (a [`ToolInstall`] plus what detection found).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStatus {
    pub tool: ToolInstall,
    pub installed: bool,
    /// `command -v` output, trimmed (set when installed).
    pub path: Option<String>,
}

/// TS: setup.ts AGENT_TOOLS.
pub const AGENT_TOOLS: [ToolInstall; 3] = [
    ToolInstall {
        key: "claude",
        label: "Claude Code",
        command: "claude",
        install_command: Some("curl -fsSL https://claude.ai/install.sh | bash"),
        description: "Anthropic Claude Code agent",
        optional: false,
    },
    ToolInstall {
        key: "pi",
        label: "Pi",
        command: "pi",
        install_command: Some("curl -fsSL https://pi.dev/install.sh | sh"),
        description: "Pi coding agent",
        optional: false,
    },
    ToolInstall {
        key: "codex",
        label: "Codex",
        command: "codex",
        install_command: Some("curl -fsSL https://chatgpt.com/codex/install.sh | sh"),
        description: "OpenAI Codex CLI agent",
        optional: false,
    },
];

/// TS: setup.ts OPTIONAL_TOOLS.
pub const OPTIONAL_TOOLS: [ToolInstall; 1] = [ToolInstall {
    key: "lazygit",
    label: "lazygit",
    command: "lazygit",
    install_command: None,
    description: "opened with o on the Git tab",
    optional: true,
}];

/// TS: setup.ts runSetup's flags (`--yes`/`-y`, `--check`; anything else ignored).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SetupArgs {
    pub assume_yes: bool,
    pub check_only: bool,
}

impl SetupArgs {
    /// TS: setup.ts `args.includes('--yes') || args.includes('-y')`, `args.includes('--check')`.
    pub fn parse(args: &[String]) -> SetupArgs {
        let has = |flag: &str| args.iter().any(|arg| arg == flag);
        SetupArgs {
            assume_yes: has("--yes") || has("-y"),
            check_only: has("--check"),
        }
    }
}

/// TS: setup.ts splitCommand result (a command line run by the user's shell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    pub file: String,
    pub args: Vec<String>,
}

/// TS: setup.ts shell (`$SHELL`, else `/bin/bash`; empty counts as unset).
pub fn shell() -> String {
    shell_from(std::env::var_os("SHELL").map(|value| value.to_string_lossy().into_owned()))
}

/// `process.env.SHELL || '/bin/bash'`.
fn shell_from(value: Option<String>) -> String {
    value
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/bin/bash".to_owned())
}

/// TS: setup.ts commandPath (`<shell> -ic 'command -v <command>'`, trimmed stdout; `None` on failure or empty
/// output).
pub async fn command_path(command: &str) -> Option<String> {
    command_path_in(&shell(), command, &ExecOptions::default()).await
}

/// [`command_path`] with an explicit shell and exec options (tests use a scratch shell and environment).
async fn command_path_in(shell: &str, command: &str, options: &ExecOptions) -> Option<String> {
    // TS interpolates the name unquoted; the names are the constants above, never user input.
    let script = format!("command -v {command}");
    let output = proc::exec(shell, &["-ic", script.as_str()], options).await.ok()?;
    let path = jsstr::js_trim(&output.stdout);
    (!path.is_empty()).then(|| path.to_owned())
}

/// TS: setup.ts detectTools (agents then optional tools, probed concurrently, in that order).
pub async fn detect_tools() -> Vec<ToolStatus> {
    detect_tools_in(&shell(), &ExecOptions::default()).await
}

/// [`detect_tools`] with an explicit shell and exec options.
async fn detect_tools_in(shell: &str, options: &ExecOptions) -> Vec<ToolStatus> {
    let probe = |tool: ToolInstall| async move {
        let path = command_path_in(shell, tool.command, options).await;
        ToolStatus {
            tool,
            installed: path.is_some(),
            path,
        }
    };
    // `Promise.all` over `[...AGENT_TOOLS, ...OPTIONAL_TOOLS]`: concurrent probes, results in list order.
    let [claude, pi, codex] = AGENT_TOOLS;
    let [lazygit] = OPTIONAL_TOOLS;
    let (claude, pi, codex, lazygit) = tokio::join!(probe(claude), probe(pi), probe(codex), probe(lazygit));
    vec![claude, pi, codex, lazygit]
}

/// TS: setup.ts splitCommand (`{file: shell(), args: ['-ic', command]}`).
pub fn split_command(command: &str) -> ShellCommand {
    split_command_in(&shell(), command)
}

fn split_command_in(shell: &str, command: &str) -> ShellCommand {
    ShellCommand {
        file: shell.to_owned(),
        args: vec!["-ic".to_owned(), command.to_owned()],
    }
}

/// TS: setup.ts runInstall (stdio inherited; the exit code, 1 on a spawn error or a signal).
pub async fn run_install(command: &str) -> i32 {
    run_shell_command(&split_command(command)).await
}

/// `spawn(file, args, {stdio: 'inherit'})`: same session and terminal as Deckhand, since installers may prompt.
async fn run_shell_command(command: &ShellCommand) -> i32 {
    let status = tokio::process::Command::new(&command.file)
        .args(&command.args)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await;
    match status {
        // `code ?? 1`: a signal leaves no code.
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => 1,
    }
}

/// TS: setup.ts printStatus as text: `Deckhand setup check`, a blank line, one line per tool (`✓`/`•`, label,
/// ` optional`, description, ` (<path>)` or ` (not installed)`), and a blank line.
pub fn format_status(statuses: &[ToolStatus]) -> String {
    let mut text = String::from("Deckhand setup check\n\n");
    for status in statuses {
        let marker = if status.installed { '✓' } else { '•' };
        let location = match (status.installed, &status.path) {
            (true, Some(path)) => format!(" ({path})"),
            // `${status.path}` of an absent path (detection never produces this).
            (true, None) => " (undefined)".to_owned(),
            (false, _) => " (not installed)".to_owned(),
        };
        let optional = if status.tool.optional { " optional" } else { "" };
        text.push_str(&format!(
            "{marker} {}{optional}: {}{location}\n",
            status.tool.label, status.tool.description
        ));
    }
    text.push('\n');
    text
}

/// TS: setup.ts printStatus (writes [`format_status`] to stdout).
pub fn print_status(statuses: &[ToolStatus]) {
    write_stdout(&format_status(statuses));
}

/// `process.stdout.write`: a closed stdout (`deckhand setup | head`) must not panic.
pub(crate) fn write_stdout(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// `console.log(line)`.
pub(crate) fn log(line: &str) {
    write_stdout(&format!("{line}\n"));
}

/// TS: setup.ts promptYes (`<question> [y/N] ` on stdout; `y`/`yes`, trimmed and lowercased, is yes; `assume_yes`
/// skips the prompt). `None` once stdin has ended: TS's `rl.question` then never settles, so runSetup goes no further
/// and the process exits 0 with nothing more printed.
pub async fn prompt_yes(question: &str, assume_yes: bool) -> Option<bool> {
    if assume_yes {
        return Some(true);
    }
    write_stdout(&format!("{question} [y/N] "));
    // A blocking read off the runtime thread (tokio's async stdin is not enabled in this workspace).
    let line = tokio::task::spawn_blocking(|| read_answer(libc::STDIN_FILENO)).await;
    line.ok().flatten().map(|line| is_yes(&line))
}

/// Node's stream read size (64 KiB).
const READ_CHUNK: usize = 64 * 1024;

/// The line a fresh readline interface on `fd` answers its question with: the bytes up to the first `\n` or `\r`
/// (readline splits on `\r\n|\n|\r`), gathered over as many reads as it takes. Whatever followed it in the same read
/// is dropped, as it goes with the interface `rl.close()` discards. `None` when input ends (or fails) first, even
/// after a partial line: the interface closes without answering. Reads `fd` directly, never through a buffer that
/// would carry input over to the next prompt.
fn read_answer(fd: std::os::fd::RawFd) -> Option<String> {
    let mut line = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    loop {
        let count = match nix::unistd::read(fd, &mut chunk) {
            Ok(0) => return None,
            Ok(count) => count,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(nix::errno::Errno::EAGAIN) => {
                // An inherited non-blocking stdin: wait for input instead of spinning.
                // SAFETY: `fd` is open for the duration of this call (read just reported it would block).
                let input = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
                let mut fds = [nix::poll::PollFd::new(input, nix::poll::PollFlags::POLLIN)];
                match nix::poll::poll(&mut fds, nix::poll::PollTimeout::NONE) {
                    Ok(_) | Err(nix::errno::Errno::EINTR) => continue,
                    Err(_) => return None,
                }
            }
            Err(_) => return None,
        };
        let got = chunk.get(..count).unwrap_or_default();
        match got.iter().position(|byte| matches!(byte, b'\n' | b'\r')) {
            Some(end) => {
                line.extend_from_slice(got.get(..end).unwrap_or_default());
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            None => line.extend_from_slice(got),
        }
    }
}

/// `answer.trim().toLowerCase()` is `y` or `yes`.
fn is_yes(answer: &str) -> bool {
    let answer = jsstr::js_to_lower(jsstr::js_trim(answer));
    answer == "y" || answer == "yes"
}

/// What runSetup does to the outside world, so its flow can be tested without a terminal or real installers.
trait SetupHost {
    async fn detect(&mut self) -> Vec<ToolStatus>;
    /// `console.log(text)`.
    fn log(&mut self, text: &str);
    /// `None`: stdin ended before an answer.
    async fn prompt(&mut self, question: &str, assume_yes: bool) -> Option<bool>;
    async fn install(&mut self, command: &str) -> i32;
}

/// The real host: the user's shell, stdout and stdin.
struct RealHost;

impl SetupHost for RealHost {
    async fn detect(&mut self) -> Vec<ToolStatus> {
        detect_tools().await
    }

    fn log(&mut self, text: &str) {
        log(text);
    }

    async fn prompt(&mut self, question: &str, assume_yes: bool) -> Option<bool> {
        prompt_yes(question, assume_yes).await
    }

    async fn install(&mut self, command: &str) -> i32 {
        run_install(command).await
    }
}

/// TS: setup.ts runSetup (exported): status, then for each missing agent its install command and (unless
/// `--check`) an offer to run it, then the status after setup and what is still missing.
pub async fn run_setup(args: &[String]) -> Result<(), DhError> {
    run_setup_with(args, &mut RealHost).await;
    Ok(())
}

/// printStatus through `host` (its trailing `console.log('')` is the final newline of [`format_status`]).
fn log_status(host: &mut impl SetupHost, statuses: &[ToolStatus]) {
    let text = format_status(statuses);
    host.log(text.strip_suffix('\n').unwrap_or(&text));
}

async fn run_setup_with(args: &[String], host: &mut impl SetupHost) {
    let SetupArgs { assume_yes, check_only } = SetupArgs::parse(args);
    let statuses = host.detect().await;
    log_status(host, &statuses);

    let missing: Vec<&ToolStatus> = statuses.iter().filter(|status| !status.installed).collect();
    if missing.is_empty() {
        host.log("Everything Deckhand knows how to check is installed.");
        return;
    }

    let missing_agents: Vec<(&str, &str)> = missing
        .iter()
        .filter_map(|status| Some((status.tool.label, status.tool.install_command?)))
        .collect();
    if missing_agents.is_empty() {
        host.log(
            "All supported agents are installed. lazygit is optional: the Git tab only needs git, and `o` there opens lazygit if you install it.",
        );
        return;
    }
    if check_only {
        host.log("Run `deckhand setup` to install missing agents. lazygit is optional and is not installed by setup.");
        return;
    }

    for (label, command) in missing_agents {
        host.log(&format!("{label} can be installed with: {command}"));
        let Some(install) = host.prompt(&format!("Install {label} now?"), assume_yes).await else {
            // TS: the pending `rl.question` never settles once stdin has ended; nothing else keeps Node running.
            return;
        };
        if install {
            let code = host.install(command).await;
            if code != 0 {
                host.log(&format!("{label} install failed. You can run this manually: {command}"));
            }
        }
    }

    let next_statuses = host.detect().await;
    host.log("\nAfter setup:");
    log_status(host, &next_statuses);
    if next_statuses
        .iter()
        .any(|status| !status.installed && status.tool.install_command.is_some())
    {
        host.log(
            "Some agents are still missing. Restart your terminal if installs succeeded but are not detected yet.",
        );
    }
    if next_statuses
        .iter()
        .any(|status| status.tool.key == "lazygit" && !status.installed)
    {
        host.log("lazygit is optional and is not installed by setup: the Git tab only needs git, and `o` there opens lazygit if you install it.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn all_tools() -> Vec<ToolInstall> {
        AGENT_TOOLS.iter().chain(OPTIONAL_TOOLS.iter()).copied().collect()
    }

    /// Statuses with the tools whose keys are in `installed` found at `/bin/<key>`.
    fn statuses(installed: &[&str]) -> Vec<ToolStatus> {
        all_tools()
            .into_iter()
            .map(|tool| {
                let found = installed.contains(&tool.key);
                ToolStatus {
                    tool,
                    installed: found,
                    path: found.then(|| format!("/bin/{}", tool.key)),
                }
            })
            .collect()
    }

    #[derive(Default)]
    struct FakeHost {
        detections: VecDeque<Vec<ToolStatus>>,
        out: String,
        prompts: Vec<(String, bool)>,
        /// `None`: stdin closed while the prompt waited.
        answers: VecDeque<Option<bool>>,
        installs: Vec<String>,
        codes: VecDeque<i32>,
    }

    impl SetupHost for FakeHost {
        async fn detect(&mut self) -> Vec<ToolStatus> {
            self.detections.pop_front().unwrap_or_default()
        }

        fn log(&mut self, text: &str) {
            self.out.push_str(text);
            self.out.push('\n');
        }

        async fn prompt(&mut self, question: &str, assume_yes: bool) -> Option<bool> {
            self.prompts.push((question.to_owned(), assume_yes));
            if assume_yes {
                return Some(true);
            }
            self.answers.pop_front().unwrap_or(Some(false))
        }

        async fn install(&mut self, command: &str) -> i32 {
            self.installs.push(command.to_owned());
            self.codes.pop_front().unwrap_or(0)
        }
    }

    fn host(detections: Vec<Vec<ToolStatus>>) -> FakeHost {
        FakeHost {
            detections: detections.into(),
            ..FakeHost::default()
        }
    }

    fn write_script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path.to_string_lossy().into_owned()
    }

    /// A non-interactive stand-in for `$SHELL -ic <script>` (never reads rc files) and a scratch PATH/HOME.
    fn fake_shell(dir: &Path, on_path: &[&str]) -> (String, ExecOptions) {
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("bin dir");
        for name in on_path {
            write_script(&bin, name, "#!/bin/sh\nexit 0\n");
        }
        let shell = write_script(
            dir,
            "shell",
            "#!/bin/sh\n[ \"$1\" = -ic ] || exit 64\nexec /bin/sh -c \"$2\"\n",
        );
        let options = ExecOptions::default()
            .env("PATH", bin.as_os_str())
            .env("HOME", dir.as_os_str());
        (shell, options)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn parses_flags_anywhere_and_ignores_the_rest() {
        assert_eq!(SetupArgs::parse(&[]), SetupArgs::default());
        assert_eq!(
            SetupArgs::parse(&args(&["--check", "extra"])),
            SetupArgs {
                assume_yes: false,
                check_only: true
            }
        );
        assert!(SetupArgs::parse(&args(&["x", "-y"])).assume_yes);
        assert!(SetupArgs::parse(&args(&["--yes"])).assume_yes);
        assert_eq!(
            SetupArgs::parse(&args(&["--YES", "-check", "--check=1"])),
            SetupArgs::default()
        );
    }

    #[test]
    fn shell_falls_back_to_bash_when_unset_or_empty() {
        assert_eq!(shell_from(None), "/bin/bash");
        assert_eq!(shell_from(Some(String::new())), "/bin/bash");
        assert_eq!(shell_from(Some("/bin/zsh".into())), "/bin/zsh");
        assert_eq!(
            split_command_in("/bin/zsh", "curl x | sh"),
            ShellCommand {
                file: "/bin/zsh".into(),
                args: vec!["-ic".into(), "curl x | sh".into()]
            }
        );
    }

    #[test]
    fn formats_status_like_print_status() {
        assert_eq!(
            format_status(&statuses(&["claude", "lazygit"])),
            "Deckhand setup check\n\n\
             ✓ Claude Code: Anthropic Claude Code agent (/bin/claude)\n\
             • Pi: Pi coding agent (not installed)\n\
             • Codex: OpenAI Codex CLI agent (not installed)\n\
             ✓ lazygit optional: opened with o on the Git tab (/bin/lazygit)\n\n"
        );
        assert_eq!(format_status(&[]), "Deckhand setup check\n\n\n");
    }

    #[test]
    fn yes_answers_are_trimmed_and_case_insensitive() {
        for answer in ["y\n", "Y", " yes \n", "YES\r\n", "\u{A0}Yes"] {
            assert!(is_yes(answer), "{answer:?}");
        }
        for answer in ["", "\n", "n", "no", "yess", "y y", "ja"] {
            assert!(!is_yes(answer), "{answer:?}");
        }
    }

    #[test]
    fn assume_yes_skips_the_prompt() {
        assert_eq!(runtime().block_on(prompt_yes("Install Pi now?", true)), Some(true));
    }

    #[test]
    fn each_prompt_reads_like_a_fresh_readline_interface() {
        let (read, write) = nix::unistd::pipe().expect("pipe");
        let fd = read.as_raw_fd();
        // The rest of the chunk that held the answer goes with the interface that read it.
        nix::unistd::write(&write, b"n\ny\n").expect("write");
        assert_eq!(read_answer(fd).as_deref(), Some("n"));
        // A line can span reads; `\r` ends a line as readline's `\r\n|\n|\r` does.
        nix::unistd::write(&write, b"ye").expect("write");
        let reader = std::thread::spawn(move || read_answer(fd));
        std::thread::sleep(std::time::Duration::from_millis(50));
        nix::unistd::write(&write, b"s\rn\n").expect("write");
        assert_eq!(reader.join().ok().flatten().as_deref(), Some("yes"));
        // End of input before a line ends answers nothing, not even the partial line.
        nix::unistd::write(&write, b"y").expect("write");
        drop(write);
        assert_eq!(read_answer(fd), None);
        // A closed stdin is the same end of input.
        assert_eq!(read_answer(-1), None);
    }

    #[test]
    fn closed_input_stops_setup_at_the_pending_prompt() {
        // TS: once stdin has ended `rl.question` never settles, so setup ends there and the process exits 0 without
        // the After-setup report.
        let before = statuses(&["pi"]);
        let mut fake = host(vec![before.clone(), statuses(&["pi"])]);
        fake.answers = vec![None].into();
        runtime().block_on(run_setup_with(&[], &mut fake));
        assert_eq!(fake.prompts, [("Install Claude Code now?".to_owned(), false)]);
        assert!(fake.installs.is_empty());
        assert_eq!(
            fake.out,
            format!(
                "{}Claude Code can be installed with: curl -fsSL https://claude.ai/install.sh | bash\n",
                format_status(&before)
            )
        );
        assert_eq!(fake.detections.len(), 1);
    }

    #[test]
    fn command_path_trims_the_shell_output_and_maps_failures_to_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shell, options) = fake_shell(dir.path(), &["claude"]);
        let runtime = runtime();
        let found = runtime.block_on(command_path_in(&shell, "claude", &options));
        assert_eq!(
            found,
            Some(dir.path().join("bin/claude").to_string_lossy().into_owned())
        );
        assert_eq!(runtime.block_on(command_path_in(&shell, "pi", &options)), None);
        // Empty output is not a path.
        let quiet = write_script(dir.path(), "quiet", "#!/bin/sh\necho '  '\n");
        assert_eq!(runtime.block_on(command_path_in(&quiet, "claude", &options)), None);
        // A shell that cannot start.
        let missing = dir.path().join("no-shell").to_string_lossy().into_owned();
        assert_eq!(runtime.block_on(command_path_in(&missing, "claude", &options)), None);
    }

    #[test]
    fn detect_tools_reports_agents_then_optional_tools_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (shell, options) = fake_shell(dir.path(), &["codex", "lazygit"]);
        let found = runtime().block_on(detect_tools_in(&shell, &options));
        let summary: Vec<(&str, bool)> = found.iter().map(|status| (status.tool.key, status.installed)).collect();
        assert_eq!(
            summary,
            [("claude", false), ("pi", false), ("codex", true), ("lazygit", true)]
        );
        assert_eq!(
            found[2].path.as_deref(),
            Some(dir.path().join("bin/codex").to_string_lossy().as_ref())
        );
        assert_eq!(found[0].path, None);
    }

    #[test]
    fn install_returns_the_exit_code_and_one_for_spawn_errors_and_signals() {
        let dir = tempfile::tempdir().expect("tempdir");
        let runtime = runtime();
        let run = |command: &str| {
            let shell = ShellCommand {
                file: "/bin/sh".into(),
                args: vec!["-c".into(), command.into()],
            };
            runtime.block_on(run_shell_command(&shell))
        };
        assert_eq!(run("exit 0"), 0);
        assert_eq!(run("exit 3"), 3);
        assert_eq!(run("kill -9 $$"), 1);
        let missing = ShellCommand {
            file: dir.path().join("no-shell").to_string_lossy().into_owned(),
            args: Vec::new(),
        };
        assert_eq!(runtime.block_on(run_shell_command(&missing)), 1);
    }

    #[test]
    fn everything_installed_stops_after_the_status() {
        let detected = statuses(&["claude", "pi", "codex", "lazygit"]);
        let mut fake = host(vec![detected.clone()]);
        runtime().block_on(run_setup_with(&[], &mut fake));
        assert_eq!(
            fake.out,
            format!(
                "{}Everything Deckhand knows how to check is installed.\n",
                format_status(&detected)
            )
        );
        assert!(fake.prompts.is_empty() && fake.installs.is_empty());
    }

    #[test]
    fn only_lazygit_missing_is_not_installed_by_setup() {
        let detected = statuses(&["claude", "pi", "codex"]);
        let mut fake = host(vec![detected.clone()]);
        runtime().block_on(run_setup_with(&args(&["--yes"]), &mut fake));
        assert_eq!(
            fake.out,
            format!(
                "{}All supported agents are installed. lazygit is optional: the Git tab only needs git, and `o` there opens lazygit if you install it.\n",
                format_status(&detected)
            )
        );
        assert!(fake.installs.is_empty());
    }

    #[test]
    fn check_only_reports_missing_agents_without_installing() {
        let detected = statuses(&["claude"]);
        let mut fake = host(vec![detected.clone()]);
        runtime().block_on(run_setup_with(&args(&["--check", "--yes"]), &mut fake));
        assert_eq!(
            fake.out,
            format!(
                "{}Run `deckhand setup` to install missing agents. lazygit is optional and is not installed by setup.\n",
                format_status(&detected)
            )
        );
        assert!(fake.prompts.is_empty() && fake.installs.is_empty());
    }

    #[test]
    fn installs_each_missing_agent_and_reports_what_is_left() {
        let before = statuses(&["claude"]);
        let after = statuses(&["claude", "codex"]);
        let mut fake = host(vec![before.clone(), after.clone()]);
        fake.codes = vec![7, 0].into();
        runtime().block_on(run_setup_with(&args(&["-y"]), &mut fake));
        assert_eq!(
            fake.prompts,
            [
                ("Install Pi now?".to_owned(), true),
                ("Install Codex now?".to_owned(), true)
            ]
        );
        assert_eq!(
            fake.installs,
            [
                "curl -fsSL https://pi.dev/install.sh | sh",
                "curl -fsSL https://chatgpt.com/codex/install.sh | sh"
            ]
        );
        let expected = format!(
            "{}Pi can be installed with: curl -fsSL https://pi.dev/install.sh | sh\n\
             Pi install failed. You can run this manually: curl -fsSL https://pi.dev/install.sh | sh\n\
             Codex can be installed with: curl -fsSL https://chatgpt.com/codex/install.sh | sh\n\
             \nAfter setup:\n{}\
             Some agents are still missing. Restart your terminal if installs succeeded but are not detected yet.\n\
             lazygit is optional and is not installed by setup: the Git tab only needs git, and `o` there opens lazygit if you install it.\n",
            format_status(&before),
            format_status(&after)
        );
        assert_eq!(fake.out, expected);
    }

    #[test]
    fn declined_installs_are_skipped_and_a_complete_result_says_nothing_more() {
        let before = statuses(&["pi", "lazygit"]);
        let after = statuses(&["claude", "pi", "codex", "lazygit"]);
        let mut fake = host(vec![before.clone(), after.clone()]);
        fake.answers = vec![Some(false), Some(true)].into();
        runtime().block_on(run_setup_with(&[], &mut fake));
        assert_eq!(
            fake.prompts,
            [
                ("Install Claude Code now?".to_owned(), false),
                ("Install Codex now?".to_owned(), false)
            ]
        );
        assert_eq!(fake.installs, ["curl -fsSL https://chatgpt.com/codex/install.sh | sh"]);
        assert!(
            fake.out
                .ends_with(&format!("\nAfter setup:\n{}", format_status(&after)))
        );
    }
}
