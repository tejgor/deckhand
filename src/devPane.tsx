import React from 'react';
import {Box, Text} from 'ink';
import type {DevRecord, SessionRecord} from './types.js';
import {THEME, compactPath, fitLines, truncate} from './ui.js';
import {workspacePaneUnavailable} from './workspace.js';

interface DevPaneProps {
	session?: SessionRecord;
	dev: DevRecord;
	width: number;
	height: number;
}

// Dev belongs to the session's workspace (worktree) and is shared by every session in it, running or not.
function fallbackMessage(session: SessionRecord | undefined, dev: DevRecord): string {
	if (!session) return 'No session selected.';
	const unavailable = workspacePaneUnavailable(session, dev);
	if (unavailable) return `Dev is unavailable: ${unavailable}.`;
	if (!dev.live && dev.content) return dev.content;
	if (!dev.live && dev.sessionId === session.id) return 'Dev command exited. r starts it again (Dev is first in the run list).';
	return dev.content || 'r starts the Dev command (first in the run list; shared by every session in this worktree).';
}

export function DevPane({session, dev, width, height}: DevPaneProps) {
	const bodyHeight = Math.max(1, height - 1);
	const lines = fitLines(fallbackMessage(session, dev), width, bodyHeight);
	const status = dev.live ? '● live' : '○ idle';
	const command = dev.command ? ` · ${dev.command}` : '';
	const pathSource = dev.cwd ?? session?.worktree?.path ?? session?.cwd ?? 'Select a session from the sidebar.';
	const cwdBudget = Math.max(8, width - status.length - 1 - command.length);
	const cwd = compactPath(pathSource, cwdBudget);

	return (
		<Box flexDirection="column" width={width} height={height}>
			<Box justifyContent="space-between" width={width}>
				<Text color={THEME.muted}>{truncate(`${cwd}${command}`, cwdBudget + command.length)}</Text>
				<Text color={dev.live ? THEME.success : THEME.muted}>{status}</Text>
			</Box>
			{lines.map((line, index) => <Text key={`dev-line-${index}`}>{line}</Text>)}
		</Box>
	);
}
