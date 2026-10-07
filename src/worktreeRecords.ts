import path from 'node:path';
import {randomUUID} from 'node:crypto';
import type {SessionRecord, SessionWorktreeRecord, WorktreeMarkers, WorktreeRecord} from './types.js';

// Merge and deletion markers describe a linked worktree, which several sessions can share (sub-sessions, attaches).
// They live in one WorktreeRecord per worktree incarnation (state.json `worktrees`); each session in it stores only the
// record's ID (`worktree.id`). The daemon projects the record's markers into the sessions it holds and sends
// (`projectWorktree`) and strips them again before saving (`storedSession`), so the record is the only persisted copy.
// Sessions in the main checkout have no record and no merge marker: `M` is worktree-only, and their old per-session
// markers became the done marker (`migrateDoneMarkers`).

export const WORKTREE_MARKERS = ['mergedAt', 'mergeMode', 'mergeTargetBranch', 'mergeSourceRef', 'mergeSourceSha', 'mergeMarkedManually', 'mergeDetected', 'deletedAt'] as const satisfies readonly (keyof WorktreeMarkers)[];
export const MERGE_MARKERS = WORKTREE_MARKERS.filter(key => key !== 'deletedAt');

/** The markers set on `source`. */
export function worktreeMarkers(source: WorktreeMarkers): WorktreeMarkers {
	const markers: Record<string, unknown> = {};
	for (const key of WORKTREE_MARKERS) if (source[key] !== undefined) markers[key] = source[key];
	return markers as WorktreeMarkers;
}

/** A copy of `source` without the given markers (all of them by default). */
export function withoutMarkers<T extends WorktreeMarkers>(source: T, keys: readonly (keyof WorktreeMarkers)[] = WORKTREE_MARKERS): T {
	const copy = {...source};
	for (const key of keys) delete copy[key];
	return copy;
}

/** The root of the session's own linked worktree: managed or attached, not the main checkout. */
export function ownWorktreePath(worktree: SessionWorktreeRecord | undefined): string | undefined {
	return worktree && worktree.mode !== 'none' && worktree.path && !worktree.isMain ? path.resolve(worktree.path) : undefined;
}

/** The live (not deleted) incarnation at a worktree root; there is at most one. Paths come from Git, so compare lexically. */
export function liveWorktreeRecord(records: Iterable<WorktreeRecord>, worktreePath: string): WorktreeRecord | undefined {
	const key = path.resolve(worktreePath);
	for (const record of records) if (!record.deletedAt && path.resolve(record.path) === key) return record;
	return undefined;
}

/** The session as the daemon holds and sends it: its worktree record's markers in `worktree`. */
export function projectWorktree(session: SessionRecord, records: ReadonlyMap<string, WorktreeRecord>): SessionRecord {
	const worktree = session.worktree;
	if (!worktree?.id) return session;
	const record = records.get(worktree.id);
	return {...session, worktree: {...withoutMarkers(worktree), ...(record ? worktreeMarkers(record) : {})}};
}

/** The session as persisted: the projected markers are dropped (the record holds them). */
export function storedSession(session: SessionRecord): SessionRecord {
	return session.worktree?.id ? {...session, worktree: withoutMarkers(session.worktree)} : session;
}

const time = (value: string | undefined): number => {
	const parsed = Date.parse(value ?? '');
	return Number.isNaN(parsed) ? 0 : parsed;
};

type MergeSource = Pick<SessionRecord, 'mergedAt' | 'mergeTargetBranch' | 'mergeSourceRef' | 'mergeMarkedManually'> & {mergeMode?: SessionWorktreeRecord['mergeMode']};

/**
 * Lifts per-session worktree markers (state written before worktree records) into records, and makes the stored state
 * consistent: every referenced record exists, no session stores projected markers, unreferenced records are dropped.
 *
 * Legacy sessions are grouped by worktree root: their own linked worktree, or, for a session without one (mode
 * `none`, e.g. a sub-session), its launch checkout when that is a linked worktree some session owns. Within a root,
 * each recorded deletion (`worktree.deletedAt`) ends one incarnation: the session that deleted it belongs to it, any
 * other to the first incarnation deleted at or after its last launch (`agentStartedAt`, else `createdAt`), and
 * sessions launched after the last deletion share the live one. An incarnation takes the most recent merge marker of
 * its sessions (a sub-session's top-level `M` marker included).
 */
export function migrateWorktreeRecords(sessions: SessionRecord[], worktrees: WorktreeRecord[], newId: () => string = randomUUID): {sessions: SessionRecord[]; worktrees: WorktreeRecord[]; changed: boolean} {
	const records = new Map<string, WorktreeRecord>();
	for (const record of worktrees) if (typeof record?.id === 'string' && typeof record.path === 'string') records.set(record.id, record);
	let changed = records.size !== worktrees.length;
	const linked = new Set([...records.values()].map(record => path.resolve(record.path)));
	for (const session of sessions) { const root = ownWorktreePath(session.worktree); if (root) linked.add(root); }
	const legacyRoot = (session: SessionRecord): string | undefined => {
		if (session.worktree?.id) return undefined;
		const own = ownWorktreePath(session.worktree);
		if (own || (session.worktree && session.worktree.mode !== 'none')) return own;
		if (session.requestedWorktreeMode && session.requestedWorktreeMode !== 'none') return undefined;
		const launch = session.launchWorktreeRoot ?? session.cwd;
		return typeof launch === 'string' && linked.has(path.resolve(launch)) ? path.resolve(launch) : undefined;
	};
	const groups = new Map<string, SessionRecord[]>();
	for (const session of sessions) {
		const id = session.worktree?.id;
		if (id && !records.has(id)) {
			// A reference without its record (edited state): rebuild it from what the session still has.
			records.set(id, {id, path: session.worktree?.path ?? session.launchWorktreeRoot ?? session.cwd, createdAt: session.createdAt, ...worktreeMarkers(session.worktree!)});
			changed = true;
		}
		const root = legacyRoot(session);
		if (root) groups.set(root, [...groups.get(root) ?? [], session]);
	}
	const assigned = new Map<string, string>();
	for (const [root, group] of groups) {
		const deletions = [...new Set(group.flatMap(session => session.worktree?.deletedAt ? [session.worktree.deletedAt] : []))].sort((a, b) => time(a) - time(b));
		const lastLaunch = (session: SessionRecord) => Math.max(time(session.createdAt), time(session.agentStartedAt));
		const incarnations = new Map<string | undefined, SessionRecord[]>();
		for (const session of group) {
			const deletedAt = session.worktree?.deletedAt ?? deletions.find(deletion => lastLaunch(session) <= time(deletion));
			incarnations.set(deletedAt, [...incarnations.get(deletedAt) ?? [], session]);
		}
		for (const [deletedAt, members] of incarnations) {
			const existing = deletedAt ? undefined : liveWorktreeRecord(records.values(), root);
			let record = existing;
			if (!record) {
				const merges: MergeSource[] = members.map(session => session.worktree && session.worktree.mode !== 'none' ? session.worktree : session);
				const latest = merges.filter(source => source.mergedAt).sort((a, b) => time(b.mergedAt) - time(a.mergedAt))[0];
				record = {
					id: newId(),
					path: root,
					createdAt: members.map(session => session.createdAt).sort((a, b) => time(a) - time(b))[0]!,
					...(latest ? worktreeMarkers({mergedAt: latest.mergedAt, mergeMode: latest.mergeMode, mergeTargetBranch: latest.mergeTargetBranch, mergeSourceRef: latest.mergeSourceRef, mergeMarkedManually: latest.mergeMarkedManually}) : {}),
					...(deletedAt ? {deletedAt} : {}),
				};
				records.set(record.id, record);
			}
			for (const session of members) assigned.set(session.id, record.id);
		}
	}
	const migrated = sessions.map(session => {
		const id = assigned.get(session.id);
		if (id) {
			changed = true;
			const lifted: SessionRecord = {...session, worktree: {...session.worktree ?? {mode: 'none'}, id}};
			// A sub-session's top-level marker now belongs to its worktree's record.
			for (const key of ['mergedAt', 'mergeTargetBranch', 'mergeSourceRef', 'mergeMarkedManually'] as const) delete lifted[key];
			return storedSession(lifted);
		}
		const stored = storedSession(session);
		if (stored !== session && WORKTREE_MARKERS.some(key => session.worktree?.[key] !== undefined)) changed = true;
		return stored;
	});
	const referenced = new Set(migrated.flatMap(session => session.worktree?.id ? [session.worktree.id] : []));
	for (const id of records.keys()) if (!referenced.has(id)) { records.delete(id); changed = true; }
	return {sessions: migrated, worktrees: [...records.values()], changed};
}

const SESSION_MERGE_FIELDS = ['mergedAt', 'mergeTargetBranch', 'mergeSourceRef', 'mergeMarkedManually'] as const;

/**
 * Before `D`, `M` doubled as "this task is done" for sessions without a worktree record (the main checkout). Such a
 * session's own merge marker (top-level, or under `worktree` when attached to the main worktree) becomes `doneAt`
 * (kept if already set) and is removed. Run after migrateWorktreeRecords, which has lifted the markers of sessions in
 * a linked worktree into their records; worktree records are untouched. Idempotent.
 */
export function migrateDoneMarkers(sessions: SessionRecord[]): {sessions: SessionRecord[]; changed: boolean} {
	let changed = false;
	const migrated = sessions.map(session => {
		if (session.worktree?.id) return session;
		const nested = session.worktree && session.worktree.mode !== 'none' ? session.worktree : undefined;
		const mergedAt = session.mergedAt ?? nested?.mergedAt;
		const leftovers = SESSION_MERGE_FIELDS.some(key => session[key] !== undefined) || Boolean(nested && MERGE_MARKERS.some(key => nested[key] !== undefined));
		if (!leftovers) return session;
		changed = true;
		const next: SessionRecord = {...session};
		for (const key of SESSION_MERGE_FIELDS) delete next[key];
		if (nested) next.worktree = withoutMarkers(nested, MERGE_MARKERS);
		if (mergedAt && !next.doneAt) next.doneAt = mergedAt;
		return next;
	});
	return {sessions: migrated, changed};
}
