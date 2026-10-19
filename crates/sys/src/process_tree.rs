//! Port of processTree.ts.
//!
//! Stopping a pane kills its process group, but an interactive shell moves jobs to process groups of their own; the
//! workspace worker snapshots the pane's descendants (pid plus start time) before signalling and SIGKILLs the ones
//! still alive afterwards. The start time guards against killing a recycled pid. Everything here is synchronous and
//! bounded (`ps` with a 1 s timeout); a failed `ps` reads as no processes.

use std::collections::BTreeMap;

/// TS: processTree.ts ProcessSnapshot (pid → start time as `ps -o lstart=` prints it).
pub type ProcessSnapshot = BTreeMap<u32, String>;

/// TS: processTree.ts ProcessEntry (one `ps -axo pid=,ppid=,lstart=` line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub ppid: u32,
    pub started: String,
}

/// The parser of TS: processTree.ts listProcesses (`ps -axo pid=,ppid=,lstart=` output; malformed lines skipped).
pub fn parse_process_list(stdout: &str) -> Vec<ProcessEntry> {
    let _ = stdout;
    todo!("I5")
}

/// TS: processTree.ts listProcesses (empty when `ps` fails or takes over 1 s).
pub fn list_processes() -> Vec<ProcessEntry> {
    todo!("I5")
}

/// The pure half of TS: processTree.ts snapshotDescendants (every descendant of `root_pid` in `processes`).
pub fn descendants_of(processes: &[ProcessEntry], root_pid: u32) -> ProcessSnapshot {
    let _ = (processes, root_pid);
    todo!("I5")
}

/// TS: processTree.ts snapshotDescendants (must be taken before signalling: once a parent dies its children are
/// reparented and can no longer be found from `root_pid`).
pub fn snapshot_descendants(root_pid: u32) -> ProcessSnapshot {
    let _ = root_pid;
    todo!("I5")
}

/// TS: processTree.ts survivors (the snapshot's processes still running: same pid and start time).
pub fn survivors(snapshot: &ProcessSnapshot) -> ProcessSnapshot {
    let _ = snapshot;
    todo!("I5")
}

/// TS: processTree.ts killSurvivors (SIGKILL to each survivor; errors ignored).
pub fn kill_survivors(snapshot: &ProcessSnapshot) {
    let _ = snapshot;
    todo!("I5")
}
