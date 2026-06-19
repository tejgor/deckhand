import type {SessionRecord} from './types.js';

function orderValue(session: SessionRecord): number {
	return typeof session.sidebarOrder === 'number' && Number.isFinite(session.sidebarOrder)
		? session.sidebarOrder
		: 0;
}

export function compareSessionOrder(a: SessionRecord, b: SessionRecord): number {
	const orderDelta = orderValue(a) - orderValue(b);
	if (orderDelta !== 0) return orderDelta;
	const createdDelta = a.createdAt.localeCompare(b.createdAt);
	if (createdDelta !== 0) return createdDelta;
	return a.id.localeCompare(b.id);
}

export function sortSessionsForSidebar(sessions: SessionRecord[]): SessionRecord[] {
	const byId = new Map(sessions.map(session => [session.id, session]));
	const children = new Map<string, SessionRecord[]>();
	const roots: SessionRecord[] = [];

	for (const session of sessions) {
		const parentId = session.parentSessionId;
		if (parentId && byId.has(parentId)) {
			const siblings = children.get(parentId) ?? [];
			siblings.push(session);
			children.set(parentId, siblings);
		} else {
			roots.push(session);
		}
	}

	roots.sort(compareSessionOrder);
	for (const siblings of children.values()) {
		siblings.sort(compareSessionOrder);
	}

	const ordered: SessionRecord[] = [];
	const visited = new Set<string>();
	const visit = (session: SessionRecord) => {
		if (visited.has(session.id)) return;
		visited.add(session.id);
		ordered.push(session);
		for (const child of children.get(session.id) ?? []) {
			visit(child);
		}
	};

	for (const root of roots) {
		visit(root);
	}
	for (const session of [...sessions].sort(compareSessionOrder)) {
		visit(session);
	}
	return ordered;
}

export function sessionDepth(session: SessionRecord, sessions: SessionRecord[]): number {
	const byId = new Map(sessions.map(item => [item.id, item]));
	let depth = 0;
	let parentId = session.parentSessionId;
	const seen = new Set<string>([session.id]);
	while (parentId && byId.has(parentId) && !seen.has(parentId)) {
		seen.add(parentId);
		depth += 1;
		parentId = byId.get(parentId)?.parentSessionId;
	}
	return depth;
}

export function sessionHasChildren(sessionId: string, sessions: SessionRecord[]): boolean {
	return sessions.some(session => session.parentSessionId === sessionId);
}

export function countSessionDescendants(sessionId: string, sessions: SessionRecord[]): number {
	const children = new Map<string, SessionRecord[]>();
	for (const session of sessions) {
		if (!session.parentSessionId) continue;
		const siblings = children.get(session.parentSessionId) ?? [];
		siblings.push(session);
		children.set(session.parentSessionId, siblings);
	}

	let count = 0;
	const stack = [...(children.get(sessionId) ?? [])];
	const seen = new Set<string>([sessionId]);
	while (stack.length > 0) {
		const child = stack.pop()!;
		if (seen.has(child.id)) continue;
		seen.add(child.id);
		count += 1;
		stack.push(...(children.get(child.id) ?? []));
	}
	return count;
}

export function filterCollapsedSessions(sessions: SessionRecord[], collapsedSessionIds: ReadonlySet<string>): SessionRecord[] {
	if (collapsedSessionIds.size === 0) return sessions;
	const byId = new Map(sessions.map(session => [session.id, session]));
	return sessions.filter(session => {
		let parentId = session.parentSessionId;
		const seen = new Set<string>([session.id]);
		while (parentId && byId.has(parentId) && !seen.has(parentId)) {
			if (collapsedSessionIds.has(parentId)) return false;
			seen.add(parentId);
			parentId = byId.get(parentId)?.parentSessionId;
		}
		return true;
	});
}
