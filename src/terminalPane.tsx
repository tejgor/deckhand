import React from 'react';
import {Box, Text} from 'ink';
import type {ActionRecord, SessionRecord, TerminalRecord} from './types.js';
import {THEME, compactPath, fitLines, truncate} from './ui.js';
import {workspacePaneUnavailable} from './workspace.js';

/** What the Terminal tab shows: the workspace's shell, or the last action run there (v switches). */
export type TerminalView = 'shell' | 'action';

interface TerminalPaneProps {
	session?: SessionRecord;
	terminal: TerminalRecord;
	action: ActionRecord;
	view: TerminalView;
	width: number;
	height: number;
}

/** Whether the selected session's workspace has an action to show (running, or finished with its output kept). */
export function hasAction(session: SessionRecord | undefined, action: ActionRecord): boolean {
	return Boolean(session && action.sessionId === session.id && action.workspace && action.command);
}

/** An action's state: running, or how it ended. */
export function actionStatus(action: ActionRecord): {text: string; color: string} {
	if (action.live) return {text: '● running', color: THEME.success};
	if (action.exitCode === 0 && !action.exitSignal) return {text: '✓ exit 0', color: THEME.success};
	if (action.exitSignal) return {text: `✗ signal ${action.exitSignal}`, color: THEME.error};
	if (typeof action.exitCode === 'number') return {text: `✗ exit ${action.exitCode}`, color: THEME.error};
	return {text: '○ stopped', color: THEME.muted};
}

// The shell belongs to the session's workspace (worktree) and is shared by every session in it, running or not.
function fallbackMessage(session: SessionRecord | undefined, terminal: TerminalRecord): string {
	if (!session) return 'No session selected.';
	const unavailable = workspacePaneUnavailable(session, terminal);
	if (unavailable) return `Terminal is unavailable: ${unavailable}.`;
	if (!terminal.live && terminal.content) return terminal.content;
	if (!terminal.live && terminal.sessionId === session.id) return 'Shell exited. Switch tabs and back to start a new one.';
	return terminal.content || 'Starting terminal…';
}

export function TerminalPane({session, terminal, action, view, width, height}: TerminalPaneProps) {
	const bodyHeight = Math.max(1, height - 1);
	const withAction = hasAction(session, action);
	const showAction = withAction && view === 'action';
	const body = showAction ? action.content || (action.live ? 'Running…' : '(no output)') : fallbackMessage(session, terminal);
	const lines = fitLines(body, width, bodyHeight);
	// With an action, the header names both views (the shown one highlighted) and the action's state.
	const shellStatus = terminal.live ? '● live' : '○ cold';
	const status = actionStatus(action);
	const actionLabel = `${action.name ?? 'action'} ${status.text}`;
	const right = withAction ? `shell │ ${actionLabel}` : shellStatus;
	const pathSource = showAction ? `$ ${action.command}` : terminal.cwd ?? session?.worktree?.path ?? session?.cwd ?? 'Select a session from the sidebar.';
	const leftBudget = Math.max(8, width - right.length - 1);
	const left = showAction ? truncate(pathSource, leftBudget) : compactPath(pathSource, leftBudget);

	return (
		<Box flexDirection="column" width={width} height={height}>
			<Box justifyContent="space-between" width={width}>
				<Text color={THEME.muted}>{truncate(left, leftBudget)}</Text>
				{withAction ? (
					<Text>
						<Text color={showAction ? THEME.muted : THEME.active} bold={!showAction}>shell</Text>
						<Text color={THEME.muted}> │ </Text>
						<Text color={showAction ? THEME.active : THEME.muted} bold={showAction}>{action.name ?? 'action'} </Text>
						<Text color={status.color}>{status.text}</Text>
					</Text>
				) : (
					<Text color={terminal.live ? THEME.success : THEME.muted}>{shellStatus}</Text>
				)}
			</Box>
			{lines.map((line, index) => <Text key={`terminal-line-${index}`}>{line}</Text>)}
		</Box>
	);
}
