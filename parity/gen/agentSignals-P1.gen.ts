/**
 * Records the pure parts of src/agentSignals.ts that unit P1 ports (I7 owns hookCommand/hookSettings/integrationArgs).
 * Regenerate: node --import tsx parity/gen/agentSignals-P1.gen.ts
 */
import {Recorder, arb, encode, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {answersPrompt, attentionMessage, codexResumeFromOutput, hookPayloadFields, hooksEnabled, needsAttention, normalizeHook, shellQuote} from '../../src/agentSignals.ts';

installDeterminism();
const recorder = new Recorder('agentSignals-P1', {cap: 1500});

recorder.addTap(await runTap({tests: ['tests/sessionFeatures.test.ts'], tap: ['agentSignals:normalizeHook', 'agentSignals:answersPrompt', 'agentSignals:hookPayloadFields', 'agentSignals:hooksEnabled', 'agentSignals:attentionMessage', 'agentSignals:codexResumeFromOutput', 'agentSignals:needsAttention']}), {module: 'agentSignals'});

// Slicing a string at a UTF-16 unit can split a surrogate pair; the port works on Rust strings that cannot hold a
// lone surrogate, so such outcomes are not recorded.
const fns: Record<string, Function> = {normalizeHook, answersPrompt, hookPayloadFields, hooksEnabled, attentionMessage, shellQuote, codexResumeFromOutput, needsAttention};
function rec(name: string, args: unknown[]): void {
	let outcome: string;
	try { outcome = JSON.stringify(encode(fns[name]!(...args))); } catch (error) { outcome = String((error as Error).message); }
	if (outcome.includes('$utf16')) return;
	try { recorder.call(name, fns[name]!, args); } catch { /* recorded as a throw */ }
}

const PROGRAMS = ['claude', 'pi', 'codex'] as const;
const EVENTS = ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'PermissionRequest', 'Stop', 'StopFailure', 'Interrupt', 'Notification', 'SubagentStop', 'bogus', ''];
const NOTIFICATIONS = ['idle_prompt', 'permission_prompt', 'elicitation_dialog', 'elicitation_url_dialog', 'agent_needs_input', 'other', undefined];
const TOOLS = ['AskUserQuestion', 'request_user_input', 'ExitPlanMode', 'Bash', 'Edit', 'Write', 'MultiEdit', 'NotebookEdit', 'apply_patch', 'WebFetch', 'mcp__github__create_issue', 'mcp__x', 'Read', '', undefined, 5];
const SESSION_IDS = ['0f8fad5b-d9cb-469f-a716-446655440000', 'a', 'abc_DEF-123', '-bad', '_bad', 'has space', '', 'x'.repeat(128), 'x'.repeat(129), 'é', 'ab😀', undefined, 7, null];
const questionBlock = (n: number) => Array.from({length: n}, (_, i) => ({question: `Question ${i + 1}?`, header: `H${i}`, options: [{label: 'Yes', description: 'do it'}, 'No', {label: ''}, {}], multiSelect: i === 1}));
const long = (n: number, ch = 'a') => ch.repeat(n);
const inputs: Record<string, unknown>[] = [
	{}, {questions: []}, {questions: [{}]}, {questions: [{question: '  '}]}, {questions: questionBlock(1)}, {questions: questionBlock(3)}, {questions: questionBlock(6)},
	{questions: [{question: 'Pick?\n\x1b[31mone\x1b[0m\ttwo', options: ['A', {label: 'B\u0007c'}, 3, null, {label: 5}]}]}, {questions: 'nope'}, {questions: [null, 'x', {question: 'ok'}]},
	{questions: [{question: long(400), options: Array.from({length: 12}, (_, i) => ({label: `opt ${i}` + long(130), description: long(210)}))}]},
	{questions: [{question: '😀'.repeat(200), header: '中'.repeat(80), options: ['é'.repeat(130)]}]},
	{plan: ''}, {plan: '# Title\n\n- step one\n- step two'}, {plan: '\n\n   \n## **Bold** `code`\n'}, {plan: '1. first\n2) second'}, {plan: '> quote\n'}, {plan: long(2100, 'p')}, {plan: 5},
	{command: 'ls -la'}, {cmd: 'echo \u001b[1mhi\u001b[0m   there'}, {command: '', cmd: 'second'}, {command: long(400)}, {file_path: '/a/b/c.ts'}, {file_path: ''}, {notebook_path: '/n/nb.ipynb'}, {path: 'rel/file.txt'},
	{file_path: '/dir/'}, {url: 'https://example.com:8080/a?b=1'}, {url: 'not a url'}, {url: 'http://'}, {url: 'file:///x/y'}, {url: 'https://中文.example/路径'}, {url: long(300, 'u')},
	{command: 5, file_path: 'x'}, {other: 'ignored', secret: 'nope'}, {command: 'ls', file_path: '/x', url: 'https://a.b', path: '/p', cmd: 'c', notebook_path: 'n'},
];
const messages = [
	undefined, '', 'plain', 'first\nsecond', 'line\n\n  \n', '# Heading\n- item\nDo you want to proceed?', '**bold** __under__ `code`', '1. one\n2) two', '> quoted', '\x1b[1mansi\x1b[0m\r\nnext\x1b[?25h',
	'tab\tsep  multiple   spaces', '😀 done 中文', long(500), 'x'.repeat(299), 'x'.repeat(300), 'x'.repeat(301), '\u0085\u009f\u007f', '   ', '- ', '###', 'a\nb\n' + long(2100, 'z'), 5, null,
];
const ERRORS = ['rate_limit', 'overloaded', 'authentication_failed', 'billing_error', 'server_error', 'unknown_kind', '', 'ünï\n😀', long(400), 5, undefined, null];

const payloads: any[] = [];
for (const hook_event_name of EVENTS) {
	for (const session_id of SESSION_IDS) payloads.push({hook_event_name, session_id});
	for (const tool_name of TOOLS) for (const tool_input of inputs) payloads.push({hook_event_name, session_id: 's1', tool_name, tool_input});
	for (const last_assistant_message of messages) payloads.push({hook_event_name, last_assistant_message});
	for (const notification_type of NOTIFICATIONS) for (const message of messages.slice(0, 12)) payloads.push({hook_event_name, notification_type, message});
	for (const error of ERRORS) { payloads.push({hook_event_name, error}); payloads.push({hook_event_name, error_type: error}); payloads.push({hook_event_name, error_type: 'x', error}); }
	for (const sub of [{agent_id: 'a1'}, {parent_session_id: 'p1'}, {agent_id: '', parent_session_id: ''}, {agent_id: 1}, {agent_id: {}}, {agent_id: 'a', notification_type: 'idle_prompt'}, {agent_id: 'a', notification_type: 'permission_prompt', message: 'wait'}]) {
		payloads.push({hook_event_name, session_id: 'sub-1', ...sub}); payloads.push({hook_event_name, ...sub, tool_name: 'Bash', tool_input: {command: 'rm -rf x'}});
	}
}
const invalid = [undefined, null, 5, 'str', true, [], [1], {}, {hook_event_name: 5}, {hook_event_name: null}, {hook_event_name: ['Stop']}, {session_id: 'x'}];
for (const payload of invalid) for (const name of ['normalizeHook', 'hookPayloadFields']) for (const program of PROGRAMS) rec(name, name === 'normalizeHook' ? [program, payload] : [payload]);
for (const payload of payloads) {
	for (const program of PROGRAMS) rec('normalizeHook', [program, payload]);
	rec('hookPayloadFields', [payload]);
}

// Random payloads: fixed seeds.
const maybe = <T>(a: fc.Arbitrary<T>) => fc.option(a, {nil: undefined});
const textish = fc.oneof(arb.text(20), fc.constantFrom(...messages.filter((m): m is string => typeof m === 'string')), fc.integer({min: 0, max: 400}).map(n => 'w'.repeat(n)));
const payloadArb = fc.record({
	hook_event_name: maybe(fc.oneof(fc.constantFrom(...EVENTS), fc.constant(3 as unknown))), session_id: maybe(fc.oneof(fc.constantFrom(...SESSION_IDS), arb.text(8))),
	agent_id: maybe(fc.oneof(fc.constant(''), fc.constant('a'), fc.constant(0 as unknown), fc.constant({} as unknown), fc.constant(false as unknown), fc.constant(true as unknown))), parent_session_id: maybe(fc.constantFrom('', 'p')),
	notification_type: maybe(fc.constantFrom(...NOTIFICATIONS.filter(Boolean) as string[])), message: maybe(textish), error: maybe(fc.oneof(fc.constantFrom(...ERRORS.filter(e => typeof e === 'string') as string[]), arb.text(10))),
	error_type: maybe(fc.constantFrom(...ERRORS.filter(e => typeof e === 'string') as string[])), tool_name: maybe(fc.constantFrom(...TOOLS)), tool_input: maybe(fc.constantFrom(...inputs)), last_assistant_message: maybe(textish),
}, {requiredKeys: []});
for (const payload of sample(payloadArb, 600, 21)) {
	for (const program of PROGRAMS) rec('normalizeHook', [program, payload]);
	rec('hookPayloadFields', [payload]);
}
for (const args of mutate(recorder.argsOf('normalizeHook').filter(a => a[1] && typeof a[1] === 'object').slice(0, 300), {variants: 2, seed: 9}).filter(a => PROGRAMS.includes(a[0] as any))) rec('normalizeHook', args);
for (const args of mutate(recorder.argsOf('hookPayloadFields').filter(a => a[0] && typeof a[0] === 'object').slice(0, 300), {variants: 2, seed: 10})) rec('hookPayloadFields', args);

// answersPrompt
const keys = ['', '\x1b', '\r', '\n', '\r\n', '1', '9', '0', 'y', 'n', 'abc', 'a1', '\x1b[A', '\x1b[B', '\x1b[1;5A', '\x1bOA', '\x1b[<0;10;5M', '\x1b[200~12\x1b[201~', '\x1b[200~paste\x1b[201~\r', '\x1b[200~unterminated 1', '\x1b[200~a\x1b[201~\x1b[200~1\x1b[201~',
	'\x1bx', '\x1b\x1b', '\x1b1', '\x1b[', '\x1b[?25h5', ' ', '\t', '😀', '中1', '１', '\u0085', '\x1b[200~\n\x1b[201~', '\x03', '\x7f', 'a\rb', '\x1b[31m', '\x1b[31m1', '\x1b[2K\n', '\x1b[0-9m'];
for (const key of keys) rec('answersPrompt', [key]);
for (const text of sample(arb.text(10, ['\x1b', '[', 'A', '~', '2', '0', '1', '\r', '\n', 'x', '😀', ';', '?', ' ', 'O', '3']), 400, 4)) rec('answersPrompt', [text]);
for (const args of mutate(recorder.argsOf('answersPrompt'), {variants: 2})) rec('answersPrompt', args);

// hooksEnabled / attentionMessage / needsAttention / shellQuote / codexResumeFromOutput
for (const setting of [undefined, true, false]) for (const program of PROGRAMS) rec('hooksEnabled', [setting, program]);
const STATES = ['unknown', 'working', 'needs-input', 'response-ended', 'failed', 'limited', undefined];
for (const state of STATES) rec('needsAttention', [state]);
for (const state of STATES.filter(Boolean)) {
	rec('attentionMessage', [state]); rec('attentionMessage', [state, undefined]);
	for (const reason of [{kind: 'notice', text: 'hi'}, {kind: 'question', text: '', options: ['a']}, {kind: 'message', text: '😀 中'}]) rec('attentionMessage', [state, reason]);
}
for (const value of ['', 'plain', "it's", "'", "''", "a'b'c", 'with space', '$HOME `x` "q" \\', 'new\nline', '😀 中文 é', "'\\''", '\\', 'tab\t']) rec('shellQuote', [value]);
for (const text of sample(arb.text(14, ["'", '\\', 'a', ' ', '😀', '中', '"', '$', '\n']), 150, 6)) rec('shellQuote', [text]);
const outputs = [
	'', 'codex resume 019a1b2c-3d4e-7000-8000-000000000001', 'To continue, run codex resume abcdefgh', 'codex resume abcdefg', 'codex   resume\n abcdefgh_-12 trailing', 'codex resume abcdefgh', 'codex resume abcdefgh\n',
	'\x1b[1mcodex resume\x1b[0m abcdefgh', 'codex resume \x1b[32mabcdefgh\x1b[0m', 'codex resume -abcdefgh', 'codex resume _abcdefgh', 'codex resume abcdefgh!', 'codex resume abcdefgh.', 'codex resume ' + 'a'.repeat(128), 'codex resume ' + 'a'.repeat(129),
	'xcodex resume abcdefgh', 'Codex resume abcdefgh', 'codex resume 😀abcdefgh', 'codex resume abcdefgh😀', 'codex resume 中文中文中文中文', 'first codex resume aaaaaaaa then codex resume bbbbbbbb', 'codex resume abcdefgh ', 'codex resume ábcdefgh',
];
for (const output of outputs) rec('codexResumeFromOutput', [output]);
for (const text of sample(arb.text(24, ['codex', ' ', 'resume', 'a', 'b', '1', '-', '_', '\x1b[0m', '\n', '😀', '.', '中']), 250, 8)) rec('codexResumeFromOutput', [text]);
for (const args of mutate(recorder.argsOf('codexResumeFromOutput'), {variants: 3})) rec('codexResumeFromOutput', args);

await recorder.write({prune: true});
