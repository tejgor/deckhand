import assert from 'node:assert/strict';
import {test} from 'node:test';
import React from 'react';
import {renderToString} from 'ink';
import {explainSettings, parseProjectConfig, trustProjectConfig, type LoadedProject, type UserSettings} from '../src/projectConfig.js';
import type {ProjectConfigDocument} from '../src/projectConfigDocument.js';
import {classifyCandidate, type SettingsInfo, type WorktreeCandidate} from '../src/settingsInfo.js';
import {SettingsPane} from '../src/settingsPane.js';
import {MAX_ACTIONS, actionNameCheck, actionNameProblem, actionSavedMessage, applyChange, cellDetail, choiceChange, choiceOptions, columnProblem, commandProblem, currentChoice, infoLayer, inheritedHint, initialColumn, initialLinks, initialText, layerActions, linkSelection, savedMessage, settingsGrid, textChange} from '../src/settingsModel.js';

const vars = {name: '<name>', repo: 'mono', repoParent: '/dev', repoRoot: '/dev/mono', home: '/home/me'};
// A SettingsInfo as the daemon builds it: explainSettings rows plus both documents as written.
function settingsInfo(repoRaw: string | undefined, defaults: Record<string, unknown> | undefined, {trusted = false, legacyDev}: {trusted?: boolean; legacyDev?: string} = {}): SettingsInfo {
	const project = {root: '/dev/mono', trustRoot: '/dev/mono', path: '/dev/mono/deckhand.json', exists: repoRaw !== undefined, fingerprint: 'f', config: repoRaw === undefined ? {} : parseProjectConfig(repoRaw)} as LoadedProject;
	let user: UserSettings = {...defaults ? {defaults} : {}, ...legacyDev ? {dev_command: legacyDev} : {}};
	if (trusted) user = trustProjectConfig(project, user);
	const document = (kind: 'global' | 'repository', raw: string | undefined): ProjectConfigDocument => ({kind, root: '/r', path: kind === 'global' ? '/home/me/.deckhand/config.json' : '/dev/mono/deckhand.json', raw: raw ?? '{}', revision: raw === undefined ? null : 'x'.repeat(64), exists: raw !== undefined});
	return {
		cwd: '/dev/mono', repo: 'mono', repository: {state: repoRaw === undefined ? 'absent' : trusted ? 'trusted' : 'untrusted', path: '/dev/mono/deckhand.json'}, needsReview: !trusted && repoRaw !== undefined,
		rows: explainSettings(project, user, {vars, user: 'me', configDir: '/state'}).rows,
		targets: {global: document('global', defaults && JSON.stringify(defaults)), repository: document('repository', repoRaw)},
		vars, defaultLocation: '/state/worktrees/<name>', insideIgnored: false, user: 'me',
	};
}
const REPO = '{"devCommand":"npm run dev","actions":{"test":"npm test"},"worktree":{"location":"{repoParent}/worktrees/{name}","files":{"backend/.env":"{repoParent}/b.env"}}}';
const GLOBAL = {defaultAgent: 'pi', worktree: {symlink: ['shared'], branchFrom: 'origin'}};

test('settings grid: each cell is that layer\'s own value; the one in effect (explainSettings ≡ resolveSettings) is marked, an untrusted repository value needs trust, built-ins show in the global column', () => {
	const grid = (info: SettingsInfo) => Object.fromEntries(settingsGrid(info).map(row => [row.def.id, row]));
	const cell = (info: SettingsInfo, id: string, kind: 'global' | 'repository') => grid(info)[id]!.cells[kind];
	const info = settingsInfo(REPO, GLOBAL);
	assert.deepEqual(settingsGrid(info).map(row => `${row.def.section}/${row.def.label}`), ['General/Default agent', 'General/Default workspace', 'Commands/Dev command', 'Commands/Setup command', 'Commands/Actions', 'Worktrees/Location', 'Worktrees/Branch from', 'Worktrees/Branch name', 'Worktrees/Linked items', 'Worktrees/Creation hook']);
	// Untrusted: the repository's Dev command sits in its cell (needs trust) while the built-in dev applies.
	assert.deepEqual(cell(info, 'devCommand', 'repository'), {text: 'npm run dev', full: 'npm run dev', set: true, effective: false, needsTrust: true});
	assert.deepEqual(cell(info, 'devCommand', 'global'), {text: 'dev', full: 'dev', set: false, effective: true, builtIn: true});
	assert.deepEqual(cell(info, 'defaultAgent', 'global'), {text: 'pi', full: 'pi', set: true, effective: true});
	assert.deepEqual(cell(info, 'defaultAgent', 'repository'), {text: '—', set: false, effective: false});
	assert.deepEqual([cell(info, 'setupCommand', 'global').text, cell(info, 'setupCommand', 'global').effective], ['—', false]);
	assert.deepEqual(cell(info, 'worktree.location', 'repository'), {text: '/dev/worktrees/<name>', full: '/dev/worktrees/<name>', template: '{repoParent}/worktrees/{name}', set: true, effective: false, needsTrust: true});
	assert.deepEqual([cell(info, 'worktree.location', 'global').text, cell(info, 'worktree.location', 'global').builtIn], ['/state/worktrees/<name>', true]);
	assert.deepEqual([cell(info, 'worktree.branchFrom', 'global').text, cell(info, 'worktree.branchFrom', 'global').effective], ['origin (no remote found)', true]);
	// Lists: a compact summary per layer (action names, a count of linked items).
	assert.deepEqual([cell(info, 'actions', 'repository').text, cell(info, 'actions', 'repository').needsTrust, cell(info, 'actions', 'global').text], ['test', true, '—']);
	assert.deepEqual([cell(info, 'worktree.links', 'global').text, cell(info, 'worktree.links', 'global').effective, cell(info, 'worktree.links', 'repository').text, cell(info, 'worktree.links', 'repository').full], ['1 item', true, '1 item', 'backend/.env']);
	// The details line names the layer and how it relates to the other one, or what the built-in means.
	assert.deepEqual(cellDetail(info, grid(info).devCommand!, 'repository'), {head: 'This repo', layer: 'repository', relation: "Applies once trusted — you'll be asked the first time it runs (or press T)", warn: true});
	assert.deepEqual(cellDetail(info, grid(info).devCommand!, 'global'), {head: 'Built-in default', relation: 'No Dev command is set, so d runs a shell command named `dev`.'});
	assert.equal(cellDetail(info, grid(info).defaultAgent!, 'global').relation, 'inherited by this repo');
	assert.equal(cellDetail(info, grid(info).defaultAgent!, 'repository').relation, 'not set: inherits global');
	assert.equal(cellDetail(info, grid(info).devCommand!, 'global').head, 'Built-in default');

	// Trusted: the repository values apply; a global value they replace is overridden (dimmed), lists merge.
	const trusted = settingsInfo(REPO, {...GLOBAL, devCommand: 'printf global'}, {trusted: true});
	assert.deepEqual([cell(trusted, 'devCommand', 'repository').effective, cell(trusted, 'devCommand', 'global').effective, cell(trusted, 'devCommand', 'global').text], [true, false, 'printf global']);
	assert.equal(cellDetail(trusted, grid(trusted).devCommand!, 'repository').relation, 'overrides global');
	assert.equal(cellDetail(trusted, grid(trusted).devCommand!, 'global').relation, 'overridden by this repo');
	assert.deepEqual([cell(trusted, 'worktree.location', 'global').text, cell(trusted, 'worktree.location', 'repository').effective], ['—', true]);
	assert.deepEqual([cell(trusted, 'worktree.links', 'global').effective, cell(trusted, 'worktree.links', 'repository').effective], [true, true]);
	// Suggestions (and hook: false) apply untrusted; the legacy dev_command shows in the global column.
	const suggestion = settingsInfo('{"defaultAgent":"codex","worktree":{"hook":false}}', GLOBAL);
	assert.deepEqual([cell(suggestion, 'defaultAgent', 'repository').effective, cell(suggestion, 'defaultAgent', 'repository').needsTrust, cell(suggestion, 'defaultAgent', 'global').effective], [true, undefined, false]);
	assert.equal(cellDetail(suggestion, grid(suggestion).defaultAgent!, 'repository').relation, 'overrides global, untrusted too: it only preselects n');
	assert.deepEqual([cell(suggestion, 'worktree.hook', 'repository').text, cell(suggestion, 'worktree.hook', 'repository').effective], ['off', true]);
	const legacy = settingsInfo('{}', undefined, {legacyDev: 'make dev'});
	assert.deepEqual(cell(legacy, 'devCommand', 'global'), {text: 'make dev', full: 'make dev', set: false, effective: true, legacy: true});

	// Clearing a repository value: the global one takes over.
	const cleared = settingsInfo(applyChange(trusted.targets.repository!, {path: ['devCommand']}), {...GLOBAL, devCommand: 'printf global'}, {trusted: true});
	assert.deepEqual([cell(cleared, 'devCommand', 'repository').text, cell(cleared, 'devCommand', 'global').effective], ['—', true]);
});

test('the grid opens on This repo when there is a repository layer, else Global; each layer\'s Actions list shows the other layer\'s as context', () => {
	const info = settingsInfo(REPO, GLOBAL);
	assert.equal(initialColumn(info), 'repository');
	assert.equal(columnProblem(info, 'global'), undefined);
	// Outside a repository (or in a bare one) only global defaults exist.
	const bare = {...info, targets: {global: info.targets.global, repositoryError: 'Bare repositories have no main checkout'}};
	assert.equal(initialColumn(bare), 'global');
	assert.match(columnProblem(bare, 'repository') ?? '', /Bare repositories/);
	assert.match(cellDetail(bare, settingsGrid(bare)[2]!, 'repository').relation, /No repository layer here/);
	// A repository file that does not exist yet is created with just the edited key.
	const absent = settingsInfo(undefined, undefined);
	assert.equal(initialColumn(absent), 'repository');
	assert.equal(applyChange(absent.targets.repository!, {path: ['defaultAgent'], value: 'codex'}), '{\n  "defaultAgent": "codex"\n}\n');
	const actions = settingsInfo('{"actions":{"test":"npm test","lint":"npm run lint"}}', {actions: {test: 'npm t', fmt: 'npm run fmt'}});
	assert.deepEqual(layerActions(actions, 'repository'), {
		own: [{name: 'test', command: 'npm test', status: '⚠ needs trust', needsTrust: true}, {name: 'lint', command: 'npm run lint', status: '⚠ needs trust', needsTrust: true}],
		context: [{name: 'fmt', command: 'npm run fmt', status: 'global'}],
	});
	assert.deepEqual(layerActions(actions, 'global'), {
		own: [{name: 'test', command: 'npm t', status: 'this repo overrides it'}, {name: 'fmt', command: 'npm run fmt'}],
		context: [{name: 'lint', command: 'npm run lint', status: 'this repo · ⚠ needs trust'}],
	});
});

test('edits become one key in one layer, preserving every other key and its place; invalid values are rejected', () => {
	const info = settingsInfo(REPO, GLOBAL);
	const repository = info.targets.repository!, global = info.targets.global!;
	const json = (raw: string) => JSON.parse(raw);
	// Choice.
	const agent = choiceOptions(info, 'defaultAgent', 'repository');
	assert.deepEqual(agent.map(option => option.label), ['claude', 'pi', 'codex']);
	assert.deepEqual(currentChoice(info, 'defaultAgent', 'repository', agent), {index: 1, current: -1}); // In effect: global pi; the repo stores none.
	assert.deepEqual(currentChoice(info, 'defaultAgent', 'global', agent), {index: 1, current: 1});
	const codex = choiceChange(info, 'defaultAgent', 'repository', agent[2]!)!;
	assert.deepEqual(json(applyChange(repository, codex)), {...json(REPO), defaultAgent: 'codex'});
	assert.deepEqual(Object.keys(json(applyChange(repository, codex))), ['devCommand', 'actions', 'worktree', 'defaultAgent']);
	assert.equal(choiceChange(info, 'defaultAgent', 'global', agent[1]!), undefined); // Global already has pi.
	assert.deepEqual(choiceOptions(info, 'worktree.branchFrom', 'global').map(option => option.value), ['current', 'default', 'origin']);
	// Hook on/off.
	const hook = choiceOptions(info, 'worktree.hook', 'global');
	assert.deepEqual(hook.map(option => [option.label, option.detail]), [['on', 'no create-worktree.sh found'], ['off', 'Deckhand creates worktrees itself']]);
	assert.deepEqual(json(applyChange(global, choiceChange(info, 'worktree.hook', 'global', hook[1]!)!)), {defaultAgent: 'pi', worktree: {symlink: ['shared'], branchFrom: 'origin', hook: false}});
	// Location presets: the repository's own template is preselected; the first option removes it, custom… types one.
	const location = choiceOptions(info, 'worktree.location', 'repository');
	assert.deepEqual(location.map(option => [option.label, option.value]), [['Deckhand default', undefined], ['next to repo', '{repoParent}/worktrees/{name}'], ['inside repo', '{repoRoot}/.worktrees/{name}'], ['custom…', undefined]]);
	assert.deepEqual([location[0]!.detail, location[2]!.detail, location[2]!.warning], ['/state/worktrees/<name>', '/dev/mono/.worktrees/<name>', '⚠ .worktrees/ is not gitignored — worktrees would show up as untracked files']);
	assert.deepEqual(currentChoice(info, 'worktree.location', 'repository', location), {index: 1, current: 1});
	assert.deepEqual(currentChoice(info, 'worktree.location', 'global', choiceOptions(info, 'worktree.location', 'global')), {index: 0, current: 0});
	assert.deepEqual(json(applyChange(repository, choiceChange(info, 'worktree.location', 'repository', location[0]!)!)).worktree, {files: {'backend/.env': '{repoParent}/b.env'}});
	assert.equal(initialText(info, 'worktree.location', 'repository'), '{repoParent}/worktrees/{name}');
	assert.match((textChange('worktree.location', '~/wt/{repo}') as {error: string}).error, /must contain \{name\}/);
	assert.deepEqual(textChange('worktree.location', '~/wt/{repo}/{name}'), {change: {path: ['worktree', 'location'], value: '~/wt/{repo}/{name}'}});
	// Text with validation errors kept for the open input.
	// A text input starts from the layer's own value, empty when it stores none (the inherited value is a hint).
	assert.equal(initialText(info, 'worktree.branchName', 'global'), '');
	assert.equal(inheritedHint(info, 'worktree.branchName', 'global'), 'built-in: {name}');
	assert.equal(initialText(info, 'devCommand', 'repository'), 'npm run dev');
	assert.equal(initialText(info, 'devCommand', 'global'), '');
	assert.equal(inheritedHint(info, 'devCommand', 'global'), 'built-in: dev');
	assert.equal(inheritedHint(settingsInfo(REPO, {devCommand: 'make dev'}), 'devCommand', 'repository'), 'global: make dev');
	for (const bad of ['feat/x', 'bad name/{name}', '{name}..x']) assert.ok('error' in textChange('worktree.branchName', bad), bad);
	assert.match((textChange('devCommand', '  ') as {error: string}).error, /x clears/);
	assert.equal((textChange('actions', '', 'lint') as {error: string}).error, 'Type a command for the action');
	const dev = textChange('devCommand', 'printf global') as {change: {path: string[]; value: string}};
	assert.deepEqual(json(applyChange(global, dev.change)), {...GLOBAL, devCommand: 'printf global'});
	// Actions: add (name, then command), edit, remove; an emptied actions object is dropped.
	for (const [name, problem] of [['', 'type a name first'], [' lint', "can't start with a space"], ['-lint', 'must start with a letter or number'], ['bad/name', 'can\'t contain "/"'], ['lint ', "can't end with a space"], ['a'.repeat(49), 'too long: 49 characters (up to 48)'], ['constructor', 'constructor is a reserved name']]) assert.equal(actionNameProblem(name), problem, name);
	for (const name of ['test', 'lint frontend', 'db.migrate', 'a'.repeat(48)]) assert.equal(actionNameProblem(name), undefined, name);
	const added = applyChange(repository, (textChange('actions', 'npm run lint', 'lint') as {change: {path: string[]; value: string}}).change);
	assert.deepEqual(json(added).actions, {test: 'npm test', lint: 'npm run lint'});
	assert.deepEqual(json(applyChange(repository, {path: ['actions', 'test'], value: 'pytest'})).actions, {test: 'pytest'});
	assert.deepEqual(Object.keys(json(applyChange(repository, {path: ['actions', 'test']}))), ['devCommand', 'worktree']);
	// Saving re-validates the whole layer and refuses to edit through malformed JSON.
	assert.throws(() => applyChange({...repository, raw: '{'}, codex), /press e to repair/);
	assert.throws(() => applyChange({...repository, raw: '{"worktree":[]}'}, {path: ['worktree', 'hook'], value: false}), /not an object/);
});

test('adding an action: the name is checked live (rules, existing names in either layer, the per-layer limit); commands must be non-empty', () => {
	const info = settingsInfo('{"actions":{"test":"npm test"}}', {actions: {lint: 'npm run lint'}});
	assert.deepEqual(actionNameCheck(info, 'repository', ''), {});
	assert.deepEqual(actionNameCheck(info, 'repository', ' x'), {error: "can't start with a space"});
	assert.deepEqual(actionNameCheck(info, 'repository', 'test'), {note: 'replaces the existing test action (this repo)'});
	assert.deepEqual(actionNameCheck(info, 'repository', 'lint'), {note: 'overrides the global lint action in this repo'});
	assert.deepEqual(actionNameCheck(info, 'global', 'test'), {note: "adds a global test action; this repo's test action still wins here"});
	assert.deepEqual(actionNameCheck(info, 'global', 'fmt'), {note: 'adds a new action to global defaults'});
	const full = settingsInfo(JSON.stringify({actions: Object.fromEntries(Array.from({length: MAX_ACTIONS}, (_, index) => [`a${index}`, 'true']))}), undefined);
	assert.match(actionNameCheck(full, 'repository', '').error ?? '', /^this repo already has 30 actions, the most allowed: remove one \(x\) first$/);
	assert.match(actionNameCheck(full, 'repository', 'new').error ?? '', /30 actions/);
	assert.deepEqual(actionNameCheck(full, 'repository', 'a1'), {note: 'replaces the existing a1 action (this repo)'});
	assert.deepEqual(actionNameCheck(full, 'global', 'new'), {note: 'adds a new action to global defaults'});
	assert.equal(commandProblem('  '), 'type a command');
	assert.equal(commandProblem('x'.repeat(8193)), 'too long: 8193 characters (up to 8192)');
	assert.equal(commandProblem('cd backend && .venv/bin/pytest -x'), undefined);
	assert.equal(actionSavedMessage('lint frontend', 'repository', 'kept'), 'Saved action lint frontend to this repo · still trusted · run it with e on a session');
	assert.equal(actionSavedMessage('lint', 'global'), 'Saved action lint to global defaults · run it with e on a session');
});

test('the status after a save says whether the repository file is still trusted', () => {
	assert.equal(savedMessage('Location', 'repository', {trust: 'kept'}), 'Saved Location to this repo · still trusted');
	assert.equal(savedMessage('Dev command', 'repository', {trust: 'created'}), 'Saved Dev command to this repo · created and trusted');
	assert.equal(savedMessage('Dev command', 'repository', {trust: 'unreviewed'}), "Saved Dev command to this repo · review required (the file had changes you haven't reviewed)");
	assert.equal(savedMessage('Location', 'repository', {trust: 'hook'}), "Saved Location to this repo · review required (the creation hook hasn't been reviewed)");
	assert.equal(savedMessage('Default agent', 'repository', {cleared: true, trust: 'kept'}), 'Cleared Default agent in this repo · still trusted');
	assert.equal(savedMessage('Dev command', 'global'), 'Saved Dev command to global defaults');
});

test('link picker: candidates classified and preselected; the selection rewrites only the target layer\'s symlink list', () => {
	const link = ['node_modules', 'frontend/node_modules', 'backend/.venv', 'venv', 'vendor', 'web/.vercel', '.env', 'backend/.env', '.env.local', 'config/.env.production', 'backend.env'];
	const skip = ['dist', 'web/build', 'out', 'coverage', '.next', '.turbo', '.cache', 'pkg/__pycache__', '.pytest_cache', '.DS_Store', 'debug.log', 'notes.txt', 'environment', 'tmp'];
	for (const entry of link) assert.equal(classifyCandidate(entry).suggestion, 'link', entry);
	for (const entry of skip) assert.equal(classifyCandidate(entry).suggestion, 'skip', entry);
	assert.deepEqual(classifyCandidate('dist'), {suggestion: 'skip', reason: 'build output'});

	const candidate = (entry: string, extra: Partial<WorktreeCandidate> = {}): WorktreeCandidate => ({path: entry, kind: 'dir', ignored: true, ...classifyCandidate(entry), ...extra});
	const candidates = [candidate('shared', {configured: 'symlink', layers: ['global']}), candidate('keep', {configured: 'symlink', layers: ['repository']}), candidate('backend/.env', {kind: 'file', configured: 'files', source: '{repoParent}/b.env', layers: ['repository']}), candidate('node_modules'), candidate('dist')];
	const links = initialLinks(candidates);
	assert.deepEqual(links, {shared: true, keep: true, 'backend/.env': true, node_modules: true, dist: false});
	const info = settingsInfo('{"devCommand":"npm run dev","worktree":{"symlink":["unknown","keep"],"files":{"backend/.env":"{repoParent}/b.env"}}}', GLOBAL);
	const repository = infoLayer(info, 'repository'), global = infoLayer(info, 'global');
	// Repository target: its entries stay in order (unknown ones too), new links are appended unless global already links them.
	const selection = linkSelection(repository, global, candidates, {...links, shared: false, dist: true});
	assert.deepEqual(selection, {symlink: ['unknown', 'keep', 'node_modules', 'dist'], added: ['node_modules', 'dist'], removed: [], stillLinked: ['shared']});
	const raw = applyChange(info.targets.repository!, {path: ['worktree', 'symlink'], value: selection.symlink});
	assert.deepEqual(JSON.parse(raw), {devCommand: 'npm run dev', worktree: {symlink: ['unknown', 'keep', 'node_modules', 'dist'], files: {'backend/.env': '{repoParent}/b.env'}}});
	// Global target: unlinking its entry removes it; files entries are never written to symlink.
	assert.deepEqual(linkSelection(global, repository, candidates, {...links, shared: false, keep: false}), {symlink: ['node_modules'], added: ['node_modules'], removed: ['shared'], stillLinked: ['keep']});
});

test('Settings pane: two value columns from 64 columns (one, with ◂ ▸, below); an option\'s warning is on its own line under its full path; edits are titled "<Setting> · <Layer>"', () => {
	const info = settingsInfo(REPO, GLOBAL);
	type Props = React.ComponentProps<typeof SettingsPane>;
	const pane = (props: Partial<Props> = {}, width = 100) => renderToString(React.createElement(SettingsPane, {info, view: 'main', row: 2, actionRow: 0, column: 'repository', sizes: {}, width, height: 30, ...props}), {columns: width}).split('\n');
	const wide = pane();
	assert.ok(wide.some(line => /Global \(all repos\) +This repo/.test(line)));
	assert.match(wide.find(line => line.includes('Dev command'))!, /❯ Dev command +● dev \(built-in\) +npm run dev ⚠ needs trust/);
	assert.ok(wide.some(line => line.includes("This repo · Applies once trusted — you'll be asked the first time it runs (or press T)")));
	assert.ok(!wide.join('\n').match(/Saves to|until trusted|pending/));
	// Narrow: only the selected column, with the indicator; ←→ (the flow) switches it.
	const narrow = pane({}, 55);
	assert.ok(narrow.some(line => line.includes('◂ Global | This repo ▸')));
	assert.match(narrow.find(line => line.includes('Dev command'))!, /Dev command +npm run dev ⚠/);
	assert.doesNotMatch(narrow.join('\n'), /dev \(built-in\)/);
	assert.match(pane({column: 'global'}, 55).find(line => line.includes('Dev command'))!, /Dev command +● dev \(built-in\)/);
	assert.ok(pane({column: 'global'}, 55).some(line => line.includes('Built-in · No Dev command is set, so d runs')));
	const options = choiceOptions(info, 'worktree.location', 'repository');
	for (const index of [0, 2]) {
		const lines = pane({row: 5, edit: {kind: 'choice', id: 'worktree.location', label: 'Location', target: 'repository', options, index, current: 1}});
		assert.ok(lines.some(line => line.includes('Location · This repo')));
		const inside = lines.findIndex(line => line.includes('inside repo'));
		assert.match(lines[inside]!, /inside repo +\/dev\/mono\/\.worktrees\/<name> /);
		assert.doesNotMatch(lines[inside]!, /⚠/);
		assert.match(lines[inside + 1]!, /^\W+⚠ \.worktrees\/ is not gitignored — worktrees would show up as untracked files/);
	}
	// An empty text input shows what applies without a value here.
	assert.ok(pane({column: 'global', edit: {kind: 'text', id: 'devCommand', label: 'Dev command', target: 'global', state: {text: '', cursor: 0}, placeholder: 'built-in: dev'}}).some(line => /Dev command · Global/.test(line)) );
	assert.ok(pane({column: 'global', edit: {kind: 'text', id: 'devCommand', label: 'Dev command', target: 'global', state: {text: '', cursor: 0}, placeholder: 'built-in: dev'}}).some(line => /› +built-in: dev/.test(line)));
	// Adding an action: numbered steps, the rules and examples, a live error; the Actions list is one layer's.
	const name = pane({view: 'actions', actionRow: 1, edit: {kind: 'text', id: 'actions', label: 'New action', target: 'repository', step: 'name', state: {text: ' bad', cursor: 4}}, editHelp: [{text: "can't start with a space", color: 'red'}, {text: 'letters, numbers, spaces, _ . - · starts with a letter or number · up to 48 characters'}, {text: 'e.g. test · lint frontend · db.migrate'}]}, 55).join('\n');
	for (const text of ['Settings › Actions · This repo', 'New action · step 1 of 2: name', "can't start with a space", 'letters, numbers, spaces', 'e.g. test · lint frontend · db.migrate']) assert.ok(name.includes(text), text);
	const command = pane({view: 'actions', actionRow: 1, edit: {kind: 'text', id: 'actions', label: 'Action lint', target: 'repository', step: 'command', entry: 'lint', state: {text: '', cursor: 0}}}, 55).join('\n');
	assert.ok(command.includes('New action · step 2 of 2: command for lint'));
});
