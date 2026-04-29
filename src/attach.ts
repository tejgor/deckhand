import {randomUUID} from 'node:crypto';
import process from 'node:process';
import type net from 'node:net';
import {attachJsonParser, openPersistentConnection, writeMessage} from './client.js';
import type {AttachTarget, SessionRecord} from './types.js';

interface AttachSessionOptions {
	title?: string;
	cwd?: string;
	scrollSensitivity?: number;
}

function clearTerminalScreen(): void {
	if (process.stdout.isTTY) {
		process.stdout.write('\x1b[2J\x1b[H');
	}
}

function attachTargetLabel(target: AttachTarget): string {
	return target === 'terminal' ? 'Terminal' : target === 'git' ? 'Git' : target === 'dev' ? 'Dev' : 'Agent';
}

function targetRequestNames(target: AttachTarget) {
	if (target === 'terminal') {
		return {
			attach: 'attach-terminal',
			input: 'terminal-input',
			resize: 'terminal-resize',
			detach: 'terminal-detach',
			output: 'terminal-output',
			detached: 'terminal-detached',
		} as const;
	}
	if (target === 'git') {
		return {
			attach: 'attach-git',
			input: 'git-input',
			resize: 'git-resize',
			detach: 'git-detach',
			output: 'git-output',
			detached: 'git-detached',
		} as const;
	}
	if (target === 'dev') {
		return {
			attach: 'attach-dev',
			input: 'dev-input',
			resize: 'dev-resize',
			detach: 'dev-detach',
			output: 'dev-output',
			detached: 'dev-detached',
		} as const;
	}
	return {
		attach: 'attach',
		input: 'input',
		resize: 'resize',
		detach: 'detach',
		output: 'output',
		detached: 'detached',
	} as const;
}

function compactPath(value: string | undefined, maxLength: number): string | undefined {
	if (!value || value.length <= maxLength) {
		return value;
	}
	if (maxLength <= 3) {
		return value.slice(-maxLength);
	}
	return `…${value.slice(-(maxLength - 1))}`;
}

function sanitizeTerminalTitle(value: string): string {
	return value.replace(/[\u0000-\u001F\u007F-\u009F]/g, ' ').replace(/\s+/g, ' ').trim();
}

function setTerminalTitle(value: string): void {
	if (!process.stdout.isTTY) {
		return;
	}
	const title = sanitizeTerminalTitle(value);
	process.stdout.write(`\x1b]0;${title}\x07\x1b]2;${title}\x07`);
}

function setProcessTitle(value: string): void {
	try {
		process.title = sanitizeTerminalTitle(value);
	} catch {
		// Best-effort only. Some platforms/hosts ignore process title changes.
	}
}

function normalizeTerminalOutput(data: string): string {
	// Some PTY programs emit 8-bit C1 controls (for example CSI as U+009B).
	// Many UTF-8 terminals do not interpret those reliably and the printable tail
	// leaks onto the screen as text like "40;3H" or ";1". Convert them to the
	// equivalent 7-bit ESC-prefixed sequences before forwarding to stdout.
	return data
		.replace(/\u008E/g, '\x1bN')
		.replace(/\u008F/g, '\x1bO')
		.replace(/\u0090/g, '\x1bP')
		.replace(/\u009B/g, '\x1b[')
		.replace(/\u009D/g, '\x1b]')
		.replace(/\u009E/g, '\x1b^')
		.replace(/\u009F/g, '\x1b_');
}

function normalizeScrollSensitivity(value: number | undefined): number {
	if (value === undefined || !Number.isFinite(value)) {
		return 0.25;
	}
	return Math.max(0, Math.min(1, value));
}

function createAttachInputNormalizer(scrollSensitivity: number): (data: string) => string {
	let verticalWheelAccumulator = 0;
	const wheelPattern = /\x1b\[<(6[4-7]);\d+;\d+[mM]/g;

	return (data: string) => data.replace(wheelPattern, match => {
		const code = Number.parseInt(match.slice(3, 5), 10);
		if (code === 64 || code === 65) {
			verticalWheelAccumulator += scrollSensitivity;
			if (verticalWheelAccumulator < 1) {
				return '';
			}
			verticalWheelAccumulator -= 1;
		}
		return match;
	});
}

function attachTargetTitleLabel(target: AttachTarget): string {
	return target === 'terminal' ? 'term' : target === 'git' ? 'git' : target === 'dev' ? 'dev' : 'agent';
}

function compactTerminalTitle(value: string, maxLength = 32): string {
	const title = sanitizeTerminalTitle(value);
	if (title.length <= maxLength) {
		return title;
	}
	return maxLength <= 1 ? title.slice(0, maxLength) : `${title.slice(0, maxLength - 1)}…`;
}

function attachedTerminalTitle(sessionId: string, target: AttachTarget, options: AttachSessionOptions): string {
	const title = compactTerminalTitle(options.title ?? sessionId);
	const label = attachTargetTitleLabel(target);
	return `ad/${label} ${title}`;
}

function writeAttachBanner(sessionId: string, target: AttachTarget, options: AttachSessionOptions): void {
	const title = options.title ?? sessionId;
	const label = attachTargetLabel(target);
	const terminalWidth = process.stdout.columns || 80;
	const width = Math.max(44, Math.min(terminalWidth, 88));
	const cwd = compactPath(options.cwd, Math.max(12, width - title.length - label.length - 18));
	const location = cwd ? ` • ${cwd}` : '';
	const headline = `deckhand ${label}: ${title}${location}`;
	const help = 'Ctrl+Space returns to deckhand';
	const border = '─'.repeat(Math.max(0, width - 2));
	const formatLine = (line: string) => {
		const content = line.length > width - 4 ? `${line.slice(0, width - 5)}…` : line;
		return `│ ${content}${' '.repeat(Math.max(0, width - content.length - 3))}│`;
	};

	clearTerminalScreen();
	process.stdout.write(`┌${border}┐\n`);
	process.stdout.write(`${formatLine(headline)}\n`);
	process.stdout.write(`${formatLine(help)}\n`);
	process.stdout.write(`└${border}┘\n\n`);
}

export async function attachSession(sessionId: string, target: AttachTarget = 'agent', options: AttachSessionOptions = {}): Promise<void> {
	if (target !== 'agent' && target !== 'git') {
		writeAttachBanner(sessionId, target, options);
	}
	const socket = await openPersistentConnection();
	const requestId = randomUUID();
	const names = targetRequestNames(target);
	const normalizeAttachInput = createAttachInputNormalizer(normalizeScrollSensitivity(options.scrollSensitivity));
	const originalProcessTitle = process.title || 'deckhand';
	let attached = false;
	let cleanedUp = false;
	let titleSet = false;

	await new Promise<void>((resolve, reject) => {
		const restoreRawMode = () => {
			if (process.stdin.isTTY) {
				process.stdin.setRawMode?.(false);
			}
		};

		const cleanup = () => {
			if (cleanedUp) {
				return;
			}
			cleanedUp = true;
			restoreRawMode();
			if (titleSet) {
				setTerminalTitle('deckhand');
				setProcessTitle(originalProcessTitle);
			}
			process.stdout.off('resize', onResize);
			process.stdin.off('data', onInput);
			stopParsing();
			socket.removeAllListeners('error');
			socket.removeAllListeners('close');
			if (!socket.destroyed) {
				socket.end();
			}
		};

		const finish = (error?: unknown) => {
			cleanup();
			if (error) {
				reject(error instanceof Error ? error : new Error(String(error)));
				return;
			}
			resolve();
		};

		const onResize = () => {
			writeMessage(socket, {
				type: names.resize,
				sessionId,
				cols: process.stdout.columns || 80,
				rows: process.stdout.rows || 24,
			});
		};

		const onInput = (data: Buffer | string) => {
			const chunk = Buffer.isBuffer(data) ? data : Buffer.from(data);
			if (chunk.includes(0x00)) {
				writeMessage(socket, {type: names.detach, sessionId});
				finish();
				return;
			}
			const normalized = normalizeAttachInput(chunk.toString('utf8'));
			if (normalized) {
				writeMessage(socket, {type: names.input, sessionId, data: normalized});
			}
		};

		const stopParsing = attachJsonParser(socket, message => {
			if (message.type === 'response' && message.requestId === requestId) {
				if (!message.ok) {
					finish(new Error(message.error || 'attach failed'));
					return;
				}
				attached = true;
				const nextTitle = attachedTerminalTitle(sessionId, target, options);
				setTerminalTitle(nextTitle);
				setProcessTitle(nextTitle);
				titleSet = true;
				if (process.stdin.isTTY) {
					process.stdin.setRawMode?.(true);
				}
				process.stdin.resume();
				process.stdin.on('data', onInput);
				process.stdout.on('resize', onResize);
				onResize();
				return;
			}

			if (message.type === names.output && message.sessionId === sessionId) {
				process.stdout.write(normalizeTerminalOutput(message.data));
				return;
			}

			if (message.type === 'session-updated' && message.session.id === sessionId) {
				const session = message.session as SessionRecord;
				if (session.status === 'exited') {
					process.stdout.write(`\n\r[session exited: ${session.title}]\n`);
					finish();
				}
				return;
			}

			if (message.type === names.detached && message.sessionId === sessionId) {
				finish();
			}
		});

		socket.once('error', error => finish(error));
		socket.once('close', () => {
			if (!cleanedUp && attached) {
				finish(new Error('daemon connection closed during attach'));
			}
		});

		writeMessage(socket, {
			type: names.attach,
			requestId,
			sessionId,
			cols: process.stdout.columns || 80,
			rows: process.stdout.rows || 24,
		});
	});
}
