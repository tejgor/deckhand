import React from 'react';
import {Box, Text} from 'ink';
import type {PreviewRecord, SessionRecord} from './types.js';
import {THEME, compactPath, fitLines, truncate} from './ui.js';

interface PreviewPaneProps {
	session?: SessionRecord;
	preview: PreviewRecord;
	width: number;
	height: number;
	spinnerFrame: string;
	focused?: boolean;
}

function fallbackMessage(session: SessionRecord | undefined, preview: PreviewRecord): string {
	if (!session) return 'No sessions yet. Press n to create one.';
	if (session.status === 'starting') return 'Starting session…';
	if (session.status === 'exited') return preview.content || 'Session exited.';
	return preview.content || 'Waiting for agent output…';
}

function worktreeBadge(session: SessionRecord): string | undefined {
	if (!session.worktree || session.worktree.mode === 'none') return undefined;
	return session.worktree.mode === 'managed' ? 'worktree' : 'attached';
}

export function PreviewPane({session, preview, width, height, focused = false}: PreviewPaneProps) {
	const badges = [session ? worktreeBadge(session) : undefined];
	if (focused) {
		badges.push(preview.scrollOffset ? `preview ↑${preview.scrollOffset}` : 'preview focus');
	} else if (preview.scrollOffset) {
		badges.push(`↑${preview.scrollOffset}`);
	}
	const badge = badges.filter(Boolean).join(' · ') || undefined;
	const bodyHeight = Math.max(1, height - 1);
	const lines = fitLines(fallbackMessage(session, preview), width, bodyHeight);
	const hasContent = Boolean(session && (preview.content || session.status === 'exited'));
	const pathSource = session ? (session.worktree?.path ?? session.cwd) : 'Select a session from the sidebar';
	const pathBudget = Math.max(8, width - (badge?.length ?? 0) - 1);
	const cwdLabel = session ? compactPath(pathSource, pathBudget) : pathSource;

	return (
		<Box flexDirection="column" width={width} height={height}>
			<Box justifyContent="space-between" width={width}>
				<Text color={THEME.muted}>{truncate(cwdLabel, pathBudget)}</Text>
				{badge ? <Text color={THEME.accent}>{badge}</Text> : null}
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
