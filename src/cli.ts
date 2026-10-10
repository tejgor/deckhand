#!/usr/bin/env node
import {spawnSync} from 'node:child_process';
import {attachSession} from './attach.js';
import {isVimFamily, terminalEditorArgs} from './desktop.js';
import {writeNotesVimScript} from './notesVim.js';
import {ensureGitRepo} from './git.js';
import {StringDecoder} from 'node:string_decoder';
import {loadAppConfig} from './storage.js';
import {loadUiState, saveUiState, type UiState} from './uiState.js';
import {errorMessage} from './ui.js';
import {request} from './client.js';
import {randomUUID} from 'node:crypto';
import {hookPayloadFields, hookSettings} from './agentSignals.js';
import {getConfigDir, isSameConfigDir} from './paths.js';
import {resetTerminalState} from './terminalState.js';
import type {RightPaneTab, UiExitResult} from './types.js';

process.title = process.env.DECKHAND_CHANNEL === 'dev' ? 'deckhand-dev' : 'deckhand';

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

// The Terminal tab's view (shell or last action) survives attach/detach, not restarts.
let terminalView: 'shell' | 'action' = 'shell';

async function runUi(uiState: UiState): Promise<UiExitResult | undefined> {
	// React picks its build from NODE_ENV when first loaded; the development build's checks roughly double the
	// UI's CPU per render. Keep it for `npm run dev` (DECKHAND_DEV) and whenever NODE_ENV is set explicitly.
	// Set only while the modules load: the daemon this UI may start inherits its environment, and with it every agent,
	// shell, Dev command and action, where NODE_ENV=production makes npm skip devDependencies.
	const setNodeEnv = !process.env.NODE_ENV && process.env.DECKHAND_DEV !== '1';
	if (setNodeEnv) process.env.NODE_ENV = 'production';
	const [{default: React}, {render}, {App}] = await Promise.all([import('react'), import('ink'), import('./app.js')]);
	if (setNodeEnv) delete process.env.NODE_ENV;
	const repoRoot = await ensureGitRepo(process.cwd());
	let saveTimer: NodeJS.Timeout | undefined;
	const scheduleSave = () => {
		if (saveTimer) clearTimeout(saveTimer);
		saveTimer = setTimeout(() => { void saveUiState(repoRoot, uiState).catch(() => {}); }, 250);
	};
	enterAlternateScreen();
	const instance = render(
		React.createElement(App, {
			repoRoot,
			cwd: repoRoot,
			initialSelectedId: uiState.selectedId,
			initialActiveTab: uiState.activeTab,
			initialSidebarWidth: uiState.sidebarWidth,
			initialSessionTabs: uiState.sessionTabs,
			initialCollapsedSessionIds: uiState.collapsedSessionIds,
			initialHiddenExitedSessionIds: uiState.hiddenExitedSessionIds,
			initialSessionFilter: uiState.sessionFilter,
			initialSessionQuery: uiState.sessionQuery,
			initialTerminalView: terminalView,
			onTerminalViewChange: view => { terminalView = view; },
			onSessionVisibilityChange: (filter, query) => { uiState.sessionFilter = filter; uiState.sessionQuery = query; scheduleSave(); },
			onSelectedIdChange: sessionId => {
				uiState.selectedId = sessionId;
				scheduleSave();
			},
			onActiveTabChange: tab => {
				uiState.activeTab = tab;
				scheduleSave();
			},
			onSessionTabChange: (sessionId, tab) => {
				uiState.sessionTabs[sessionId] = tab;
				scheduleSave();
			},
			onSidebarWidthChange: width => {
				uiState.sidebarWidth = width;
				scheduleSave();
			},
			onCollapsedSessionIdsChange: sessionIds => {
				uiState.collapsedSessionIds = sessionIds;
				scheduleSave();
			},
			onHiddenExitedSessionIdsChange: sessionIds => {
				uiState.hiddenExitedSessionIds = sessionIds;
				scheduleSave();
			},
		}),
		{
			// App handles Ctrl+C: Esc-like on the first press in the config editor, quit otherwise.
			exitOnCtrlC: false,
			patchConsole: false,
		},
	);
	try {
		return (await instance.waitUntilExit()) as UiExitResult | undefined;
	} finally {
		instance.clear();
		instance.cleanup();
		if (saveTimer) clearTimeout(saveTimer);
		// Restore the terminal before any I/O that can fail.
		leaveAlternateScreen();
		clearTerminalScreen();
		await saveUiState(repoRoot, uiState).catch(error => {
			process.stderr.write(`deckhand: could not save UI state: ${errorMessage(error)}\n`);
		});
	}
}

const MAX_HOOK_INPUT_BYTES = 8 * 1024 * 1024;

async function main(): Promise<void> {
	if (process.env.DECKHAND_CHANNEL === 'dev' && ['status', 'stop'].includes(process.argv[2] ?? '')) {
		let daemon: {home: string; channel: string; version: number};
		try { daemon = await request({type: 'ping', requestId: randomUUID()}, 1500); }
		catch { console.log(`No isolated dev daemon running (${getConfigDir()})`); return; }
		if (!isSameConfigDir(daemon.home) || daemon.channel !== 'dev') throw new Error('Refusing to control a daemon outside the isolated dev namespace');
		if (process.argv[2] === 'stop') { await request({type: 'shutdown', requestId: randomUUID()}, 1500); console.log('Stopping isolated dev daemon. Production daemon untouched.'); }
		else console.log(`Isolated dev daemon running (protocol v${daemon.version}): ${daemon.home}`);
		return;
	}
	if (process.argv[2] === 'hook') {
		// Hooks are advisory, bounded, and never approve or block agent actions.
		// Claude runs them async, so they can arrive out of order: the daemon drops a signal older than the last one.
		const sentAt = Date.now();
		const inputTimer = setTimeout(() => process.stdin.destroy(new Error('Hook input timed out')), 500);
		try {
			let raw = '', bytes = 0;
			const decoder = new StringDecoder('utf8');
			// Large PostToolUse payloads are read (bounded) so their event isn't lost; only signal fields are sent (hookPayloadFields).
			for await (const chunk of process.stdin) { bytes += Buffer.byteLength(chunk); if (bytes > MAX_HOOK_INPUT_BYTES) throw new Error('Hook input too large'); raw += decoder.write(chunk); }
			raw += decoder.end(); clearTimeout(inputTimer);
			const sessionId = process.env.DECKHAND_SESSION_ID;
			const launchId = process.env.DECKHAND_LAUNCH_ID;
			const token = process.env.DECKHAND_HOOK_TOKEN;
			if (sessionId && launchId && token) await request({type: 'agent-hook', requestId: randomUUID(), sessionId, launchId, token, payload: hookPayloadFields(JSON.parse(raw)), sentAt}, 1200);
		} catch { /* Disconnected/unsupported callbacks must not affect permissions. */ }
		finally { clearTimeout(inputTimer); }
		process.stdout.write('{}\n');
		return;
	}
	if (process.argv[2] === 'hooks') {
		const settings = hookSettings();
		if (process.argv[3] === 'codex') { delete settings.hooks.Notification; delete settings.hooks.StopFailure; }
		else delete settings.hooks.Interrupt;
		process.stdout.write(`${JSON.stringify(settings, null, 2)}\n`);
		return;
	}
	if (process.argv.includes('--session-worker')) {
		const {runSessionWorker} = await import('./sessionWorker.js');
		await runSessionWorker();
		return;
	}
	if (process.argv.includes('--workspace-worker')) {
		const {runWorkspaceWorker} = await import('./sessionWorker.js');
		await runWorkspaceWorker();
		return;
	}

	if (process.argv.includes('--daemon')) {
		const {InkDaemon} = await import('./daemon.js');
		const daemon = new InkDaemon();
		await daemon.start();
		await new Promise(() => {});
		return;
	}

	if (process.argv[2] === 'setup' || process.argv[2] === 'doctor') {
		const {runSetup} = await import('./setup.js');
		await runSetup(process.argv.slice(3));
		return;
	}

	const uiState = await loadUiState(await ensureGitRepo(process.cwd()));
	while (true) {
		const result = await runUi(uiState);
		if (!result || result.kind === 'quit') {
			return;
		}
		if (result.kind === 'attach') {
			uiState.selectedId = result.sessionId;
			uiState.activeTab = result.target === 'terminal' || result.target === 'action' ? 'terminal' : result.target === 'git' ? 'git' : result.target === 'dev' ? 'dev' : 'preview';
			uiState.sessionTabs[result.sessionId] = uiState.activeTab;
			clearTerminalScreen();
			try {
				const config = await loadAppConfig();
				await attachSession(result.sessionId, result.target, {title: result.title, program: result.program, scrollSensitivity: config.attach_scroll_sensitivity});
			} catch (error) {
				const message = error instanceof Error ? error.message : String(error);
				process.stderr.write(`\nattach failed: ${message}\n`);
				await new Promise(resolve => setTimeout(resolve, 1500));
			}
			clearTerminalScreen();
		}
		if (result.kind === 'edit-note') {
			uiState.selectedId = result.sessionId;
			uiState.activeTab = 'notes';
			uiState.sessionTabs[result.sessionId] = 'notes';
			clearTerminalScreen();
			// Without the script vim still edits the note; only the Deckhand keys are missing.
			const vimScript = isVimFamily(result.editor) ? await writeNotesVimScript().catch(() => undefined) : undefined;
			// Synchronous: the editor owns the terminal and stdin until it quits. The daemon's watcher picks up the saved
			// file, so the Notes tab shows it when the UI comes back.
			const editor = spawnSync(result.editor.command, terminalEditorArgs(result.editor, result.file, vimScript), {stdio: 'inherit'});
			// Only a failed launch is reported: vim exits non-zero after any error message seen while editing.
			if (editor.error) {
				process.stderr.write(`\n${result.editor.command} failed: ${errorMessage(editor.error)}; the note is ${result.file}\n`);
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
