import assert from 'node:assert/strict';
import {test} from 'node:test';
import {AGENTS, launchArgs, relaunchPlan} from '../src/agents.js';
import type {AgentSessionRef, ProgramKey} from '../src/types.js';

const id = (provider: ProgramKey, value: string): AgentSessionRef => ({provider, kind: 'id', value});

test('each agent builds its create, resume, fork and fresh argv from one table', () => {
	assert.deepEqual(launchArgs('claude', {kind: 'new', ref: id('claude', 'c1'), name: 'dh-a'}), ['--session-id', 'c1', '--name', 'dh-a']);
	assert.deepEqual(launchArgs('claude', {kind: 'resume', ref: id('claude', 'c1')}), ['--resume', 'c1']);
	assert.deepEqual(launchArgs('claude', {kind: 'resume', ref: {provider: 'claude', kind: 'name', value: 'dh-old'}}), ['--resume', 'dh-old']);
	assert.deepEqual(launchArgs('claude', {kind: 'fork', parent: id('claude', 'p'), ref: id('claude', 'c2'), name: 'dh-b'}), ['--resume', 'p', '--fork-session', '--session-id', 'c2', '--name', 'dh-b']);
	assert.deepEqual(launchArgs('pi', {kind: 'new', ref: id('pi', 'p1'), name: 'dh-a'}), ['--session-id', 'p1', '--name', 'dh-a']);
	assert.deepEqual(launchArgs('pi', {kind: 'resume', ref: id('pi', 'p1')}), ['--session-id', 'p1']);
	assert.deepEqual(launchArgs('pi', {kind: 'resume', ref: {provider: 'pi', kind: 'path', value: '/s.jsonl'}}), ['--session', '/s.jsonl']);
	assert.deepEqual(launchArgs('pi', {kind: 'fork', parent: id('pi', 'p'), ref: id('pi', 'c'), name: 'dh-b'}), ['--fork', 'p', '--session-id', 'c', '--name', 'dh-b']);
	assert.deepEqual(launchArgs('codex', {kind: 'new', name: 'dh-a'}), []);
	assert.deepEqual(launchArgs('codex', {kind: 'resume', ref: id('codex', 'x1')}), ['resume', 'x1']);
	assert.deepEqual(launchArgs('codex', {kind: 'fork', parent: id('codex', 'x0'), name: 'dh-b'}), ['fork', 'x0']);
	assert.equal(AGENTS.codex.forksAcrossDirectories, false);
	assert.ok(AGENTS.claude.forksAcrossDirectories && AGENTS.pi.forksAcrossDirectories);
});

test('relaunch resumes a fork\'s own conversation and forks the parent again only when it has none', () => {
	const parent = id('claude', 'parent');
	const fork = {program: 'claude' as const, subSessionKind: 'forked' as const, forkedFromAgentSessionRef: parent};
	assert.deepEqual(relaunchPlan({...fork, agentSessionRef: id('claude', 'child')}, 'resume', false, 'dh-x'), {kind: 'resume', ref: id('claude', 'child')});
	const again = relaunchPlan({...fork, agentSessionRef: parent}, 'resume', false, 'dh-x');
	assert.equal(again?.kind, 'fork'); assert.ok(again?.kind === 'fork' && again.ref && again.ref.value !== 'parent');
	assert.equal(relaunchPlan({...fork, agentSessionRef: id('claude', 'child')}, 'resume', true, 'dh-x')?.kind, 'fork'); // Never launched.
	assert.equal(relaunchPlan({...fork, agentSessionRef: id('claude', 'child')}, 'fresh', false, 'dh-x')?.kind, 'new');
	const codexFork = {program: 'codex' as const, subSessionKind: 'forked' as const, forkedFromAgentSessionRef: id('codex', 'x0')};
	assert.deepEqual(relaunchPlan(codexFork, 'resume', false, 'dh-x'), {kind: 'fork', parent: id('codex', 'x0'), ref: undefined, name: 'dh-x'});
	assert.equal(relaunchPlan({program: 'codex'}, 'resume', false, 'dh-x'), undefined); // Unknown ID: refused, never fresh.
});
