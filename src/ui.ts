import type {SessionRecord} from './types.js';

// Use ANSI named colors so the palette inherits from the user's terminal theme
// instead of locking in hex values that look wrong against custom palettes.
export const THEME = {
	accent: 'magenta',
	accentSoft: 'magentaBright',
	active: 'cyan',
	muted: 'gray',
	border: 'gray',
	borderActive: 'magenta',
	borderDanger: 'red',
	success: 'green',
	warn: 'yellow',
	error: 'red',
} as const;

export function errorMessage(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

const TERMINAL_ESCAPE_PATTERN = /\u001B(?:\[[0-?]*[ -/]*[@-~]|\][^\u0007\u001B]*(?:\u0007|\u001B\\)?|[@-Z\\-_])/g;
// C0/C1 controls except tab and newline (multi-line display text keeps those).
export const DISPLAY_CONTROL_PATTERN = /[\u0000-\u0008\u000B-\u001F\u007F-\u009F]/g;

// Single-line typed input: drop escape sequences and every control, tab/newline included.
export function stripTerminalControls(text: string): string {
	return text.replace(TERMINAL_ESCAPE_PATTERN, '').replace(DISPLAY_CONTROL_PATTERN, '').replace(/[\t\n]/g, '');
}

// Plain-text view of captured command output: drop escape sequences, keep what
// a carriage return would leave visible on each line, and remove other controls.
export function plainTerminalText(text: string): string {
	return text
		.replace(TERMINAL_ESCAPE_PATTERN, '')
		.split('\n')
		.map(line => {
			const trimmed = line.replace(/\r+$/, '');
			return trimmed.slice(trimmed.lastIndexOf('\r') + 1).replace(DISPLAY_CONTROL_PATTERN, '').replace(/\t/g, '  ');
		})
		.join('\n');
}

export function truncate(text: string, width: number): string {
	if (width <= 0) return '';
	if (text.length <= width) return text;
	return width === 1 ? text.slice(0, 1) : `${text.slice(0, width - 1)}…`;
}

export function fitLines(text: string, width: number, height: number): string[] {
	const rawLines = text.length > 0 ? text.split('\n') : [''];
	const lines = rawLines.map(line => truncate(line, width));
	const fitted = lines.length >= height ? lines.slice(0, height) : [...lines, ...Array.from({length: height - lines.length}, () => '')];
	// Ink can collapse empty <Text> nodes. Render blank terminal rows as a
	// space so full-screen TUIs keep their vertical positioning in previews.
	return fitted.map(line => (line === '' ? ' ' : line));
}

export function compactPath(path: string, width: number): string {
	const home = process.env.HOME;
	const display = home && path.startsWith(home) ? `~${path.slice(home.length)}` : path;
	if (display.length <= width) return display;
	const parts = display.split('/').filter(Boolean);
	if (parts.length <= 2) return truncate(display, width);
	const compact = `…/${parts.slice(-2).join('/')}`;
	return truncate(compact, width);
}

export function programGlyph(program: SessionRecord['program']): string {
	switch (program) {
		case 'claude':
			return '✶';
		case 'pi':
			return 'π';
		case 'codex':
			return '◇';
	}
}

export function statusGlyph(session: SessionRecord, spinnerFrame: string): string {
	if (session.status === 'running') {
		if (session.attention?.state === 'needs-input') return '?';
		if (session.attention?.state === 'response-ended') return '◆';
		if (session.attention?.state === 'failed') return '!';
		if (session.attention?.state === 'limited') return '⌛';
		if (session.attention?.state === 'working') return spinnerFrame;
	}
	if (session.status === 'exited' && (session.exitReason === 'failed' || session.exitReason === 'interrupted')) return '!';
	switch (session.status) {
		case 'starting':
			return spinnerFrame;
		case 'running':
			if (session.agentStatus === 'active') return spinnerFrame;
			if (session.agentStatus === 'idle') return '●';
			return '◌';
		case 'exited':
			return '○';
	}
}

export function statusColor(session: SessionRecord): string {
	if (session.attention?.state === 'failed' || session.cleanupError || session.exitReason === 'failed') return THEME.error;
	if (session.attention?.state === 'needs-input' || session.attention?.state === 'limited' || session.exitReason === 'interrupted') return THEME.warn;
	switch (session.status) {
		case 'starting':
			return THEME.warn;
		case 'running':
			return session.agentStatus === 'unknown' || !session.agentStatus ? THEME.warn : THEME.success;
		case 'exited':
			return THEME.muted;
	}
}

export function displaySessionTitle(session: SessionRecord, sessions: SessionRecord[]): string {
	const parent = session.parentSessionId ? sessions.find(candidate => candidate.id === session.parentSessionId) : undefined;
	const parentPrefix = parent ? `${parent.title.trim().replace(/\s+/g, ' ')} / ` : '';
	const normalizedTitle = session.title.trim().replace(/\s+/g, ' ');
	const localTitle = parentPrefix && normalizedTitle.startsWith(parentPrefix)
		? normalizedTitle.slice(parentPrefix.length)
		: normalizedTitle;
	return localTitle.replace(/\s+\/\s+/g, '/');
}

export function statusLabel(session?: SessionRecord): string {
	if (!session) return '—';
	if (session.status === 'exited') return session.exitReason ?? 'exited';
	if (session.status === 'starting') return session.setup?.state === 'running' ? 'preparing workspace' : 'starting';
	if (session.attention && session.attention.state !== 'unknown') return session.attention.state === 'response-ended' ? 'response ended (not task success)' : session.attention.state;
	return session.agentStatus ?? 'running';
}
