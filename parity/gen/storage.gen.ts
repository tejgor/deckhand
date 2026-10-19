/**
 * Gen script for src/storage.ts. Regenerate with `node --import tsx parity/gen/storage.gen.ts`.
 * Directory cases treat the first op argument as the Deckhand home ($ROOT); the replay sets DECKHAND_HOME to it.
 * Skipped: stateFileDisplayPath (depends on the process cwd, covered by a ported test) and lock-contention timing.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, arb, fc, installDeterminism, mutate, recordDirCase, registerFn, sample} from '../lib.ts';
import * as storage from '../../src/storage.ts';

installDeterminism();
const recorder = new Recorder('storage');

// Pure function: sortSessionsNewestFirst (tests don't call it directly; generated + mutated).
const statuses = ['running', 'exited', 'starting', 'stopping'];
const stamps = ['2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.001Z', '2025-12-31T23:59:59.999Z', '2026-09-10T09:00:00.000Z', '2026-09-10T09:00:00Z', '', 'a', 'b', 'B'];
const sessionArb = fc.record({id: fc.constantFrom('s1', 's2', 's3', 's4', '😀', '中文'), status: fc.constantFrom(...statuses), createdAt: fc.constantFrom(...stamps), title: arb.text(6)});
for (const args of sample(fc.tuple(fc.array(sessionArb, {maxLength: 8})), 150)) recorder.call('sortSessionsNewestFirst', storage.sortSessionsNewestFirst, args);
recorder.call('sortSessionsNewestFirst', storage.sortSessionsNewestFirst, [[]]);
for (const args of mutate(recorder.argsOf('sortSessionsNewestFirst'), {variants: 2}).slice(0, 80)) recorder.call('sortSessionsNewestFirst', storage.sortSessionsNewestFirst, args);
await recorder.write({prune: true});

// Directory cases. `home` is always args[0].
type Fn = (...args: any[]) => unknown;
const inHome = (fn: Fn): Fn => async (home: string, ...rest: unknown[]) => { process.env['DECKHAND_HOME'] = home; return fn(...rest); };
const write = (rel: string, content: string | object, mode?: number) => async (root: string) => {
	await fs.mkdir(path.dirname(path.join(root, rel)), {recursive: true});
	await fs.writeFile(path.join(root, rel), typeof content === 'string' ? content : JSON.stringify(content, null, 2) + '\n', mode ? {mode} : undefined);
};
const names: string[] = [];
async function dirCase(name: string, fn: string, run: Fn, setup: (root: string) => Promise<void> | void, rest: unknown[] = []) {
	await recordDirCase('storage', name, {setup, fn, args: root => [root, ...rest], run: inHome(run)});
	names.push(`storage/${name}`);
}

const session = (id: string, extra: Record<string, unknown> = {}) => ({id, title: `Session ${id}`, program: 'claude', command: 'claude', cwd: '/r', repoRoot: '/r', worktree: {mode: 'none'}, status: 'running', createdAt: '2026-09-10T09:00:00.000Z', updatedAt: '2026-09-10T09:00:00.000Z', ...extra});

// ensureConfigDir
await recordDirCase('storage', 'ensure-config-dir-missing', {setup: () => {}, fn: 'ensureConfigDir', args: root => [path.join(root, 'a', 'b')], run: inHome(storage.ensureConfigDir)});
names.push('storage/ensure-config-dir-missing');
await dirCase('ensure-config-dir-existing', 'ensureConfigDir', storage.ensureConfigDir, () => {});
await dirCase('ensure-config-dir-loose-mode', 'ensureConfigDir', storage.ensureConfigDir, async root => { await fs.chmod(root, 0o755); });
await dirCase('ensure-config-dir-group-mode', 'ensureConfigDir', storage.ensureConfigDir, async root => { await fs.chmod(root, 0o770); });

// loadState
await dirCase('load-state-missing', 'loadState', storage.loadState, () => {});
await dirCase('load-state-empty-file', 'loadState', storage.loadState, write('state.json', ''));
await dirCase('load-state-whitespace-file', 'loadState', storage.loadState, write('state.json', ' \n\t\n'));
await dirCase('load-state-corrupt', 'loadState', storage.loadState, write('state.json', '{"sessions": [ oops'));
await dirCase('load-state-corrupt-unicode', 'loadState', storage.loadState, write('state.json', '{"sessions":["😀中'));
await dirCase('load-state-empty-object', 'loadState', storage.loadState, write('state.json', '{}'));
await dirCase('load-state-not-arrays', 'loadState', storage.loadState, write('state.json', {sessions: 'x', worktrees: {}}));
await dirCase('load-state-null-json', 'loadState', storage.loadState, write('state.json', 'null'));
await dirCase('load-state-array-json', 'loadState', storage.loadState, write('state.json', '[]'));
await dirCase('load-state-plain', 'loadState', storage.loadState, write('state.json', {sessions: [session('a', {futureField: {k: [1, 'two', null]}}), session('b', {status: 'exited', title: '😀 中文'})], worktrees: []}));
await dirCase('load-state-keeps-unknown-top-keys', 'loadState', storage.loadState, write('state.json', {version: 3, sessions: [session('a')], worktrees: [], extra: 1}));
await dirCase('load-state-done-marker-migration', 'loadState', storage.loadState, write('state.json', {sessions: [session('a', {mergedAt: '2026-09-11T00:00:00.000Z', mergeTargetBranch: 'main', mergeSourceRef: 'x', mergeMarkedManually: true}), session('b', {doneAt: '2026-09-12T00:00:00.000Z', mergedAt: '2026-09-11T00:00:00.000Z'}), session('c', {worktree: {mode: 'attach', path: '/r', isMain: true, mergedAt: '2026-09-13T00:00:00.000Z', mergeMode: 'merge'}})], worktrees: []}));
await dirCase('load-state-worktree-records-kept', 'loadState', storage.loadState, write('state.json', {
	sessions: [session('a', {cwd: '/w', worktree: {mode: 'managed', path: '/w', id: 'wt-1', mergedAt: 'projected'}}), session('b', {cwd: '/w', worktree: {mode: 'managed', path: '/w', id: 'wt-1'}})],
	worktrees: [{id: 'wt-1', path: '/w', createdAt: '2026-09-10T09:00:00.000Z', mergedAt: '2026-09-11T00:00:00.000Z'}],
}));
await dirCase('load-state-unreferenced-record-dropped', 'loadState', storage.loadState, write('state.json', {sessions: [session('a')], worktrees: [{id: 'wt-9', path: '/gone', createdAt: '2026-09-10T09:00:00.000Z'}]}));
await dirCase('load-state-missing-record-rebuilt', 'loadState', storage.loadState, write('state.json', {sessions: [session('a', {cwd: '/w', worktree: {mode: 'managed', path: '/w', id: 'wt-2', deletedAt: '2026-09-12T00:00:00.000Z'}})], worktrees: []}));

// saveState
await dirCase('save-state-new', 'saveState', storage.saveState, () => {}, [{sessions: [], worktrees: []}]);
await dirCase('save-state-overwrites', 'saveState', storage.saveState, write('state.json', {sessions: [session('old')], worktrees: []}, 0o644), [{sessions: [session('new', {title: '😀 中文 é'})], worktrees: [{id: 'w', path: '/w', createdAt: 'x'}]}]);
await dirCase('save-state-undefined-props', 'saveState', storage.saveState, () => {}, [{sessions: [{...session('a'), pid: undefined, exitCode: null}], worktrees: []}]);
await dirCase('save-state-key-order', 'saveState', storage.saveState, () => {}, [{worktrees: [], sessions: [{status: 'exited', id: 'z', title: 't'}]}]);
await dirCase('save-state-loose-dir', 'saveState', storage.saveState, async root => { await fs.chmod(root, 0o755); }, [{sessions: [], worktrees: []}]);

// markAllNonExitedSessionsExited
const mark = (name: string, setup: (root: string) => Promise<void> | void) => dirCase(`mark-exited-${name}`, 'markAllNonExitedSessionsExited', storage.markAllNonExitedSessionsExited, setup);
await mark('missing-state', () => {});
await mark('nothing-to-change', write('state.json', {sessions: [session('a', {status: 'exited'})], worktrees: []}));
await mark('dev-running-cleared', write('state.json', {sessions: [session('a', {status: 'exited', devRunning: true})], worktrees: []}));
await mark('running', write('state.json', {sessions: [session('a', {pid: 123, devRunning: true, attention: {state: 'working', event: 'UserPromptSubmit', at: 'x', extra: 1}}), session('b', {status: 'exited', exitCode: 1, exitSignal: null}), session('c', {status: 'starting', exitCode: 7})], worktrees: []}));
await mark('setup-running', write('state.json', {sessions: [session('a', {setup: {command: 'npm ci', state: 'running', output: 'partial 😀'}}), session('b', {setup: {command: 'x', state: 'complete', output: 'done'}}), session('c', {setup: {command: 'x', state: 'pending', output: ''}})], worktrees: []}));
await mark('keeps-identity', write('state.json', {sessions: [session('a', {archivedAt: 'yesterday', notes: 'keep', agentSessionRef: {provider: 'codex', kind: 'id', value: 'native'}, setup: {command: 'setup', state: 'running', output: 'partial'}})], worktrees: []}));
await mark('signal-kept', write('state.json', {sessions: [session('a', {exitSignal: 9})], worktrees: []}));
await mark('migrated-written-back', write('state.json', {sessions: [session('a', {status: 'exited', mergedAt: '2026-09-11T00:00:00.000Z'})], worktrees: []}));
await mark('empty-file', write('state.json', ''));
await mark('corrupt', write('state.json', '{nope'));

// loadAppConfig
const cfg = (content: string | object) => write('config.json', content);
const load = (name: string, setup: (root: string) => Promise<void> | void) => dirCase(`load-config-${name}`, 'loadAppConfig', storage.loadAppConfig, setup);
await load('missing', () => {});
await load('empty-object', cfg({}));
await load('full', cfg({dev_command: 'npm run dev', attach_scroll_sensitivity: 2.5, trustedProjects: {'/a': ['h1', 'h2'], '/b': 'legacy'}, agent_hooks: true, notifications: true, defaults: {setupCommand: 'npm ci'}, future_setting: 1}));
await load('wrong-types', cfg({dev_command: 5, attach_scroll_sensitivity: '2', trustedProjects: [], agent_hooks: 'yes', notifications: 'true', defaults: null}));
await load('trusted-mixed', cfg({trustedProjects: {'/a': ['x', 1, null, 'y'], '/b': [], '/c': '', '/d': 5, '/e': [2], '/中😀': ['z']}}));
await load('trusted-null', cfg({trustedProjects: null, dev_command: ''}));
await load('unicode', cfg({dev_command: '😀 中文 é', defaults: [1, 'two', {three: null}]}));
await load('not-object-array', cfg('[]'));
await load('not-object-null', cfg('null'));
await load('not-object-number', cfg('3'));
await load('tightens-dir', async root => { await fs.chmod(root, 0o755); await cfg({notifications: true})(root); });

// updateAppConfig with patches
const patches: Record<string, [unknown, object | undefined]> = {
	'dev-command-new-file': [{dev_command: 'dev'}, undefined],
	'notifications-new-file': [{notifications: true}, undefined],
	'patch-keeps-unknown': [{dev_command: 'dev'}, {future_setting: {keep: true}, trustedProjects: {'/legacy': 'old'}}],
	'patch-removes-key': [{dev_command: undefined}, {dev_command: 'x', other: 1}],
	'patch-trusted': [{trustedProjects: {'/a': ['h']}}, {trustedProjects: {'/a': 'old'}}],
	'patch-empty': [{}, {a: 1}],
	'patch-noop-same-value': [{dev_command: 'x'}, {dev_command: 'x', z: [1, 2]}],
	'patch-notifications-false': [{notifications: false}, {notifications: true}],
	'patch-agent-hooks': [{agent_hooks: false}, {}],
	'patch-sensitivity': [{attach_scroll_sensitivity: 3.25}, {attach_scroll_sensitivity: 1}],
	'patch-defaults': [{defaults: {setupCommand: 'npm ci', actions: {test: 'npm test'}}}, {future_setting: 1}],
	'patch-unicode': [{dev_command: '😀 中文 é x'}, {}],
	'patch-key-order': [{dev_command: 'd'}, {zeta: 1, alpha: 2, trustedProjects: {}}],
	'patch-undefined-defaults-removes': [{defaults: undefined}, {defaults: {a: 1}}],
};
for (const [name, [patch, existing]] of Object.entries(patches)) await dirCase(`update-config-${name}`, 'updateAppConfig', storage.updateAppConfig, existing ? cfg(existing) : () => {}, [patch]);
await dirCase('update-config-not-object', 'updateAppConfig', storage.updateAppConfig, cfg('[1]'), [{dev_command: 'x'}]);
await dirCase('update-config-stale-lock', 'updateAppConfig', storage.updateAppConfig, async root => {
	await cfg({dev_command: 'a'})(root);
	await fs.writeFile(path.join(root, 'config.json.lock'), 'stale');
	const old = new Date('2020-01-01T00:00:00Z');
	await fs.utimes(path.join(root, 'config.json.lock'), old, old);
}, [{notifications: true}]);

// Updater functions (the replay dispatches on the registered name).
const addTrust = registerFn('addTrust', (current: any) => ({...current, trustedProjects: {...current.trustedProjects, '/new': ['hash']}}));
const clearDev = registerFn('clearDev', (current: any) => ({...current, dev_command: undefined}));
const identity = registerFn('identity', (current: any) => current);
const dropTrusted = registerFn('dropTrusted', (current: any) => ({...current, trustedProjects: {}}));
await dirCase('update-config-fn-add-trust', 'updateAppConfig', storage.updateAppConfig, cfg({trustedProjects: {'/old': 'legacy'}, keep: 1}), [addTrust]);
await dirCase('update-config-fn-clear-dev', 'updateAppConfig', storage.updateAppConfig, cfg({dev_command: 'x', keep: 1}), [clearDev]);
await dirCase('update-config-fn-identity', 'updateAppConfig', storage.updateAppConfig, cfg({trustedProjects: {'/old': 'legacy'}, notifications: 'str'}), [identity]);
await dirCase('update-config-fn-drop-trusted', 'updateAppConfig', storage.updateAppConfig, cfg({trustedProjects: {'/old': ['a']}}), [dropTrusted]);

