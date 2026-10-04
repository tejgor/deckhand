import {execFileSync} from 'node:child_process';

// pid -> start time. The start time guards against killing a recycled PID.
export type ProcessSnapshot = Map<number, string>;

interface ProcessEntry {
	pid: number;
	ppid: number;
	started: string;
}

function listProcesses(): ProcessEntry[] {
	try {
		const stdout = execFileSync('ps', ['-axo', 'pid=,ppid=,lstart='], {encoding: 'utf8', timeout: 1000});
		return stdout.split('\n').flatMap(line => {
			const match = /^\s*(\d+)\s+(\d+)\s+(.+?)\s*$/.exec(line);
			return match ? [{pid: Number(match[1]), ppid: Number(match[2]), started: match[3]!}] : [];
		});
	} catch {
		return [];
	}
}

// Must be taken before signalling: once a parent dies its children are
// reparented to init/launchd and can no longer be found from rootPid.
export function snapshotDescendants(rootPid: number): ProcessSnapshot {
	const processes = listProcesses();
	const children = new Map<number, ProcessEntry[]>();
	for (const entry of processes) children.set(entry.ppid, [...(children.get(entry.ppid) ?? []), entry]);
	const snapshot: ProcessSnapshot = new Map();
	const queue = [rootPid];
	while (queue.length > 0) {
		for (const child of children.get(queue.pop()!) ?? []) {
			if (snapshot.has(child.pid)) continue;
			snapshot.set(child.pid, child.started);
			queue.push(child.pid);
		}
	}
	return snapshot;
}

export function killSurvivors(snapshot: ProcessSnapshot): void {
	if (snapshot.size === 0) return;
	const current = new Map(listProcesses().map(entry => [entry.pid, entry.started]));
	for (const [pid, started] of snapshot) {
		if (current.get(pid) !== started) continue;
		try { process.kill(pid, 'SIGKILL'); } catch {}
	}
}
