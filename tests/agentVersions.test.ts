import assert from 'node:assert/strict';
import {test} from 'node:test';
import React from 'react';
import {renderToString} from 'ink';
import {AGENTS} from '../src/agents.js';
import {agentUpdateStatus, compareVersions, installedVersions, parseVersion, sessionOutdated, updateHint} from '../src/agentVersions.js';
import {AgentsPane} from '../src/agentsFlow.js';
import {sessionDetails, sidebarRows} from '../src/sidebarModel.js';
import {Sidebar} from '../src/sidebar.js';
import type {AgentVersionInfo, AgentVersions, ProgramKey, SessionRecord} from '../src/types.js';

const NOW = Date.parse('2026-10-05T12:00:00.000Z');
const ago = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();
const plain = (text: string) => text.replace(/\x1b\[[0-9;]*m/g, '');
let order = 0;
const session = (id: string, fields: Partial<SessionRecord> = {}): SessionRecord => ({
	id, title: id, program: 'claude', command: 'claude', cwd: '/repo', repoRoot: '/repo', launchWorktreeRoot: '/repo', worktree: {mode: 'none'},
	status: 'running', agentStatus: 'idle', agentStatusUpdatedAt: ago(3), createdAt: ago(300), updatedAt: ago(3), sidebarOrder: order++, ...fields,
});
const info = (program: ProgramKey, fields: Partial<AgentVersionInfo> = {}): AgentVersionInfo => ({program, running: 0, outdated: 0, ...fields});
const versions = (fields: Partial<Record<ProgramKey, Partial<AgentVersionInfo>>> = {}): AgentVersions => ({
	claude: info('claude', fields.claude), pi: info('pi', fields.pi), codex: info('codex', fields.codex),
});

test('agent versions: each agent\'s --version output parses to x.y.z; comparison is numeric and ignores suffixes', () => {
	assert.equal(parseVersion('2.1.287 (Claude Code)\n'), '2.1.287');
	assert.equal(parseVersion('codex-cli 0.157.0'), '0.157.0');
	assert.equal(parseVersion('1.0.2'), '1.0.2');
	assert.equal(parseVersion('codex-cli 0.162.0-alpha.16'), '0.162.0');
	assert.equal(parseVersion('v20.11.1'), '20.11.1');
	for (const text of [undefined, '', 'Claude Code', 'version 2.1']) assert.equal(parseVersion(text), undefined, String(text));
	assert.ok(compareVersions('2.1.287', '2.1.290') < 0);
	assert.ok(compareVersions('0.160.1', '0.157.0') > 0);
	assert.ok(compareVersions('1.10.0', '1.9.9') > 0); // Numbers, not strings.
	assert.equal(compareVersions('0.162.0-alpha.16', '0.162.0'), 0);
	assert.equal(compareVersions('garbage', '1.0.0'), 0);
	// The per-agent table: npm package of the latest release and the agent's own update command.
	assert.deepEqual(Object.fromEntries(Object.entries(AGENTS).map(([program, spec]) => [program, [spec.version.npmPackage, ...spec.version.updateArgs]])), {
		claude: ['@anthropic-ai/claude-code', 'update'],
		pi: ['@earendil-works/pi-coding-agent', 'update', '--self'],
		codex: ['@openai/codex', 'update'],
	});
});

test('agent versions: status per agent, outdated sessions (running only), the header hint', () => {
	assert.equal(agentUpdateStatus(info('claude', {installed: '2.1.287', latest: '2.1.291'})), 'update available');
	assert.equal(agentUpdateStatus(info('claude', {installed: '2.1.291', latest: '2.1.291'})), 'up to date');
	assert.equal(agentUpdateStatus(info('claude', {installed: '2.1.292', latest: '2.1.291'})), 'up to date');
	assert.equal(agentUpdateStatus(info('claude', {installed: '2.1.287'})), 'latest unknown');
	assert.equal(agentUpdateStatus(info('claude', {latest: '2.1.291'})), 'not installed');
	assert.equal(agentUpdateStatus(undefined), 'not installed');

	assert.equal(sessionOutdated({status: 'running', agentVersion: '2.1.287'}, '2.1.290'), true);
	assert.equal(sessionOutdated({status: 'running', agentVersion: '2.1.290'}, '2.1.290'), false);
	assert.equal(sessionOutdated({status: 'running', agentVersion: '2.1.291'}, '2.1.290'), false); // Downgraded: a restart would not be newer.
	assert.equal(sessionOutdated({status: 'exited', agentVersion: '2.1.287'}, '2.1.290'), false); // Its next s uses the installed one anyway.
	assert.equal(sessionOutdated({status: 'starting', agentVersion: '2.1.287'}, '2.1.290'), false);
	assert.equal(sessionOutdated({status: 'running'}, '2.1.290'), false);
	assert.equal(sessionOutdated({status: 'running', agentVersion: '2.1.287'}, undefined), false);

	assert.equal(updateHint(versions({codex: {installed: '0.157.0', latest: '0.160.1'}, pi: {installed: '1.0.4', latest: '1.0.4'}})), 'codex update · U');
	assert.equal(updateHint(versions({codex: {installed: '0.157.0', latest: '0.160.1'}, pi: {installed: '1.0.2', latest: '1.0.4'}})), '2 agent updates · U');
	assert.equal(updateHint(versions({claude: {installed: '2.1.287'}, codex: {installed: '0.160.1', latest: '0.160.1'}})), ''); // Unknown or current.
	assert.equal(updateHint(versions({pi: {latest: '1.0.4'}})), ''); // Not installed is not an update.
	assert.equal(updateHint(undefined), '');
	assert.deepEqual(installedVersions(versions({claude: {installed: '2.1.290'}, pi: {latest: '1.0.4'}})), {claude: '2.1.290'});
});

test('sidebar: ↑ before the agent glyph on outdated running sessions, dropped first; the details name both versions', () => {
	const all = [
		session('old', {title: 'old build', agentVersion: '2.1.287'}),
		session('new', {title: 'new build', agentVersion: '2.1.290'}),
		session('gone', {title: 'exited', agentVersion: '2.1.287', status: 'exited', exitReason: 'completed'}),
		session('pi', {title: 'pi one', program: 'pi', agentVersion: '1.0.2'}),
	];
	const installed = {claude: '2.1.290'};
	const texts = (width: number, rows = all) => sidebarRows({rows, allSessions: all, firstNumber: 1, numberWidth: 1, width, spinnerFrame: '⠋', filter: 'all', installedVersions: installed}).map(row => row.parts.map(part => part.text).join(''));
	assert.deepEqual(texts(31), [
		'  1 ● old build             ↑ ✶',
		'  2 ● new build               ✶',
		'  3 ○ exited                  ✶',
		'  4 ● pi one                  π', // No installed version known for pi: never marked.
	]);
	const [marked] = sidebarRows({rows: [all[0]!], allSessions: all, firstNumber: 1, numberWidth: 1, width: 31, spinnerFrame: '⠋', filter: 'all', installedVersions: installed});
	assert.deepEqual(marked!.parts.filter(part => part.text.trim()).map(part => part.role).slice(-2), ['outdated', 'agent']);
	// Short of room, ↑ goes before +N (and the rest); the agent glyph stays.
	const parent = session('p', {title: 'parent session', agentVersion: '2.1.287'});
	const child = session('k', {title: 'child', parentSessionId: 'p', subSessionKind: 'clean'});
	const rowsOf = (width: number) => sidebarRows({rows: [parent], allSessions: [parent, child], firstNumber: 1, numberWidth: 1, width, spinnerFrame: '⠋', filter: 'all', collapsedSessionIds: new Set(['p']), installedVersions: installed})[0]!.parts.map(part => part.text).join('');
	assert.equal(rowsOf(30), '  1 ▸ ● parent session  +1 ↑ ✶');
	assert.equal(rowsOf(17), '  1 ▸ ● par… +1 ✶');
	// Without versions nothing changes.
	assert.equal(sidebarRows({rows: [all[0]!], allSessions: all, firstNumber: 1, numberWidth: 1, width: 31, spinnerFrame: '⠋', filter: 'all'})[0]!.parts.map(part => part.text).join(''), '  1 ● old build               ✶');

	const details = (id: string, width: number) => sessionDetails(all.find(item => item.id === id), all, width, 10, NOW, installed).map(line => line.map(part => part.text).join(''));
	assert.deepEqual(details('old', 48).slice(2, 3), ['✶ claude 2.1.287 · 2.1.290 installed · idle · 3m']);
	assert.deepEqual(details('old', 40).slice(2, 3), ['✶ claude 2.1.287 → 2.1.290 · idle · 3m']);
	assert.deepEqual(details('old', 31).slice(2, 3), ['✶ 2.1.287 → 2.1.290 · idle · 3m']);
	assert.deepEqual(details('old', 30).slice(2, 3), ['✶ 2.1.287 → 2.1.290 · idle']);
	assert.deepEqual(details('old', 20).slice(2, 3), ['✶ claude · idle · 3m']); // Too narrow for versions: the plain line.
	assert.deepEqual(details('old', 18).slice(2, 3), ['✶ idle · 3m']);
	assert.deepEqual(details('new', 48).slice(2, 3), ['✶ claude · idle · 3m']);
	assert.deepEqual(details('gone', 48).slice(2, 3), ['✶ claude · exited · 3m']);

	// Rendered at the default sidebar width (34), the outdated session selected.
	const lines = plain(renderToString(React.createElement(Sidebar, {sessions: all, allSessions: all, selectedId: 'old', width: 34, height: 11, spinnerFrame: '⠋', filter: 'all', query: '', now: NOW, installedVersions: installed}), {columns: 34})).split('\n');
	assert.deepEqual(lines, [
		'╭────────────────────────────────╮',
		'│ Sessions               all 4/4 │',
		'│› 1 ● old build             ↑ ✶ │',
		'│╎ 2 ● new build               ✶ │',
		'│╎ 3 ○ exited                  ✶ │',
		'│╎ 4 ● pi one                  π │',
		'│ ────────────────────────────── │',
		'│ old build                      │',
		'│ ✶ 2.1.287 → 2.1.290 · idle     │',
		'│ main checkout · shared with 3  │',
		'╰────────────────────────────────╯',
	]);
});

test('Agents screen: one row per agent with versions and status, details of the selected one, confirmation, results', () => {
	const fixture = versions({
		claude: {installed: '2.1.287', latest: '2.1.291', path: '/Users/me/.local/bin/claude', checkedAt: ago(3), running: 3, outdated: 1},
		codex: {installed: '0.160.1', latest: '0.160.1', path: '/usr/local/bin/codex', checkedAt: ago(3)},
		pi: {installed: '1.0.2', path: '/usr/local/bin/pi', checkedAt: ago(3), error: 'latest unknown: npm view timed out'},
	});
	const render = (props: Partial<React.ComponentProps<typeof AgentsPane>>, width = 100, height = 22) => plain(renderToString(React.createElement(AgentsPane, {versions: fixture, selected: 0, width, height, now: NOW, ...props}), {columns: width}));
	const screen = render({});
	for (const text of ['Agents', 'Running sessions keep their version until restarted', 'Claude    2.1.287   latest 2.1.291  1 session on an older version', 'update available', 'Pi        1.0.2     latest ?', 'latest unknown', 'Codex     0.160.1   latest 0.160.1', 'up to date',
		'Claude · update available', 'Installed  2.1.287  /Users/me/.local/bin/claude', 'Latest     2.1.291  npm @anthropic-ai/claude-code · checked 3m ago', 'Update     claude update', '3 running sessions · 1 on an older version (restart to update)', 'enter update', 'r re-check', 'esc back']) assert.ok(screen.includes(text), `${text}\n${screen}`);
	assert.ok(render({selected: 1}).includes('latest unknown: npm view timed out'));
	assert.ok(render({versions: versions({pi: {}})}).includes('not installed'));
	assert.ok(render({versions: versions({pi: {}}), selected: 1}).includes('Run `deckhand setup` to install it.'));
	assert.ok(render({checking: true}).includes('Checking the latest releases…'));
	const confirm = render({confirming: 'claude'});
	for (const text of ['Update Claude?', '3 running Claude sessions keep their current version until restarted.', 'Runs claude update.', 'enter update · esc cancel']) assert.ok(confirm.includes(text), `${text}\n${confirm}`);
	assert.ok(render({working: new Set(['claude'])}).includes('Running claude update… sessions keep running.'));
	assert.ok(render({working: new Set(['claude'])}).includes('updating…'));
	const done = render({outcomes: {claude: {program: 'claude', ok: true, exitCode: 0, command: 'claude update', output: 'Checking…\nUpdated to 2.1.291', before: '2.1.287', after: '2.1.291', versions: fixture}}});
	assert.ok(done.includes('Updated 2.1.287 → 2.1.291. Running sessions keep their version until restarted.'), done);
	const failed = render({outcomes: {claude: {program: 'claude', ok: false, exitCode: 1, command: 'claude update', output: 'line 1\nError: permission denied', before: '2.1.287', after: '2.1.287', versions: fixture}}});
	for (const text of ['claude update failed (exit 1):', 'line 1', 'Error: permission denied']) assert.ok(failed.includes(text), `${text}\n${failed}`);
});
