//! Stopping a pane kills its process group, but an interactive shell moves jobs to process groups of their own; the
//! workspace worker snapshots the pane's descendants (pid plus start time) before signalling and SIGKILLs the ones
//! still alive afterwards. The start time guards against killing a recycled pid. Everything here is synchronous and
//! bounded (`ps` with a 1 s timeout); a failed `ps` reads as no processes.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use deckhand_core::jsstr::is_js_whitespace;
use nix::sys::signal::Signal;

use crate::proc::kill_pid;

/// TS: processTree.ts ProcessSnapshot (pid → start time as `ps -o lstart=` prints it).
pub type ProcessSnapshot = BTreeMap<u32, String>;

/// TS: processTree.ts ProcessEntry (one `ps -axo pid=,ppid=,lstart=` line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub ppid: u32,
    pub started: String,
}

/// `execFileSync`'s `timeout`.
const PS_TIMEOUT: Duration = Duration::from_millis(1000);
/// `execFileSync`'s default `maxBuffer` (1 MiB): more output fails the call.
const PS_MAX_BUFFER: usize = 1024 * 1024;

/// A JS `.` (anything but a line terminator; `\n` never occurs after the split).
fn is_line_terminator(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `/^\s*(\d+)\s+(\d+)\s+(.+?)\s*$/.exec(line)` (JS `\s`, ASCII `\d`, `.` without line terminators).
fn parse_line(line: &str) -> Option<ProcessEntry> {
    let rest = line.trim_start_matches(is_js_whitespace);
    let pid_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (pid, rest) = rest.split_at(pid_len);
    let after_pid = rest.trim_start_matches(is_js_whitespace);
    if pid.is_empty() || after_pid.len() == rest.len() {
        return None;
    }
    let ppid_len = after_pid.bytes().take_while(u8::is_ascii_digit).count();
    let (ppid, rest) = after_pid.split_at(ppid_len);
    let tail = rest.trim_start_matches(is_js_whitespace);
    let gap = &rest[..rest.len() - tail.len()];
    if ppid.is_empty() || gap.is_empty() {
        return None;
    }
    let started = if tail.is_empty() {
        // Backtracking: `\s+` gives up its last characters until `.+?` can take one (not the gap's first: `\s+`
        // needs at least one).
        gap.chars()
            .skip(1)
            .filter(|ch| !is_line_terminator(*ch))
            .last()?
            .to_string()
    } else {
        // `tail` starts with a non-space, so `.+?` takes it up to the trailing whitespace.
        let started = tail.trim_end_matches(is_js_whitespace);
        if started.chars().any(is_line_terminator) {
            return None;
        }
        started.to_string()
    };
    // `Number(digits)`: pids beyond u32 cannot be signalled anyway.
    Some(ProcessEntry {
        pid: pid.parse().ok()?,
        ppid: ppid.parse().ok()?,
        started,
    })
}

/// The parser of TS: processTree.ts listProcesses (`ps -axo pid=,ppid=,lstart=` output; malformed lines skipped).
pub fn parse_process_list(stdout: &str) -> Vec<ProcessEntry> {
    stdout.split('\n').filter_map(parse_line).collect()
}

/// `execFileSync('ps', ['-axo', 'pid=,ppid=,lstart='], {encoding: 'utf8', timeout: 1000})`: the stdout, or `None`
/// where it throws (spawn failure, timeout, non-zero exit, signal, over `maxBuffer`).
fn run_ps() -> Option<String> {
    let mut child = Command::new("ps")
        .args(["-axo", "pid=,ppid=,lstart="])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    // Drained on a thread so a full pipe cannot stall `ps` past the deadline.
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let read = stdout.by_ref().take(PS_MAX_BUFFER as u64 + 1).read_to_end(&mut out);
        read.map(|_| out)
    });
    let deadline = Instant::now() + PS_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => break None,
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let out = reader.join().ok()?.ok()?;
    if !status.success() || out.len() > PS_MAX_BUFFER {
        return None;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// TS: processTree.ts listProcesses (empty when `ps` fails or takes over 1 s).
pub fn list_processes() -> Vec<ProcessEntry> {
    run_ps().map(|stdout| parse_process_list(&stdout)).unwrap_or_default()
}

/// The pure half of TS: processTree.ts snapshotDescendants (every descendant of `root_pid` in `processes`).
pub fn descendants_of(processes: &[ProcessEntry], root_pid: u32) -> ProcessSnapshot {
    let mut children: HashMap<u32, Vec<&ProcessEntry>> = HashMap::new();
    for entry in processes {
        children.entry(entry.ppid).or_default().push(entry);
    }
    let mut snapshot = ProcessSnapshot::new();
    let mut queue = vec![root_pid];
    while let Some(parent) = queue.pop() {
        for child in children.get(&parent).map(Vec::as_slice).unwrap_or_default() {
            if snapshot.contains_key(&child.pid) {
                continue;
            }
            snapshot.insert(child.pid, child.started.clone());
            queue.push(child.pid);
        }
    }
    snapshot
}

/// TS: processTree.ts snapshotDescendants (must be taken before signalling: once a parent dies its children are
/// reparented and can no longer be found from `root_pid`).
pub fn snapshot_descendants(root_pid: u32) -> ProcessSnapshot {
    descendants_of(&list_processes(), root_pid)
}

/// TS: processTree.ts survivors (the snapshot's processes still running: same pid and start time).
pub fn survivors(snapshot: &ProcessSnapshot) -> ProcessSnapshot {
    if snapshot.is_empty() {
        return snapshot.clone();
    }
    // A Map built from entries: the last entry of a pid wins.
    let current: HashMap<u32, String> = list_processes()
        .into_iter()
        .map(|entry| (entry.pid, entry.started))
        .collect();
    snapshot
        .iter()
        .filter(|(pid, started)| current.get(pid) == Some(started))
        .map(|(pid, started)| (*pid, started.clone()))
        .collect()
}

/// TS: processTree.ts killSurvivors (SIGKILL to each survivor; errors ignored).
pub fn kill_survivors(snapshot: &ProcessSnapshot) {
    for pid in survivors(snapshot).keys() {
        let _ = kill_pid(*pid, Signal::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_testkit::{Case, replay, tags};
    use serde_json::Value;
    use std::path::Path;

    fn sleep_ms(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// `process.kill(pid, 0)` succeeds.
    fn is_alive(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else { return false };
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
    }

    fn command_of(pid: u32) -> String {
        Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_default()
    }

    fn snapshot_arg(case: &Case) -> Result<ProcessSnapshot, String> {
        let Value::Object(map) = case.arg(0) else {
            return Err("expected a $map".into());
        };
        let Some(Value::Array(entries)) = map.get("$map") else {
            return Err("expected a $map".into());
        };
        entries
            .iter()
            .map(|entry| {
                let pid = entry
                    .get(0)
                    .and_then(Value::as_u64)
                    .and_then(|pid| u32::try_from(pid).ok());
                let started = entry.get(1).and_then(Value::as_str);
                match (pid, started) {
                    (Some(pid), Some(started)) => Ok((pid, started.to_string())),
                    _ => Err(format!("bad entry {entry}")),
                }
            })
            .collect()
    }

    fn snapshot_value(snapshot: &ProcessSnapshot) -> Value {
        tags::map(
            snapshot
                .iter()
                .map(|(pid, started)| (Value::from(*pid), Value::from(started.clone()))),
        )
    }

    #[test]
    fn survivors_parity() {
        replay("processTree/survivors", |case: &Case| -> Result<Value, String> {
            Ok(snapshot_value(&survivors(&snapshot_arg(case)?)))
        });
    }

    #[test]
    fn kill_survivors_parity() {
        replay("processTree/killSurvivors", |case: &Case| -> Result<Value, String> {
            kill_survivors(&snapshot_arg(case)?);
            Ok(tags::undefined())
        });
    }

    #[test]
    fn parses_ps_lines_like_the_ts_regex() {
        let out = parse_process_list(
            "    1     0 Mon Jan  5 10:00:00 2026\n  42  1   Tue Jan  6 11:00:00 2026   \nbad line\n\n7 8\n9 10  \n11 12 \u{a0}\n13 14 a\rb\n15 16 x\r",
        );
        let entry = |pid, ppid, started: &str| ProcessEntry {
            pid,
            ppid,
            started: started.into(),
        };
        assert_eq!(
            out,
            vec![
                entry(1, 0, "Mon Jan  5 10:00:00 2026"),
                entry(42, 1, "Tue Jan  6 11:00:00 2026"),
                entry(9, 10, " "),
                entry(11, 12, "\u{a0}"),
                entry(15, 16, "x"),
            ]
        );
    }

    #[test]
    fn descendants_follow_children_transitively() {
        let entry = |pid, ppid| ProcessEntry {
            pid,
            ppid,
            started: format!("s{pid}"),
        };
        let processes = vec![entry(10, 1), entry(11, 10), entry(12, 11), entry(13, 10), entry(20, 1)];
        let snapshot = descendants_of(&processes, 10);
        assert_eq!(snapshot.keys().copied().collect::<Vec<_>>(), vec![11, 12, 13]);
        assert_eq!(snapshot.get(&12).map(String::as_str), Some("s12"));
        assert!(descendants_of(&processes, 99).is_empty());
    }

    /// node-pty's spawn: `program` in a new session with a pty as its controlling terminal.
    fn spawn_in_pty(program: &str, args: &[&str]) -> (std::process::Child, std::os::fd::OwnedFd) {
        use std::os::unix::process::CommandExt;
        let pty = nix::pty::openpty(
            Some(&nix::pty::Winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            }),
            None,
        )
        .expect("openpty");
        let slave = pty.slave;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(Path::new("/tmp"))
            .stdin(Stdio::from(slave.try_clone().expect("dup")))
            .stdout(Stdio::from(slave.try_clone().expect("dup")))
            .stderr(Stdio::from(slave));
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
        let child = command.spawn().expect("spawn in pty");
        // node-pty keeps reading the master; unread output would block the shell's exit (the tty drains on close).
        let mut reader = std::fs::File::from(pty.master.try_clone().expect("dup"));
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while matches!(reader.read(&mut buffer), Ok(read) if read > 0) {}
        });
        (child, pty.master)
    }

    /// `finally { term.kill('SIGKILL') }`, plus the job itself so a failed run leaves nothing behind.
    struct Cleanup {
        term: std::process::Child,
        sleeper: Option<u32>,
    }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.term.kill();
            let _ = self.term.wait();
            if let Some(sleeper) = self.sleeper {
                let _ = kill_pid(sleeper, Signal::SIGKILL);
            }
        }
    }

    #[test]
    fn kills_a_sighup_ignoring_job_in_its_own_process_group() {
        if !Path::new("/bin/zsh").exists() {
            eprintln!("skipped: zsh not installed");
            return;
        }
        // Compound command, so zsh cannot exec it and runs it as a job in a separate process group.
        let (term, _master) = spawn_in_pty(
            "/bin/zsh",
            &["-f", "-ic", "sh -c 'trap \"\" HUP; exec sleep 60'; echo done"],
        );
        let term_pid = term.id();
        let mut cleanup = Cleanup { term, sleeper: None };
        // Poll: under a loaded test run the shell can take well over 500ms to start the job.
        let mut descendants = snapshot_descendants(term_pid);
        let deadline = Instant::now() + Duration::from_millis(5000);
        while cleanup.sleeper.is_none() && Instant::now() < deadline {
            descendants = snapshot_descendants(term_pid);
            cleanup.sleeper = descendants.keys().copied().find(|pid| command_of(*pid) == "sleep 60");
            if cleanup.sleeper.is_none() {
                sleep_ms(100);
            }
        }
        let sleeper = cleanup.sleeper.expect("expected to find the sleep descendant");

        let _ = crate::proc::kill_group(term_pid, Signal::SIGHUP);
        sleep_ms(500);
        assert!(is_alive(sleeper), "the job should survive the group SIGHUP");
        let _ = crate::proc::kill_group(term_pid, Signal::SIGKILL);
        sleep_ms(200);
        assert!(is_alive(sleeper), "the group SIGKILL should not reach the job");

        kill_survivors(&descendants);
        sleep_ms(200);
        assert!(!is_alive(sleeper));
    }

    #[test]
    fn does_not_kill_a_pid_whose_start_time_no_longer_matches() {
        let mut child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        sleep_ms(100);
        kill_survivors(&ProcessSnapshot::from([(
            child.id(),
            "Thu Jan  1 00:00:00 1970".to_string(),
        )]));
        sleep_ms(100);
        let alive = matches!(child.try_wait(), Ok(None));
        let _ = child.kill();
        let _ = child.wait();
        assert!(alive);
    }
}
