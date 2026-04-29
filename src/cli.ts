#!/usr/bin/env node
import React from 'react';
import {render} from 'ink';
import {attachSession} from './attach.js';
import {App} from './app.js';
import {InkDaemon} from './daemon.js';
import {ensureGitRepo} from './git.js';
import {runSessionWorker} from './sessionWorker.js';
import {loadAppConfig} from './storage.js';
import type {UiExitResult} from './types.js';

process.title = 'deckhand';

function resetTerminalState(): void {
	if (!process.stdout.isTTY) {
		return;
	}

	process.stdout.write([
		'\x1b[0m',
		'\x1b[?25h',
		'\x1b[?7h',
		'\x1b[?6l',
		'\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l',
		'\x1b[?2004l',
		'\x1b[r',
	].join(''));
}

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

async function runUi(sidebarWidth: {current?: number}): Promise<UiExitResult | undefined> {
	const repoRoot = await ensureGitRepo(process.cwd());
	enterAlternateScreen();
	const instance = render(
		React.createElement(App, {
			repoRoot,
			cwd: repoRoot,
			initialSidebarWidth: sidebarWidth.current,
			onSidebarWidthChange: width => {
				sidebarWidth.current = width;
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

	const sidebarWidth: {current?: number} = {};
	while (true) {
		const result = await runUi(sidebarWidth);
		if (!result || result.kind === 'quit') {
			return;
		}
		if (result.kind === 'attach') {
			clearTerminalScreen();
			try {
				const config = await loadAppConfig();
				await attachSession(result.sessionId, result.target, {title: result.title, cwd: result.cwd, scrollSensitivity: config.attach_scroll_sensitivity});
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
