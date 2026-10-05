import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {explainSettings, loadProjectConfig, parseProjectConfig, isProjectTrusted, projectNeedsReview, trustProjectConfig, resolveDevCommand, resolveSettings, resolveSetupCommand, type LoadedProject, type ProjectConfig, type SettingRow, type UserSettings} from '../src/projectConfig.js';
import {readProjectConfigDocument} from '../src/projectConfigDocument.js';
import {git, repo} from './helpers.js';
test('strict project schema rejects malformed/unknown commands and defaults', () => {
	for (const value of ['[]', '{"unknown":true}', '{"defaultAgent":["pi"]}', '{"defaultWorkspace":null}', '{"setupCommand":""}', '{"actions":{"bad/name":"echo ok"}}', '{"actions":{"__proto__":"echo bad"}}']) assert.throws(() => parseProjectConfig(value));
	assert.deepEqual(parseProjectConfig('{"defaultAgent":"codex","actions":{"test":"npm test"}}'), {defaultAgent: 'codex', actions: {test: 'npm test'}});
});
test('effective settings: global defaults overlaid by the repository override only while trusted (its agent/workspace suggestions always); actions merge by name', () => {
	const project = {root: '/r', trustRoot: '/r', path: '/r/deckhand.json', exists: true, fingerprint: 'f', config: {defaultAgent: 'pi', devCommand: 'repo dev', setupCommand: 'repo setup', actions: {test: 'repo test', lint: 'repo lint'}}} as LoadedProject;
	const user = {dev_command: 'legacy', defaults: {defaultAgent: 'codex', defaultWorkspace: 'new', setupCommand: 'npm ci', actions: {test: 'global test', fmt: 'global fmt'}}};
	// Untrusted: only the repository's defaultAgent applies (it preselects the picker and runs nothing).
	assert.deepEqual(resolveSettings(project, user), {defaultAgent: 'pi', defaultWorkspace: 'new', setupCommand: 'npm ci', devCommand: 'legacy', actions: {test: 'global test', fmt: 'global fmt'}});
	const trusted = trustProjectConfig(project, user);
	assert.deepEqual(resolveSettings(project, trusted), {defaultAgent: 'pi', defaultWorkspace: 'new', setupCommand: 'repo setup', devCommand: 'repo dev', actions: {test: 'repo test', fmt: 'global fmt', lint: 'repo lint'}});
	assert.equal(resolveDevCommand(undefined, {}), 'dev');
	assert.equal(resolveDevCommand(project, {dev_command: 'legacy', defaults: {devCommand: 'npm run dev'}}), 'npm run dev');
	assert.throws(() => resolveSettings(project, {defaults: {setupCommand: ''}}), /Invalid "defaults".*Invalid command/);
	// An untrusted repository setupCommand is refused unless the user reviewed exactly these bytes and skipped them.
	assert.throws(() => resolveSetupCommand(project, user), /not trusted/);
	assert.equal(resolveSetupCommand(project, user, 'f'), 'npm ci');
	assert.equal(resolveSetupCommand(project, trusted), 'repo setup');
});
// Rebuilds a ProjectConfig from the rows that are in effect (not built-in, unset or pending).
function fromRows(rows: SettingRow[]): ProjectConfig {
	const config: Record<string, unknown> = {}, worktree: Record<string, unknown> = {};
	for (const row of rows) {
		if (row.source === 'built-in default' || row.source === 'not set') continue;
		const [scope, field] = row.key.startsWith('worktree.') ? [worktree, row.key.slice(9)] : [config, row.key];
		if (field === 'symlink') scope.symlink = [...scope.symlink as string[] ?? [], row.raw];
		else if (row.entry !== undefined) scope[field] = {...scope[field] as object, [row.entry]: row.raw};
		else scope[field] = row.raw;
	}
	return Object.keys(worktree).length ? {...config, worktree} as ProjectConfig : config as ProjectConfig;
}
const find = (rows: SettingRow[], key: string, entry?: string) => rows.find(row => row.key === key && row.entry === entry)!;
test('explainSettings: sources per row, pending untrusted values, list merges, legacy and built-in defaults; values equal resolveSettings', () => {
	const vars = {name: '<name>', repo: 'mono', repoParent: '/dev', repoRoot: '/dev/mono', home: '/home/me'};
	const context = {vars, user: 'tejas', configDir: '/state'};
	const repoConfig: ProjectConfig = {defaultWorkspace: 'new', devCommand: 'cd frontend && npm run dev', actions: {test: 'pytest', lint: 'repo lint'}, worktree: {location: '{repoParent}/worktrees/{name}', hook: false, symlink: ['frontend/node_modules', 'node_modules'], files: {'backend/local.cfg': '{repoParent}/backend.cfg'}}};
	const project = {root: '/dev/mono', trustRoot: '/dev/mono', path: '/dev/mono/deckhand.json', exists: true, fingerprint: 'f', config: repoConfig} as LoadedProject;
	const user: UserSettings = {dev_command: 'legacy dev', defaults: {defaultAgent: 'codex', actions: {lint: 'npm run lint'}, worktree: {branchName: '{user}/{name}', symlink: ['node_modules'], files: {'backend/local.cfg': '/g/cfg'}}}};
	const trusted = trustProjectConfig(project, user);
	const cases: Array<[LoadedProject | undefined, UserSettings]> = [[undefined, {}], [undefined, user], [project, user], [project, trusted], [{...project, config: {}}, {defaults: {devCommand: 'npm start'}}]];
	for (const [p, u] of cases) assert.deepEqual(fromRows(explainSettings(p, u, context).rows), resolveSettings(p, u));

	// Built-in defaults only.
	const empty = explainSettings(undefined, {}, context).rows;
	assert.deepEqual([find(empty, 'defaultAgent').value, find(empty, 'defaultAgent').source], ['claude', 'built-in default']);
	assert.deepEqual([find(empty, 'devCommand').value, find(empty, 'setupCommand').source, find(empty, 'actions').source], ['dev', 'not set', 'not set']);
	assert.equal(find(empty, 'worktree.location').value, '/state/worktrees/<name>');
	assert.deepEqual([find(empty, 'worktree.hook').value, find(empty, 'worktree.hook').note], ['on', 'no .claude/scripts/create-worktree.sh detected']);

	// Global only (repository untrusted): legacy dev_command, repo values pending; the repository's hook: false applies anyway.
	const pending = explainSettings(project, user, context).rows;
	assert.deepEqual([find(pending, 'devCommand').source, find(pending, 'devCommand').value, find(pending, 'devCommand').pending?.value], ['legacy dev_command', 'legacy dev', 'cd frontend && npm run dev']);
	// The repository's suggestions apply without trust, so they are not pending.
	assert.deepEqual([find(pending, 'defaultWorkspace').source, find(pending, 'defaultWorkspace').value, find(pending, 'defaultWorkspace').pending], ['repo', 'new', undefined]);
	assert.match(find(pending, 'defaultWorkspace').note ?? '', /applies without trust/);
	assert.deepEqual([find(pending, 'actions', 'lint').source, find(pending, 'actions', 'lint').pending?.value], ['global', 'repo lint']);
	assert.deepEqual([find(pending, 'actions', 'test').source, find(pending, 'actions', 'test').pending?.value], ['not set', 'pytest']);
	assert.deepEqual([find(pending, 'worktree.symlink', 'frontend/node_modules').source, find(pending, 'worktree.symlink', 'node_modules').source], ['not set', 'global']);
	assert.deepEqual([find(pending, 'worktree.hook').value, find(pending, 'worktree.hook').source, find(pending, 'worktree.hook').pending], ['off', 'repo', undefined]);
	assert.deepEqual([find(pending, 'worktree.branchName').value, find(pending, 'worktree.branchName').raw], ['tejas/<name>', '{user}/{name}']);
	assert.equal(find(pending, 'worktree.location').pending?.value, '/dev/worktrees/<name>');
	assert.ok(pending.every(row => !row.pending?.same));
	// An untrusted value equal to the one in effect (here the built-in dev and a global action) is marked `same`.
	const same = explainSettings({...project, config: {devCommand: 'dev', actions: {lint: 'npm run lint'}}}, user, context).rows;
	assert.deepEqual([find(same, 'devCommand').source, find(same, 'devCommand').pending], ['legacy dev_command', {value: 'dev', raw: 'dev'}]);
	assert.deepEqual(find(explainSettings({...project, config: {devCommand: 'dev'}}, {}, context).rows, 'devCommand').pending, {value: 'dev', raw: 'dev', same: true});
	assert.deepEqual(find(same, 'actions', 'lint').pending, {value: 'npm run lint', raw: 'npm run lint', same: true});

	// Trusted: repository wins field by field and per entry, global entries it does not override stay.
	const merged = explainSettings(project, trusted, context).rows;
	assert.deepEqual([find(merged, 'devCommand').source, find(merged, 'defaultAgent').source, find(merged, 'worktree.branchName').source, find(merged, 'worktree.branchFrom').source], ['repo', 'global', 'global', 'built-in default']);
	assert.deepEqual([find(merged, 'actions', 'lint').source, find(merged, 'actions', 'lint').note, find(merged, 'actions', 'test').source], ['repo', 'overrides global npm run lint', 'repo']);
	assert.deepEqual([find(merged, 'worktree.files', 'backend/local.cfg').value, find(merged, 'worktree.files', 'backend/local.cfg').raw, find(merged, 'worktree.location').value], ['/dev/backend.cfg', '{repoParent}/backend.cfg', '/dev/worktrees/<name>']);
	assert.ok(merged.every(row => !row.pending));
	// A non-legacy global devCommand, and invalid global defaults reported while the rest still resolves.
	assert.equal(find(explainSettings(undefined, {dev_command: 'x', defaults: {devCommand: 'npm start'}}).rows, 'devCommand').source, 'global');
	const broken = explainSettings(project, {...trusted, defaults: {setupCommand: ''}}, context);
	assert.match(broken.globalError ?? '', /Invalid "defaults"/);
	assert.deepEqual([find(broken.rows, 'devCommand').source, find(broken.rows, 'defaultWorkspace').value], ['repo', 'new']);
});
test('the repository override is the main checkout\'s live deckhand.json; worktree copies are ignored', async t => {
	const cwd = await repo(), other = await repo(); t.after(async () => { await fs.rm(cwd, {recursive: true, force: true}); await fs.rm(other, {recursive: true, force: true}); });
	assert.equal(projectNeedsReview(await loadProjectConfig(other), {}), false); // Nothing to trust.
	const raw = JSON.stringify({devCommand: 'touch DO_NOT_EXECUTE', defaultWorkspace: 'new'});
	await fs.writeFile(path.join(cwd, 'deckhand.json'), raw); await fs.writeFile(path.join(other, 'deckhand.json'), raw);
	await git(cwd, 'add', 'deckhand.json'); await git(cwd, 'commit', '-m', 'project config');
	const worktree = path.join(cwd, 'linked'); await git(cwd, 'worktree', 'add', '-b', 'linked', worktree);
	const project = await loadProjectConfig(worktree);
	assert.equal(project.path, path.join(await fs.realpath(cwd), 'deckhand.json')); assert.equal(projectNeedsReview(project, {}), true);
	assert.equal(resolveDevCommand(project, {dev_command: 'global'}), 'global');
	const config = trustProjectConfig(project, {dev_command: 'global'});
	assert.equal(resolveDevCommand(project, config), 'touch DO_NOT_EXECUTE');
	assert.equal(isProjectTrusted(await loadProjectConfig(cwd), config), true); // Same repository from any checkout.
	assert.equal(isProjectTrusted(await loadProjectConfig(other), config), false);
	await fs.writeFile(path.join(worktree, 'deckhand.json'), '{"devCommand":"printf worktree-copy"}');
	assert.equal((await loadProjectConfig(worktree)).fingerprint, project.fingerprint);
	// An uncommitted edit in the main checkout applies to worktree sessions at once, and needs a fresh review.
	await fs.writeFile(path.join(cwd, 'deckhand.json'), '{"devCommand":"printf live"}');
	const live = await loadProjectConfig(worktree);
	assert.equal(live.config.devCommand, 'printf live'); assert.equal(projectNeedsReview(live, config), true);
	assert.equal(resolveDevCommand(live, trustProjectConfig(live, config)), 'printf live');
	await assert.rejects(fs.access(path.join(cwd, 'DO_NOT_EXECUTE')));
});
test('trust keeps several fingerprints per repository, newest first and capped', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const first = await loadProjectConfig(cwd); let config = trustProjectConfig(first, {});
	await fs.writeFile(path.join(cwd, 'deckhand.json'), '{}'); const second = await loadProjectConfig(cwd);
	config = trustProjectConfig(second, trustProjectConfig(first, config));
	assert.equal(isProjectTrusted(first, config) && isProjectTrusted(second, config), true);
	assert.deepEqual(config.trustedProjects?.[first.trustRoot], [second.fingerprint, first.fingerprint]);
	for (let i = 0; i < 30; i++) config = trustProjectConfig({...first, fingerprint: String(i)}, config);
	assert.equal(config.trustedProjects?.[first.trustRoot]?.length, 20); assert.equal(isProjectTrusted(first, config), false);
});
test('trust roots are unique for submodules and bare repositories; bare repositories have no repository override', async t => {
	const outer = await repo(), inner = await repo(); t.after(async () => { await fs.rm(outer, {recursive: true, force: true}); await fs.rm(inner, {recursive: true, force: true}); });
	await git(outer, '-c', 'protocol.file.allow=always', 'submodule', 'add', inner, 'sub');
	const submodule = await loadProjectConfig(path.join(outer, 'sub'));
	assert.equal(submodule.trustRoot, await fs.realpath(path.join(outer, 'sub'))); assert.notEqual(submodule.trustRoot, (await loadProjectConfig(outer)).trustRoot);
	const bare = path.join(outer, 'bare.git'), linked = path.join(outer, 'bare-linked');
	await git(outer, 'clone', '--bare', inner, bare); await git(bare, 'worktree', 'add', '-b', 'linked', linked);
	const project = await loadProjectConfig(linked);
	assert.equal(project.trustRoot, await fs.realpath(bare)); assert.equal(project.root, await fs.realpath(linked));
	assert.equal(project.path, undefined); assert.equal(project.exists, false);
	await assert.rejects(readProjectConfigDocument(linked), /Bare repositories/);
});
test('malformed, oversized and symlinked config errors are not hidden', async t => {
	const cwd = await repo(); t.after(() => fs.rm(cwd, {recursive: true, force: true}));
	const file = path.join(cwd, 'deckhand.json');
	await fs.writeFile(file, '{'); await assert.rejects(loadProjectConfig(cwd), /deckhand.json/);
	await fs.writeFile(file, ' '.repeat(70000)); await assert.rejects(loadProjectConfig(cwd), /64 KiB/);
	await fs.rm(file); await fs.writeFile(path.join(cwd, 'target.json'), '{}'); await fs.symlink(path.join(cwd, 'target.json'), file);
	await assert.rejects(loadProjectConfig(cwd), /must not be a symlink/);
});
