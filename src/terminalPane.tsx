import React from 'react';
import {Box, Text} from 'ink';
import type {SessionRecord, TerminalRecord} from './types.js';
import {THEME, compactPath, fitLines, truncate} from './ui.js';
import {workspacePaneUnavailable} from './workspace.js';

interface TerminalPaneProps {
	session?: SessionRecord;
	terminal: TerminalRecord;
	width: number;
	height: number;
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

export function TerminalPane({session, terminal, width, height}: TerminalPaneProps) {
	const bodyHeight = Math.max(1, height - 1);
	const lines = fitLines(fallbackMessage(session, terminal), width, bodyHeight);
	const status = terminal.live ? '● live' : '○ cold';
	const pathSource = terminal.cwd ?? session?.worktree?.path ?? session?.cwd ?? 'Select a session from the sidebar.';
	const cwdBudget = Math.max(8, width - status.length - 1);
	const cwd = compactPath(pathSource, cwdBudget);

	return (
		<Box flexDirection="column" width={width} height={height}>
			<Box justifyContent="space-between" width={width}>
				<Text color={THEME.muted}>{truncate(cwd, cwdBudget)}</Text>
				<Text color={terminal.live ? THEME.success : THEME.muted}>{status}</Text>
			</Box>
			{lines.map((line, index) => <Text key={`terminal-line-${index}`}>{line}</Text>)}
		</Box>
	);
}
