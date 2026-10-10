import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {getCliEntryPath, isDevRuntime, getTsxLoaderPath} from './paths.js';
import type {AgentSessionRef, ProgramKey} from './types.js';
const exec = promisify(execFile);
export type AttentionState = 'unknown' | 'working' | 'needs-input' | 'response-ended' | 'failed' | 'limited';
/**
 * Why a session needs you, as one line for the details block and notifications: the question it asks (and its
 * options), the plan it wants approved, the action it asks permission for, the last line of its reply, a failure,
 * or the agent's own notification text. Held in memory only: the daemon never writes it to state.json.
 */
export interface AttentionReason {kind: 'question' | 'plan' | 'permission' | 'message' | 'failure' | 'notice'; text: string; options?: string[]}
export interface AgentSignal {state: AttentionState; event: string; nativeRef?: AgentSessionRef; reason?: AttentionReason; fromSubagent?: boolean}

// Tools that stop the agent to ask you something. Claude's AskUserQuestion and ExitPlanMode are documented hook
// targets; Codex's request_user_input is assumed to use the same `questions` shape (not verified against a real payload).
const QUESTION_TOOLS = new Set(['AskUserQuestion', 'request_user_input']);
const PLAN_TOOLS = new Set(['ExitPlanMode']);
// Events a native subagent may report: a permission prompt or question it raises blocks the root session's TUI, and
// its tool use after that shows the prompt was answered. Its other events (Stop, SessionStart, ...) are not the root's.
const SUBAGENT_EVENTS = new Set(['PreToolUse', 'PostToolUse', 'PermissionRequest', 'Notification']);
const MAX_REASON_LENGTH = 300;

export function normalizeHook(program: ProgramKey, payload: unknown): AgentSignal | undefined {
	if (!payload || typeof payload !== 'object' || Array.isArray(payload)) throw new Error('Hook input must be an object');
	const raw = payload as Record<string, unknown>;
	const event = raw.hook_event_name;
	if (typeof event !== 'string') throw new Error('Hook event is missing');
	const fromSubagent = Boolean(raw.agent_id || raw.parent_session_id);
	if (fromSubagent && !SUBAGENT_EVENTS.has(event)) return undefined;
	let state: AttentionState;
	let reason: AttentionReason | undefined;
	switch (event) {
		case 'SessionStart': state = 'unknown'; break;
		case 'UserPromptSubmit': case 'PostToolUse': state = 'working'; break;
		// A question or plan approval arrives as a tool call: the agent waits on you, it is not working.
		case 'PreToolUse': reason = askReason(raw); state = reason ? 'needs-input' : 'working'; break;
		case 'PermissionRequest': state = 'needs-input'; reason = askReason(raw) ?? permissionReason(raw); break;
		case 'Stop': state = 'response-ended'; reason = messageReason(raw.last_assistant_message); break;
		case 'StopFailure': {
			const error = typeof raw.error_type === 'string' ? raw.error_type : typeof raw.error === 'string' ? raw.error : '';
			state = error === 'rate_limit' ? 'limited' : 'failed';
			reason = {kind: 'failure', text: FAILURE_TEXT[error] ?? (error ? `failed: ${oneLine(error)}` : 'request failed')};
			break;
		}
		case 'Interrupt': state = 'unknown'; break;
		case 'Notification':
			if (!['permission_prompt', 'idle_prompt', 'elicitation_dialog', 'agent_needs_input'].includes(String(raw.notification_type))) return undefined;
			state = 'needs-input';
			if (typeof raw.message === 'string' && oneLine(raw.message)) reason = {kind: 'notice', text: oneLine(raw.message)};
			break;
		default: return undefined;
	}
	if (fromSubagent) return {state, event, ...reason ? {reason} : {}, fromSubagent};
	const id = raw.session_id;
	const nativeRef = program !== 'pi' && typeof id === 'string' && /^[a-zA-Z0-9][a-zA-Z0-9_-]{0,127}$/.test(id)
		? {provider: program, kind: 'id' as const, value: id} : undefined;
	return {state, event, nativeRef, ...reason ? {reason} : {}};
}

const FAILURE_TEXT: Record<string, string> = {
	rate_limit: 'rate limited', overloaded: 'API overloaded', authentication_failed: 'authentication failed',
	billing_error: 'billing problem', server_error: 'API server error',
};

/** Agent text as one display line: controls (escape sequences included) and runs of whitespace become one space. */
function oneLine(text: string): string {
	const line = text.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '').replace(/[\u0000-\u001F\u007F-\u009F]+/g, ' ').replace(/\s+/g, ' ').trim();
	return line.length > MAX_REASON_LENGTH ? `${line.slice(0, MAX_REASON_LENGTH - 1)}…` : line;
}
/** A Markdown line without its heading, list, quote and emphasis markup. */
function plainLine(text: string): string {
	return oneLine(text.replace(/^\s*(?:#{1,6}\s+|[-*+>]\s+|\d+[.)]\s+)+/, '').replace(/\*\*|__|`/g, ''));
}
function record(value: unknown): Record<string, unknown> {
	return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

/** A question tool's first question (`asks (+1): …` when it asks more) with its options, or a plan to approve. */
function askReason(raw: Record<string, unknown>): AttentionReason | undefined {
	const tool = raw.tool_name, input = record(raw.tool_input);
	if (typeof tool !== 'string') return undefined;
	if (PLAN_TOOLS.has(tool)) {
		const first = typeof input.plan === 'string' ? input.plan.split('\n').map(plainLine).find(Boolean) : undefined;
		return {kind: 'plan', text: first ? `plan ready: ${first}` : 'plan ready for approval'};
	}
	if (!QUESTION_TOOLS.has(tool)) return undefined;
	const questions = Array.isArray(input.questions) ? input.questions.map(record).filter(item => typeof item.question === 'string' && oneLine(item.question)) : [];
	const first = questions[0];
	if (!first) return {kind: 'question', text: 'asks you a question'};
	const options = (Array.isArray(first.options) ? first.options : [])
		.map(option => oneLine(typeof option === 'string' ? option : String(record(option).label ?? ''))).filter(Boolean);
	const more = questions.length > 1 ? ` (+${questions.length - 1})` : '';
	return {kind: 'question', text: `asks${more}: ${oneLine(first.question as string)}`, ...options.length ? {options} : {}};
}

const EDIT_TOOLS = new Set(['Edit', 'Write', 'MultiEdit', 'NotebookEdit', 'apply_patch']);
/** What a permission prompt asks to do: the command it would run, the file it would edit, the host or tool it would use. */
function permissionReason(raw: Record<string, unknown>): AttentionReason {
	const tool = typeof raw.tool_name === 'string' ? oneLine(raw.tool_name) : '';
	const input = record(raw.tool_input);
	const text = (key: string) => typeof input[key] === 'string' ? oneLine(input[key] as string) : '';
	const command = text('command') || text('cmd');
	if (command) return {kind: 'permission', text: `wants to run: ${command}`};
	if (EDIT_TOOLS.has(tool)) {
		const file = text('file_path') || text('notebook_path') || text('path');
		return {kind: 'permission', text: file ? `wants to edit: ${file.split('/').at(-1)}` : 'wants to edit files'};
	}
	const url = text('url');
	if (url) { let host = url; try { host = new URL(url).host || url; } catch { /* not a URL: show it as given */ } return {kind: 'permission', text: `wants to fetch: ${host}`}; }
	const mcp = tool.match(/^mcp__(.+?)__(.+)$/);
	if (mcp) return {kind: 'permission', text: `wants to use: ${mcp[1]} ${mcp[2]}`};
	return {kind: 'permission', text: tool ? `wants to use: ${tool}` : 'asks for permission'};
}

/** The last line of the agent's reply, where a question to you usually is. */
function messageReason(message: unknown): AttentionReason | undefined {
	if (typeof message !== 'string') return undefined;
	const last = message.split('\n').map(plainLine).filter(Boolean).at(-1);
	return last ? {kind: 'message', text: `said: ${last}`} : undefined;
}

// What `deckhand hook` forwards to the daemon: the lifecycle fields, plus only what a reason line is made from (a
// question or plan tool's input, a permission prompt's command/file/URL, the end of the last reply, a notification's
// text), each bounded. Other tool inputs and outputs never leave the hook process.
const SIGNAL_FIELDS = ['hook_event_name', 'session_id', 'agent_id', 'parent_session_id', 'notification_type', 'error', 'error_type', 'tool_name'] as const;
const MAX_HOOK_FIELD_LENGTH = 256;
const MAX_HOOK_TEXT_LENGTH = 2000;
const PERMISSION_INPUT_FIELDS = ['command', 'cmd', 'file_path', 'notebook_path', 'path', 'url'] as const;

export function hookPayloadFields(payload: unknown): Record<string, unknown> {
	if (!payload || typeof payload !== 'object' || Array.isArray(payload)) throw new Error('Hook input must be an object');
	const raw = payload as Record<string, unknown>;
	const fields: Record<string, unknown> = {};
	for (const field of SIGNAL_FIELDS) {
		const value = raw[field];
		if (value === undefined || value === null || value === false || value === '') continue;
		// Non-string values keep their truthiness (e.g. agent_id) without copying their contents.
		fields[field] = typeof value === 'string' ? value.slice(0, MAX_HOOK_FIELD_LENGTH) : typeof value === 'object' ? '[object]' : String(value).slice(0, MAX_HOOK_FIELD_LENGTH);
	}
	const event = raw.hook_event_name, tool = raw.tool_name, input = record(raw.tool_input);
	const bounded = (value: unknown, max: number) => typeof value === 'string' ? value.slice(0, max) : undefined;
	if (typeof tool === 'string' && (event === 'PreToolUse' || event === 'PermissionRequest')) {
		if (QUESTION_TOOLS.has(tool)) {
			const questions = (Array.isArray(input.questions) ? input.questions : []).slice(0, 4).map(record).map(item => ({
				question: bounded(item.question, MAX_REASON_LENGTH), header: bounded(item.header, 60),
				options: (Array.isArray(item.options) ? item.options : []).slice(0, 8).map(option => typeof option === 'string'
					? {label: option.slice(0, 120)} : {label: bounded(record(option).label, 120), description: bounded(record(option).description, 200)}),
				multiSelect: item.multiSelect === true,
			}));
			fields.tool_input = {questions};
		} else if (PLAN_TOOLS.has(tool)) fields.tool_input = {plan: bounded(input.plan, MAX_HOOK_TEXT_LENGTH)};
		else if (event === 'PermissionRequest') fields.tool_input = Object.fromEntries(PERMISSION_INPUT_FIELDS.flatMap(key => typeof input[key] === 'string' ? [[key, (input[key] as string).slice(0, MAX_HOOK_FIELD_LENGTH)]] : []));
	}
	if (event === 'Stop' && typeof raw.last_assistant_message === 'string') fields.last_assistant_message = raw.last_assistant_message.slice(-MAX_HOOK_TEXT_LENGTH);
	if (event === 'Notification' && typeof raw.message === 'string') fields.message = raw.message.slice(0, MAX_HOOK_FIELD_LENGTH);
	return fields;
}

/** Whether launches pass agent signals: the user's setting, else on for Claude only (Codex needs its own hooks set up first). */
export function hooksEnabled(setting: boolean | undefined, program: ProgramKey): boolean {
	return setting ?? program === 'claude';
}
/** The reason as a notification's text, else the state in words. */
export function attentionMessage(state: AttentionState, reason?: AttentionReason): string {
	if (reason) return reason.text;
	return state === 'needs-input' ? 'needs input' : state === 'response-ended' ? 'response ended' : state === 'limited' ? 'rate limited' : state;
}
export function shellQuote(value: string): string { return `'${value.replace(/'/g, `'\\''`)}'`; }
export function hookCommand(): string {
	return [process.execPath, ...(isDevRuntime() ? ['--import', getTsxLoaderPath()] : []), getCliEntryPath(), 'hook'].map(shellQuote).join(' ');
}
/**
 * Hook settings for every supported event. `async` (Claude) runs them in the background, so the agent never waits on
 * Deckhand: they only report, never decide. Codex's hooks.json gets plain entries (its docs don't list `async`).
 */
export function hookSettings(options: {async?: boolean} = {}): {hooks: Record<string, Array<{hooks: Array<{type: string; command: string; timeout: number; async?: boolean}>}>>} {
	const command = hookCommand();
	const hook = {type: 'command', command, timeout: 2, ...options.async ? {async: true} : {}};
	return {hooks: Object.fromEntries(['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PermissionRequest', 'Stop', 'StopFailure', 'Notification', 'Interrupt'].map(event => [event, [{hooks: [hook]}]]))};
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
	const settings = hookSettings({async: true});
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
