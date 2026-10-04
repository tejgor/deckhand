import React from 'react';
import {Box, Text} from 'ink';
import type {PreviewRecord, SessionRecord} from './types.js';
import {THEME, compactPath, fitLines, plainTerminalText, truncate, statusLabel} from './ui.js';

interface PreviewPaneProps {
	session?: SessionRecord;
	preview: PreviewRecord;
	width: number;
	height: number;
	spinnerFrame: string;
	focused?: boolean;
}

// Show the most recent setup output (the tail), as plain text, below a header line.
function setupMessage(setup: NonNullable<SessionRecord['setup']>, rows: number): string {
	const output = plainTerminalText(setup.output).replace(/\n+$/, '');
	const tail = output && rows > 1 ? output.split('\n').slice(-(rows - 1)) : [];
	return [plainTerminalText(`Setup (${setup.state}): ${setup.command}`).replace(/\n/g, ' '), ...tail].join('\n');
}

function fallbackMessage(session: SessionRecord | undefined, preview: PreviewRecord, rows: number): string {
	if (!session) return 'No sessions yet. Press n to create one.';
	if (session.status === 'starting') return session.setup ? setupMessage(session.setup, rows) : 'Starting session…';
	if (session.status === 'exited' && session.setup && (session.setup.state === 'failed' || session.setup.state === 'cancelled')) {
		// Setup runs before the agent, so its output is the relevant context here.
		const note = preview.content.startsWith('Setup (') ? '' : plainTerminalText(preview.content).trim().split('\n').slice(0, 2).join('\n');
		return note ? `${note}\n${setupMessage(session.setup, rows - note.split('\n').length)}` : setupMessage(session.setup, rows);
	}
	if (session.status === 'exited') return preview.content || 'Session exited.';
	return preview.content || 'Waiting for agent output…';
}

function worktreeBadge(session: SessionRecord): string | undefined {
	if (!session.worktree || session.worktree.mode === 'none') return undefined;
	return session.worktree.mode === 'managed' ? 'worktree' : 'attached';
}

export function PreviewPane({session, preview, width, height, focused = false}: PreviewPaneProps) {
	const badges = [session ? worktreeBadge(session) : undefined, session ? statusLabel(session) : undefined];
	if (focused) {
		badges.push(preview.scrollOffset ? `preview ↑${preview.scrollOffset}` : 'preview focus');
	} else if (preview.scrollOffset) {
		badges.push(`↑${preview.scrollOffset}`);
	}
	// Keep the header on one row: the badge yields to at least 8 columns of path.
	const badge = truncate(badges.filter(Boolean).join(' · '), Math.max(0, width - 9)) || undefined;
	const bodyHeight = Math.max(1, height - 1);
	const lines = fitLines(fallbackMessage(session, preview, bodyHeight), width, bodyHeight);
	const hasContent = Boolean(session && (preview.content || session.status === 'exited'));
	const pathSource = session ? (session.worktree?.path ?? session.cwd) : 'Select a session from the sidebar';
	const pathBudget = Math.max(1, width - (badge ? badge.length + 1 : 0));
	const cwdLabel = session ? compactPath(pathSource, pathBudget) : pathSource;

	return (
		<Box flexDirection="column" width={width} height={height}>
			<Box justifyContent="space-between" width={width}>
				<Text color={THEME.muted} wrap="truncate-end">{truncate(cwdLabel, pathBudget)}</Text>
				{badge ? <Text color={THEME.accent} wrap="truncate-end">{badge}</Text> : null}
			</Box>
			{lines.map((line, index) => (
				<Text
					key={`preview-line-${index}`}
					color={!hasContent && index === 0 ? THEME.active : !hasContent ? THEME.muted : undefined}
				>
					{line}
				</Text>
			))}
		</Box>
	);
}
