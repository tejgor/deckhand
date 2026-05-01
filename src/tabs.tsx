import React from 'react';
import {Box, Text} from 'ink';
import type {RightPaneTab} from './types.js';
import {THEME} from './ui.js';

const TABS: Array<{key: RightPaneTab; label: string; glyph: string}> = [
	{key: 'preview', label: 'Preview', glyph: '◈'},
	{key: 'terminal', label: 'Terminal', glyph: '⌁'},
	{key: 'git', label: 'Git', glyph: '⑂'},
	{key: 'dev', label: 'Dev', glyph: '▹'},
];

export function TabBar({activeTab, width, devRunning = false}: {activeTab: RightPaneTab; width: number; devRunning?: boolean}) {
	return (
		<Box width={width}>
			{TABS.map((tab, index) => {
				const active = tab.key === activeTab;
				return (
					<Box key={tab.key} marginRight={index === TABS.length - 1 ? 0 : 2}>
						<Text color={active ? THEME.active : tab.key === 'dev' && devRunning ? THEME.success : THEME.muted} bold={active} underline={active}>
							{tab.glyph} {tab.label}{tab.key === 'dev' && devRunning ? ' ●' : ''}
						</Text>
					</Box>
				);
			})}
		</Box>
	);
}
