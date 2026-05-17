import React from 'react';
import {Box, Text} from 'ink';
import type {RightPaneTab} from './types.js';
import {THEME} from './ui.js';

const TABS: Array<{key: RightPaneTab; label: string; glyph: string; hotkey: string}> = [
	{key: 'preview', label: 'Preview', glyph: '◈', hotkey: 'p'},
	{key: 'terminal', label: 'Terminal', glyph: '⌁', hotkey: 't'},
	{key: 'git', label: 'Git', glyph: '⑂', hotkey: 'g'},
	{key: 'dev', label: 'Dev', glyph: '▹', hotkey: 'd'},
];

export function TabBar({activeTab, width, devRunning = false}: {activeTab: RightPaneTab; width: number; devRunning?: boolean}) {
	return (
		<Box width={width}>
			{TABS.map((tab, index) => {
				const active = tab.key === activeTab;
				return (
					<Box key={tab.key} marginRight={index === TABS.length - 1 ? 0 : 2}>
						<Text color={active ? THEME.active : tab.key === 'dev' && devRunning ? THEME.success : THEME.muted} bold={active} underline={active}>
							{tab.glyph} {tab.hotkey}:{tab.label}{tab.key === 'dev' && devRunning ? ' ●' : ''}
						</Text>
					</Box>
				);
			})}
		</Box>
	);
}
