import path from 'node:path';
import {createHash} from 'node:crypto';
import type {SessionRecord} from './types.js';

// A workspace is the git worktree a session runs in. It is derived, never stored: every session in one
// worktree shares the workspace's companion panes (Terminal, Git and Dev), which outlive the agents.

/**
 * The workspace root identifying the session's worktree, or undefined when it has none:
 * - worktree-backed sessions (managed/attached, including one attached to the main checkout): the worktree path
 * - sessions without a worktree: the launch checkout's root (`launchWorktreeRoot`, else `cwd` for old records)
 * - a deleted worktree has none (`worktree.deletedAt`, projected from the worktree's record onto every session of it,
 *   src/worktreeRecords.ts), so an old session never shares a workspace with a new worktree later created at that
 *   path; neither has a session still waiting for its new/selected worktree (it would otherwise resolve to the
 *   checkout it was launched from).
 * Lexical (path.resolve); every path here comes from Git (`rev-parse --show-toplevel`, `worktree list`).
 */
export function workspaceKey(session: Pick<SessionRecord, 'cwd' | 'launchWorktreeRoot' | 'worktree' | 'requestedWorktreeMode'>): string | undefined {
	const worktree = session.worktree;
	if (worktree?.deletedAt) return undefined;
	if (worktree && worktree.mode !== 'none') return worktree.path ? path.resolve(worktree.path) : undefined;
	if (session.requestedWorktreeMode && session.requestedWorktreeMode !== 'none') return undefined;
	return path.resolve(session.launchWorktreeRoot ?? session.cwd);
}

/** Why `workspaceKey` is undefined, for errors and the workspace panes. */
export function noWorkspaceReason(session: Pick<SessionRecord, 'worktree'>): string {
	return session.worktree?.deletedAt ? 'its worktree was deleted' : 'its worktree is not ready yet';
}

/**
 * Why the session's workspace panes (Terminal, Git, Dev) are unavailable, or undefined when they are available. The
 * daemon also treats a session whose worktree it is still preparing as having none, and says so with a record for
 * that session without a `workspace`.
 */
export function workspacePaneUnavailable(session: Parameters<typeof workspaceKey>[0] & Pick<SessionRecord, 'id'>, record: {sessionId?: string; workspace?: string}): string | undefined {
	return !workspaceKey(session) || (record.sessionId === session.id && !record.workspace) ? noWorkspaceReason(session) : undefined;
}

/** File-name-safe identity of a workspace worker (PID/log files live beside session workers'). */
export function workspaceWorkerId(workspace: string): string {
	return `workspace-${createHash('sha256').update(workspace).digest('hex').slice(0, 16)}`;
}
