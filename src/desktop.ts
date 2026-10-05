import {spawn, spawnSync} from 'node:child_process';
import {errorMessage} from './ui.js';

interface EditorCommand {command: string; args: string[]; label: string}

function resolveEditorCommand(): EditorCommand | undefined {
	const cli = spawnSync('sh', ['-lc', 'command -v cursor || command -v code'], {encoding: 'utf8'});
	const command = cli.status === 0 ? cli.stdout.trim().split('\n')[0] : undefined;
	if (command) {
		return {command, args: [], label: command.includes('cursor') ? 'Cursor' : 'Code'};
	}
	if (process.platform === 'darwin') {
		return {command: 'open', args: ['-a', 'Cursor'], label: 'Cursor'};
	}
	return undefined;
}

// Launch a GUI helper without tying its lifetime or stdio to the terminal UI.
// Errors are reported through onError instead of crashing the UI.
function spawnDetached(command: string, args: string[], onError: (message: string) => void): boolean {
	try {
		const child = spawn(command, args, {detached: true, stdio: 'ignore'});
		child.on('error', error => onError(errorMessage(error)));
		child.unref();
		return true;
	} catch (error) {
		onError(errorMessage(error));
		return false;
	}
}

/**
 * Opens `target` in Cursor or VS Code; with `line`, at that line (`-g <path>:<line>`, both CLIs; VS Code's defaults
 * reuse the last active window). The macOS `open -a` fallback just opens the file.
 */
export function openInEditor(target: string, onError: (message: string) => void, line?: number): string | undefined {
	const editor = resolveEditorCommand();
	if (!editor) {
		onError('could not find cursor or code command on PATH');
		return undefined;
	}
	const cli = editor.command !== 'open';
	const args = line !== undefined && cli ? ['-g', `${target}:${Math.max(1, Math.floor(line))}`] : [target];
	return spawnDetached(editor.command, [...editor.args, ...args], onError) ? editor.label : undefined;
}

export function openUrl(url: string, onError: (message: string) => void): boolean {
	if (!/^https:\/\/[^\s]+$/.test(url)) {
		onError('refusing to open a non-https URL');
		return false;
	}
	return spawnDetached(process.platform === 'darwin' ? 'open' : 'xdg-open', [url], onError);
}
