//! deckhand_term::pty against /bin/sh: output, EOF, exit statuses, resize, signals, session/tty setup, env/cwd,
//! close-on-exec and the drop guard.

use std::ffi::OsString;
use std::time::Duration;

use deckhand_term::pty::{Pty, PtyExit, PtyOptions, RESIZE_ERROR};
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(10);

fn options(cols: u16, rows: u16) -> PtyOptions {
    PtyOptions {
        name: Some("xterm-256color".to_string()),
        cols,
        rows,
        cwd: None,
        env: Some(vec![
            (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
            (OsString::from("LC_ALL"), OsString::from("C")),
        ]),
    }
}

fn sh(script: &str, options: PtyOptions) -> Pty {
    Pty::spawn("/bin/sh", &["-c".to_string(), script.to_string()], options).expect("spawn")
}

/// Reads until EOF, returning everything as (lossy) text.
async fn read_to_eof(pty: &Pty) -> String {
    let mut output = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let read = timeout(LIMIT, pty.read(&mut buf))
            .await
            .expect("read timed out")
            .expect("read");
        if read == 0 {
            break;
        }
        output.extend_from_slice(&buf[..read]);
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// Reads until `needle` shows up.
async fn read_until(pty: &Pty, needle: &str) -> String {
    let mut output = Vec::new();
    let mut buf = [0u8; 4096];
    while !String::from_utf8_lossy(&output).contains(needle) {
        let read = timeout(LIMIT, pty.read(&mut buf))
            .await
            .expect("read timed out")
            .expect("read");
        assert!(
            read > 0,
            "EOF before {needle:?}: {:?}",
            String::from_utf8_lossy(&output)
        );
        output.extend_from_slice(&buf[..read]);
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// Waits for the exit while draining output: on macOS a child cannot finish exiting (it sits in state `E`) until
/// the master has read what it wrote, e.g. the echoed `^C`.
async fn wait_for(pty: &Pty, limit: Duration) -> Option<PtyExit> {
    let deadline = tokio::time::Instant::now() + limit;
    let mut buf = [0u8; 4096];
    let mut eof = false;
    loop {
        tokio::select! {
            exit = pty.wait() => return Some(exit),
            read = pty.read(&mut buf), if !eof => eof = read.map_or(true, |n| n == 0),
            () = tokio::time::sleep_until(deadline) => return None,
        }
    }
}

async fn wait(pty: &Pty) -> PtyExit {
    wait_for(pty, LIMIT).await.expect("exit timed out")
}

fn alive(pid: i32) -> bool {
    nix::sys::signal::kill(Pid::from_raw(pid), None).is_ok()
}

#[tokio::test(flavor = "current_thread")]
async fn echo_reaches_eof_and_exit_zero() {
    let pty = sh("echo hi", options(80, 24));
    assert!(pty.pid() > 0);
    let output = read_to_eof(&pty).await;
    assert_eq!(output, "hi\r\n");
    assert_eq!(
        wait(&pty).await,
        PtyExit {
            exit_code: 0,
            signal: 0
        }
    );
    assert_eq!(
        pty.try_exit(),
        Some(PtyExit {
            exit_code: 0,
            signal: 0
        })
    );
    // EOF is sticky.
    let mut buf = [0u8; 16];
    assert_eq!(pty.read(&mut buf).await.expect("read after eof"), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn exit_code_is_reported() {
    let pty = sh("exit 7", options(80, 24));
    read_to_eof(&pty).await;
    assert_eq!(
        wait(&pty).await,
        PtyExit {
            exit_code: 7,
            signal: 0
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn signal_death_reports_numeric_signal() {
    let pty = sh("kill -9 $$", options(80, 24));
    assert_eq!(
        wait(&pty).await,
        PtyExit {
            exit_code: 0,
            signal: 9
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn missing_command_exits_one() {
    let pty = Pty::spawn("deckhand-no-such-command", &[], options(80, 24)).expect("spawn still succeeds");
    assert_eq!(
        wait(&pty).await,
        PtyExit {
            exit_code: 1,
            signal: 0
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn initial_size_and_resize_reach_the_child() {
    let pty = sh("stty size; read line; stty size", options(100, 30));
    assert_eq!((pty.cols(), pty.rows()), (100, 30));
    read_until(&pty, "30 100").await;
    pty.resize(132, 50).expect("resize");
    assert_eq!((pty.cols(), pty.rows()), (132, 50));
    pty.write(b"\n").await.expect("write");
    let rest = read_to_eof(&pty).await;
    assert!(rest.contains("50 132"), "{rest:?}");
    assert_eq!(wait(&pty).await.exit_code, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn resize_rejects_zero_with_node_pty_message() {
    let pty = sh("exit 0", options(80, 24));
    let error = pty.resize(0, 10).expect_err("zero cols");
    assert_eq!(error.to_string(), RESIZE_ERROR);
    wait(&pty).await;
}

#[tokio::test(flavor = "current_thread")]
async fn default_size_when_zero() {
    let pty = sh("stty size", options(0, 0));
    assert_eq!((pty.cols(), pty.rows()), (80, 24));
    assert!(read_to_eof(&pty).await.contains("24 80"));
}

#[tokio::test(flavor = "current_thread")]
async fn sigwinch_is_delivered_on_resize() {
    let pty = sh(
        "trap 'echo winch; exit 0' WINCH; echo ready; while :; do sleep 0.05; done",
        options(80, 24),
    );
    read_until(&pty, "ready").await;
    pty.resize(81, 24).expect("resize");
    read_until(&pty, "winch").await;
    assert_eq!(wait(&pty).await.exit_code, 0);
}

#[tokio::test(flavor = "current_thread")]
async fn child_leads_its_own_session_with_controlling_tty() {
    let pty = sh(
        "ps -o pid=,pgid=,sess= -p $$; test -t 0 && echo stdin-tty; exec 3</dev/tty && echo ctty-ok",
        options(80, 24),
    );
    let output = read_to_eof(&pty).await;
    let pid = pty.pid();
    let fields: Vec<i32> = output
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|f| f.parse().ok())
        .collect();
    assert_eq!(fields.first(), Some(&pid), "{output:?}");
    assert_eq!(fields.get(1), Some(&pid), "pgid == pid: {output:?}");
    // macOS ps reports the session as a kernel address; check pid == sid where it is a pid.
    #[cfg(target_os = "linux")]
    assert_eq!(fields.get(2), Some(&pid), "sid == pid: {output:?}");
    assert!(output.contains("stdin-tty"), "{output:?}");
    assert!(output.contains("ctty-ok"), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn env_cwd_and_term_are_applied() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().canonicalize().expect("canonical");
    let mut opts = options(80, 24);
    opts.cwd = Some(cwd.clone());
    if let Some(env) = opts.env.as_mut() {
        env.push((OsString::from("DECKHAND_TEST_VAR"), OsString::from("value with spaces")));
        env.push((OsString::from("PWD"), OsString::from("/stale")));
    }
    let pty = sh("echo \"[$TERM][$DECKHAND_TEST_VAR][$PWD]\"; pwd -P", opts);
    let output = read_to_eof(&pty).await;
    let cwd = cwd.to_string_lossy();
    assert!(
        output.contains(&format!("[xterm-256color][value with spaces][{cwd}]")),
        "{output:?}"
    );
    assert!(output.contains(&format!("\r\n{cwd}\r\n")), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn input_is_echoed_and_processed() {
    let pty = sh("read line; echo \"got:$line\"", options(80, 24));
    pty.write("héllo\r".as_bytes()).await.expect("write");
    let output = read_to_eof(&pty).await;
    assert!(output.contains("got:héllo"), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn ctrl_c_interrupts_the_foreground_job() {
    let pty = sh("echo ready; exec sleep 30", options(80, 24));
    read_until(&pty, "ready").await;
    // A ^C that lands while sh is still forking/execing may be absorbed; the foreground job dies on a later one.
    let mut exit = None;
    for _ in 0..50 {
        pty.write(b"\x03").await.expect("write ^C");
        exit = wait_for(&pty, Duration::from_millis(200)).await;
        if exit.is_some() {
            break;
        }
    }
    assert_eq!(
        exit,
        Some(PtyExit {
            exit_code: 0,
            signal: 2
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn kill_group_reaches_background_children() {
    let pty = sh("sleep 30 & echo \"bg:$!\"; wait", options(80, 24));
    let output = read_until(&pty, "\r\n").await;
    let background: i32 = output
        .split("bg:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|pid| pid.parse().ok())
        .expect("background pid");
    assert!(alive(background));
    pty.kill_group(Signal::SIGTERM).expect("killpg");
    assert_eq!(
        wait(&pty).await,
        PtyExit {
            exit_code: 0,
            signal: 15
        }
    );
    for _ in 0..100 {
        if !alive(background) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The background sleep is our grandchild: reparented to init, which reaps it.
    panic!("background child {background} survived killpg");
}

#[tokio::test(flavor = "current_thread")]
async fn kill_defaults_to_sighup_on_the_leader() {
    let pty = sh("echo ready; while :; do sleep 0.05; done", options(80, 24));
    read_until(&pty, "ready").await;
    pty.kill(None).expect("kill");
    assert_eq!(wait(&pty).await.signal, Signal::SIGHUP as i32);
}

#[tokio::test(flavor = "current_thread")]
async fn master_and_slave_do_not_leak_into_the_child() {
    let probe = Pty::spawn("/bin/sh", &["-c".into(), "sleep 30".into()], options(80, 24)).expect("probe");
    let leaked = probe.master_fd();
    let pty = sh(
        &format!("if [ -e /dev/fd/{leaked} ]; then echo leaked; else echo clean; fi"),
        options(80, 24),
    );
    let output = read_to_eof(&pty).await;
    assert!(output.contains("clean"), "fd {leaked}: {output:?}");
    let own = pty.master_fd();
    let pty2 = sh(
        &format!("ls /dev/fd/ | tr '\\n' ' '; echo; [ -e /dev/fd/{own} ] && echo own-leaked"),
        options(80, 24),
    );
    let output = read_to_eof(&pty2).await;
    assert!(!output.contains("own-leaked"), "{output:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn drop_kills_the_process_group() {
    let pty = sh("sleep 30 & echo \"bg:$!\"; wait", options(80, 24));
    let output = read_until(&pty, "\r\n").await;
    let background: i32 = output
        .split("bg:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|pid| pid.parse().ok())
        .expect("background pid");
    let leader = pty.pid();
    drop(pty);
    for _ in 0..200 {
        if !alive(leader) && !alive(background) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("leader {leader} or background {background} survived drop");
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_reader_and_writer_share_the_pty() {
    let pty = std::rc::Rc::new(sh(
        "while read line; do echo \"<$line>\"; [ \"$line\" = q ] && exit 3; done",
        options(80, 24),
    ));
    let local = tokio::task::LocalSet::new();
    let writer = pty.clone();
    let output = local
        .run_until(async move {
            let write = tokio::task::spawn_local(async move {
                for line in ["a", "b", "q"] {
                    writer.write(format!("{line}\r").as_bytes()).await.expect("write");
                }
            });
            let text = read_to_eof(&pty).await;
            write.await.expect("writer task");
            (text, wait(&pty).await)
        })
        .await;
    assert!(
        output.0.contains("<a>") && output.0.contains("<b>") && output.0.contains("<q>"),
        "{:?}",
        output.0
    );
    assert_eq!(output.1.exit_code, 3);
}

#[tokio::test(flavor = "current_thread")]
async fn large_output_is_fully_read() {
    let pty = sh(
        "i=0; while [ $i -lt 2000 ]; do echo line-$i; i=$((i+1)); done",
        options(80, 24),
    );
    let output = read_to_eof(&pty).await;
    assert_eq!(output.lines().count(), 2000);
    assert!(output.ends_with("line-1999\r\n"));
}
