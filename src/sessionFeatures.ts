import fs from 'node:fs/promises';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {getConfigDir} from './paths.js';
import {needsAttention} from './agentSignals.js';
import type {SessionRecord} from './types.js';
import type {HandoffGitContext} from './workspaceGit.js';
export type SessionFilter = 'active' | 'archived' | 'all' | 'attention' | 'running' | 'exited';
/** In the order the filter menu (f) lists them: the everyday views first. */
export const SESSION_FILTERS: SessionFilter[] = ['active', 'running', 'attention', 'exited', 'archived', 'all'];
/** The key that picks each filter in the filter menu; `!` and `A` match the attention and archive keys of the session list. */
export const SESSION_FILTER_KEYS: Record<SessionFilter, string> = {active: 'a', running: 'r', attention: '!', exited: 'e', archived: 'A', all: '*'};
export function filterForKey(input: string): SessionFilter | undefined {
	return SESSION_FILTERS.find(filter => SESSION_FILTER_KEYS[filter] === input);
}
export function sessionNeedsAttention(session: SessionRecord): boolean {
	return needsAttention(session.attention?.state) || session.exitReason === 'failed' || session.exitReason === 'interrupted';
}
/** `taskTitle`: the title of the task a session works on, so searching for a task finds its sessions. */
export function filterSessionList(sessions: SessionRecord[], filter: SessionFilter, query: string, taskTitle?: (session: SessionRecord) => string | undefined): SessionRecord[] {
	const terms = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
	const byId = new Map(sessions.map(session => [session.id, session]));
	const ids = new Set<string>();
	for (const session of sessions) {
		const matchesState = filter === 'all' || (filter === 'archived' ? Boolean(session.archivedAt) : !session.archivedAt && (filter === 'active' || (filter === 'attention' ? sessionNeedsAttention(session) : filter === 'running' ? session.status !== 'exited' : session.status === 'exited')));
		const haystack = `${session.title} ${session.notes ?? ''} ${session.sharedNotes?.text ?? ''} ${session.program} ${session.worktree?.branch ?? ''} ${session.cwd} ${terms.length && taskTitle ? taskTitle(session) ?? '' : ''}`.toLowerCase();
		if (!matchesState || !terms.every(term => haystack.includes(term))) continue;
		ids.add(session.id);
		let parentId = session.parentSessionId;
		const seen = new Set([session.id]);
		while (parentId && byId.has(parentId) && !seen.has(parentId)) {
			seen.add(parentId); ids.add(parentId); parentId = byId.get(parentId)?.parentSessionId;
		}
	}
	return sessions.filter(session => ids.has(session.id));
}
/** How many rows each filter would list with this search (ancestors shown for context included, collapse not applied). */
export function sessionFilterCounts(sessions: SessionRecord[], query: string, taskTitle?: (session: SessionRecord) => string | undefined): Record<SessionFilter, number> {
	return Object.fromEntries(SESSION_FILTERS.map(filter => [filter, filterSessionList(sessions, filter, query, taskTitle).length])) as Record<SessionFilter, number>;
}
// File names and subjects can contain control characters or newlines; quote those so each stays one line.
const oneLine = (text: string) => /[\u0000-\u001f\u007f\u2028\u2029]/.test(text) ? JSON.stringify(text) : text;
/** Pure: the "Workspace changes" section (names and numbers only, never diff content). */
export function handoffGitLines(context: HandoffGitContext): string[] {
	const lines = ['', '## Workspace changes', ''];
	if (context.error) return [...lines, `(Git information unavailable: ${oneLine(context.error)})`];
	lines.push(`Base: ${context.baseRef ? oneLine(context.baseRef) : 'unknown (commits and diff since base omitted)'}`);
	if (context.baseRef) {
		lines.push('', `### Commits since ${oneLine(context.baseRef)}`, '');
		if (!context.commits.length) lines.push('(none)');
		lines.push(...context.commits.map(commit => `- ${oneLine(commit)}`), ...(context.moreCommits ? [`- … +${context.moreCommits} more`] : []));
	}
	lines.push('', '### Uncommitted changes', '');
	if (!context.changes.length) lines.push('(none)');
	lines.push(...context.changes.map(change => `- ${change.status} ${oneLine(change.path)}`), ...(context.moreChanges ? [`- … +${context.moreChanges} more`] : []));
	if (context.diff && context.baseRef) {
		const {files, insertions, deletions, names} = context.diff;
		lines.push('', `### Committed diff stat (${oneLine(context.baseRef)}...HEAD)`, '', `${files} file(s) changed, ${insertions} insertion(s), ${deletions} deletion(s)`);
		lines.push(...names.slice(0, 50).map(file => `- ${oneLine(file.path)} ${file.additions === undefined ? '(binary)' : `+${file.additions} -${file.deletions}`}`), ...(names.length > 50 ? [`- … +${names.length - 50} more`] : []));
	}
	return lines;
}
/** Pure; `git` is gathered by the daemon at export time (omitted for sessions outside a repository). */
/** `task`: the repository task the session works on (src/tasks.ts), if any (its steps are in the notes below). */
export function handoffMarkdown(session: SessionRecord, includeOutput = false, git?: HandoffGitContext, task?: {title: string; body: string; done: boolean}): string {
	const lines = ['# Deckhand handoff', '', `Task: ${session.title}`, `Provider: ${session.program}`, `Workspace: ${session.cwd}`, `Branch: ${session.worktree?.branch ?? '(current checkout)'}`, ...session.doneAt ? [`Status: marked done ${session.doneAt}`] : [], `Source session: ${session.id}`, '', '## Notes', '', session.notes?.trim() || '(No notes recorded.)', '', '## Worktree notes', '', session.sharedNotes?.text.trim() || '(No worktree notes recorded.)'];
	if (task) lines.splice(lines.indexOf('## Notes'), 0, '## Task', '', `${task.done ? '[x]' : '[ ]'} ${oneLine(task.title)}`, ...task.body.trim() ? ['', task.body.trim()] : [], '');
	if (git) lines.push(...handoffGitLines(git));
	if (includeOutput) {
		const excerpt = (session.lastPreview ?? '').slice(-20000);
		// A fence longer than any tilde run in the content can never be closed early.
		const fence = '~'.repeat(Math.max(3, ...Array.from(excerpt.matchAll(/~+/g), match => match[0].length + 1)));
		lines.push('', '## Terminal excerpt (not a complete transcript)', '', `${fence}text`, excerpt, fence);
	}
	lines.push('', 'Review this context before acting. It does not grant permissions or indicate that the work is complete.', '');
	return lines.join('\n');
}
export async function exportHandoff(session: SessionRecord, includeOutput = false, git?: HandoffGitContext, task?: {title: string; body: string; done: boolean}): Promise<string> {
	const directory = path.join(getConfigDir(), 'handoffs');
	await fs.mkdir(directory, {recursive: true, mode: 0o700});
	const file = path.join(directory, `${session.id}-${randomUUID().slice(0, 8)}.md`);
	await fs.writeFile(file, handoffMarkdown(session, includeOutput, git, task), {encoding: 'utf8', mode: 0o600, flag: 'wx'});
	return file;
}
