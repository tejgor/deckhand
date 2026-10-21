//! Async coordination: `KeyedQueue`, `Coalescer`, `SharedSlot`, `Timer`.
//!
//! The TS daemon leans on a few promise idioms; these are their single-threaded Rust forms for a tokio
//! `current_thread` runtime with a `LocalSet` (nothing here is `Send`):
//!
//! - [`KeyedQueue`]: `queue = queue.catch(() => {}).then(run)` per key (notesStore.ts `serialized`, the per-file
//!   write chains). Tasks of one key run one at a time in call order; a failure never blocks the next.
//! - [`Coalescer`]: daemon.ts `persist()`: at most one run in flight plus one queued; every caller arriving while a
//!   run is queued shares it.
//! - [`SharedSlot`] / [`KeyedSharedSlot`]: `pending ??= start().finally(() => pending = undefined)`: concurrent
//!   callers share one in-flight run and all get its result.
//! - [`Timer`], [`Interval`], [`KeyedTimers`]: `setTimeout`/`clearTimeout`/`setInterval`, cancelled on drop.
//!
//! Like JS promises, work starts when it is requested, not when it is awaited: tasks are spawned with
//! `tokio::task::spawn_local`, so these must be used inside a `LocalSet` (every Deckhand process runs one), and a
//! returned handle may be dropped without cancelling the work (`void persist()`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::FutureExt;
use futures::future::{LocalBoxFuture, Shared};
use tokio::task::JoinHandle;

/// A spawned local task: awaiting it gives the task's output (a panic in the task is resumed in the awaiter).
/// Dropping it detaches the task (it keeps running), as dropping a JS promise does; [`Task::abort`] cancels it.
#[derive(Debug)]
pub struct Task<T> {
    handle: JoinHandle<T>,
}

impl<T> Task<T> {
    /// Cancels the task at its next await point.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Whether the task has finished.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        match Pin::new(&mut self.handle).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(value)) => Poll::Ready(value),
            Poll::Ready(Err(error)) => match error.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                // Aborted: a JS promise never settles in that case; neither does this.
                Err(_) => Poll::Pending,
            },
        }
    }
}

/// Spawns `future` on the current `LocalSet` (panics outside one, like `tokio::task::spawn_local`).
pub fn spawn<F>(future: F) -> Task<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    Task {
        handle: tokio::task::spawn_local(future),
    }
}

/// A cloneable handle to one run's result: every clone resolves to the same value.
pub type SharedTask<T> = Shared<LocalBoxFuture<'static, T>>;

fn share<T: Clone + 'static>(task: Task<T>) -> SharedTask<T> {
    task.boxed_local().shared()
}

/// Resolves once the previous run of a chain has settled (completed, panicked or been aborted).
type Settled = Shared<LocalBoxFuture<'static, ()>>;

/// Per-key serialized async tasks (TS: a `Map<key, Promise>` of `.catch(() => {}).then(run)` chains). A task starts
/// once every task requested before it for the same key has settled; tasks of different keys run concurrently.
pub struct KeyedQueue<K> {
    tails: Rc<RefCell<HashMap<K, (u64, Settled)>>>,
    next_id: Rc<Cell<u64>>,
}

impl<K> Default for KeyedQueue<K> {
    fn default() -> Self {
        KeyedQueue {
            tails: Rc::new(RefCell::new(HashMap::new())),
            next_id: Rc::new(Cell::new(0)),
        }
    }
}

impl<K> Clone for KeyedQueue<K> {
    fn clone(&self) -> Self {
        KeyedQueue {
            tails: Rc::clone(&self.tails),
            next_id: Rc::clone(&self.next_id),
        }
    }
}

/// Removes a key's tail when the run that owns it settles (also on panic or abort).
struct TailCleanup<K: Eq + Hash> {
    tails: Rc<RefCell<HashMap<K, (u64, Settled)>>>,
    key: Option<K>,
    id: u64,
}

impl<K: Eq + Hash> Drop for TailCleanup<K> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take()
            && let Ok(mut tails) = self.tails.try_borrow_mut()
            && tails.get(&key).is_some_and(|(id, _)| *id == self.id)
        {
            tails.remove(&key);
        }
    }
}

impl<K: Eq + Hash + Clone + 'static> KeyedQueue<K> {
    /// An empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues `task` behind the tasks already requested for `key` and spawns it: the order is fixed at call time.
    /// The task's failure (an `Err` output, a panic) does not stop the tasks after it.
    pub fn run<F, Fut, T>(&self, key: K, task: F) -> Task<T>
    where
        F: FnOnce() -> Fut + 'static,
        Fut: Future<Output = T> + 'static,
        T: 'static,
    {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        let (done_tx, done_rx) = futures::channel::oneshot::channel::<()>();
        let previous = self.tails.borrow().get(&key).map(|(_, settled)| settled.clone());
        // This run has settled once the previous one has and this one has dropped its sender (finished, panicked or
        // was aborted, even while still waiting its turn).
        let waits_for = previous.clone();
        let settled: Settled = async move {
            if let Some(previous) = waits_for {
                previous.await;
            }
            let _ = done_rx.await;
        }
        .boxed_local()
        .shared();
        self.tails.borrow_mut().insert(key.clone(), (id, settled));
        let cleanup = TailCleanup {
            tails: Rc::clone(&self.tails),
            key: Some(key),
            id,
        };
        spawn(async move {
            let _done = done_tx;
            let _cleanup = cleanup;
            if let Some(previous) = previous {
                previous.await;
            }
            task().await
        })
    }

    /// Whether a task for `key` is queued or running.
    pub fn is_busy(&self, key: &K) -> bool {
        self.tails.borrow().contains_key(key)
    }

    /// How many keys have queued or running tasks.
    pub fn busy_keys(&self) -> usize {
        self.tails.borrow().len()
    }

    /// Resolves once every task requested so far for `key` has settled.
    pub fn idle(&self, key: &K) -> impl Future<Output = ()> + 'static {
        let tail = self.tails.borrow().get(key).map(|(_, settled)| settled.clone());
        async move {
            if let Some(tail) = tail {
                tail.await;
            }
        }
    }
}

struct CoalescerState<T: Clone + 'static> {
    queued: Option<SharedTask<T>>,
    in_flight: Option<SharedTask<T>>,
}

/// daemon.ts `persist()`: runs `task` at most once at a time. A request while nothing is queued queues a new run
/// that starts after the one in flight (whatever its result); requests arriving while that run is still queued share
/// it. So a burst of requests costs at most two runs, and the last request always sees a run that started after it.
pub struct Coalescer<T: Clone + 'static> {
    state: Rc<RefCell<CoalescerState<T>>>,
    task: Rc<dyn Fn() -> LocalBoxFuture<'static, T>>,
}

impl<T: Clone + 'static> Clone for Coalescer<T> {
    fn clone(&self) -> Self {
        Coalescer {
            state: Rc::clone(&self.state),
            task: Rc::clone(&self.task),
        }
    }
}

impl<T: Clone + 'static> Coalescer<T> {
    /// A coalescer for `task` (called once per run, when the run starts: read the state to write then).
    pub fn new<F, Fut>(task: F) -> Self
    where
        F: Fn() -> Fut + 'static,
        Fut: Future<Output = T> + 'static,
    {
        Coalescer {
            state: Rc::new(RefCell::new(CoalescerState {
                queued: None,
                in_flight: None,
            })),
            task: Rc::new(move || task().boxed_local()),
        }
    }

    /// Requests a run (TS `persist()`): the queued run's result, shared by every caller until it starts.
    pub fn request(&self) -> SharedTask<T> {
        if let Some(queued) = &self.state.borrow().queued {
            return queued.clone();
        }
        let previous = self.state.borrow().in_flight.clone();
        let state = Rc::clone(&self.state);
        let task = Rc::clone(&self.task);
        let run = share(spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            state.borrow_mut().queued = None;
            let future = task();
            future.await
        }));
        let mut state = self.state.borrow_mut();
        state.queued = Some(run.clone());
        state.in_flight = Some(run.clone());
        run
    }

    /// Whether a run is queued (not started yet).
    pub fn is_queued(&self) -> bool {
        self.state.borrow().queued.is_some()
    }

    /// Resolves with the latest requested run's result, or `None` when nothing was ever requested.
    pub fn settled(&self) -> impl Future<Output = Option<T>> + 'static {
        let last = self.state.borrow().in_flight.clone();
        async move {
            match last {
                Some(run) => Some(run.await),
                None => None,
            }
        }
    }
}

/// One in-flight run shared by concurrent callers (TS `this.pending ??= start().finally(() => this.pending =
/// undefined)`): while a run is in flight, [`SharedSlot::get_or_start`] returns it instead of starting another; once
/// it settles the slot is empty again (results are not cached).
pub struct SharedSlot<T: Clone + 'static> {
    slots: KeyedSharedSlot<(), T>,
}

impl<T: Clone + 'static> Default for SharedSlot<T> {
    fn default() -> Self {
        SharedSlot {
            slots: KeyedSharedSlot::default(),
        }
    }
}

impl<T: Clone + 'static> Clone for SharedSlot<T> {
    fn clone(&self) -> Self {
        SharedSlot {
            slots: self.slots.clone(),
        }
    }
}

impl<T: Clone + 'static> SharedSlot<T> {
    /// An empty slot.
    pub fn new() -> Self {
        Self::default()
    }

    /// The run in flight, or a new one of `start` (spawned now).
    pub fn get_or_start<F, Fut>(&self, start: F) -> SharedTask<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T> + 'static,
    {
        self.slots.get_or_start((), start)
    }

    /// The run in flight, if any.
    pub fn get(&self) -> Option<SharedTask<T>> {
        self.slots.get(&())
    }

    /// Whether a run is in flight.
    pub fn is_running(&self) -> bool {
        self.slots.contains(&())
    }
}

/// In-flight runs by key, each tagged with the id of the call that started it.
type SlotMap<K, T> = Rc<RefCell<HashMap<K, (u64, SharedTask<T>)>>>;

/// [`SharedSlot`] per key (TS `Map<key, Promise>` of in-flight starts, e.g. the daemon's workspace starts).
pub struct KeyedSharedSlot<K, T: Clone + 'static> {
    runs: SlotMap<K, T>,
    next_id: Rc<Cell<u64>>,
}

impl<K, T: Clone + 'static> Default for KeyedSharedSlot<K, T> {
    fn default() -> Self {
        KeyedSharedSlot {
            runs: Rc::new(RefCell::new(HashMap::new())),
            next_id: Rc::new(Cell::new(0)),
        }
    }
}

impl<K, T: Clone + 'static> Clone for KeyedSharedSlot<K, T> {
    fn clone(&self) -> Self {
        KeyedSharedSlot {
            runs: Rc::clone(&self.runs),
            next_id: Rc::clone(&self.next_id),
        }
    }
}

/// Empties a key's slot when the run that owns it settles (also on panic or abort).
struct SlotCleanup<K: Eq + Hash, T: Clone + 'static> {
    runs: SlotMap<K, T>,
    key: Option<K>,
    id: u64,
}

impl<K: Eq + Hash, T: Clone + 'static> Drop for SlotCleanup<K, T> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take()
            && let Ok(mut runs) = self.runs.try_borrow_mut()
            && runs.get(&key).is_some_and(|(id, _)| *id == self.id)
        {
            runs.remove(&key);
        }
    }
}

impl<K: Eq + Hash + Clone + 'static, T: Clone + 'static> KeyedSharedSlot<K, T> {
    /// No runs.
    pub fn new() -> Self {
        Self::default()
    }

    /// The run in flight for `key`, or a new one of `start` (spawned now).
    pub fn get_or_start<F, Fut>(&self, key: K, start: F) -> SharedTask<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T> + 'static,
    {
        if let Some(run) = self.get(&key) {
            return run;
        }
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        let cleanup = SlotCleanup {
            runs: Rc::clone(&self.runs),
            key: Some(key.clone()),
            id,
        };
        let future = start();
        let run = share(spawn(async move {
            let _cleanup = cleanup;
            future.await
        }));
        // The spawned task cannot have run yet (spawn_local only queues it), so this insert precedes its cleanup.
        self.runs.borrow_mut().insert(key, (id, run.clone()));
        run
    }

    /// The run in flight for `key`.
    pub fn get(&self, key: &K) -> Option<SharedTask<T>> {
        self.runs.borrow().get(key).map(|(_, run)| run.clone())
    }

    /// Whether a run is in flight for `key`.
    pub fn contains(&self, key: &K) -> bool {
        self.runs.borrow().contains_key(key)
    }

    /// How many runs are in flight.
    pub fn len(&self) -> usize {
        self.runs.borrow().len()
    }

    /// Whether nothing is in flight.
    pub fn is_empty(&self) -> bool {
        self.runs.borrow().is_empty()
    }
}

/// `setTimeout(callback, delay)`: the callback runs once after `delay` on the current `LocalSet`, unless the timer is
/// cleared ([`Timer::clear`], `clearTimeout`) or dropped first. [`Timer::detach`] lets it fire after the handle is
/// gone.
#[derive(Debug)]
pub struct Timer {
    handle: Option<JoinHandle<()>>,
    fired: Rc<Cell<bool>>,
}

impl Timer {
    /// Schedules `callback` after `delay` (TS `setTimeout`).
    pub fn after<F>(delay: Duration, callback: F) -> Timer
    where
        F: FnOnce() + 'static,
    {
        let fired = Rc::new(Cell::new(false));
        let flag = Rc::clone(&fired);
        let handle = tokio::task::spawn_local(async move {
            tokio::time::sleep(delay).await;
            flag.set(true);
            callback();
        });
        Timer {
            handle: Some(handle),
            fired,
        }
    }

    /// Cancels the timer if it has not fired (TS `clearTimeout`).
    pub fn clear(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    /// Whether the callback is still to run.
    pub fn is_pending(&self) -> bool {
        self.handle.is_some() && !self.fired.get()
    }

    /// Whether the callback has run (or started running).
    pub fn has_fired(&self) -> bool {
        self.fired.get()
    }

    /// Lets the timer fire even though this handle is dropped.
    pub fn detach(mut self) {
        self.handle = None;
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.clear();
    }
}

/// `setInterval(callback, period)`: the callback runs every `period` (first after one period) until the interval is
/// cleared or dropped. A late tick is not made up for (the next one is a full period later).
#[derive(Debug)]
pub struct Interval {
    handle: Option<JoinHandle<()>>,
}

impl Interval {
    /// Starts the interval (TS `setInterval`).
    pub fn every<F>(period: Duration, mut callback: F) -> Interval
    where
        F: FnMut() + 'static,
    {
        let handle = tokio::task::spawn_local(async move {
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticks.tick().await;
                callback();
            }
        });
        Interval { handle: Some(handle) }
    }

    /// Stops the interval (TS `clearInterval`).
    pub fn clear(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }

    /// Whether it still ticks.
    pub fn is_active(&self) -> bool {
        self.handle.is_some()
    }
}

impl Drop for Interval {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Debounce timers by key (TS `Map<key, Timeout>` with `clearTimeout(timers.get(key)); timers.set(key,
/// setTimeout(...))`): scheduling a key replaces its pending timer; a fired timer removes itself.
pub struct KeyedTimers<K> {
    timers: Rc<RefCell<HashMap<K, (u64, Timer)>>>,
    next_id: Rc<Cell<u64>>,
}

impl<K> Default for KeyedTimers<K> {
    fn default() -> Self {
        KeyedTimers {
            timers: Rc::new(RefCell::new(HashMap::new())),
            next_id: Rc::new(Cell::new(0)),
        }
    }
}

impl<K> Clone for KeyedTimers<K> {
    fn clone(&self) -> Self {
        KeyedTimers {
            timers: Rc::clone(&self.timers),
            next_id: Rc::clone(&self.next_id),
        }
    }
}

impl<K: Eq + Hash + Clone + 'static> KeyedTimers<K> {
    /// No timers.
    pub fn new() -> Self {
        Self::default()
    }

    /// (Re)schedules `callback` for `key` after `delay`, clearing the key's pending timer.
    pub fn schedule<F>(&self, key: K, delay: Duration, callback: F)
    where
        F: FnOnce() + 'static,
    {
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        let timers = Rc::clone(&self.timers);
        let own_key = key.clone();
        let timer = Timer::after(delay, move || {
            // Forget this timer before the callback runs, so the callback may schedule the key again. The removed
            // Timer is dropped after the borrow ends; aborting a task from inside itself is harmless here because
            // the handle is detached first.
            let removed = {
                let mut map = timers.borrow_mut();
                if map.get(&own_key).is_some_and(|(current, _)| *current == id) {
                    map.remove(&own_key)
                } else {
                    None
                }
            };
            if let Some((_, timer)) = removed {
                timer.detach();
            }
            callback();
        });
        let previous = self.timers.borrow_mut().insert(key, (id, timer));
        drop(previous);
    }

    /// Clears `key`'s pending timer.
    pub fn clear(&self, key: &K) {
        let removed = self.timers.borrow_mut().remove(key);
        drop(removed);
    }

    /// Clears every pending timer.
    pub fn clear_all(&self) {
        let removed: Vec<_> = self.timers.borrow_mut().drain().collect();
        drop(removed);
    }

    /// Whether `key` has a pending timer.
    pub fn is_pending(&self, key: &K) -> bool {
        self.timers.borrow().contains_key(key)
    }

    /// How many timers are pending.
    pub fn len(&self) -> usize {
        self.timers.borrow().len()
    }

    /// Whether no timer is pending.
    pub fn is_empty(&self) -> bool {
        self.timers.borrow().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::task::LocalSet;
    use tokio::time::sleep;

    fn log() -> Rc<RefCell<Vec<String>>> {
        Rc::new(RefCell::new(Vec::new()))
    }

    fn push(log: &Rc<RefCell<Vec<String>>>, entry: impl Into<String>) {
        log.borrow_mut().push(entry.into());
    }

    fn entries(log: &Rc<RefCell<Vec<String>>>) -> Vec<String> {
        log.borrow().clone()
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn keyed_queue_serializes_per_key_in_call_order() {
        LocalSet::new()
            .run_until(async {
                let queue = KeyedQueue::<&'static str>::new();
                let events = log();
                let mut tasks = Vec::new();
                for (key, name, ms) in [("a", "a1", 30), ("b", "b1", 10), ("a", "a2", 5), ("a", "a3", 1)] {
                    let events = Rc::clone(&events);
                    tasks.push(queue.run(key, move || async move {
                        push(&events, format!("start {name}"));
                        sleep(Duration::from_millis(ms)).await;
                        push(&events, format!("end {name}"));
                        name
                    }));
                }
                assert!(queue.is_busy(&"a") && queue.is_busy(&"b"));
                let mut results = Vec::new();
                for task in tasks {
                    results.push(task.await);
                }
                assert_eq!(results, ["a1", "b1", "a2", "a3"]);
                assert_eq!(
                    entries(&events),
                    [
                        "start a1", "start b1", "end b1", "end a1", "start a2", "end a2", "start a3", "end a3"
                    ]
                );
                // Idle keys are forgotten.
                assert_eq!(queue.busy_keys(), 0);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn keyed_queue_continues_after_failures_panics_and_aborts() {
        LocalSet::new()
            .run_until(async {
                let queue = KeyedQueue::<u8>::new();
                let events = log();
                let failing = queue.run(1, || async { Err::<(), String>("boom".into()) });
                let e1 = Rc::clone(&events);
                let panicking = queue.run(1, move || async move {
                    push(&e1, "panicking");
                    panic!("task panicked");
                });
                let e2 = Rc::clone(&events);
                let slow = queue.run(1, move || async move {
                    sleep(Duration::from_millis(50)).await;
                    push(&e2, "slow done");
                });
                let e3 = Rc::clone(&events);
                let aborted = queue.run(1, move || async move {
                    push(&e3, "aborted ran");
                });
                let e4 = Rc::clone(&events);
                let last = queue.run(1, move || async move {
                    push(&e4, "last");
                    7
                });
                // Aborted while waiting for its turn: the next task still waits for `slow`.
                aborted.abort();
                assert_eq!(failing.await, Err("boom".into()));
                assert_eq!(last.await, 7);
                assert_eq!(entries(&events), ["panicking", "slow done", "last"]);
                drop((panicking, slow));
                queue.idle(&1).await;
                assert!(!queue.is_busy(&1));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn keyed_queue_runs_dropped_tasks() {
        LocalSet::new()
            .run_until(async {
                let queue = KeyedQueue::<u8>::new();
                let events = log();
                let e = Rc::clone(&events);
                drop(queue.run(0, move || async move { push(&e, "ran") }));
                queue.idle(&0).await;
                assert_eq!(entries(&events), ["ran"]);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn coalescer_runs_at_most_one_plus_one_queued() {
        LocalSet::new()
            .run_until(async {
                let runs = Rc::new(Cell::new(0u32));
                let counter = Rc::clone(&runs);
                let coalescer = Coalescer::new(move || {
                    let run = counter.get() + 1;
                    counter.set(run);
                    async move {
                        sleep(Duration::from_millis(100)).await;
                        run
                    }
                });
                let first = coalescer.request();
                assert!(coalescer.is_queued());
                // Still queued (not started): shared.
                let same = coalescer.request();
                tokio::task::yield_now().await;
                // Started: the next request queues a second run behind it, and later ones share that.
                assert!(!coalescer.is_queued());
                let second = coalescer.request();
                let third = coalescer.request();
                let fourth = coalescer.request();
                assert_eq!((first.await, same.await), (1, 1));
                assert_eq!((second.await, third.await, fourth.await), (2, 2, 2));
                assert_eq!(runs.get(), 2);
                assert_eq!(coalescer.settled().await, Some(2));
                // A dropped request still runs.
                drop(coalescer.request());
                sleep(Duration::from_millis(200)).await;
                assert_eq!(runs.get(), 3);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn coalescer_queued_run_reads_state_when_it_starts() {
        LocalSet::new()
            .run_until(async {
                let value = Rc::new(Cell::new(0));
                let written = log();
                let (v, w) = (Rc::clone(&value), Rc::clone(&written));
                let coalescer = Coalescer::new(move || {
                    let snapshot = v.get();
                    let w = Rc::clone(&w);
                    async move {
                        sleep(Duration::from_millis(10)).await;
                        push(&w, snapshot.to_string());
                    }
                });
                value.set(1);
                let a = coalescer.request();
                tokio::task::yield_now().await;
                value.set(2);
                let b = coalescer.request();
                value.set(3);
                let c = coalescer.request();
                a.await;
                b.await;
                c.await;
                assert_eq!(entries(&written), ["1", "3"]);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shared_slot_dedupes_in_flight_work() {
        LocalSet::new()
            .run_until(async {
                let slot = SharedSlot::<u32>::new();
                let starts = Rc::new(Cell::new(0));
                let start = |starts: &Rc<Cell<u32>>| {
                    let starts = Rc::clone(starts);
                    move || {
                        starts.set(starts.get() + 1);
                        let n = starts.get();
                        async move {
                            sleep(Duration::from_millis(20)).await;
                            n * 10
                        }
                    }
                };
                let a = slot.get_or_start(start(&starts));
                let b = slot.get_or_start(start(&starts));
                assert!(slot.is_running());
                assert_eq!((a.await, b.await), (10, 10));
                assert!(!slot.is_running());
                // Settled: the next call starts again.
                assert_eq!(slot.get_or_start(start(&starts)).await, 20);
                assert_eq!(starts.get(), 2);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn keyed_shared_slot_is_per_key_and_clears_on_panic() {
        LocalSet::new()
            .run_until(async {
                let slots = KeyedSharedSlot::<&'static str, Result<u8, String>>::new();
                let x = slots.get_or_start("x", || async {
                    sleep(Duration::from_millis(5)).await;
                    Ok(1)
                });
                let y = slots.get_or_start("y", || async { Err("bad".to_owned()) });
                let x2 = slots.get_or_start("x", || async { Ok(99) });
                assert_eq!(slots.len(), 2);
                assert_eq!(x.await, Ok(1));
                assert_eq!(x2.await, Ok(1));
                assert_eq!(y.await, Err("bad".to_owned()));
                assert!(slots.is_empty());
                let boom = slots.get_or_start("z", || async { panic!("start failed") });
                let caught = std::panic::AssertUnwindSafe(boom).catch_unwind().await;
                assert!(caught.is_err());
                assert!(!slots.contains(&"z"));
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn timer_fires_once_and_can_be_cleared_or_dropped() {
        LocalSet::new()
            .run_until(async {
                let events = log();
                let e = Rc::clone(&events);
                let timer = Timer::after(Duration::from_millis(100), move || push(&e, "fired"));
                assert!(timer.is_pending());
                sleep(Duration::from_millis(99)).await;
                assert!(entries(&events).is_empty());
                sleep(Duration::from_millis(2)).await;
                assert_eq!(entries(&events), ["fired"]);
                assert!(timer.has_fired() && !timer.is_pending());

                let e = Rc::clone(&events);
                let mut cleared = Timer::after(Duration::from_millis(10), move || push(&e, "cleared"));
                cleared.clear();
                assert!(!cleared.is_pending());
                let e = Rc::clone(&events);
                drop(Timer::after(Duration::from_millis(10), move || push(&e, "dropped")));
                let e = Rc::clone(&events);
                Timer::after(Duration::from_millis(10), move || push(&e, "detached")).detach();
                sleep(Duration::from_millis(50)).await;
                assert_eq!(entries(&events), ["fired", "detached"]);
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn interval_ticks_every_period_until_cleared() {
        LocalSet::new()
            .run_until(async {
                let ticks = Rc::new(Cell::new(0));
                let t = Rc::clone(&ticks);
                let mut interval = Interval::every(Duration::from_millis(10), move || t.set(t.get() + 1));
                sleep(Duration::from_millis(5)).await;
                assert_eq!(ticks.get(), 0);
                sleep(Duration::from_millis(30)).await;
                assert_eq!(ticks.get(), 3);
                interval.clear();
                sleep(Duration::from_millis(100)).await;
                assert_eq!(ticks.get(), 3);
                assert!(!interval.is_active());
            })
            .await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn keyed_timers_debounce_per_key() {
        LocalSet::new()
            .run_until(async {
                let events = log();
                let timers = KeyedTimers::<&'static str>::new();
                for _ in 0..3 {
                    let e = Rc::clone(&events);
                    timers.schedule("a", Duration::from_millis(120), move || push(&e, "a"));
                    sleep(Duration::from_millis(50)).await;
                }
                let e = Rc::clone(&events);
                timers.schedule("b", Duration::from_millis(10), move || push(&e, "b"));
                let e = Rc::clone(&events);
                timers.schedule("c", Duration::from_millis(10), move || push(&e, "c"));
                timers.clear(&"c");
                assert_eq!(timers.len(), 2);
                sleep(Duration::from_millis(200)).await;
                assert_eq!(entries(&events), ["b", "a"]);
                assert!(timers.is_empty());
                // A callback may reschedule its own key.
                let again = timers.clone();
                let e = Rc::clone(&events);
                timers.schedule("r", Duration::from_millis(5), move || {
                    let e2 = Rc::clone(&e);
                    again.schedule("r", Duration::from_millis(5), move || push(&e2, "r2"));
                    push(&e, "r1");
                });
                sleep(Duration::from_millis(20)).await;
                assert_eq!(entries(&events), ["b", "a", "r1", "r2"]);
                let e = Rc::clone(&events);
                timers.schedule("z", Duration::from_millis(5), move || push(&e, "z"));
                timers.clear_all();
                sleep(Duration::from_millis(20)).await;
                assert_eq!(entries(&events).len(), 4);
            })
            .await;
    }
}
