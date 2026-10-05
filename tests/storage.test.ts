import assert from 'node:assert/strict';
import {test, type TestContext} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {saveState, markAllNonExitedSessionsExited, updateAppConfig, loadAppConfig, ensureConfigDir} from '../src/storage.js';
import {loadUiState, saveUiState, normalizeUiState} from '../src/uiState.js';
import {readGlobalDefaultsDocument, saveGlobalDefaultsDocument} from '../src/projectConfigDocument.js';
import {resolveSettings} from '../src/projectConfig.js';
import type {SessionRecord} from '../src/types.js';
import {exec, tempDir, withEnv} from './helpers.js';
async function isolatedHome(t: TestContext): Promise<string> {
	const home = await tempDir(t, 'deckhand-storage-'); withEnv(t, {DECKHAND_HOME: home}); return home;
}
test('crash recovery marks live sessions interrupted and keeps their identity, archive state and notes', async t => {
	await isolatedHome(t);
	const session = {id: 'id', status: 'running', archivedAt: 'yesterday', notes: 'keep', agentSessionRef: {provider: 'codex', kind: 'id', value: 'native'}, setup: {command: 'setup', state: 'running', output: 'partial'}} as SessionRecord;
	await saveState({sessions: [session], worktrees: []}); const {sessions: [recovered]} = await markAllNonExitedSessionsExited();
	assert.equal(recovered?.exitReason, 'interrupted'); assert.equal(recovered?.setup?.state, 'failed');
	assert.equal(recovered?.agentSessionRef?.value, 'native'); assert.equal(recovered?.archivedAt, 'yesterday'); assert.equal(recovered?.notes, 'keep');
});
test('config is private, keeps unknown keys without writing absent defaults, and reads legacy single-fingerprint trust', async t => {
	const home = await isolatedHome(t), file = path.join(home, 'config.json');
	await fs.chmod(home, 0o755); await ensureConfigDir(); assert.equal((await fs.stat(home)).mode & 0o777, 0o700);
	await fs.writeFile(file, JSON.stringify({future_setting: {keep: true}, trustedProjects: {'/legacy': 'old'}}));
	assert.deepEqual((await loadAppConfig()).trustedProjects, {'/legacy': ['old']});
	await updateAppConfig({dev_command: 'dev'});
	assert.deepEqual(JSON.parse(await fs.readFile(file, 'utf8')), {future_setting: {keep: true}, trustedProjects: {'/legacy': 'old'}, dev_command: 'dev'});
	assert.equal((await fs.stat(file)).mode & 0o777, 0o600);
});
test('global defaults round-trip through config.json, keep other keys, and are validated with a revision check', async t => {
	const home = await isolatedHome(t), file = path.join(home, 'config.json');
	await fs.writeFile(file, JSON.stringify({future_setting: 1, trustedProjects: {'/r': ['x']}, dev_command: 'legacy'}));
	const starter = await readGlobalDefaultsDocument(); assert.equal(starter.exists, false); assert.equal(starter.revision, null);
	await assert.rejects(saveGlobalDefaultsDocument('{"setupCommand":""}', null), /Invalid command/);
	await assert.rejects(saveGlobalDefaultsDocument('{"unknown":1}', null), /Unknown defaults setting/);
	const saved = await saveGlobalDefaultsDocument('{"setupCommand":"npm ci","actions":{"test":"npm test"}}', null);
	assert.deepEqual(JSON.parse(await fs.readFile(file, 'utf8')), {future_setting: 1, trustedProjects: {'/r': ['x']}, dev_command: 'legacy', defaults: {setupCommand: 'npm ci', actions: {test: 'npm test'}}});
	assert.deepEqual(resolveSettings(undefined, await loadAppConfig()), {setupCommand: 'npm ci', devCommand: 'legacy', actions: {test: 'npm test'}});
	await updateAppConfig({notifications: true}); // Unrelated updates keep the defaults and their revision.
	assert.equal((await readGlobalDefaultsDocument()).revision, saved.revision);
	await assert.rejects(saveGlobalDefaultsDocument('{}', starter.revision), /changed on disk/);
	// Invalid defaults on disk surface where used and can still be opened for repair.
	await updateAppConfig(current => ({...current, defaults: {defaultAgent: 'nope'}}));
	const invalid = await loadAppConfig(); assert.throws(() => resolveSettings(undefined, invalid), /Invalid "defaults"/);
	const broken = await readGlobalDefaultsDocument(); assert.match(broken.raw, /nope/);
	await saveGlobalDefaultsDocument('{}', broken.revision);
	assert.deepEqual(resolveSettings(undefined, await loadAppConfig()), {devCommand: 'legacy'});
});
test('config updates serialize within and across processes and recover from a stale lock', async t => {
	const home = await isolatedHome(t), file = path.join(home, 'config.json');
	const trust = (root: string) => updateAppConfig(current => ({...current, trustedProjects: {...current.trustedProjects, [root]: ['hash']}}));
	await Promise.all([updateAppConfig({dev_command: 'dev'}), trust('/one'), trust('/two'), updateAppConfig({notifications: true})]);
	const config = await loadAppConfig();
	assert.equal(config.dev_command, 'dev'); assert.equal(config.notifications, true); assert.deepEqual(config.trustedProjects, {'/one': ['hash'], '/two': ['hash']});
	await fs.writeFile(`${file}.lock`, ''); const stale = new Date(Date.now() - 60_000); await fs.utimes(`${file}.lock`, stale, stale);
	const storage = JSON.stringify(new URL('../src/storage.ts', import.meta.url).href);
	const worker = (id: number) => `import {updateAppConfig} from ${storage};\nfor (let i = 0; i < 5; i++) await updateAppConfig(c => ({...c, trustedProjects: {...c.trustedProjects, ['/p${id}-' + i]: ['x']}}));`;
	await Promise.all([1, 2].map(id => exec(process.execPath, ['--import', 'tsx', '--input-type=module', '-e', worker(id)], {env: {...process.env, DECKHAND_HOME: home}})));
	assert.equal(Object.keys(JSON.parse(await fs.readFile(file, 'utf8')).trustedProjects).length, 12);
	await assert.rejects(fs.access(`${file}.lock`));
});
test('UI state is validated and stored per repository', async t => {
	await isolatedHome(t);
	const state = normalizeUiState({activeTab: 'notes', selectedId: 'id', sessionFilter: 'archived', sessionQuery: 'search', sessionTabs: {id: 'notes', bad: 'invalid'}, collapsedSessionIds: ['id', 123]});
	assert.deepEqual(state.collapsedSessionIds, ['id']); assert.deepEqual(state.sessionTabs, {id: 'notes'});
	assert.equal(normalizeUiState({activeTab: ['notes'], sessionTabs: {bad: ['notes']}}).activeTab, undefined);
	await Promise.all([saveUiState('/one', state), saveUiState('/two', normalizeUiState({activeTab: 'git'}))]);
	const one = await loadUiState('/one');
	assert.equal(one.selectedId, 'id'); assert.equal(one.sessionFilter, 'archived'); assert.equal(one.sessionQuery, 'search');
	assert.equal((await loadUiState('/two')).activeTab, 'git');
});
