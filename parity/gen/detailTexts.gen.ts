/**
 * Parity fixtures for src/detailTexts.ts (fixtures/parity/detailTexts/). Regenerate with
 *
 *   node --import tsx parity/gen/detailTexts.gen.ts
 *
 * All five functions are pure text builders: trustReviewText, cleanupOverrideText, projectActions,
 * workspaceSummaryText, createPrConfirmText. Inputs are exhaustive over the branch flags (trusted / needsReview /
 * hook / disabled hook / PR state / links ...), plus fast-check text with unicode in every free-form field.
 * `project.config` is printed with JSON.stringify(.., null, 2), so its key order is part of the fixture (TS order).
 * Object keys that look like integers are avoided in action maps (JS orders them first; Rust maps would not).
 */
import {Recorder, arb, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {cleanupOverrideText, createPrConfirmText, projectActions, trustReviewText, workspaceSummaryText} from '../../src/detailTexts.ts';

installDeterminism();
const recorder = new Recorder('detailTexts', {cap: 600});

// 1. Tap: tests/cleanupView.test.ts calls cleanupOverrideText.
recorder.addTap(await runTap({tests: ['tests/cleanupView.test.ts'], tap: ['detailTexts:*']}), {module: 'detailTexts'});

const unicode = arb.text(14);
const word = fc.constantFrom('alpha', 'beta', 'src/main.ts', 'é', '中文', '😀', 'a b', '', 'with\nnewline', 'tab\there', '"quoted"', 'back\\slash', 'main', 'feature/中文-😀');

// 2. trustReviewText(project, purpose?).
interface ProjectOver {path?: string; exists: boolean; trusted: boolean; needsReview: boolean; hook: 'none' | 'creation' | 'global' | 'repository'; config: Record<string, unknown>; purpose?: string}
const configs: Record<string, unknown>[] = [
	{}, {devCommand: 'npm run dev', actions: {test: 'npm test', lint: 'eslint .'}, worktree: {hook: false}},
	{defaultAgent: 'claude', defaultWorkspace: 'new', setupCommand: 'make setup', note: 'é 中 😀  '},
];
const project = (over: ProjectOver): Record<string, unknown> => {
	const record: Record<string, unknown> = {root: '/repo', trustRoot: '/repo'};
	if (over.path !== undefined) record.path = over.path;
	record.config = over.config; record.exists = over.exists; record.fingerprint = 'f'.repeat(16);
	if (over.hook === 'creation') record.creationHook = {file: '/repo/create-worktree.sh', content: '#!/bin/bash\necho "hi 😀"\n', fingerprint: 'abc123'};
	if (over.hook === 'global' || over.hook === 'repository') record.disabledHook = {file: '/repo/create-worktree.sh', by: over.hook};
	record.trusted = over.trusted; record.needsReview = over.needsReview;
	record.effective = {devCommand: 'npm run dev'};
	return record;
};
for (const path of [undefined, '/repo/deckhand.json']) for (const exists of [true, false]) for (const [trusted, needsReview] of [[false, false], [true, false], [false, true], [true, true]] as const) for (const hook of ['none', 'creation', 'global', 'repository'] as const) for (const config of configs) for (const purpose of [undefined, '', 'Run `npm test` from this repository.']) {
	if (config !== configs[0] && (path === undefined || !exists) && hook !== 'none') continue; // keep the corpus smaller
	recorder.call('trustReviewText', trustReviewText, [project({path, exists, trusted, needsReview, hook, config}), ...purpose === undefined ? [] : [purpose]]);
}
recorder.call('trustReviewText', trustReviewText, [project({path: '/repo/deckhand.json', exists: true, trusted: false, needsReview: true, hook: 'creation', config: configs[1]!}), undefined]);
for (const [path, text, purpose, hookFile, hookContent] of sample(fc.tuple(word, unicode, fc.option(unicode, {nil: undefined}), word, unicode), 60, 31)) {
	const record = project({path, exists: true, trusted: true, needsReview: false, hook: 'creation', config: {devCommand: text, actions: {[`a-${hookFile}`]: text}}});
	(record.creationHook as Record<string, string>).file = hookFile; (record.creationHook as Record<string, string>).content = hookContent;
	recorder.call('trustReviewText', trustReviewText, [record, ...purpose === undefined ? [] : [purpose]]);
}
for (const args of mutate(recorder.argsOf('trustReviewText').slice(0, 40), {variants: 2})) recorder.call('trustReviewText', trustReviewText, args);

// 3. cleanupOverrideText(inspection | undefined).
type FileState = 'changed' | 'untracked' | 'ignored';
const states: FileState[] = ['changed', 'untracked', 'ignored'];
const inspection = (reasons: string[], files: {path: string; state: FileState}[] | undefined, dirty: number, untracked: number, ignored: number): Record<string, unknown> => ({reasons, ...files === undefined ? {} : {files}, dirtyFiles: dirty, untrackedFiles: untracked, ignoredFiles: ignored});
recorder.call('cleanupOverrideText', cleanupOverrideText, [undefined]);
for (const reasons of [[], ['2 uncommitted files'], ['2 uncommitted files', '1 unpushed commit', 'Branch is not merged: 中文 😀']]) {
	for (const files of [undefined, [], [{path: 'a.txt', state: 'changed'}], states.map((state, index) => ({path: `dir/file-${index} é.txt`, state})), Array.from({length: 12}, (_, index) => ({path: `f${index}`, state: states[index % 3]!}))] as ({path: string; state: FileState}[] | undefined)[]) {
		for (const counts of [[0, 0, 0], [1, 0, 0], [2, 3, 4], [0, 0, 1], [5, 5, 5], [20, 0, 0]] as const) recorder.call('cleanupOverrideText', cleanupOverrideText, [inspection(reasons, files, counts[0], counts[1], counts[2])]);
	}
}
for (const [reason, filePath, state, extra] of sample(fc.tuple(unicode, unicode, fc.constantFrom(...states), fc.integer({min: 0, max: 6})), 80, 32)) {
	recorder.call('cleanupOverrideText', cleanupOverrideText, [inspection(reason ? [reason, `${reason}!`] : [], [{path: filePath, state}], extra, 1, 0)]);
}

// 4. projectActions(project | undefined).
recorder.call('projectActions', projectActions, [undefined]);
const actionProject = (effective: Record<string, string> | undefined, pending: Record<string, string> | undefined, needsReview: boolean): Record<string, unknown> => ({
	...project({path: '/repo/deckhand.json', exists: true, trusted: !needsReview, needsReview, hook: 'none', config: pending === undefined ? {} : {actions: pending}}),
	effective: effective === undefined ? {} : {actions: effective},
});
const actionSets: Array<Record<string, string> | undefined> = [undefined, {}, {test: 'npm test'}, {test: 'npm test', build: 'npm run build'}, {lint: 'eslint .', test: 'jest', 'dev-中文': 'echo 😀'}, {constructor: 'x', toString: 'y'}];
for (const effective of actionSets) for (const pending of actionSets) for (const needsReview of [false, true]) recorder.call('projectActions', projectActions, [actionProject(effective, pending, needsReview)]);
for (const [names, commands, needsReview] of sample(fc.tuple(fc.uniqueArray(fc.constantFrom('a', 'b', 'test', 'build', 'é', '中', 'x-y', 'deploy', 'z_1'), {maxLength: 5}), fc.array(unicode, {minLength: 5, maxLength: 5}), fc.boolean()), 100, 33)) {
	const effective = Object.fromEntries(names.filter((_, index) => index % 2 === 0).map((name, index) => [name, commands[index]!]));
	const pending = Object.fromEntries(names.filter((_, index) => index % 3 !== 1).map((name, index) => [name, commands[4 - index]!]));
	recorder.call('projectActions', projectActions, [actionProject(effective, pending, needsReview)]);
}

// 5. workspaceSummaryText(summary | undefined, prLoading?, options?) and createPrConfirmText(summary).
interface Summary {cwd: string; branch: string; head: string; baseRef?: string; changedFiles: number; additions: number; deletions: number; untrackedFiles: number; ahead?: number; behind?: number; commitsAheadOfBase?: number; pushRemote?: string; pr?: Record<string, unknown>; prError?: string}
const summary = (over: Partial<Summary> = {}): Summary => ({cwd: '/work/tree', branch: 'feature/x', head: 'abc1234 Add the thing', changedFiles: 3, additions: 10, deletions: 2, untrackedFiles: 1, ...over});
const prs: Array<Record<string, unknown> | undefined> = [undefined, {number: 12, url: 'https://github.com/o/r/pull/12', state: 'OPEN', checks: 'passing'}, {number: 7, url: 'https://github.com/o/r/pull/7', state: 'MERGED', checks: 'unknown', headSha: 'deadbeef', baseBranch: 'main'}, {number: 0, url: '', state: '', checks: 'failing'}];
const variants: Partial<Summary>[] = [
	{}, {head: ''}, {baseRef: 'origin/main'}, {baseRef: ''}, {ahead: 0, behind: 0}, {ahead: 2, behind: 5, commitsAheadOfBase: 3}, {commitsAheadOfBase: 0}, {pushRemote: 'origin'}, {pushRemote: ''},
	{changedFiles: 0, additions: 0, deletions: 0, untrackedFiles: 0}, {branch: 'feature/中文-😀', cwd: '/tmp/work tree/é', head: '😀 emoji'}, {prError: 'gh not installed'}, {prError: ''},
	{changedFiles: 1234, additions: 99999, deletions: 5, untrackedFiles: 42, ahead: 10, behind: 0},
];
const linkSets: Array<{linked: string[]; notes: string[]} | undefined> = [undefined, {linked: [], notes: []}, {linked: ['.env', 'node_modules'], notes: []}, {linked: ['a'], notes: ['skipped b: exists', 'skipped c: 中文 😀']}, {linked: [], notes: ['one note']}];
recorder.call('workspaceSummaryText', workspaceSummaryText, [undefined]);
recorder.call('workspaceSummaryText', workspaceSummaryText, [undefined, true, {creatingPr: true}]);
for (const variant of variants) for (const pr of prs) {
	const value = summary({...variant, ...pr ? {pr} : {}});
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value]);
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value, false]);
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value, true]);
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value, false, {}]);
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value, false, {creatingPr: true}]);
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value, true, {creatingPr: true}]);
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value, false, {creatingPr: false}]);
	for (const links of linkSets) recorder.call('workspaceSummaryText', workspaceSummaryText, [value, false, {links}]);
	recorder.call('createPrConfirmText', createPrConfirmText, [value]);
}
for (const [cwd, branch, head, remote, error] of sample(fc.tuple(word, word, unicode, fc.option(word, {nil: undefined}), fc.option(unicode, {nil: undefined})), 80, 34)) {
	const value = summary({cwd, branch, head, ...remote === undefined ? {} : {pushRemote: remote}, ...error === undefined ? {} : {prError: error}});
	recorder.call('workspaceSummaryText', workspaceSummaryText, [value, false, {links: {linked: [cwd], notes: [head, branch]}}]);
	recorder.call('createPrConfirmText', createPrConfirmText, [value]);
}
for (const args of mutate(recorder.argsOf('workspaceSummaryText').filter(args => args[0] !== undefined).slice(0, 60), {variants: 2})) recorder.call('workspaceSummaryText', workspaceSummaryText, args);
for (const args of mutate(recorder.argsOf('createPrConfirmText').slice(0, 40), {variants: 2})) recorder.call('createPrConfirmText', createPrConfirmText, args);

await recorder.write({prune: true});
