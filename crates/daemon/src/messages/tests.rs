use super::*;

use std::cell::RefCell;
use std::path::Path;

use deckhand_protocol::Ordered;
use deckhand_protocol::wire::{AgentCaller, AgentHook};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::hooks::tests::Home;
use crate::launch::Draft;
use crate::server::tests::{hello_line, local, session, test_client, test_daemon};
use crate::server::{Client, ClientId};
use crate::worker_channel::{ChannelEvents, ProcessExit, WorkerChannel, WorkerKind};

/// The other end of a session's fake worker: what the daemon types into its agent.
struct Fake {
    theirs: BufReader<UnixStream>,
    _events: ChannelEvents,
}

impl Fake {
    /// The next `input` the daemon sent, within 2 s.
    async fn typed(&mut self) -> String {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(2), self.theirs.read_line(&mut line))
            .await
            .expect("something was typed")
            .unwrap();
        let event: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["type"], "input", "{event}");
        assert_eq!(event["target"], "agent", "{event}");
        event["data"].as_str().unwrap_or_default().to_owned()
    }

    /// A message: its paste, then the Enter that submits it. The pasted text.
    async fn delivered(&mut self) -> String {
        let pasted = self.typed().await;
        assert_eq!(self.typed().await, "\r");
        pasted
            .strip_prefix("\x1b[200~")
            .and_then(|text| text.strip_suffix("\x1b[201~"))
            .unwrap_or_else(|| panic!("one bracketed paste: {pasted:?}"))
            .to_owned()
    }

    /// Nothing is typed for a while.
    async fn nothing_typed(&mut self) {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_millis(300), self.theirs.read_line(&mut line)).await;
        assert!(read.is_err(), "typed: {line}");
    }
}

/// Session `id` (titled `title`, under `parent`), running at launch `launch-<id>` with token `token-<id>`, idle at
/// its input, and its fake worker.
async fn running(daemon: &Daemon, id: &str, title: &str, parent: Option<&str>) -> (Rc<WorkerRuntime>, Fake) {
    let mut record = session(id, "/r", "/r");
    record.title = title.to_owned();
    record.launch_id = Some(format!("launch-{id}"));
    record.parent_session_id = parent.map(str::to_owned);
    record.agent_status = Some(AgentActivityStatus::Idle);
    daemon.set_session(record);
    let (ours, theirs) = UnixStream::pair().unwrap();
    let (channel, events) =
        WorkerChannel::attach(WorkerKind::Session, None, ours, std::future::pending::<ProcessExit>());
    let mut theirs = BufReader::new(theirs);
    theirs
        .get_mut()
        .write_all(format!("{}\n", hello_line()).as_bytes())
        .await
        .unwrap();
    let worker = Rc::new(WorkerRuntime {
        channel,
        attached_agent: RefCell::new(None),
        delete_worktree_on_exit: Cell::new(false),
        delete_branch_on_exit: Cell::new(false),
        hook_token: format!("token-{id}"),
        launch_id: format!("launch-{id}"),
        allow_data_loss: Cell::new(false),
        screen: RefCell::new(Some("╭───╮\n│ > │\n╰───╯".into())),
        draft: RefCell::new(None),
        last_hook_at: Cell::new(None),
        preview_styled: Cell::new(false),
        working_settle: RefCell::new(None),
        inbox: Inbox::default(),
    });
    daemon.launch.borrow_mut().workers.insert(id.into(), Rc::clone(&worker));
    (
        worker,
        Fake {
            theirs,
            _events: events,
        },
    )
}

/// A session of the list that is not running (no worker).
fn stopped(daemon: &Daemon, id: &str, title: &str, parent: Option<&str>, status: SessionStatus) {
    let mut record = session(id, "/r", "/r");
    record.title = title.to_owned();
    record.parent_session_id = parent.map(str::to_owned);
    record.status = status;
    daemon.set_session(record);
}

fn daemon_in(dir: &Path) -> Daemon {
    let daemon = test_daemon(dir);
    daemon.notes_tasks.borrow_mut().notes = None;
    daemon
}

fn caller(id: &str) -> AgentCaller {
    AgentCaller {
        session_id: id.to_owned(),
        launch_id: format!("launch-{id}"),
        token: format!("token-{id}"),
    }
}

fn send(to: &str, text: &str, from: Option<&str>, wait: Option<i64>) -> SendMessage {
    SendMessage {
        request_id: "r".into(),
        session_id: to.to_owned(),
        text: Some(text.to_owned()),
        wait: wait.map(|_| true),
        timeout_ms: wait,
        caller: from.map(caller),
    }
}

fn reply(id: &str, text: &str, from: &str) -> ReplyMessage {
    ReplyMessage {
        request_id: "r".into(),
        message_id: id.to_owned(),
        text: Some(text.to_owned()),
        caller: Some(caller(from)),
    }
}

/// A lifecycle hook from session `id`'s agent.
async fn hook(daemon: &Daemon, id: &str, payload: Value) {
    daemon
        .handle_agent_hook(&AgentHook {
            request_id: "h".into(),
            session_id: id.to_owned(),
            launch_id: format!("launch-{id}"),
            token: format!("token-{id}"),
            payload,
            sent_at: None,
        })
        .await
        .unwrap();
}

async fn send_ok(daemon: &Daemon, request: SendMessage) -> SendMessageResult {
    let (client, _rx) = test_client(9);
    daemon.handle_send_message(&client, &request).await.unwrap()
}

async fn send_refused(daemon: &Daemon, request: SendMessage) -> String {
    let (client, _rx) = test_client(9);
    daemon
        .handle_send_message(&client, &request)
        .await
        .unwrap_err()
        .to_string()
}

/// `send --wait` running in the background (the test acts meanwhile, then awaits it).
fn waiting(daemon: &Daemon, request: SendMessage) -> deckhand_sys::async_util::Task<Result<SendMessageResult>> {
    let daemon = daemon.clone();
    spawn(async move {
        let (client, _rx) = test_client(8);
        daemon.handle_send_message(&client, &request).await
    })
}

/// Lets spawned work (deliveries, saves) run.
async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

fn status_of(daemon: &Daemon, id: &str) -> MessageStatus {
    daemon.message(id).unwrap().status
}

fn set_attention(daemon: &Daemon, id: &str, state: AttentionState) {
    daemon.patch_session(id, |session| {
        session.touch("attention");
        session.attention = Some(
            serde_json::from_value(json!({"state": state.as_str(), "event": "Seed", "at": "2026-01-01T00:00:00.000Z"}))
                .unwrap(),
        );
    });
}

fn record(id: &str, from: Option<&str>, to: &str, status: MessageStatus) -> MessageRecord {
    MessageRecord {
        id: id.to_owned(),
        kind: MessageKind::Message,
        from_session_id: from.map(str::to_owned),
        from_title: from.map(str::to_owned),
        to_session_id: Some(to.to_owned()),
        to_title: Some(to.to_owned()),
        text: "hi".into(),
        text_truncated: None,
        wants_answer: None,
        in_reply_to: None,
        replies: None,
        status,
        created_at: "2026-01-01T00:00:00.000Z".into(),
        delivered_at: None,
        updated_at: "2026-01-01T00:00:00.000Z".into(),
        note: None,
        extra: IndexMap::new(),
    }
}

#[test]
fn headers_say_who_sent_it_and_how_to_answer() {
    let mut message = record("7a3f01", Some("a"), "b", MessageStatus::Held);
    message.from_title = Some("Login UI".into());
    message.text = "Which token store?".into();
    assert_eq!(
        rendered(&message),
        "[Message 7a3f01 from \"Login UI\" via deckhand]\nWhich token store?"
    );
    message.wants_answer = Some(true);
    assert_eq!(
        rendered(&message),
        "[Message 7a3f01 from \"Login UI\", which wants an answer.\n Reply once you have it: deckhand reply 7a3f01 \
         \"<answer>\"]\nWhich token store?"
    );
    let mut by_hand = record("7a3f02", None, "b", MessageStatus::Held);
    by_hand.text = "Rebase first".into();
    assert_eq!(
        rendered(&by_hand),
        "[Message 7a3f02 from you (the user) via deckhand]\nRebase first"
    );
    let mut answer = record("9c0d11", Some("b"), "a", MessageStatus::Held);
    answer.kind = MessageKind::Reply;
    answer.from_title = Some("Token storage".into());
    answer.in_reply_to = Some("7a3f01".into());
    answer.text = "The keychain".into();
    assert_eq!(
        rendered(&answer),
        "[Reply from \"Token storage\" to your message 7a3f01]\nThe keychain"
    );
    let mut notice = record("5e6f22", None, "a", MessageStatus::Held);
    notice.kind = MessageKind::Notice;
    notice.text = "[Not delivered: \"Token storage\" exited]\nYour message 7a3f01: Which token store?".into();
    assert_eq!(rendered(&notice), notice.text);
}

#[test]
fn typed_text_holds_the_queue_until_it_is_sent_or_cleared() {
    assert_eq!(typed_effect("fix the"), Some(true));
    assert_eq!(typed_effect("\x1b[200~pasted\rlines\x1b[201~"), Some(true));
    assert_eq!(typed_effect("fix it\r"), Some(false));
    assert_eq!(typed_effect("\r"), Some(false));
    assert_eq!(typed_effect("\x03"), Some(false));
    assert_eq!(typed_effect("\x15"), Some(false));
    assert_eq!(typed_effect("\rnext"), Some(true));
    // Moving the cursor or a digit answering a menu leaves the input as it was.
    assert_eq!(typed_effect("\x1b[A"), None);
    assert_eq!(typed_effect("2"), None);
}

#[test]
fn the_log_keeps_held_messages_and_the_last_50_of_each_session() {
    let known: HashSet<String> = ["a", "b", "c"].map(str::to_owned).into_iter().collect();
    let mut messages: Vec<MessageRecord> = (0..60)
        .map(|n| record(&format!("{n:06}"), Some("a"), "b", MessageStatus::Delivered))
        .collect();
    messages.insert(0, record("held00", Some("c"), "b", MessageStatus::Held));
    messages.push(record("gone00", Some("x"), "y", MessageStatus::Delivered));
    prune(&mut messages, &known);
    let ids: Vec<&str> = messages.iter().map(|message| message.id.as_str()).collect();
    // The held one stays however old; of a's and b's, the newest 50; one of no known session goes.
    assert_eq!(ids.first(), Some(&"held00"));
    assert_eq!(ids.len(), 51, "{ids:?}");
    assert_eq!(ids.get(1), Some(&"000010"));
    assert!(!ids.contains(&"gone00"));
}

#[test]
fn messages_to_a_session_are_typed_one_at_a_time_in_the_order_sent() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, _) = running(&daemon, "a", "Login UI", Some("o")).await;
        let (b, mut fake) = running(&daemon, "b", "Token storage", Some("o")).await;
        stopped(&daemon, "o", "Orchestrator", None, SessionStatus::Running);
        b.inbox.signals.set(true);
        let first = send_ok(&daemon, send("b", "First", Some("a"), None)).await.message;
        let second = send_ok(&daemon, send("b", "Second", None, None)).await.message;
        // The first is typed at once; the second waits for the turn the first starts to end.
        assert!(fake.delivered().await.ends_with("\nFirst"));
        assert_eq!(status_of(&daemon, &first.id), MessageStatus::Delivered);
        assert_eq!(status_of(&daemon, &second.id), MessageStatus::Held);
        hook(&daemon, "b", json!({"hook_event_name": "UserPromptSubmit"})).await;
        fake.nothing_typed().await;
        hook(&daemon, "b", json!({"hook_event_name": "Stop"})).await;
        assert_eq!(
            fake.delivered().await,
            format!("[Message {} from you (the user) via deckhand]\nSecond", second.id)
        );
        assert_eq!(status_of(&daemon, &second.id), MessageStatus::Delivered);
    });
}

#[test]
fn an_idle_target_gets_one_paste_under_its_header_then_enter() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, _) = running(&daemon, "a", "Login UI", None).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        let (client, mut rx) = test_client(1);
        daemon.server.borrow_mut().clients.insert(
            ClientId(1),
            Client {
                handle: client,
                subscription: ClientSubscription {
                    repo_root: Some("/r".into()),
                    ..ClientSubscription::default()
                },
            },
        );
        let sent = send_ok(
            &daemon,
            send("b", "Store tokens in the keychain.\nSay when done.", Some("a"), None),
        )
        .await;
        let id = sent.message.id.clone();
        assert_eq!(id.len(), 6);
        assert_eq!(
            fake.delivered().await,
            format!("[Message {id} from \"Login UI\" via deckhand]\nStore tokens in the keychain.\nSay when done.")
        );
        let message = daemon.message(&id).unwrap();
        assert_eq!(message.status, MessageStatus::Delivered);
        assert!(message.delivered_at.is_some());
        // The UIs of the repository heard of it, queued then delivered.
        let mut updates = Vec::new();
        while let Ok(line) = rx.try_recv() {
            let event: Value = serde_json::from_str(&line).unwrap();
            if event["type"] == "messages-updated" {
                updates.push(event["messages"][0]["status"].as_str().unwrap_or_default().to_owned());
            }
        }
        assert_eq!(updates, ["held", "delivered"]);
        // And the file has it.
        daemon.save_messages().await.unwrap();
        let file: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(MESSAGES_FILE)).unwrap()).unwrap();
        assert_eq!(file["messages"][0]["id"], id.as_str());
    });
}

#[test]
fn a_message_is_held_while_its_target_starts_works_waits_or_you_type() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (b, mut fake) = running(&daemon, "b", "Token storage", None).await;
        // Starting.
        daemon.patch_session("b", |session| session.status = SessionStatus::Starting);
        let id = send_ok(&daemon, send("b", "Hello", None, None)).await.message.id;
        fake.nothing_typed().await;
        daemon.patch_session("b", |session| session.status = SessionStatus::Running);
        // Working on a turn (a signal, or a screen that keeps changing).
        set_attention(&daemon, "b", AttentionState::Working);
        daemon.pump_messages("b");
        fake.nothing_typed().await;
        set_attention(&daemon, "b", AttentionState::Unknown);
        daemon.patch_session("b", |session| session.agent_status = Some(AgentActivityStatus::Active));
        daemon.pump_messages("b");
        fake.nothing_typed().await;
        daemon.patch_session("b", |session| session.agent_status = Some(AgentActivityStatus::Idle));
        // Waiting on a question or permission: never typed into a dialog.
        set_attention(&daemon, "b", AttentionState::NeedsInput);
        daemon.pump_messages("b");
        fake.nothing_typed().await;
        set_attention(&daemon, "b", AttentionState::ResponseEnded);
        // A menu on the screen.
        *b.screen.borrow_mut() = Some("Do you trust the files in this folder?\n❯ 1. Yes".into());
        daemon.pump_messages("b");
        fake.nothing_typed().await;
        *b.screen.borrow_mut() = Some("> ".into());
        // A task draft waiting to be typed.
        *b.draft.borrow_mut() = Some(Draft {
            text: "Task".into(),
            timer: Timer::after(Duration::from_secs(60), || {}),
            settle: None,
        });
        daemon.pump_messages("b");
        fake.nothing_typed().await;
        *b.draft.borrow_mut() = None;
        // You typed text you have not sent: held until Enter, and for a moment after any key.
        daemon.messages_on_input("b", "half a sente");
        b.inbox.typed_at.set(None);
        daemon.pump_messages("b");
        fake.nothing_typed().await;
        assert_eq!(status_of(&daemon, &id), MessageStatus::Held);
        daemon.messages_on_input("b", "nce\r");
        assert!(!b.inbox.user_text.get());
        fake.nothing_typed().await;
        // Once the typing pause is over, it is typed.
        b.inbox.typed_at.set(Some(Instant::now() - TYPING_QUIET));
        daemon.pump_messages("b");
        assert!(fake.delivered().await.ends_with("\nHello"));
        assert_eq!(status_of(&daemon, &id), MessageStatus::Delivered);
    });
}

#[test]
fn a_session_messages_only_its_tree_and_never_itself() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        // root ─ a, b ─ c; another tree: t ─ u.
        stopped(&daemon, "root", "Orchestrator", None, SessionStatus::Running);
        let (_a, _) = running(&daemon, "a", "Login UI", Some("root")).await;
        stopped(&daemon, "b", "Token storage", Some("root"), SessionStatus::Starting);
        stopped(&daemon, "c", "Keychain", Some("b"), SessionStatus::Starting);
        stopped(&daemon, "t", "Other idea", None, SessionStatus::Starting);
        stopped(&daemon, "u", "Other work", Some("t"), SessionStatus::Starting);
        for to in ["root", "b", "c"] {
            send_ok(&daemon, send(to, "hi", Some("a"), None)).await;
        }
        for to in ["t", "u"] {
            assert_eq!(
                send_refused(&daemon, send(to, "hi", Some("a"), None)).await,
                NOT_IN_TREE
            );
        }
        assert_eq!(send_refused(&daemon, send("a", "hi", Some("a"), None)).await, TO_ITSELF);
        // A stale caller is refused before anything else.
        let mut stale = send("b", "hi", Some("a"), None);
        stale.caller = Some(AgentCaller {
            token: "other".into(),
            ..caller("a")
        });
        assert_eq!(send_refused(&daemon, stale).await, crate::hooks::STALE_CALLER);
        // By hand: any session.
        send_ok(&daemon, send("u", "hi", None, None)).await;
        // An exited target is refused; nothing is queued.
        stopped(&daemon, "x", "Done", Some("root"), SessionStatus::Exited);
        assert_eq!(
            send_refused(&daemon, send("x", "hi", Some("a"), None)).await,
            "\"Done\" has exited: nothing was sent"
        );
        assert_eq!(daemon.messages.borrow().messages.len(), 4);
    });
}

#[test]
fn a_message_past_20_kb_is_refused_with_a_hint() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        stopped(&daemon, "b", "Token storage", None, SessionStatus::Starting);
        let long = "x".repeat(MAX_MESSAGE_BYTES + 1);
        let refused = send_refused(&daemon, send("b", &long, None, None)).await;
        assert!(
            refused.starts_with("The message is 21 KB; at most 20 KB fits."),
            "{refused}"
        );
        assert!(refused.contains("deckhand note add"), "{refused}");
        assert_eq!(
            send_refused(&daemon, send("b", " \n", None, None)).await,
            "The message is empty"
        );
        send_ok(&daemon, send("b", &"x".repeat(MAX_MESSAGE_BYTES), None, None)).await;
    });
}

#[test]
fn a_reply_goes_to_the_waiting_send_and_is_not_typed_into_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        stopped(&daemon, "o", "Orchestrator", None, SessionStatus::Running);
        let (_a, mut sender) = running(&daemon, "a", "Login UI", Some("o")).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("o")).await;
        let waited = waiting(&daemon, send("b", "Which store?", Some("a"), Some(60_000)));
        let typed = fake.delivered().await;
        let id = daemon.messages.borrow().messages[0].id.clone();
        assert!(typed.starts_with(&format!("[Message {id} from \"Login UI\", which wants an answer.")));
        let answer = daemon
            .handle_reply_message(&reply(&id, "The keychain", "b"))
            .await
            .unwrap();
        assert_eq!(answer.status, MessageStatus::Returned);
        let result = waited.await.unwrap();
        let outcome = result.outcome.unwrap();
        assert_eq!(outcome.kind, WaitOutcomeKind::Replied);
        assert_eq!(outcome.reply.unwrap().text, "The keychain");
        assert_eq!(result.message.status, MessageStatus::Answered);
        assert_eq!(result.message.replies, Some(vec![answer.id.clone()]));
        // Not typed into the sender as well.
        sender.nothing_typed().await;
        // A second reply has no waiting command left: it is queued to the sender and typed in.
        let again = daemon
            .handle_reply_message(&reply(&id[..4], "Or a file", "b"))
            .await
            .unwrap();
        assert_eq!(
            sender.delivered().await,
            format!("[Reply from \"Token storage\" to your message {id}]\nOr a file")
        );
        assert_eq!(status_of(&daemon, &again.id), MessageStatus::Delivered);
        assert_eq!(daemon.message(&id).unwrap().replies.unwrap().len(), 2);
    });
}

#[test]
fn a_reply_without_a_waiting_send_is_queued_to_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (a, mut sender) = running(&daemon, "a", "Login UI", None).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        let id = send_ok(&daemon, send("b", "Tell me when done", Some("a"), None))
            .await
            .message
            .id;
        fake.delivered().await;
        // The sender works on something else: the reply waits for it.
        set_attention(&daemon, "a", AttentionState::Working);
        let answer = daemon.handle_reply_message(&reply(&id, "Done", "b")).await.unwrap();
        assert_eq!(answer.status, MessageStatus::Held);
        sender.nothing_typed().await;
        a.inbox.signals.set(true);
        hook(&daemon, "a", json!({"hook_event_name": "Stop"})).await;
        assert_eq!(
            sender.delivered().await,
            format!("[Reply from \"Token storage\" to your message {id}]\nDone")
        );
    });
}

#[test]
fn only_the_session_a_message_was_sent_to_may_reply() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, _) = running(&daemon, "a", "Login UI", None).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        let (_c, _) = running(&daemon, "c", "Docs", Some("a")).await;
        let id = send_ok(&daemon, send("b", "Question", Some("a"), None))
            .await
            .message
            .id;
        fake.delivered().await;
        let refused = daemon.handle_reply_message(&reply(&id, "Me!", "c")).await.unwrap_err();
        assert!(
            refused
                .to_string()
                .starts_with(&format!("Message {id} was not sent to this session"))
        );
        let mut by_hand = reply(&id, "Me!", "b");
        by_hand.caller = None;
        let refused = daemon.handle_reply_message(&by_hand).await.unwrap_err();
        assert_eq!(refused.to_string(), REPLY_BY_HAND);
        let refused = daemon
            .handle_reply_message(&reply("zzzz", "Me!", "b"))
            .await
            .unwrap_err();
        assert_eq!(refused.to_string(), "No message zzzz");
        // A held message has not reached it.
        set_attention(&daemon, "b", AttentionState::Working);
        let held = send_ok(&daemon, send("b", "Later", Some("a"), None)).await.message.id;
        let refused = daemon
            .handle_reply_message(&reply(&held, "Early", "b"))
            .await
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            format!("Message {held} has not reached this session")
        );
        daemon.handle_reply_message(&reply(&id, "Fine", "b")).await.unwrap();
    });
}

#[test]
fn a_reply_to_a_message_sent_by_hand_goes_back_to_the_user() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_b, mut fake) = running(&daemon, "b", "Token storage", None).await;
        let id = send_ok(&daemon, send("b", "Status?", None, None)).await.message.id;
        fake.delivered().await;
        let answer = daemon.handle_reply_message(&reply(&id, "Halfway", "b")).await.unwrap();
        assert_eq!(answer.status, MessageStatus::Returned);
        assert_eq!(answer.to_session_id, None);
        assert_eq!(status_of(&daemon, &id), MessageStatus::Answered);
    });
}

#[test]
fn send_wait_returns_early_when_the_target_waits_on_a_question() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, _) = running(&daemon, "a", "Login UI", None).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        // Already waiting when it is sent: told at once; the message stays queued.
        hook(
            &daemon,
            "b",
            json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}),
        )
        .await;
        let result = send_ok(&daemon, send("b", "Hi", Some("a"), Some(60_000))).await;
        let outcome = result.outcome.unwrap();
        assert_eq!(outcome.kind, WaitOutcomeKind::Waiting);
        assert_eq!(outcome.reason.as_deref(), Some("wants to run: rm -rf build"));
        assert_eq!(result.message.status, MessageStatus::Held);
        fake.nothing_typed().await;
        // Asked after the message was typed: the wait ends with what it asks.
        hook(&daemon, "b", json!({"hook_event_name": "UserPromptSubmit"})).await;
        hook(&daemon, "b", json!({"hook_event_name": "Stop"})).await;
        fake.delivered().await;
        let waited = waiting(&daemon, send("b", "Another", Some("a"), Some(60_000)));
        settle().await;
        hook(&daemon, "b", json!({"hook_event_name": "UserPromptSubmit"})).await;
        hook(&daemon, "b", json!({"hook_event_name": "Stop"})).await;
        fake.delivered().await;
        hook(&daemon, "b", json!({"hook_event_name": "UserPromptSubmit"})).await;
        hook(
            &daemon,
            "b",
            json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
                "tool_input": {"questions": [{"question": "Which store?", "options": [{"label": "Keychain"}]}]}}),
        )
        .await;
        let outcome = waited.await.unwrap().outcome.unwrap();
        assert_eq!(outcome.kind, WaitOutcomeKind::Waiting);
        assert!(outcome.reason.unwrap_or_default().contains("Which store?"));
    });
}

#[test]
fn send_wait_returns_when_the_target_ends_its_turn_without_replying() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, _) = running(&daemon, "a", "Login UI", None).await;
        let (b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        b.inbox.signals.set(true);
        let waited = waiting(&daemon, send("b", "Which store?", Some("a"), Some(60_000)));
        fake.delivered().await;
        hook(&daemon, "b", json!({"hook_event_name": "UserPromptSubmit"})).await;
        hook(&daemon, "b", json!({"hook_event_name": "Stop"})).await;
        let result = waited.await.unwrap();
        assert_eq!(result.outcome.unwrap().kind, WaitOutcomeKind::Ended);
        assert_eq!(result.message.status, MessageStatus::Delivered);
    });
}

#[test]
fn without_turn_signals_an_ended_turn_is_not_taken_for_no_reply() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, _) = running(&daemon, "a", "Login UI", None).await;
        let (b, mut fake) = running(&daemon, "b", "Pi helper", Some("a")).await;
        let waited = waiting(&daemon, send("b", "Which store?", Some("a"), Some(1_000)));
        fake.delivered().await;
        // Activity stands in for the turn: it starts and ends, and the wait goes on to its timeout.
        daemon
            .set_worker_agent_status("b", AgentActivityStatus::Active)
            .await
            .unwrap();
        assert!(b.inbox.turn.borrow().as_ref().is_some_and(|turn| turn.started));
        daemon
            .set_worker_agent_status("b", AgentActivityStatus::Idle)
            .await
            .unwrap();
        assert!(b.inbox.turn.borrow().is_none());
        assert_eq!(waited.await.unwrap().outcome.unwrap().kind, WaitOutcomeKind::Timeout);
    });
}

#[test]
fn send_wait_returns_when_the_target_exits_and_a_held_message_is_dropped_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, mut sender) = running(&daemon, "a", "Login UI", None).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        set_attention(&daemon, "b", AttentionState::Working);
        let waited = waiting(&daemon, send("b", "Waited for", Some("a"), Some(60_000)));
        settle().await;
        let queued = send_ok(&daemon, send("b", "Queued and forgotten", Some("a"), None))
            .await
            .message
            .id;
        fake.nothing_typed().await;
        daemon.handle_worker_session_exit("b", Some(0), None, "").await.unwrap();
        // The waiting command learns it exited; no notice for that one.
        let result = waited.await.unwrap();
        assert_eq!(result.outcome.unwrap().kind, WaitOutcomeKind::Exited);
        assert_eq!(result.message.status, MessageStatus::Dropped);
        assert_eq!(status_of(&daemon, &queued), MessageStatus::Dropped);
        assert_eq!(
            daemon.message(&queued).unwrap().note.as_deref(),
            Some("\"Token storage\" exited before it was delivered")
        );
        // The other's sender gets a notice in its queue.
        assert_eq!(
            sender.delivered().await,
            format!("[Not delivered: \"Token storage\" exited]\nYour message {queued}: Queued and forgotten")
        );
        // A later message to it is refused.
        assert!(
            send_refused(&daemon, send("b", "Again", Some("a"), None))
                .await
                .contains("has exited")
        );
    });
}

#[test]
fn send_wait_times_out_and_a_later_reply_is_queued_to_the_sender() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, mut sender) = running(&daemon, "a", "Login UI", None).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        let waited = waiting(&daemon, send("b", "Which store?", Some("a"), Some(300)));
        fake.delivered().await;
        let result = waited.await.unwrap();
        assert_eq!(result.outcome.unwrap().kind, WaitOutcomeKind::Timeout);
        assert!(daemon.messages.borrow().waiters.is_empty());
        let id = result.message.id;
        // Queued to the sender (and typed in at once, as it is idle).
        let answer = daemon.handle_reply_message(&reply(&id, "Keychain", "b")).await.unwrap();
        assert_ne!(answer.status, MessageStatus::Returned);
        assert_eq!(
            sender.delivered().await,
            format!("[Reply from \"Token storage\" to your message {id}]\nKeychain")
        );
    });
}

#[test]
fn a_waiting_command_that_went_away_leaves_later_replies_to_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, mut sender) = running(&daemon, "a", "Login UI", None).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        let (client, _rx) = test_client(3);
        let gone = client.clone();
        let task = {
            let daemon = daemon.clone();
            spawn(async move {
                daemon
                    .handle_send_message(&client, &send("b", "Which store?", Some("a"), Some(60_000)))
                    .await
            })
        };
        fake.delivered().await;
        gone.destroy();
        let id = daemon.messages.borrow().messages[0].id.clone();
        let answer = daemon.handle_reply_message(&reply(&id, "Keychain", "b")).await.unwrap();
        assert_ne!(answer.status, MessageStatus::Returned);
        sender.delivered().await;
        assert!(task.await.is_err());
    });
}

#[test]
fn no_turn_after_a_delivery_stops_the_queue_and_tells_the_waiting_sender() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        daemon.messages.borrow_mut().turn_start_wait = Duration::from_millis(200);
        let (_a, _) = running(&daemon, "a", "Login UI", None).await;
        let (b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        b.inbox.signals.set(true);
        let waited = waiting(&daemon, send("b", "First", Some("a"), Some(60_000)));
        fake.delivered().await;
        let result = waited.await.unwrap();
        assert_eq!(result.outcome.unwrap().kind, WaitOutcomeKind::NotStarted);
        assert!(
            result
                .message
                .note
                .unwrap_or_default()
                .starts_with("Typed, but no turn started")
        );
        // The next message is not typed blindly after it.
        send_ok(&daemon, send("b", "Second", Some("a"), None)).await;
        fake.nothing_typed().await;
        // Once you sent what was in the input, the queue goes on.
        daemon.messages_on_input("b", "\r");
        b.inbox.typed_at.set(None);
        daemon.pump_messages("b");
        assert!(fake.delivered().await.ends_with("\nSecond"));
    });
}

#[test]
fn a_held_message_is_cancelled_only_by_its_sender() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_a, _) = running(&daemon, "a", "Login UI", None).await;
        let (_c, _) = running(&daemon, "c", "Docs", Some("a")).await;
        let (_b, mut fake) = running(&daemon, "b", "Token storage", Some("a")).await;
        set_attention(&daemon, "b", AttentionState::Working);
        let waited = waiting(&daemon, send("b", "Never mind", Some("a"), Some(60_000)));
        settle().await;
        let id = daemon.messages.borrow().messages[0].id.clone();
        let cancel = |from: Option<&str>| CancelMessage {
            request_id: "r".into(),
            message_id: id.clone(),
            caller: from.map(caller),
        };
        let refused = daemon.handle_cancel_message(&cancel(Some("c"))).await.unwrap_err();
        assert_eq!(
            refused.to_string(),
            format!("Message {id} was not sent by this session")
        );
        let cancelled = daemon.handle_cancel_message(&cancel(Some("a"))).await.unwrap();
        assert_eq!(cancelled.status, MessageStatus::Cancelled);
        assert_eq!(waited.await.unwrap().outcome.unwrap().kind, WaitOutcomeKind::Cancelled);
        // Never typed, and not cancelled twice.
        set_attention(&daemon, "b", AttentionState::ResponseEnded);
        daemon.pump_messages("b");
        fake.nothing_typed().await;
        let refused = daemon.handle_cancel_message(&cancel(None)).await.unwrap_err();
        assert_eq!(
            refused.to_string(),
            format!("Message {id} is cancelled: only a held message can be cancelled")
        );
    });
}

#[test]
fn held_messages_survive_a_daemon_restart_and_are_delivered_when_idle() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let id = {
            let daemon = daemon_in(dir.path());
            stopped(&daemon, "b", "Token storage", None, SessionStatus::Starting);
            let id = send_ok(&daemon, send("b", "After the restart", None, None))
                .await
                .message
                .id;
            daemon.save_messages().await.unwrap();
            id
        };
        // A newer Deckhand's record and key come back as they were.
        let path = dir.path().join(MESSAGES_FILE);
        let mut file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        file["messages"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id": "future", "status": "bounced"}));
        file["version"] = json!(2);
        std::fs::write(&path, file.to_string()).unwrap();

        let daemon = daemon_in(dir.path());
        // The previous daemon's sessions are back (exited by the recovery), the target among them.
        stopped(&daemon, "b", "Token storage", None, SessionStatus::Exited);
        daemon.load_messages().await.unwrap();
        assert_eq!(status_of(&daemon, &id), MessageStatus::Held);
        // Resumed and idle: delivered.
        let (_b, mut fake) = running(&daemon, "b", "Token storage", None).await;
        daemon.pump_messages("b");
        assert!(fake.delivered().await.ends_with("\nAfter the restart"));
        daemon.save_messages().await.unwrap();
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["version"], 2);
        assert_eq!(file["messages"][1], json!({"id": "future", "status": "bounced"}));
        assert_eq!(file["messages"][0]["status"], "delivered");

        // A held message whose session is gone at startup is dropped.
        let daemon = daemon_in(dir.path());
        stopped(&daemon, "a", "Login UI", None, SessionStatus::Running);
        let mut stored: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        stored["messages"][0]["status"] = json!("held");
        stored["messages"][0]["fromSessionId"] = json!("a");
        std::fs::write(&path, stored.to_string()).unwrap();
        daemon.load_messages().await.unwrap();
        assert_eq!(status_of(&daemon, &id), MessageStatus::Dropped);
    });
}

#[test]
fn list_messages_keeps_to_the_repository_or_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        stopped(&daemon, "b", "Token storage", None, SessionStatus::Starting);
        let mut other: Ordered<SessionRecord> = session("z", "/elsewhere", "/elsewhere");
        other.status = SessionStatus::Starting;
        daemon.set_session(other);
        let here = send_ok(&daemon, send("b", "Here", None, None)).await.message.id;
        send_ok(&daemon, send("z", "There", None, None)).await;
        let list = |repo_root: Option<&str>, session_id: Option<&str>| {
            daemon
                .handle_list_messages(&ListMessages {
                    request_id: "r".into(),
                    repo_root: repo_root.map(str::to_owned),
                    session_id: session_id.map(str::to_owned),
                })
                .unwrap()
                .into_iter()
                .map(|message| message.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(list(Some("/r"), None), std::slice::from_ref(&here));
        assert_eq!(list(None, Some("b")), [here]);
        assert_eq!(list(None, None).len(), 2);
    });
}

#[test]
fn a_session_relaunched_with_an_unread_reply_or_a_question_takes_its_messages() {
    let dir = tempfile::tempdir().unwrap();
    let _home = Home::set(dir.path());
    local(async {
        let daemon = daemon_in(dir.path());
        let (_b, mut fake) = running(&daemon, "b", "Token storage", None).await;
        for state in [AttentionState::NeedsInput, AttentionState::ResponseEnded] {
            // Carried over a relaunch: the agent no longer asks, and an unread reply never held a message.
            set_attention(&daemon, "b", state);
            daemon.patch_session("b", |session| deckhand_core::agent_signals::carry_over(session));
            assert!(
                daemon
                    .get_session("b")
                    .unwrap()
                    .attention
                    .as_ref()
                    .unwrap()
                    .carried
                    .is_some()
            );
            let id = send_ok(&daemon, send("b", "Hello", None, None)).await.message.id;
            assert!(fake.delivered().await.ends_with("\nHello"), "{state:?}");
            assert_eq!(status_of(&daemon, &id), MessageStatus::Delivered);
            // Its turn ends before the next one is typed.
            let worker = daemon.launch.borrow().workers.get("b").cloned().unwrap();
            worker.inbox.turn.borrow_mut().take();
        }
    });
}
