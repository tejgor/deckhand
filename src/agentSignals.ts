import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {getCliEntryPath, isDevRuntime, getTsxLoaderPath} from './paths.js';
import type {AgentSessionRef, ProgramKey} from './types.js';
const exec = promisify(execFile);
export type AttentionState = 'unknown' | 'working' | 'needs-input' | 'response-ended' | 'failed' | 'limited';
export interface AgentSignal {state: AttentionState; event: string; nativeRef?: AgentSessionRef}
export function normalizeHook(program: ProgramKey, payload: unknown): AgentSignal | undefined {
	if (!payload || typeof payload !== 'object' || Array.isArray(payload)) throw new Error('Hook input must be an object');
	const raw = payload as Record<string, unknown>;
	if (raw.agent_id || raw.parent_session_id) return undefined; // Native subagents are not the root PTY session.
	const event = raw.hook_event_name;
	if (typeof event !== 'string') throw new Error('Hook event is missing');
	let state: AttentionState;
	switch (event) {
		case 'SessionStart': state = 'unknown'; break;
		case 'UserPromptSubmit': case 'PreToolUse': case 'PostToolUse': state = 'working'; break;
		case 'PermissionRequest': state = 'needs-input'; break;
		case 'Stop': state = 'response-ended'; break;
		case 'StopFailure': state = raw.error === 'rate_limit' || raw.error_type === 'rate_limit' ? 'limited' : 'failed'; break;
		case 'Interrupt': state = 'unknown'; break;
		case 'Notification':
			if (!['permission_prompt', 'idle_prompt', 'elicitation_dialog', 'agent_needs_input'].includes(String(raw.notification_type))) return undefined;
			state = 'needs-input'; break;
		default: return undefined;
	}
	const id = raw.session_id;
	const nativeRef = program !== 'pi' && typeof id === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$/.test(id)
		? {provider: program, kind: 'id' as const, value: id} : undefined;
	return {state, event, nativeRef};
}
export function shellQuote(value: string): string { return `'${value.replace(/'/g, `'\\''`)}'`; }
export function hookCommand(): string {
	return [process.execPath, ...(isDevRuntime() ? ['--import', getTsxLoaderPath()] : []), getCliEntryPath(), 'hook'].map(shellQuote).join(' ');
}
export function hookSettings(): {hooks: Record<string, Array<{hooks: Array<{type: string; command: string; timeout: number}>}>>} {
	const command = hookCommand();
	return {hooks: Object.fromEntries(['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PermissionRequest', 'Stop', 'StopFailure', 'Notification', 'Interrupt'].map(event => [event, [{hooks: [{type: 'command', command, timeout: 2}]}]]))};
}
const helpCache = new Map<string, Promise<string>>();
async function programHelp(command: string): Promise<string> {
	const cached = helpCache.get(command);
	if (cached) return cached;
	// Only successful probes are cached; a transient failure (timeout, upgrade in progress) is retried next launch.
	const probe: Promise<string> = exec(command, ['--help'], {timeout: 5000, maxBuffer: 512 * 1024}).then(result => result.stdout).catch(() => {
		if (helpCache.get(command) === probe) helpCache.delete(command);
		return '';
	});
	helpCache.set(command, probe);
	return probe;
}
export async function integrationArgs(program: ProgramKey, command: string, enabled: boolean): Promise<string[]> {
	const isolatedCodex = program === 'codex' && process.env.DECKHAND_CHANNEL === 'dev';
	if (!enabled && !isolatedCodex) return [];
	const help = await programHelp(command);
	if (program === 'codex') {
		if (isolatedCodex && !help.includes('--no-daemon')) throw new Error('This Codex version cannot verify isolated native-daemon operation (--no-daemon missing). Upgrade Codex or test with Claude/Pi.');
		return help.includes('--no-daemon') ? ['--no-daemon'] : [];
	}
	if (program !== 'claude' || !help.includes('--settings')) return [];
	// Additional settings only; never alter permissions or replace the native TUI.
	const settings = hookSettings();
	delete settings.hooks.StopFailure; // Opt-in manual configuration for version-specific failure events.
	delete settings.hooks.Interrupt; // Claude doesn't expose Codex's Interrupt event.
	return ['--settings', JSON.stringify(settings)];
}
export function codexResumeFromOutput(output: string): AgentSessionRef | undefined {
	const plain = output.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '');
	const id = plain.match(/codex\s+resume\s+([a-zA-Z0-9][a-zA-Z0-9_-]{7,127})(?:\s|$)/)?.[1];
	return id ? {provider: 'codex', kind: 'id', value: id} : undefined;
}
export function needsAttention(state: AttentionState | undefined): boolean {
	return state === 'needs-input' || state === 'response-ended' || state === 'failed' || state === 'limited';
}
