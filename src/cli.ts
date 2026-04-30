#!/usr/bin/env node
import React from 'react';
import {render} from 'ink';
import {attachSession} from './attach.js';
import {App} from './app.js';
import {InkDaemon} from './daemon.js';
import {ensureGitRepo} from './git.js';
import {runSessionWorker} from './sessionWorker.js';
import {loadAppConfig} from './storage.js';
import {resetTerminalState} from './terminalState.js';
import type {RightPaneTab, UiExitResult} from './types.js';

process.title = 'deckhand';

function clearTerminalScreen(): void {
	if (process.stdout.isTTY) {
		resetTerminalState();
		process.stdout.write('\x1b[?47l\x1b[?1047l\x1b[?1049l\x1b[2J\x1b[H');
	}
}

function enterAlternateScreen(): void {
	if (process.stdout.isTTY) {
		resetTerminalState();
		process.stdout.write('\x1b[?1049h\x1b[2J\x1b[H');
	}
}

function leaveAlternateScreen(): void {
	if (process.stdout.isTTY) {
		resetTerminalState();
		process.stdout.write('\x1b[?1049l');
	}
}

async function runUi(uiState: {selectedId?: string; activeTab?: RightPaneTab; sidebarWidth?: number}): Promise<UiExitResult | undefined> {
	const repoRoot = await ensureGitRepo(process.cwd());
	enterAlternateScreen();
	const instance = render(
		React.createElement(App, {
			repoRoot,
			cwd: repoRoot,
			initialSelectedId: uiState.selectedId,
			initialActiveTab: uiState.activeTab,
			initialSidebarWidth: uiState.sidebarWidth,
			onSelectedIdChange: sessionId => {
				uiState.selectedId = sessionId;
			},
			onActiveTabChange: tab => {
				uiState.activeTab = tab;
			},
			onSidebarWidthChange: width => {
				uiState.sidebarWidth = width;
			},
		}),
		{
			exitOnCtrlC: true,
			patchConsole: false,
		},
	);
	try {
		return (await instance.waitUntilExit()) as UiExitResult | undefined;
	} finally {
		instance.clear();
		instance.cleanup();
		leaveAlternateScreen();
		clearTerminalScreen();
	}
}

async function main(): Promise<void> {
	if (process.argv.includes('--session-worker')) {
		await runSessionWorker();
		return;
	}

	if (process.argv.includes('--daemon')) {
		const daemon = new InkDaemon();
		await daemon.start();
		await new Promise(() => {});
		return;
	}

	const uiState: {selectedId?: string; activeTab?: RightPaneTab; sidebarWidth?: number} = {};
	while (true) {
		const result = await runUi(uiState);
		if (!result || result.kind === 'quit') {
			return;
		}
		if (result.kind === 'attach') {
			uiState.selectedId = result.sessionId;
			uiState.activeTab = result.target === 'terminal' ? 'terminal' : result.target === 'git' ? 'git' : result.target === 'dev' ? 'dev' : 'preview';
			clearTerminalScreen();
			try {
				const config = await loadAppConfig();
				await attachSession(result.sessionId, result.target, {title: result.title, scrollSensitivity: config.attach_scroll_sensitivity});
			} catch (error) {
				const message = error instanceof Error ? error.message : String(error);
				process.stderr.write(`\nattach failed: ${message}\n`);
				await new Promise(resolve => setTimeout(resolve, 1500));
			}
			clearTerminalScreen();
		}
	}
}

main().catch(error => {
	console.error(error instanceof Error ? error.message : String(error));
	process.exit(1);
});
