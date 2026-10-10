import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {tempDir, withEnv} from './helpers.js';
import {SESSION_FILTERS, filterForKey, filterSessionList, handoffMarkdown, sessionFilterCounts} from '../src/sessionFeatures.js';
import {normalizeHook, codexResumeFromOutput, needsAttention, integrationArgs, hookPayloadFields, hooksEnabled, hookSettings, attentionMessage} from '../src/agentSignals.js';
import type {SessionRecord} from '../src/types.js';
const root = {id: 'root', title: 'Parent', cwd: '/repo', program: 'claude', status: 'running', notes: '', repoRoot: '/repo'} as SessionRecord;
const child = {...root, id: 'child', title: 'Parent / Child', parentSessionId: 'root', archivedAt: 'yesterday', notes: 'special context'};
test('filters retain ancestry/order and distinguish archive, running, attention and search', () => {
	assert.deepEqual(filterSessionList([root, child], 'active', '').map(item => item.id), ['root']);
	assert.deepEqual(filterSessionList([root, child], 'archived', 'special').map(item => item.id), ['root', 'child']);
	assert.deepEqual(filterSessionList([root, child], 'all', 'not found'), []);
	assert.deepEqual(filterSessionList([{...root, attention: {state: 'needs-input', event: 'PermissionRequest', at: 'now'}}], 'attention', '').map(item => item.id), ['root']);
	assert.deepEqual(filterSessionList([{...root, status: 'exited', exitReason: 'interrupted'}], 'attention', '').map(item => item.id), ['root']);
});
test('the filter menu picks each filter by one key and counts the rows each would list', () => {
	assert.deepEqual(SESSION_FILTERS.map(filter => filterForKey(filter === 'archived' ? 'A' : filter === 'attention' ? '!' : filter === 'all' ? '*' : filter[0]!)), SESSION_FILTERS);
	assert.equal(filterForKey('x'), undefined);
	assert.equal(filterForKey('f'), undefined);
	const exited = {...root, id: 'gone', status: 'exited', exitReason: 'failed'} as SessionRecord;
	// The archived child brings its parent along for context; search narrows every count.
	assert.deepEqual(sessionFilterCounts([root, child, exited], ''), {active: 2, running: 1, attention: 1, exited: 1, archived: 2, all: 3});
	assert.deepEqual(sessionFilterCounts([root, child, exited], 'special'), {active: 0, running: 0, attention: 0, exited: 0, archived: 2, all: 2});
});
test('handoffs include notes by default, and clearly label optional terminal excerpts', () => {
	const session = {...root, notes: 'Implement the feature', lastPreview: 'sensitive terminal output'};
	assert.match(handoffMarkdown(session), /Implement the feature/); assert.doesNotMatch(handoffMarkdown(session), /sensitive terminal output/);
	assert.match(handoffMarkdown(session, true), /not a complete transcript/);
	// The done marker (D) is stated; search ignores it.
	assert.match(handoffMarkdown({...session, doneAt: '2026-10-05T12:00:00.000Z'}), /\nStatus: marked done 2026-10-05T12:00:00\.000Z\n/);
	assert.doesNotMatch(handoffMarkdown(session), /Status: marked done/);
	// The work's task comes first (its steps are in the notes).
	assert.match(handoffMarkdown(session, false, undefined, {title: 'Add OAuth', body: 'with PKCE', done: false}), /## Task\n\n\[ \] Add OAuth\n\nwith PKCE\n\n## Notes/);
	assert.doesNotMatch(handoffMarkdown(session), /## Task/);
	assert.deepEqual(filterSessionList([{...root, doneAt: '2026-10-05T12:00:00.000Z'}], 'active', 'done').map(item => item.id), []);
});
test('lifecycle evidence is advisory, version tolerant, and excludes native subagents', () => {
	assert.equal(normalizeHook('claude', {hook_event_name: 'PermissionRequest'})?.state, 'needs-input');
	assert.equal(normalizeHook('claude', {hook_event_name: 'Stop'})?.state, 'response-ended');
	assert.equal(normalizeHook('claude', {hook_event_name: 'StopFailure', error_type: 'rate_limit'})?.state, 'limited');
	assert.equal(normalizeHook('codex', {hook_event_name: 'SessionStart', session_id: 'native_123'})?.nativeRef?.value, 'native_123');
	assert.equal(normalizeHook('codex', {hook_event_name: 'SessionStart', session_id: '--bad'})?.nativeRef, undefined);
	assert.equal(normalizeHook('claude', {hook_event_name: 'Stop', agent_id: 'native-child'}), undefined);
	assert.equal(normalizeHook('claude', {hook_event_name: 'FutureEvent'}), undefined);
	assert.equal(needsAttention('working'), false); assert.equal(needsAttention('response-ended'), true);
	assert.throws(() => normalizeHook('claude', 'invalid'));
});
test('signals say why a session waits: the question and its options, the plan, the permission asked for, the last line of a reply, a failure', () => {
	const ask = {hook_event_name: 'PreToolUse', tool_name: 'AskUserQuestion', tool_input: {questions: [
		{question: 'Which auth\nlibrary?', header: 'Auth', options: [{label: 'Lucia', description: 'small'}, {label: 'Auth.js'}], multiSelect: false},
		{question: 'Keep sessions?', options: [{label: 'Yes'}]},
	]}};
	// A question is no tool work: the session waits on you.
	assert.deepEqual(normalizeHook('claude', ask)?.reason, {kind: 'question', text: 'asks (+1): Which auth library?', options: ['Lucia', 'Auth.js']});
	assert.equal(normalizeHook('claude', ask)?.state, 'needs-input');
	assert.deepEqual(normalizeHook('claude', {...ask, tool_input: {}})?.reason, {kind: 'question', text: 'asks you a question'});
	assert.deepEqual(normalizeHook('claude', {hook_event_name: 'PreToolUse', tool_name: 'ExitPlanMode', tool_input: {plan: '\n## **Plan:** move auth to `lucia`\n- step'}})?.reason, {kind: 'plan', text: 'plan ready: Plan: move auth to lucia'});
	assert.deepEqual(normalizeHook('claude', {hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: {command: 'ls'}}), {state: 'working', event: 'PreToolUse', nativeRef: undefined});
	// Permission prompts: the command, the file's name, the host, the MCP server and tool.
	const permission = (tool_name: string, tool_input: unknown) => normalizeHook('claude', {hook_event_name: 'PermissionRequest', tool_name, tool_input})?.reason?.text;
	assert.equal(permission('Bash', {command: 'npm   publish\n--tag next', description: 'Publish'}), 'wants to run: npm publish --tag next');
	assert.equal(permission('Edit', {file_path: '/repo/src/auth.ts'}), 'wants to edit: auth.ts');
	assert.equal(permission('WebFetch', {url: 'https://example.com/a?b'}), 'wants to fetch: example.com');
	assert.equal(permission('mcp__github__create_issue', {}), 'wants to use: github create_issue');
	assert.equal(permission('Glob', {}), 'wants to use: Glob');
	assert.equal(permission('AskUserQuestion', ask.tool_input), 'asks (+1): Which auth library?');
	// A reply's last non-empty line, without Markdown or terminal controls; a failure in words.
	assert.deepEqual(normalizeHook('claude', {hook_event_name: 'Stop', last_assistant_message: 'Done.\n\n- **Should I** also update `e2e`?\x1b[2J\n\n'})?.reason, {kind: 'message', text: 'said: Should I also update e2e?'});
	assert.equal(normalizeHook('claude', {hook_event_name: 'Stop', last_assistant_message: '  \n'})?.reason, undefined);
	assert.deepEqual(normalizeHook('claude', {hook_event_name: 'StopFailure', error_type: 'overloaded'})?.reason, {kind: 'failure', text: 'API overloaded'});
	assert.equal(normalizeHook('claude', {hook_event_name: 'StopFailure', error_type: 'rate_limit'})?.reason?.text, 'rate limited');
	assert.deepEqual(normalizeHook('claude', {hook_event_name: 'Notification', notification_type: 'idle_prompt', message: 'Claude is waiting for your input'})?.reason, {kind: 'notice', text: 'Claude is waiting for your input'});
	assert.equal(normalizeHook('claude', {hook_event_name: 'Stop', last_assistant_message: 'x'.repeat(500)})?.reason?.text.length, 'said: '.length + 300);
	// A native subagent's permission prompt blocks the root's TUI: reported, never with its identity.
	assert.deepEqual(normalizeHook('claude', {hook_event_name: 'PermissionRequest', agent_id: 'sub', session_id: 'abc', tool_name: 'Bash', tool_input: {command: 'rm x'}}), {state: 'needs-input', event: 'PermissionRequest', reason: {kind: 'permission', text: 'wants to run: rm x'}, fromSubagent: true});
	assert.equal(normalizeHook('claude', {hook_event_name: 'PostToolUse', agent_id: 'sub'})?.fromSubagent, true);
	assert.equal(normalizeHook('claude', {hook_event_name: 'SessionStart', agent_id: 'sub', session_id: 'abc'}), undefined);
	assert.equal(attentionMessage('needs-input'), 'needs input');
	assert.equal(attentionMessage('response-ended', {kind: 'message', text: 'said: ok'}), 'said: ok');
});
test('the hook bridge forwards only what a reason is made from, bounded', () => {
	const big = 'y'.repeat(5000);
	assert.deepEqual(hookPayloadFields({hook_event_name: 'PreToolUse', tool_name: 'Bash', tool_input: {command: 'cat secrets.txt'}, transcript_path: '/t', cwd: '/c'}), {hook_event_name: 'PreToolUse', tool_name: 'Bash'});
	assert.deepEqual(hookPayloadFields({hook_event_name: 'PostToolUse', tool_name: 'Bash', tool_input: {command: 'cat secrets.txt'}, tool_response: {stdout: 'SECRET'}}), {hook_event_name: 'PostToolUse', tool_name: 'Bash'});
	assert.deepEqual(hookPayloadFields({hook_event_name: 'PermissionRequest', tool_name: 'Bash', tool_input: {command: big, description: 'd', env: {A: '1'}}}).tool_input, {command: 'y'.repeat(256)});
	const questions = Array.from({length: 6}, (_, index) => ({question: `q${index}`, options: Array.from({length: 10}, (_, option) => ({label: `o${option}`, description: big, extra: 1}))}));
	const forwarded = hookPayloadFields({hook_event_name: 'PreToolUse', tool_name: 'AskUserQuestion', tool_input: {questions}}).tool_input as {questions: Array<{options: Array<{description: string}>}>};
	assert.equal(forwarded.questions.length, 4); assert.equal(forwarded.questions[0]!.options.length, 8); assert.equal(forwarded.questions[0]!.options[0]!.description.length, 200);
	assert.deepEqual(Object.keys(forwarded.questions[0]!.options[0]!), ['label', 'description']);
	assert.equal((hookPayloadFields({hook_event_name: 'PreToolUse', tool_name: 'ExitPlanMode', tool_input: {plan: big}}).tool_input as {plan: string}).plan.length, 2000);
	// The end of the reply is kept (that is where a question is).
	assert.equal(hookPayloadFields({hook_event_name: 'Stop', last_assistant_message: `${big}END`}).last_assistant_message, `${'y'.repeat(1997)}END`);
	assert.equal(hookPayloadFields({hook_event_name: 'Notification', message: big}).message, 'y'.repeat(256));
	assert.equal(hookPayloadFields({hook_event_name: 'UserPromptSubmit', prompt: 'my prompt'}).prompt, undefined);
	// What the bridge forwards normalizes like the full payload.
	const ask = {hook_event_name: 'PreToolUse', tool_name: 'AskUserQuestion', tool_input: {questions: [{question: 'Which?', options: [{label: 'A'}]}]}};
	assert.deepEqual(normalizeHook('claude', hookPayloadFields(ask)), normalizeHook('claude', ask));
});
test('agent signals default on for Claude only, as async hooks; Codex\'s hooks.json entries stay plain', () => {
	assert.equal(hooksEnabled(undefined, 'claude'), true); assert.equal(hooksEnabled(undefined, 'codex'), false); assert.equal(hooksEnabled(undefined, 'pi'), false);
	assert.equal(hooksEnabled(true, 'codex'), true); assert.equal(hooksEnabled(false, 'claude'), false);
	assert.equal(hookSettings({async: true}).hooks.PreToolUse![0]!.hooks[0]!.async, true);
	assert.equal('async' in hookSettings().hooks.Stop![0]!.hooks[0]!, false);
});
test('dev Codex requires native-daemon isolation even when lifecycle hooks are off', async t => {
	const directory = await tempDir(t, 'deckhand-capabilities-'); withEnv(t, {DECKHAND_CHANNEL: 'dev'});
	const supported = path.join(directory, 'supported'), unsupported = path.join(directory, 'unsupported');
	await fs.writeFile(supported, '#!/bin/sh\nprintf "resume --no-daemon\\n"\n', {mode: 0o755});
	await fs.writeFile(unsupported, '#!/bin/sh\nprintf "resume\\n"\n', {mode: 0o755});
	assert.deepEqual(await integrationArgs('codex', supported, false), ['--no-daemon']);
	await assert.rejects(integrationArgs('codex', unsupported, false), /--no-daemon missing/);
});
test('Codex exit hints preserve exact identities without last-session guesses', () => {
	assert.equal(codexResumeFromOutput('\x1b[32mTo continue: codex resume 12345678-abcd\x1b[0m\n')?.value, '12345678-abcd');
	assert.equal(codexResumeFromOutput('codex resume --last'), undefined);
});
