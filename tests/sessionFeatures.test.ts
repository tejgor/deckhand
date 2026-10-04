import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {tempDir, withEnv} from './helpers.js';
import {filterSessionList, handoffMarkdown} from '../src/sessionFeatures.js';
import {normalizeHook, codexResumeFromOutput, needsAttention, integrationArgs} from '../src/agentSignals.js';
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
test('handoffs include notes by default, and clearly label optional terminal excerpts', () => {
	const session = {...root, notes: 'Implement the feature', lastPreview: 'sensitive terminal output'};
	assert.match(handoffMarkdown(session), /Implement the feature/); assert.doesNotMatch(handoffMarkdown(session), /sensitive terminal output/);
	assert.match(handoffMarkdown(session, true), /not a complete transcript/);
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
