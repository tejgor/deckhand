/**
 * Gen script for src/uiState.ts. Regenerate with `node --import tsx parity/gen/uiState.gen.ts`.
 * normalizeUiState is a pure function (jsonl). loadUiState / saveUiState are directory cases whose first op argument
 * is the Deckhand home ($ROOT); the replay sets DECKHAND_HOME to it. Repo roots are absolute paths under $ROOT.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {Recorder, arb, fc, installDeterminism, mutate, recordDirCase, runTap, sample} from '../lib.ts';
import * as uiState from '../../src/uiState.ts';

installDeterminism();
const recorder = new Recorder('uiState');

// 1. Tap: calls tests/storage.test.ts makes to normalizeUiState (cross-module calls only).
recorder.addTap(await runTap({tests: ['tests/storage.test.ts'], tap: ['uiState:normalizeUiState']}), {module: 'uiState'});

// 2. Hand-written edge cases.
const full = {selectedId: 's1', activeTab: 'git', sidebarWidth: 31.5, sessionFilter: 'attention', sessionQuery: 'q', sessionTabs: {a: 'notes', b: 'dev'}, collapsedSessionIds: ['x', 'y'], hiddenExitedSessionIds: ['z'], readReplies: {a: '2026-01-01T00:00:00.000Z'}};
const edge: unknown[] = [
	undefined, null, 0, 1, '', 'str', true, [], [1, 2], {}, full,
	{...full, activeTab: 'nope'}, {...full, activeTab: 5}, {...full, sessionFilter: 'nope'}, {...full, sessionFilter: 'archived'}, {...full, sessionFilter: 'all'},
	{...full, sidebarWidth: Number.NaN}, {...full, sidebarWidth: 0}, {...full, sidebarWidth: -4}, {...full, sidebarWidth: '3'}, {...full, sidebarWidth: 1e21},
	{...full, selectedId: 5}, {...full, selectedId: ''}, {...full, selectedId: '😀中文'},
	{...full, sessionQuery: 'x'.repeat(4096)}, {...full, sessionQuery: 'x'.repeat(4097)}, {...full, sessionQuery: 'é'.repeat(5000)}, {...full, sessionQuery: 'a'.repeat(4095) + '中b'}, {...full, sessionQuery: 7},
	{...full, sessionTabs: null}, {...full, sessionTabs: []}, {...full, sessionTabs: ['preview', 'bad']}, {...full, sessionTabs: 'preview'}, {...full, sessionTabs: {a: 'preview', b: 'x', c: 3, d: null, '😀': 'terminal'}},
	{...full, collapsedSessionIds: 'x'}, {...full, collapsedSessionIds: ['a', 1, null, 'b', {}, []]}, {...full, hiddenExitedSessionIds: {0: 'a'}},
	{...full, collapsedSessionIds: Array.from({length: 10005}, (_, i) => `id${i}`)}, {...full, collapsedSessionIds: [...Array.from({length: 9999}, () => 1), 'a', 'b', 'c']},
	{...full, readReplies: null}, {...full, readReplies: ['a', 'b']}, {...full, readReplies: {a: 'x', b: 5, c: null, d: 'y'}},
	{...full, readReplies: Object.fromEntries(Array.from({length: 1005}, (_, i) => [`s${i}`, `t${i}`]))},
	{...full, readReplies: Object.fromEntries([['b', 'x'], ['10', 'x'], ['2', 'y'], ['a', 'z']])},
	{...full, sessionTabs: {'2': 'git', '1': 'dev', b: 'notes', a: 'preview'}},
	{extra: 1, selectedId: 'a'}, {selectedId: undefined, sessionFilter: undefined},
];
for (const value of edge) recorder.call('normalizeUiState', uiState.normalizeUiState, [value]);

// 3. fast-check: structurally random objects mixing valid and invalid values per field.
const key = fc.constantFrom('a', 'b', 'c', '😀', '中', '1', '2', '', ' ');
const junk = fc.oneof(fc.constant(null), fc.integer(), fc.boolean(), arb.text(5), fc.constantFrom('preview', 'terminal', 'git', 'dev', 'notes', 'bad'), fc.array(arb.text(3), {maxLength: 3}), fc.constant(Number.NaN), fc.constant(Number.POSITIVE_INFINITY), fc.double({noNaN: true, noDefaultInfinity: true}));
const tab = fc.oneof(fc.constantFrom('preview', 'terminal', 'git', 'dev', 'notes', 'bad', 'files'), junk);
const idList = fc.oneof(fc.array(fc.oneof(arb.text(4), junk), {maxLength: 6}), junk);
const arbState = fc.record({
	selectedId: fc.oneof(arb.text(5), junk), activeTab: tab, sidebarWidth: fc.oneof(fc.integer({min: -10, max: 200}), junk),
	sessionFilter: fc.oneof(fc.constantFrom('active', 'running', 'attention', 'exited', 'archived', 'all', 'x'), junk), sessionQuery: fc.oneof(arb.text(30), junk),
	sessionTabs: fc.oneof(fc.dictionary(key, tab, {maxKeys: 5}), junk), collapsedSessionIds: idList, hiddenExitedSessionIds: idList,
	readReplies: fc.oneof(fc.dictionary(key, fc.oneof(arb.text(6), junk), {maxKeys: 5}), junk),
}, {requiredKeys: []});
for (const value of sample(arbState, 180)) recorder.call('normalizeUiState', uiState.normalizeUiState, [value]);

// 4. Mutations of the recorded inputs.
for (const args of mutate(recorder.argsOf('normalizeUiState'), {variants: 2}).slice(0, 100)) recorder.call('normalizeUiState', uiState.normalizeUiState, args);
await recorder.write({prune: true});

// 5. Directory cases.
type Fn = (...args: any[]) => unknown;
const inHome = (fn: Fn): Fn => async (home: string, ...rest: unknown[]) => { process.env['DECKHAND_HOME'] = home; return fn(...rest); };
const file = 'ui-state.json';
const put = (content: string | object) => async (root: string) => { await fs.writeFile(path.join(root, file), typeof content === 'string' ? content : JSON.stringify(content, null, 2) + '\n'); };
async function load(name: string, repo: string, setup: (root: string) => Promise<void> | void) {
	await recordDirCase('uiState', name, {setup, fn: 'loadUiState', args: root => [root, path.join(root, repo)], run: inHome(uiState.loadUiState)});
}
async function save(name: string, repo: string, state: unknown, setup: (root: string) => Promise<void> | void = () => {}) {
	await recordDirCase('uiState', name, {setup, fn: 'saveUiState', args: root => [root, path.join(root, repo), state], run: inHome(uiState.saveUiState)});
}
const keyed = (root: string, repo: string, state: unknown) => ({[path.join(root, repo)]: state});

await load('load-missing-file', 'repo', () => {});
await load('load-invalid-json', 'repo', put('{nope'));
await load('load-empty-file', 'repo', put(''));
await load('load-array-root', 'repo', put('[]'));
await load('load-null-root', 'repo', put('null'));
await load('load-other-repo', 'repo', async root => put({[path.join(root, 'other')]: full})(root));
await load('load-repo-found', 'repo', async root => put(keyed(root, 'repo', full))(root));
await load('load-repo-normalized', 'repo', async root => put(keyed(root, 'repo', {activeTab: 'zzz', sessionFilter: 'bad', sidebarWidth: 'wide', collapsedSessionIds: [1, 'a'], sessionTabs: {a: 'git', b: 'bad'}}))(root));
await load('load-repo-non-object-entry', 'repo', async root => put(keyed(root, 'repo', 'string'))(root));
await load('load-repo-unicode', '😀 中文/repo', async root => put(keyed(root, '😀 中文/repo', {...full, selectedId: '中', sessionQuery: '😀'}))(root));
await load('load-path-resolved', 'a/../repo', async root => put(keyed(root, 'repo', full))(root));
await load('load-is-directory', 'repo', async root => { await fs.mkdir(path.join(root, file)); });

await save('save-new-file', 'repo', full);
await save('save-new-dir-missing', 'repo', full, async root => { await fs.rmdir(root).catch(() => {}); await fs.mkdir(root); });
await save('save-defaults', 'repo', {sessionTabs: {}, collapsedSessionIds: [], hiddenExitedSessionIds: [], readReplies: {}});
await save('save-normalizes', 'repo', {...full, activeTab: 'bad', sessionFilter: 'zzz', sidebarWidth: Number.NaN, sessionQuery: 'x'.repeat(5000)});
await save('save-undefined-props', 'repo', {selectedId: undefined, sessionTabs: {}, collapsedSessionIds: [], hiddenExitedSessionIds: [], readReplies: {}});
await save('save-unicode', '😀 中文/repo', {...full, selectedId: '😀', sessionQuery: '中文 é', collapsedSessionIds: ['😀', '中'], readReplies: {'😀': '中'}});
await save('save-keeps-other-repos', 'repo', full, async root => put({[path.join(root, 'other')]: {future: 1, selectedId: 'o'}, '/elsewhere': 5})(root));
await save('save-replaces-repo', 'repo', {...full, selectedId: 'new'}, async root => put(keyed(root, 'repo', {...full, selectedId: 'old'}))(root));
await save('save-key-order-appends', 'zzz', full, async root => put({[path.join(root, 'b')]: full, [path.join(root, 'a')]: full})(root));
await save('save-over-invalid-json', 'repo', full, put('{broken'));
await save('save-over-array-root', 'repo', full, put('[1]'));
await save('save-over-empty-file', 'repo', full, put(''));
await save('save-resolves-repo-path', 'a/../repo', full);
await save('save-readreplies-cap', 'repo', {sessionTabs: {}, collapsedSessionIds: [], hiddenExitedSessionIds: [], readReplies: Object.fromEntries(Array.from({length: 1010}, (_, i) => [`s${i}`, `t${i}`]))});
await save('save-preserves-mode-0600', 'repo', full, async root => { await put({})(root); await fs.chmod(path.join(root, file), 0o644); });

// The legacy home (fixtures/state/home-v45): one repository's entry is replaced in place, the others (unknown keys
// and invalid values included) are written back as read (fixtures/state/MIGRATIONS.md row 35).
await recordDirCase('uiState', 'save-home-v45', {
	setup: async root => { await fs.copyFile(fileURLToPath(new URL('../../fixtures/state/home-v45/ui-state.json', import.meta.url)), path.join(root, file)); },
	fn: 'saveUiState', args: root => [root, '/tmp/deckhand-fixture/repos/lib', {...full, sessionQuery: 'lib'}], run: inHome(uiState.saveUiState),
});
