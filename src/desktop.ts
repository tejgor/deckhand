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

export function openInEditor(target: string, onError: (message: string) => void): string | undefined {
	const editor = resolveEditorCommand();
	if (!editor) {
		onError('could not find cursor or code command on PATH');
		return undefined;
	}
	return spawnDetached(editor.command, [...editor.args, target], onError) ? editor.label : undefined;
}

export function openUrl(url: string, onError: (message: string) => void): boolean {
	if (!/^https:\/\/[^\s]+$/.test(url)) {
		onError('refusing to open a non-https URL');
		return false;
	}
	return spawnDetached(process.platform === 'darwin' ? 'open' : 'xdg-open', [url], onError);
}
