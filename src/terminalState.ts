import process from 'node:process';

const RESET_TERMINAL_STATE_SEQUENCE = [
	'\x1b[0m', // reset attributes
	'\x1b[?25h', // show cursor
	'\x1b[?7h', // enable line wrapping
	'\x1b[?6l', // origin mode off
	'\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l', // mouse/focus modes off
	'\x1b[?2004l', // bracketed paste off
	'\x1b[r', // reset scroll region
].join('');

export function resetTerminalState(): void {
	if (!process.stdout.isTTY) {
		return;
	}

	process.stdout.write(RESET_TERMINAL_STATE_SEQUENCE);
}
