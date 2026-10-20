/**
 * Gen script for src/settingsModel.ts. Regenerate with `node --import tsx parity/gen/settingsModel.gen.ts`.
 * Taps tests/settingsModel.test.ts (and the callers it drives), then records every pure function over a corpus of
 * SettingsInfo values built the way the daemon builds them (explainSettings rows + both documents).
 */
import {Recorder, arb, fc, installDeterminism, runTap, sample} from '../lib.ts';
import {explainSettings, parseProjectConfig, trustProjectConfig, type LoadedProject, type UserSettings} from '../../src/projectConfig.ts';
import type {ProjectConfigDocument} from '../../src/projectConfigDocument.ts';
import type {SettingsInfo, WorktreeCandidate} from '../../src/settingsInfo.ts';
import * as M from '../../src/settingsModel.ts';

installDeterminism();
const recorder = new Recorder('settingsModel');
const FNS = ['isAppFlag', 'settingPath', 'infoLayer', 'layerSets', 'otherTarget', 'targetName', 'applyChange', 'ownValue', 'layerName', 'initialColumn', 'columnProblem', 'homePath', 'codexHookNote', 'cellDetail', 'layerActions', 'previewLocation', 'choiceOptions', 'currentChoice', 'choiceChange', 'initialText', 'inheritedHint', 'actionNameProblem', 'actionCount', 'actionNameCheck', 'commandProblem', 'textChange', 'initialLinks', 'linkSelection', 'trustNote', 'savedMessage', 'actionSavedMessage'];
recorder.addTap(await runTap({tests: ['tests/settingsModel.test.ts'], tap: FNS.map(fn => `settingsModel:${fn}`)}), {module: 'settingsModel'});

const rec = (fn: string, impl: Function, args: unknown[]): void => { try { recorder.call(fn, impl, args); } catch {} };
type Kind ='global' | 'repository';
const IDS: M.SettingId[] = M.SETTINGS.map(def => def.id);
const KINDS: Kind[] = ['global', 'repository'];
const vars = {name: '<name>', repo: 'mono', repoParent: '/dev', repoRoot: '/dev/mono', home: '/home/me'};
const STATES = ['trusted', 'untrusted', 'absent', 'invalid', 'bare', 'none'] as const;

interface Options {trusted?: boolean; legacyDev?: string; globalRaw?: string; noVars?: boolean; hookFile?: string; defaultBranch?: string; originBranch?: string; insideIgnored?: boolean; agentHooks?: boolean; notifications?: boolean; codexHooks?: SettingsInfo['codexHooks']; state?: typeof STATES[number]; repoError?: string; globalError?: string; noRepoDoc?: boolean; noGlobalDoc?: boolean; repoExists?: boolean}
function build(repoRaw: string | undefined, defaults: Record<string, unknown> | undefined, o: Options = {}): SettingsInfo {
	let config: any = {};
	try { config = repoRaw === undefined ? {} : parseProjectConfig(repoRaw); } catch {}
	const project = {root: '/dev/mono', trustRoot: '/dev/mono', path: '/dev/mono/deckhand.json', exists: repoRaw !== undefined, fingerprint: 'f', config} as LoadedProject;
	let user: UserSettings = {...defaults ? {defaults} : {}, ...o.legacyDev ? {dev_command: o.legacyDev} : {}};
	if (o.trusted) user = trustProjectConfig(project, user);
	const document = (kind: Kind, raw: string | undefined, exists = raw !== undefined): ProjectConfigDocument => ({kind, root: '/r', path: kind === 'global' ? '/home/me/.deckhand/config.json' : '/dev/mono/deckhand.json', raw: raw ?? '{}', revision: exists ? 'x'.repeat(64) : null, exists});
	const globalRaw = o.globalRaw ?? (defaults && JSON.stringify(defaults));
	const targets: any = {};
	if (!o.noGlobalDoc) targets.global = document('global', globalRaw);
	if (!o.noRepoDoc) targets.repository = document('repository', repoRaw, o.repoExists);
	if (o.globalError) targets.globalError = o.globalError;
	if (o.repoError) targets.repositoryError = o.repoError;
	let rows: SettingsInfo['rows'] = [];
	try { rows = explainSettings(project, user, {vars, user: 'me', configDir: '/state'}).rows; } catch {}
	const state = o.state ?? (repoRaw === undefined ? 'absent' : o.trusted ? 'trusted' : 'untrusted');
	return {
		cwd: '/dev/mono', repo: 'mono', repository: {state, path: '/dev/mono/deckhand.json', ...state === 'invalid' ? {error: 'Unexpected token } in JSON'} : {}}, needsReview: !o.trusted && repoRaw !== undefined, rows, targets,
		...o.hookFile ? {hookFile: o.hookFile} : {}, ...o.noVars ? {} : {vars}, defaultLocation: '/state/worktrees/<name>', insideIgnored: o.insideIgnored ?? false,
		...o.defaultBranch ? {defaultBranch: o.defaultBranch} : {}, ...o.originBranch ? {originBranch: o.originBranch} : {}, user: 'me',
		...o.agentHooks !== undefined ? {agentHooks: o.agentHooks} : {}, notifications: o.notifications ?? false, ...o.codexHooks ? {codexHooks: o.codexHooks} : {},
	} as SettingsInfo;
}

const REPOS: (string | undefined)[] = [
	undefined, '{}', '{"devCommand":"npm run dev"}',
	'{"devCommand":"npm run dev","actions":{"test":"npm test"},"worktree":{"location":"{repoParent}/worktrees/{name}","files":{"backend/.env":"{repoParent}/b.env"}}}',
	'{"defaultAgent":"codex","defaultWorkspace":"new","setupCommand":"npm ci","actions":{"test":"npm t","lint frontend":"eslint .","db.migrate":"make m"},"worktree":{"branchFrom":"default","branchName":"{user}/{name}","symlink":["node_modules","shared"],"hook":false}}',
	'{"worktree":{"location":"{repoRoot}/.worktrees/{name}","branchFrom":"origin","symlink":["a","b"]}}',
	'{"worktree":{"location":"/custom/{name}","hook":true}}', '{"worktree":{"location":"~/wt/{name}"}}', '{"worktree":{"location":"/home/me/x/{name}"}}', '{"worktree":{"location":"{nope}/{name}"}}',
	'{"actions":{"x":"y"}}', '{"actions":"nope"}', '{"worktree":"nope"}', '{"devCommand":5}', '{"defaultAgent":"nope"}', '{"unknown":1}',
	'{not json', '[]', 'null', '"str"', '﻿{"devCommand":"bom"}', '{"devCommand":"日本語 😀"}', '{"actions":{"a":"1","b":"2"},"worktree":{"symlink":["shared"],"files":{"x":"y"}}}',
];
const GLOBALS: (Record<string, unknown> | undefined)[] = [
	undefined, {}, {defaultAgent: 'pi', worktree: {symlink: ['shared'], branchFrom: 'origin'}},
	{defaultAgent: 'codex', defaultWorkspace: 'existing', devCommand: 'yarn dev', setupCommand: 'yarn', actions: {test: 'yarn test', build: 'yarn build'}, worktree: {location: '{repoParent}/wt/{name}', branchName: '{user}/{name}', branchFrom: 'default', hook: false, files: {'a.cfg': '/x/a.cfg'}}},
	{actions: {test: 'global test'}}, {worktree: {location: '{repoRoot}/.worktrees/{name}', symlink: ['node_modules']}}, {devCommand: 'make dev'},
];
const OPTS: Options[] = [
	{}, {trusted: true}, {legacyDev: 'legacy dev'}, {trusted: true, hookFile: '/dev/mono/.claude/scripts/create-worktree.sh'}, {hookFile: '/dev/mono/.claude/scripts/create-worktree.sh'},
	{defaultBranch: 'main', originBranch: 'main', insideIgnored: true}, {defaultBranch: 'trunk'}, {originBranch: 'develop'}, {noVars: true},
	{agentHooks: true, notifications: true}, {agentHooks: false}, {notifications: true},
	{agentHooks: true, codexHooks: {state: 'missing', file: '/home/me/.codex/hooks.json', fileExists: false, command: 'deckhand hooks codex'}},
	{agentHooks: true, codexHooks: {state: 'missing', file: '/home/me/.codex/hooks.json', fileExists: true, command: 'deckhand hooks codex'}},
	{agentHooks: true, codexHooks: {state: 'other', file: '/elsewhere/hooks.json', fileExists: true, command: 'deckhand hooks codex'}},
	{noRepoDoc: true, repoError: 'not a git repository'}, {noRepoDoc: true}, {noGlobalDoc: true, globalError: 'config.json is unreadable'}, {noGlobalDoc: true},
	{state: 'invalid'}, {state: 'bare'}, {state: 'none'}, {globalRaw: '{broken'}, {globalRaw: '[1]'}, {repoExists: false},
];

const infos: SettingsInfo[] = [];
for (const repo of REPOS) for (const defaults of GLOBALS) infos.push(build(repo, defaults, {}), build(repo, defaults, {trusted: true}));
for (const [index, options] of OPTS.entries()) for (const [offset, repo] of REPOS.entries()) infos.push(build(repo, GLOBALS[(index + offset) % GLOBALS.length], options));
infos.push(...sample(fc.tuple(fc.constantFrom(...REPOS), fc.constantFrom(...GLOBALS), fc.constantFrom(...OPTS), fc.boolean()), 150, 11).map(([repo, defaults, options, trusted]) => build(repo, defaults, {...options, trusted})));
const infoSample = (count: number, seed: number) => sample(fc.constantFrom(...infos), count, seed);

// Pure helpers -------------------------------------------------------------------------------------------------
for (const id of IDS) { rec('isAppFlag', M.isAppFlag, [id]); for (const entry of [undefined, 'test', 'a.b', '']) rec('settingPath', M.settingPath, entry === undefined ? [id] : [id, entry]); }
for (const kind of KINDS) for (const fn of ['otherTarget', 'targetName', 'layerName'] as const) rec(fn, M[fn], [kind]);
for (const value of ['/home/me/x', '/home/me', '/home/me/', '/home/mex', '/home', '', '/other', '~/x', '/home/me/日本/😀']) for (const home of [undefined, '/home/me', '', '/']) rec('homePath', M.homePath, home === undefined ? [value] : [value, home]);
for (const [value, home] of sample(fc.tuple(arb.text(14), arb.text(8)), 80, 3)) rec('homePath', M.homePath, [value, home]);
for (const [text] of sample(fc.tuple(arb.text(14)), 60, 4)) rec('homePath', M.homePath, [`/h/${text}`, '/h']);

// Documents and layers -----------------------------------------------------------------------------------------
const docs = ['{}', '{"a":1}', '{"worktree":{"symlink":["x"]}}', '{broken', '[1]', 'null', '"s"', '﻿{"devCommand":"x"}', '{"actions":{"a":"b"},"worktree":{"branchFrom":"origin","symlink":["n"],"files":{"f":"g"}}}', '{"devCommand":"d","unknown":{"k":[1,2]}}', '{"worktree":5}', '{"actions":[]}', ''];
for (const raw of docs) for (const exists of [true, false]) for (const kind of KINDS) rec('infoLayer', M.infoLayer, [{targets: {[kind]: {kind, raw, exists, root: '/r', path: '/p', revision: null}}}, kind]);
for (const info of infos) for (const kind of KINDS) rec('infoLayer', M.infoLayer, [info, kind]);
const layers: (M.Layer | undefined)[] = [undefined, {}, {devCommand: 'x'}, {actions: {a: 'b'}}, {actions: 'str'}, {actions: []}, {worktree: {symlink: ['a']}}, {worktree: {files: {a: 'b'}}}, {worktree: {symlink: 'str', files: []}}, {worktree: 'str'}, {worktree: null}, {defaultAgent: null}, {actions: {'x.y': 1}}];
for (const layer of layers) for (const id of IDS) for (const entry of [undefined, 'a', 'x.y']) {
	rec('layerSets', M.layerSets, entry === undefined ? [layer, id] : [layer, id, entry]);
	rec('ownValue', M.ownValue, entry === undefined ? [layer, id] : [layer, id, entry]);
}
for (const info of infos.slice(0, 120)) for (const id of IDS) for (const kind of KINDS) {
	rec('ownValue', M.ownValue, [M.infoLayer(info, kind), id]);
	rec('layerSets', M.layerSets, [M.infoLayer(info, kind), id]);
}

// applyChange --------------------------------------------------------------------------------------------------
const changes: M.SettingChange[] = [
	{path: ['devCommand'], value: 'make dev'}, {path: ['devCommand']}, {path: ['devCommand'], value: ''}, {path: ['devCommand'], value: 5}, {path: ['actions', 'test'], value: 'npm test'}, {path: ['actions', 'test']}, {path: ['actions', 'a']}, {path: ['actions', 'bad name!'], value: 'x'},
	{path: ['actions', ' x'], value: 'x'}, {path: ['actions', 'a'], value: ''}, {path: ['actions']}, {path: ['actions'], value: {z: 'y'}}, {path: ['worktree', 'symlink'], value: ['a', 'b']}, {path: ['worktree', 'symlink']}, {path: ['worktree', 'symlink'], value: ['../escape']},
	{path: ['worktree', 'location'], value: '{repoParent}/wt/{name}'}, {path: ['worktree', 'location']}, {path: ['worktree', 'location'], value: '/no-name'}, {path: ['worktree', 'branchFrom'], value: 'origin'}, {path: ['worktree', 'branchFrom'], value: 'bogus'},
	{path: ['worktree', 'hook'], value: false}, {path: ['worktree', 'hook']}, {path: ['worktree', 'branchName'], value: '{user}/{name}'}, {path: ['defaultAgent'], value: 'codex'}, {path: ['defaultAgent'], value: 'nope'}, {path: ['defaultWorkspace'], value: 'new'},
	{path: ['setupCommand'], value: '日本語 😀 && echo'}, {path: ['agent_hooks'], value: true}, {path: ['notifications'], value: true}, {path: ['a', 'b', 'c'], value: 1}, {path: ['a', 'b', 'c']}, {path: ['unknownKey'], value: 1},
];
for (const raw of docs) for (const exists of [true, false]) for (const kind of KINDS) for (const change of changes) rec('applyChange', M.applyChange, [{raw, exists, kind}, change]);
for (const info of infos.slice(0, 80)) for (const kind of KINDS) for (const change of changes.slice(0, 14)) if (info.targets[kind]) rec('applyChange', M.applyChange, [info.targets[kind]!, change]);

// Grid and details ---------------------------------------------------------------------------------------------
for (const info of infoSample(60, 21)) for (const row of M.settingsGrid(info)) for (const kind of KINDS) rec('cellDetail', M.cellDetail, [info, row, kind]);
for (const info of infos) for (const kind of KINDS) { rec('layerActions', M.layerActions, [info, kind]); rec('actionCount', M.actionCount, [info, kind]); rec('columnProblem', M.columnProblem, [info, kind]); }
for (const info of infos) rec('initialColumn', M.initialColumn, [info]);
for (const command of ['deckhand hooks codex', '', '日本語']) for (const fileExists of [true, false]) for (const state of ['missing', 'other'] as const) for (const home of [undefined, '/home/me', '/x']) for (const file of ['/home/me/.codex/hooks.json', '/other/h.json', '/home/me']) rec('codexHookNote', M.codexHookNote, home === undefined ? [{state, file, fileExists, command}] : [{state, file, fileExists, command}, home]);

// Controls -----------------------------------------------------------------------------------------------------
const templates = ['', '{name}', '{repoParent}/worktrees/{name}', '{repoRoot}/.worktrees/{name}', '/abs/{name}', '~/x/{name}', '{nope}', '{repo}-{name}', '/home/me/{name}', '/home/me', '../{name}', '日本語/{name}', '{name', 'no placeholders'];
for (const template of templates) for (const v of [undefined, vars, {...vars, home: '/dev'}, {...vars, repoRoot: '/dev/日本 😀'}]) rec('previewLocation', M.previewLocation, [template, v]);
for (const template of sample(arb.text(12), 40, 5)) rec('previewLocation', M.previewLocation, [template, vars]);
for (const info of infoSample(160, 22)) for (const id of IDS) for (const kind of KINDS) {
	const options = M.choiceOptions(info, id, kind);
	rec('choiceOptions', M.choiceOptions, [info, id, kind]);
	rec('currentChoice', M.currentChoice, [info, id, kind, options]);
	for (const option of options) rec('choiceChange', M.choiceChange, [info, id, kind, option]);
}
for (const info of infoSample(80, 23)) for (const id of ['devCommand', 'setupCommand', 'worktree.branchName', 'defaultAgent', 'actions'] as M.SettingId[]) for (const kind of KINDS) for (const entry of [undefined, 'test', 'a', 'zzz']) {
	rec('initialText', M.initialText, entry === undefined ? [info, id, kind] : [info, id, kind, entry]);
	rec('inheritedHint', M.inheritedHint, entry === undefined ? [info, id, kind] : [info, id, kind, entry]);
}
for (const info of infoSample(40, 24)) for (const id of IDS) for (const kind of KINDS) rec('inheritedHint', M.inheritedHint, [info, id, kind]);

// Action names, commands, text changes -------------------------------------------------------------------------
const names = ['', ' ', ' x', 'x ', 'a', 'test', 'lint frontend', 'db.migrate', 'a_b-c.d', '1abc', '-x', '_x', '.x', 'a\tb', '\tx', 'a/b', 'a"b', 'a😀b', 'é', 'aé', 'a中', 'a\u0000', 'a​', 'a b', 'á', 'a\n', 'a'.repeat(48), 'a'.repeat(49), 'a'.repeat(100), `${'a'.repeat(47)}😀`, 'constructor', '__proto__', 'toString', 'hasOwnProperty', 'a~', 'a$', 'a{', 'a€', 'a©', 'a‑b', '٣x', 'x٣', 'a٣', 'a ', 'a\u{E0001}', 'a\u{10FFFF}', 'a\u{1F600}'];
for (const name of names) rec('actionNameProblem', M.actionNameProblem, [name]);
for (const name of sample(arb.text(60), 120, 6)) rec('actionNameProblem', M.actionNameProblem, [name]);
for (const name of sample(arb.text(10, ['a', 'b', '1', ' ', '_', '.', '-', '😀', '中', '\t']), 120, 7)) rec('actionNameProblem', M.actionNameProblem, [name]);
const checkNames = ['', 'test', 'a', 'b', 'lint frontend', 'bad name!', ' x', 'new one', 'zzz', 'x'];
for (const info of infos) for (const kind of KINDS) for (const name of checkNames) rec('actionNameCheck', M.actionNameCheck, [info, kind, name]);
const full = (count: number) => build(JSON.stringify({actions: Object.fromEntries(Array.from({length: count}, (_, index) => [`n${index}`, 'c']))}), {actions: Object.fromEntries(Array.from({length: count}, (_, index) => [`g${index}`, 'c']))}, {});
for (const count of [29, 30, 31]) for (const kind of KINDS) for (const name of ['', 'n1', 'g1', 'fresh', 'bad!']) { rec('actionNameCheck', M.actionNameCheck, [full(count), kind, name]); rec('actionCount', M.actionCount, [full(count), kind]); }
const commands = ['', ' ', '\t\n', 'npm test', ' x ', 'a\0b', '\0', 'x'.repeat(8192), 'x'.repeat(8193), '😀'.repeat(4096), '😀'.repeat(4097), '中'.repeat(8192), '日本語 && echo 😀'];
for (const text of commands) rec('commandProblem', M.commandProblem, [text]);
for (const text of sample(arb.text(30), 100, 8)) rec('commandProblem', M.commandProblem, [text]);
const texts = ['', ' ', 'make dev', '  ', '\t', 'a\0b', 'x'.repeat(9000), '{name}', '{user}/{name}', '/abs/{name}', '{nope}', 'bad name', '日本語 😀', 'ok', '../x', 'a b', 'refs/heads/x', '-x'];
for (const id of IDS) for (const text of texts) for (const entry of [undefined, 'test', ' bad', '', 'a.b']) rec('textChange', M.textChange, entry === undefined ? [id, text] : [id, text, entry]);
for (const text of sample(arb.text(20), 80, 9)) for (const id of ['devCommand', 'setupCommand', 'worktree.branchName', 'worktree.location', 'actions'] as M.SettingId[]) rec('textChange', M.textChange, id === 'actions' ? [id, text, 'name'] : [id, text]);

// Links --------------------------------------------------------------------------------------------------------
const cand = (path: string, extra: Partial<WorktreeCandidate> = {}): WorktreeCandidate => ({path, kind: 'dir', ignored: true, suggestion: 'skip', ...extra} as WorktreeCandidate);
const candidateSets: WorktreeCandidate[][] = [
	[], [cand('node_modules', {suggestion: 'link'})], [cand('node_modules', {suggestion: 'link', configured: 'symlink' as any}), cand('x1', {kind: 'file', suggestion: 'link'}), cand('dist')],
	[cand('a', {configured: 'files' as any}), cand('b', {configured: 'symlink' as any}), cand('c', {suggestion: 'link'})], [cand('日本', {suggestion: 'link'}), cand('😀'), cand('.venv', {suggestion: 'link', configured: 'symlink' as any})],
];
for (const candidates of candidateSets) for (const suggest of [undefined, true, false]) rec('initialLinks', M.initialLinks, suggest === undefined ? [candidates] : [candidates, suggest]);
const linkLayers: (M.Layer | undefined)[] = [undefined, {}, {worktree: {symlink: ['node_modules']}}, {worktree: {symlink: ['a', 'b', 'x']}}, {worktree: {symlink: ['c', 'b']}}, {worktree: {symlink: 'bad'}}, {worktree: {symlink: ['a', 1, null, 'c']}}, {worktree: {files: {a: 'b'}}}, {worktree: {symlink: ['日本', '.venv']}}];
const linkMaps: Record<string, boolean>[] = [{}, {node_modules: true}, {node_modules: false}, {a: true, b: false, c: true}, {a: false, b: false, c: false}, {a: true, b: true, c: true}, {'日本': true, '😀': true, '.venv': false}, {x: true}];
for (const target of linkLayers) for (const other of linkLayers) for (const candidates of candidateSets) for (const links of linkMaps) rec('linkSelection', M.linkSelection, [target, other, candidates, links]);

// Messages -----------------------------------------------------------------------------------------------------
const trusts = [undefined, 'kept', 'created', 'unreviewed', 'hook'] as const;
for (const trust of trusts) rec('trustNote', M.trustNote, [trust]);
for (const label of ['Dev command', '', '日本語 😀', 'a\nb']) for (const kind of KINDS) for (const trust of trusts) for (const cleared of [undefined, true, false]) {
	rec('savedMessage', M.savedMessage, [label, kind, ...cleared === undefined && trust === undefined ? [] : [{...cleared === undefined ? {} : {cleared}, ...trust === undefined ? {} : {trust}}]]);
	rec('actionSavedMessage', M.actionSavedMessage, trust === undefined ? [label, kind] : [label, kind, trust]);
}

await recorder.write({prune: true});
