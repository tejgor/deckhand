import type {PreviewRecord, SessionRecord} from './types.js';

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
	}
}

export function statusGlyph(session: SessionRecord, spinnerFrame: string): string {
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
	switch (session.status) {
		case 'starting':
			return THEME.warn;
		case 'running':
			return session.agentStatus === 'unknown' || !session.agentStatus ? THEME.warn : THEME.success;
		case 'exited':
			return THEME.muted;
	}
}

export function previewStatusIcon(session: SessionRecord | undefined, preview: PreviewRecord, spinnerFrame: string): string {
	if (!session) return '';
	if (session.status === 'starting') return spinnerFrame;
	if (session.status === 'exited') return '○';
	const agentStatus = preview.agentStatus ?? session.agentStatus;
	if (agentStatus === 'active') return spinnerFrame;
	if (agentStatus === 'idle') return '●';
	return '◌';
}

export function statusLabel(session?: SessionRecord): string {
	if (!session) return '—';
	if (session.status === 'exited') return 'exited';
	if (session.status === 'starting') return 'starting';
	return session.agentStatus ?? 'running';
}
