//! The flow pattern of the screens with their own state (agents, changes, notes, settings, tasks, worktrees) and the
//! help guide.
//!
//! In TS each screen is a hook: it keeps its own state, takes the app's current values and callbacks as options on
//! every render, and returns `{handleInput, render, hint, open, ...}`. In Rust a screen is a struct implementing
//! [`Flow`]: its state is the struct, the per-render options are its [`Flow::Props`] (built by the app from its state
//! when it calls in), and the callbacks become [`FlowEffect`]s the flow emits through its [`FlowCx`] (applied by the
//! host once the call returns). Async work (`void client.x().then(...)`) and timers go through the cx too: their
//! continuations run later on the flow itself, found in the host by the cx's locator, with a fresh cx whose effects
//! the host applies in turn.

use std::future::Future;
use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use deckhand_core::key::Input;
use deckhand_protocol::data::{AgentVersions, ChangesRecord, SessionRecord, Task, TasksDoc};
use deckhand_sys::async_util::{Task as LocalTask, Timer};

use crate::app::event_loop::Dispatch;
use crate::app::state::Mode;

/// A deferred action on the host (TS a callback such as Settings' `back` passed to `onReview`).
pub type HostCallback<H> = Box<dyn FnOnce(&mut H)>;

/// A deferred action on a flow, run with a fresh cx (TS a `.then` continuation inside a hook).
pub type FlowThen<H, F> = Box<dyn FnOnce(&mut F, &mut FlowCx<H, F>)>;

/// What a flow asks of the app (TS: the callbacks in each `use*Flow` options object).
pub enum FlowEffect<H> {
    /// TS `onExit()`: back to the session list (browse).
    Exit,
    /// TS `setMode(mode)` (agents: `agents` / `browse`; settings: its modes / `browse`).
    SetMode(Mode),
    /// TS `setBusy(busy)`.
    SetBusy(bool),
    /// TS `setError(message)`.
    SetError(Option<String>),
    /// TS `setStatusMessage(message)`.
    SetStatus(Option<String>),
    /// TS agentsFlow `setVersions(versions)`.
    Versions(Box<AgentVersions>),
    /// TS changesFlow `onChanges(record)`: a refreshed Changes record (kept only if still for the selection).
    Changes(Box<ChangesRecord>),
    /// TS changesFlow `onAttach()`: attach the selected session's lazygit (the Git target).
    AttachGit,
    /// TS notesFlow `onTasks(doc)` / tasksFlow `onDoc(doc)`: the task list as it is now.
    TasksDoc(Box<TasksDoc>),
    /// TS tasksFlow `onStart(task)`: the new-session form, filled in from the task.
    StartTask(Box<Task>),
    /// TS tasksFlow / worktreesFlow `onGoTo(sessionId)`: select that session (revealing it).
    GoTo(String),
    /// TS tasksFlow `onOpenNote(sessionId)`: that session's Notes tab.
    OpenNote(String),
    /// TS tasksFlow `onSession(session)`: an updated session record.
    Session(Box<SessionRecord>),
    /// TS settingsFlow `onReview(cwd, back)`: the inline trust review of `cwd`; `back` runs when it closes.
    Review { cwd: String, back: HostCallback<H> },
    /// Not in the TS: Settings saved a new wheel speed (`scroll_speed`), for Preview focus to use right away.
    ScrollSpeed(f64),
}

impl<H> std::fmt::Debug for FlowEffect<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlowEffect::Exit => f.write_str("Exit"),
            FlowEffect::SetMode(mode) => f.debug_tuple("SetMode").field(mode).finish(),
            FlowEffect::SetBusy(busy) => f.debug_tuple("SetBusy").field(busy).finish(),
            FlowEffect::SetError(error) => f.debug_tuple("SetError").field(error).finish(),
            FlowEffect::SetStatus(status) => f.debug_tuple("SetStatus").field(status).finish(),
            FlowEffect::Versions(_) => f.write_str("Versions(..)"),
            FlowEffect::Changes(_) => f.write_str("Changes(..)"),
            FlowEffect::AttachGit => f.write_str("AttachGit"),
            FlowEffect::TasksDoc(_) => f.write_str("TasksDoc(..)"),
            FlowEffect::StartTask(task) => f.debug_tuple("StartTask").field(&task.id).finish(),
            FlowEffect::GoTo(id) => f.debug_tuple("GoTo").field(id).finish(),
            FlowEffect::OpenNote(id) => f.debug_tuple("OpenNote").field(id).finish(),
            FlowEffect::Session(session) => f.debug_tuple("Session").field(&session.id).finish(),
            FlowEffect::Review { cwd, .. } => f.debug_struct("Review").field("cwd", cwd).finish_non_exhaustive(),
            FlowEffect::ScrollSpeed(speed) => f.debug_tuple("ScrollSpeed").field(speed).finish(),
        }
    }
}

/// The app side of the flows: applies what they emit.
pub trait FlowHost: Sized + 'static {
    /// Applies a flow's effects in order (TS: the callbacks ran during the flow's call).
    fn apply_flow_effects(&mut self, effects: Vec<FlowEffect<Self>>);
}

/// Finds a flow's state inside its host (a field accessor such as `|app| &mut app.flows.tasks`).
pub type Locate<H, F> = fn(&mut H) -> &mut F;

/// A flow call's context: the effects it emits and the way back to it for async continuations.
pub struct FlowCx<H: FlowHost, F: 'static> {
    effects: Vec<FlowEffect<H>>,
    dispatch: Dispatch<H>,
    locate: Locate<H, F>,
}

impl<H: FlowHost, F: 'static> std::fmt::Debug for FlowCx<H, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowCx")
            .field("effects", &self.effects)
            .finish_non_exhaustive()
    }
}

impl<H: FlowHost, F: 'static> FlowCx<H, F> {
    /// A context for one call into the flow `locate` finds.
    pub fn new(dispatch: Dispatch<H>, locate: Locate<H, F>) -> Self {
        FlowCx {
            effects: Vec::new(),
            dispatch,
            locate,
        }
    }

    /// Emits an effect (applied by the host after the call, in order).
    pub fn emit(&mut self, effect: FlowEffect<H>) {
        self.effects.push(effect);
    }

    /// TS `setError(message)`.
    pub fn set_error(&mut self, error: impl Into<Option<String>>) {
        self.emit(FlowEffect::SetError(error.into()));
    }

    /// TS `setStatusMessage(message)`.
    pub fn set_status(&mut self, status: impl Into<Option<String>>) {
        self.emit(FlowEffect::SetStatus(status.into()));
    }

    /// TS `setBusy(busy)`.
    pub fn set_busy(&mut self, busy: bool) {
        self.emit(FlowEffect::SetBusy(busy));
    }

    /// TS `onExit()`.
    pub fn exit(&mut self) {
        self.emit(FlowEffect::Exit);
    }

    /// The effects emitted so far (taken).
    pub fn take_effects(&mut self) -> Vec<FlowEffect<H>> {
        std::mem::take(&mut self.effects)
    }

    /// The host's dispatcher (for updates that are not the flow's own).
    pub fn dispatch(&self) -> &Dispatch<H> {
        &self.dispatch
    }

    /// A continuation that runs `then` on the flow inside `host` with a fresh cx and applies its effects.
    fn resume<T: 'static>(
        locate: Locate<H, F>,
        dispatch: Dispatch<H>,
        then: impl FnOnce(&mut F, T, &mut FlowCx<H, F>) + 'static,
    ) -> impl FnOnce(&mut H, T) + 'static {
        move |host: &mut H, value: T| {
            let mut cx = FlowCx::new(dispatch, locate);
            then(locate(host), value, &mut cx);
            let effects = cx.take_effects();
            host.apply_flow_effects(effects);
        }
    }

    /// `void work.then(value => then(flow, value))`: runs `future` on the local set and `then` with its output on
    /// the flow. Abort the returned task to drop a stale result (TS's `cancelled` flags).
    pub fn spawn<T: 'static>(
        &self,
        future: impl Future<Output = T> + 'static,
        then: impl FnOnce(&mut F, T, &mut FlowCx<H, F>) + 'static,
    ) -> LocalTask<()> {
        let resume = Self::resume(self.locate, self.dispatch.clone(), then);
        self.dispatch.spawn(future, resume)
    }

    /// TS `setTimeout(() => then(flow), delay)` on the flow (cancelled when the timer is dropped or cleared).
    pub fn after(&self, delay: Duration, then: impl FnOnce(&mut F, &mut FlowCx<H, F>) + 'static) -> Timer {
        let resume = Self::resume(self.locate, self.dispatch.clone(), move |flow, (), cx| then(flow, cx));
        self.dispatch.after(delay, move |host| resume(host, ()))
    }

    /// A callback the host can run later on the flow (TS Settings' `back`: `() => load(cwd)`).
    pub fn callback(&self, then: impl FnOnce(&mut F, &mut FlowCx<H, F>) + 'static) -> HostCallback<H> {
        let resume = Self::resume(self.locate, self.dispatch.clone(), move |flow, (), cx| then(flow, cx));
        Box::new(move |host| resume(host, ()))
    }
}

/// One *Flow.tsx screen.
pub trait Flow<H: FlowHost>: Sized + 'static {
    /// TS: the hook's options that are values (the client, the selected session, records, `focused`, ...), as the
    /// app has them at the time of the call. Callbacks are [`FlowEffect`]s instead.
    type Props<'a>;

    /// TS `handleInput(input, key)`.
    fn handle_input(&mut self, props: &Self::Props<'_>, input: &Input, cx: &mut FlowCx<H, Self>);

    /// TS `render(width, height)`: draws the screen into `area`.
    fn render(&mut self, props: &Self::Props<'_>, buf: &mut Buffer, area: Rect);

    /// TS `hint(width)`: the footer's key hint while the flow has the footer (empty when it writes its own).
    fn hint(&self, props: &Self::Props<'_>, width: usize) -> String {
        let _ = (props, width);
        String::new()
    }
}

/// Calls `call` on the flow `locate` finds in `host` with a fresh cx, then applies the effects it emitted (the
/// host's side of every flow call).
pub fn with_flow<H: FlowHost, F: 'static, R>(
    host: &mut H,
    dispatch: &Dispatch<H>,
    locate: Locate<H, F>,
    call: impl FnOnce(&mut F, &mut FlowCx<H, F>) -> R,
) -> R {
    let mut cx = FlowCx::new(dispatch.clone(), locate);
    let result = call(locate(host), &mut cx);
    let effects = cx.take_effects();
    host.apply_flow_effects(effects);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counter {
        count: i32,
        log: Vec<String>,
    }

    #[derive(Default)]
    struct Host {
        flow: Counter,
        applied: Vec<String>,
        backs: Vec<HostCallback<Host>>,
    }

    impl FlowHost for Host {
        fn apply_flow_effects(&mut self, effects: Vec<FlowEffect<Self>>) {
            for effect in effects {
                match effect {
                    FlowEffect::Review { cwd, back } => {
                        self.applied.push(format!("review {cwd}"));
                        self.backs.push(back);
                    }
                    other => self.applied.push(format!("{other:?}")),
                }
            }
        }
    }

    impl Flow<Host> for Counter {
        type Props<'a> = i32;

        fn handle_input(&mut self, step: &i32, input: &Input, cx: &mut FlowCx<Host, Self>) {
            if input.input == "+" {
                self.count += step;
                cx.set_status(format!("now {}", self.count));
            }
            if input.input == "r" {
                let back = cx.callback(|flow, cx| {
                    flow.log.push("back".to_string());
                    cx.exit();
                });
                cx.emit(FlowEffect::Review {
                    cwd: "/repo".to_string(),
                    back,
                });
            }
        }

        fn render(&mut self, _step: &i32, _buf: &mut Buffer, _area: Rect) {}
    }

    #[test]
    fn effects_apply_after_the_call_in_order() {
        let (dispatch, _rx) = Dispatch::<Host>::channel();
        let mut host = Host::default();
        with_flow(
            &mut host,
            &dispatch,
            |host| &mut host.flow,
            |flow, cx| {
                flow.handle_input(&2, &Input::text("+"), cx);
                flow.handle_input(&2, &Input::text("+"), cx);
            },
        );
        assert_eq!(host.flow.count, 4);
        assert_eq!(
            host.applied,
            ["SetStatus(Some(\"now 2\"))", "SetStatus(Some(\"now 4\"))"]
        );
        assert_eq!(Counter::default().hint(&0, 10), "");
    }

    #[test]
    fn callbacks_run_on_the_flow_later() {
        let (dispatch, _rx) = Dispatch::<Host>::channel();
        let mut host = Host::default();
        with_flow(
            &mut host,
            &dispatch,
            |host| &mut host.flow,
            |flow, cx| {
                flow.handle_input(&1, &Input::text("r"), cx);
            },
        );
        assert_eq!(host.applied, ["review /repo"]);
        let back = host.backs.pop().expect("back callback");
        back(&mut host);
        assert_eq!(host.flow.log, ["back"]);
        assert_eq!(host.applied, ["review /repo", "Exit"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn spawned_work_resumes_on_the_flow() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let (dispatch, mut rx) = Dispatch::<Host>::channel();
                let mut host = Host::default();
                let cx = FlowCx::new(dispatch.clone(), |host: &mut Host| &mut host.flow);
                let _task = cx.spawn(async { 7 }, |flow: &mut Counter, value, cx| {
                    flow.count = value;
                    cx.set_error("late".to_string());
                });
                let _timer = cx.after(Duration::from_millis(1), |flow: &mut Counter, _cx| flow.count += 1);
                for _ in 0..2 {
                    let update = rx.recv().await.expect("update");
                    update(&mut host);
                }
                assert_eq!(host.flow.count, 8);
                assert_eq!(host.applied, ["SetError(Some(\"late\"))"]);
            })
            .await;
    }
}
