//! `deckhand mcp`: the `deckhand` commands as one MCP tool, for Codex (not in the TS).
//!
//! Codex runs shell commands in a sandbox that keeps the CLI from reaching the daemon's socket, but starts MCP
//! servers as its own children, outside it (`deckhand_sys::agent_commands` adds this one at launch). The server
//! speaks MCP over stdio: one JSON-RPC 2.0 message per line on stdin and stdout, logs on stderr. Its one tool,
//! [`TOOL_NAME`], runs this executable with the words it is given (only the agent commands,
//! [`AGENT_WORDS`]), in this process's environment, which Codex forwards from its own (`env_vars`): the caller's
//! `DECKHAND_SESSION_ID`, `DECKHAND_LAUNCH_ID` and `DECKHAND_HOOK_TOKEN`, and the Deckhand home and channel.
//! Messages are handled one at a time, in order.

use std::future::Future;
use std::io::{BufRead, Read, Write};
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use deckhand_core::error::DhError;

use crate::commands::args::AGENT_WORDS;

/// The protocol versions this server speaks, newest first. An `initialize` asking for another gets the newest.
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The tool's name.
pub const TOOL_NAME: &str = "deckhand";

/// The tool's description. Codex keeps it in context in every session, so it stays short.
pub const TOOL_DESCRIPTION: &str = "Run a deckhand command for this Deckhand session, only when the user asks for a \
Deckhand action. args are the command's words, e.g. [\"guide\"], [\"task\", \"add\", \"Fix login\"].";

/// The `args` property's description.
const ARGS_DESCRIPTION: &str = "The words after `deckhand`, one string each (no shell quoting).";

/// The longest one command may run before it is stopped (`new` waits up to 20 s for its worktree). Codex's own
/// limit on a call (`tool_timeout_sec`) is longer.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// The shortest hook token kept out of the tool's output (a UUID is 36 characters).
const MIN_REDACTED_TOKEN: usize = 16;

/// The longest message line read; a longer one is answered as a parse error.
const MAX_LINE_BYTES: usize = 4 << 20;

/// JSON-RPC error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// What a command printed, and how it ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    /// The exit status (none: killed by a signal).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Stopped after [`COMMAND_TIMEOUT`].
    pub timed_out: bool,
}

/// The server: how a command runs (this executable, or a test's stand-in) and what it knows of its session.
pub struct Server<R> {
    run: R,
    /// Whether this process runs for a Deckhand session (`DECKHAND_SESSION_ID` and the rest set).
    in_session: bool,
    /// The hook token, kept out of whatever the tool answers (should a command ever print it).
    token: Option<String>,
}

impl<R, F> Server<R>
where
    R: Fn(Vec<String>) -> F,
    F: Future<Output = std::io::Result<Ran>>,
{
    pub fn new(run: R, in_session: bool, token: Option<String>) -> Self {
        Server {
            run,
            in_session,
            // Deckhand's tokens are UUIDs; a short value would only garble the output.
            token: token.filter(|token| token.len() >= MIN_REDACTED_TOKEN),
        }
    }

    /// Handles one line from stdin: the reply to write, if any (notifications and responses get none).
    pub async fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => {
                log(&format!("unreadable message: {error}"));
                return Some(error_reply(&Value::Null, PARSE_ERROR, "Parse error"));
            }
        };
        let Value::Object(message) = message else {
            return Some(error_reply(
                &Value::Null,
                INVALID_REQUEST,
                "Invalid Request: one JSON-RPC object per line (batches are not supported)",
            ));
        };
        let id = message.get("id");
        let Some(method) = message.get("method") else {
            // A response to a request of ours (this server sends none) or junk: nothing to answer.
            if id.is_none() || !(message.contains_key("result") || message.contains_key("error")) {
                log("ignored a message without a method");
            }
            return None;
        };
        let Some(id) = id else {
            // A notification (`notifications/initialized`, `notifications/cancelled`, ...): never answered.
            return None;
        };
        let (Some(method), true) = (method.as_str(), id.is_string() || id.is_number()) else {
            return Some(error_reply(&Value::Null, INVALID_REQUEST, "Invalid Request"));
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => result_reply(id, initialize_result(&params)),
            "ping" => result_reply(id, json!({})),
            "tools/list" => result_reply(id, json!({ "tools": [tool()] })),
            "tools/call" => match self.call(&params).await {
                Ok(result) => result_reply(id, result),
                Err(message) => error_reply(id, INVALID_PARAMS, &message),
            },
            _ => error_reply(id, METHOD_NOT_FOUND, &format!("Method not found: {method}")),
        })
    }

    /// `tools/call`: an unknown tool is a protocol error; anything wrong with the command is the tool's error, for
    /// the agent to read and correct.
    async fn call(&self, params: &Value) -> Result<Value, String> {
        let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
        if name != TOOL_NAME {
            return Err(format!("Unknown tool: {name}"));
        }
        let args = match command_words(params.get("arguments")) {
            Ok(args) => args,
            Err(message) => return Ok(tool_result(message, true)),
        };
        if !self.in_session {
            return Ok(tool_result(
                "The deckhand tool works only inside a Deckhand session, and DECKHAND_SESSION_ID is not set here."
                    .to_owned(),
                true,
            ));
        }
        let word = args.first().cloned().unwrap_or_default();
        let ran = match (self.run)(args).await {
            Ok(ran) => ran,
            Err(error) => {
                log(&format!("`deckhand {word}` could not run: {error}"));
                return Ok(tool_result(format!("deckhand {word} could not run: {error}"), true));
            }
        };
        log(&format!("`deckhand {word}` ended: {:?}", ran.code));
        let (text, failed) = output_text(&word, &ran);
        let text = match &self.token {
            Some(token) => text.replace(token.as_str(), "<redacted>"),
            None => text,
        };
        Ok(tool_result(text, failed))
    }
}

/// The command words in a call's `arguments` (`{"args": [...]}`): strings only, a leading `deckhand` dropped, the
/// first word one of [`AGENT_WORDS`].
pub fn command_words(arguments: Option<&Value>) -> Result<Vec<String>, String> {
    let words = arguments
        .and_then(|arguments| arguments.get("args"))
        .and_then(Value::as_array)
        .ok_or("args must be an array of strings, e.g. [\"guide\"].")?;
    let mut args = words
        .iter()
        .map(|word| word.as_str().map(str::to_owned))
        .collect::<Option<Vec<String>>>()
        .ok_or("args must be an array of strings, e.g. [\"guide\"].")?;
    if args.first().map(String::as_str) == Some("deckhand") {
        args.remove(0);
    }
    match args.first() {
        None => Err("args is empty: pass the command's words, e.g. [\"guide\"].".to_owned()),
        Some(word) if AGENT_WORDS.contains(&word.as_str()) => Ok(args),
        Some(word) => Err(format!(
            "`deckhand {word}` is not available here. The tool runs: {}.",
            AGENT_WORDS.join(", ")
        )),
    }
}

/// What the tool answers for a command that ran, and whether it failed.
pub fn output_text(word: &str, ran: &Ran) -> (String, bool) {
    if ran.timed_out {
        return (
            format!(
                "deckhand {word} did not finish within {} s and was stopped.",
                COMMAND_TIMEOUT.as_secs()
            ),
            true,
        );
    }
    let parts: Vec<&str> = [ran.stdout.trim_end(), ran.stderr.trim_end()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    let failed = ran.code != Some(0);
    let text = if parts.is_empty() {
        match ran.code {
            Some(0) => "(no output)".to_owned(),
            Some(code) => format!("deckhand {word} failed (exit status {code})."),
            None => format!("deckhand {word} was killed."),
        }
    } else {
        parts.join("\n")
    };
    (text, failed)
}

/// The `initialize` result: the client's protocol version when this server speaks it, else the newest.
fn initialize_result(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "deckhand", "version": deckhand_protocol::BUILD_VERSION },
    })
}

/// The tool's definition. It changes Deckhand's state (it may end a session), so with approval left to the agent
/// it asks first.
pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": ARGS_DESCRIPTION,
                },
            },
            "required": ["args"],
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "openWorldHint": false,
        },
    })
}

fn tool_result(text: String, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

fn result_reply(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_reply(id: &Value, code: i64, message: &str) -> Value {
    let mut error = Map::new();
    error.insert("code".to_owned(), Value::from(code));
    error.insert("message".to_owned(), Value::from(message));
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn log(message: &str) {
    eprintln!("deckhand mcp: {message}");
}

/// Runs `deckhand <args>` (this executable), its output captured, stopped after [`COMMAND_TIMEOUT`].
async fn run_deckhand(args: Vec<String>) -> std::io::Result<Ran> {
    let exe = deckhand_sys::paths::cli_entry_path().map_err(std::io::Error::other)?;
    let child = tokio::process::Command::new(exe)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    // On a timeout the dropped child is killed.
    match tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output()).await {
        Ok(output) => {
            let output = output?;
            Ok(Ran {
                code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                timed_out: false,
            })
        }
        Err(_) => Ok(Ran {
            timed_out: true,
            ..Ran::default()
        }),
    }
}

/// stdin's lines, read by a plain thread (as `hook` does) and sent on as they come; the channel closes at the end
/// of the input. A line past [`MAX_LINE_BYTES`] arrives as `None` (answered as a parse error).
fn stdin_lines() -> mpsc::UnboundedReceiver<Option<String>> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let spawned = std::thread::Builder::new()
        .name("deckhand-mcp-stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut line = Vec::new();
            loop {
                line.clear();
                let limit = u64::try_from(MAX_LINE_BYTES).unwrap_or(u64::MAX).saturating_add(1);
                match Read::take(&mut stdin, limit).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let message = if line.len() > MAX_LINE_BYTES {
                    // Skip the rest of it.
                    if !line.ends_with(b"\n") && stdin.skip_until(b'\n').is_err() {
                        break;
                    }
                    None
                } else {
                    Some(String::from_utf8_lossy(&line).into_owned())
                };
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
    if let Err(error) = spawned {
        log(&format!("could not read stdin: {error}"));
    }
    receiver
}

/// `deckhand mcp`: serves until stdin ends.
pub async fn run() -> Result<ExitCode, DhError> {
    let in_session = crate::commands::caller_from_env().is_some();
    let token = std::env::var("DECKHAND_HOOK_TOKEN").ok();
    let server = Server::new(run_deckhand, in_session, token);
    let mut lines = stdin_lines();
    while let Some(line) = lines.recv().await {
        let reply = match line {
            None => Some(error_reply(&Value::Null, PARSE_ERROR, "Parse error: message too long")),
            Some(line) if line.trim().is_empty() => None,
            Some(line) => server.handle_line(&line).await,
        };
        if let Some(reply) = reply {
            let mut stdout = std::io::stdout().lock();
            // A closed stdout means Codex is gone; stdin ends soon after.
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// A stand-in for running a command.
    type Fake = Box<dyn Fn(Vec<String>) -> std::future::Ready<std::io::Result<Ran>>>;
    /// The commands it was given.
    type Calls = Rc<RefCell<Vec<Vec<String>>>>;

    /// A server whose commands are recorded and answered with `ran` instead of run.
    fn fake_server(in_session: bool, ran: Ran) -> (Server<Fake>, Calls) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let seen = Rc::clone(&calls);
        let run: Fake = Box::new(move |args: Vec<String>| {
            seen.borrow_mut().push(args);
            std::future::ready(Ok(ran.clone()))
        });
        let server = Server::new(run, in_session, Some("6f1c0d2e-secret-token".to_owned()));
        (server, calls)
    }

    fn reply(server: &Server<Fake>, line: &str) -> Option<Value> {
        crate::block_on_local(server.handle_line(line)).unwrap()
    }

    fn call(args: Value) -> String {
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "deckhand", "arguments": {"args": args}}})
            .to_string()
    }

    fn ok(stdout: &str) -> Ran {
        Ran {
            code: Some(0),
            stdout: stdout.to_owned(),
            ..Ran::default()
        }
    }

    #[test]
    fn initialize_answers_the_clients_version_when_spoken_else_the_newest() {
        let (server, _) = fake_server(true, ok(""));
        let initialize = |version: &str| {
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "codex", "version": "0.163.0"}}})
            .to_string()
        };
        let answer = reply(&server, &initialize("2025-06-18")).unwrap();
        assert_eq!(answer["jsonrpc"], "2.0");
        assert_eq!(answer["id"], 1);
        assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(answer["result"]["capabilities"], json!({"tools": {}}));
        assert_eq!(
            answer["result"]["serverInfo"],
            json!({"name": "deckhand", "version": deckhand_protocol::BUILD_VERSION})
        );
        let answer = reply(&server, &initialize("2099-01-01")).unwrap();
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        // The notification that follows gets no reply; nor does any other.
        assert_eq!(
            reply(&server, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            None
        );
        assert_eq!(
            reply(
                &server,
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}"#
            ),
            None
        );
        assert_eq!(
            reply(&server, r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#),
            Some(json!({"jsonrpc": "2.0", "id": "p", "result": {}}))
        );
    }

    #[test]
    fn tools_list_offers_the_one_deckhand_tool() {
        let (server, _) = fake_server(true, ok(""));
        let answer = reply(&server, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#).unwrap();
        let tools = answer["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        let tool = &tools[0];
        assert_eq!(tool["name"], "deckhand");
        assert_eq!(
            tool["inputSchema"],
            json!({"type": "object", "properties": {"args": {"type": "array", "items": {"type": "string"},
                "description": ARGS_DESCRIPTION}}, "required": ["args"]})
        );
        assert_eq!(
            tool["annotations"],
            json!({"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false})
        );
        // In context in every Codex session: kept short.
        assert!(tool.to_string().len() <= 600, "{} bytes", tool.to_string().len());
    }

    #[test]
    fn tools_call_runs_an_agent_command_and_answers_its_output() {
        let (server, calls) = fake_server(true, ok("# deckhand: guide\n"));
        let answer = reply(&server, &call(json!(["guide"]))).unwrap();
        assert_eq!(
            answer["result"],
            json!({"content": [{"type": "text", "text": "# deckhand: guide"}], "isError": false})
        );
        // Words go through as given; a leading `deckhand` is dropped.
        reply(
            &server,
            &call(json!([
                "deckhand",
                "task",
                "add",
                "Fix login",
                "--details",
                "a \"b\"\n$(c)"
            ])),
        );
        assert_eq!(
            *calls.borrow(),
            [
                vec!["guide".to_owned()],
                ["task", "add", "Fix login", "--details", "a \"b\"\n$(c)"]
                    .map(str::to_owned)
                    .to_vec()
            ]
        );
    }

    #[test]
    fn tools_call_refuses_other_commands_as_a_tool_error_without_running_them() {
        let (server, calls) = fake_server(true, ok(""));
        for args in [
            json!(["restart", "--yes"]),
            json!(["setup"]),
            json!(["mcp"]),
            json!(["--daemon"]),
            json!(["hook"]),
            json!([]),
            json!(["deckhand"]),
            json!([1, 2]),
            json!("guide"),
        ] {
            let answer = reply(&server, &call(args.clone())).unwrap();
            assert_eq!(answer["result"]["isError"], true, "{args}: {answer}");
            assert!(answer["result"]["content"][0]["text"].is_string(), "{answer}");
        }
        assert!(calls.borrow().is_empty());
        let refused = reply(&server, &call(json!(["restart"]))).unwrap();
        assert_eq!(
            refused["result"]["content"][0]["text"],
            "`deckhand restart` is not available here. The tool runs: new, ls, task, step, note, kill, remove, guide."
        );
        // Outside a Deckhand session nothing runs.
        let (outside, calls) = fake_server(false, ok(""));
        let answer = reply(&outside, &call(json!(["ls"]))).unwrap();
        assert_eq!(answer["result"]["isError"], true);
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn a_failing_command_is_a_tool_error_with_its_output_and_never_the_token() {
        let failed = Ran {
            code: Some(1),
            stdout: String::new(),
            stderr: "No task matches zz (token 6f1c0d2e-secret-token)\n".to_owned(),
            timed_out: false,
        };
        let (server, _) = fake_server(true, failed);
        let answer = reply(&server, &call(json!(["task", "done", "zz"]))).unwrap();
        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(
            answer["result"]["content"][0]["text"],
            "No task matches zz (token <redacted>)"
        );
        assert_eq!(
            output_text(
                "new",
                &Ran {
                    timed_out: true,
                    ..Ran::default()
                }
            )
            .0,
            "deckhand new did not finish within 60 s and was stopped."
        );
        assert_eq!(
            output_text(
                "kill",
                &Ran {
                    code: Some(2),
                    ..Ran::default()
                }
            ),
            ("deckhand kill failed (exit status 2).".to_owned(), true)
        );
        assert_eq!(output_text("ls", &ok("")), ("(no output)".to_owned(), false));
        let both = Ran {
            code: Some(0),
            stdout: "out\n".into(),
            stderr: "note\n".into(),
            timed_out: false,
        };
        assert_eq!(output_text("ls", &both), ("out\nnote".to_owned(), false));
    }

    #[test]
    fn protocol_errors_get_their_json_rpc_codes_and_the_server_keeps_going() {
        let (server, _) = fake_server(true, ok(""));
        let code =
            |line: &str| reply(&server, line).map(|answer| (answer["id"].clone(), answer["error"]["code"].clone()));
        assert_eq!(code("{not json"), Some((Value::Null, json!(PARSE_ERROR))));
        assert_eq!(
            code(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#),
            Some((Value::Null, json!(INVALID_REQUEST)))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#),
            Some((Value::Null, json!(INVALID_REQUEST)))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":4,"method":5}"#),
            Some((Value::Null, json!(INVALID_REQUEST)))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#),
            Some((json!(4), json!(METHOD_NOT_FOUND)))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":5,"method":"server/discover"}"#),
            Some((json!(5), json!(METHOD_NOT_FOUND)))
        );
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"bash","arguments":{}}}"#),
            Some((json!(6), json!(INVALID_PARAMS)))
        );
        // A response to nothing it sent is ignored.
        assert_eq!(code(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#), None);
        // Still answering.
        assert_eq!(
            code(r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#),
            Some((json!(8), Value::Null))
        );
    }
}
