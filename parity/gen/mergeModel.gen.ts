/**
 * Gen script for src/mergeModel.ts. Regenerate with `node --import tsx parity/gen/mergeModel.gen.ts`.
 *
 * compactPath reads $HOME, so HOME is pinned to a path no input starts with, and recorded paths never use the
 * tests' /home/me (rewritten to /srv/me). truncate cuts by UTF-16 length: calls whose output would hold a lone
 * surrogate (a cut inside a pair) are not recorded, since Rust strings cannot hold one.
 */
import {Recorder, arb, decode, encode, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {CONFLICT_FILES_SHOWN, conflictView, mergeConfirmLayout, mergeConfirmRows, mergeOptions, overlappingFiles, surprisingTarget} from '../../src/mergeModel.ts';

process['env'].HOME = '/nonexistent-parity-home';
const MODULE = 'mergeModel';
installDeterminism();
const recorder = new Recorder(MODULE);

const rewrite = (value: unknown): unknown => {
	if (typeof value === 'string') return value.replaceAll('/home/me', '/srv/me');
	if (Array.isArray(value)) return value.map(rewrite);
	if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, rewrite(entry)]));
	return value;
};
function rec(name: string, fn: Function, args: unknown[]): void {
	args = rewrite(args) as unknown[];
	let out: unknown;
	try { out = fn(...structuredClone(args)); } catch { recorder.call(name, fn, structuredClone(args)); return; }
	if (JSON.stringify(encode(out)).includes('"$utf16"')) return;
	recorder.call(name, fn, structuredClone(args));
}

const NAMES = ['mergeOptions', 'surprisingTarget', 'overlappingFiles', 'mergeConfirmLayout', 'mergeConfirmRows', 'conflictView'];
const fns: Record<string, Function> = {mergeOptions, surprisingTarget, overlappingFiles, mergeConfirmLayout, mergeConfirmRows, conflictView};

// 1. Tap the existing tests (args only; re-called here so the recorded output is independent of the tests' HOME).
const tapped = await runTap({tests: ['tests/mergeModel.test.ts'], tap: NAMES.map(name => `mergeModel:${name}`)});
for (const record of tapped) if (record.module === 'mergeModel') rec(record.fn, fns[record.fn]!, decode(JSON.parse(record.line).args) as unknown[]);
const seeds = (name: string) => recorder.argsOf(name);

// 2. Hand-written edge cases.
const preview = (fields: object = {}) => ({
	sourceRef: 'feat/auth', sourceSha: 'a'.repeat(40), targetRoot: '/srv/code/app', targetBranch: 'main', targetIsMain: true, defaultBranch: 'main',
	commitCount: 8, commits: ['add login form', 'wire the session cookie', 'fix tests', 'rename handler', 'docs', 'lint'],
	diff: {files: 5, insertions: 120, deletions: 14}, uncommitted: 4, overlap: {committed: [], uncommitted: []}, ...fields,
});
const input = (fields: object = {}) => ({title: 'auth refactor', preview: preview(), commitFirst: true, commitMessage: 'auth refactor', notes: [], width: 60, height: 40, ...fields});
const noteEntries = [{key: 'w', title: 'Worktree notes · feat/auth', lines: ['- [ ] review', 'ship it']}, {key: 's', title: 'Session notes', lines: []}, {key: 'x', title: '笔记 😀', lines: ['中文 line', 'é combining', 'a'.repeat(90)]}];

rec('mergeOptions', mergeOptions, [undefined]);
for (const targetBranch of ['main', undefined, '', 'release/1.0', '功能', 'x😀y']) rec('mergeOptions', mergeOptions, [preview({targetBranch})]);
for (const args of [{targetIsMain: false}, {targetBranch: undefined}, {targetBranch: ''}, {defaultBranch: undefined}, {defaultBranch: 'master'}, {targetBranch: 'dev', defaultBranch: undefined}, {}]) rec('surprisingTarget', surprisingTarget, [preview(args)]);
for (const [committed, uncommitted, uncommittedCount] of [[[], [], 0], [['a', 'b', 'a'], ['b', 'c', 'c'], 2], [['a'], ['b'], 0], [[], ['z', 'é', '😀', 'é'], 1], [['2', '1'], ['1', '3'], 5]] as const) {
	for (const commitFirst of [true, false]) rec('overlappingFiles', overlappingFiles, [preview({overlap: {committed, uncommitted}, uncommitted: uncommittedCount}), commitFirst]);
}

const layouts: object[] = [
	input(), input({notes: noteEntries}), input({preview: undefined}), input({preview: undefined, previewError: 'git exploded'}), input({preview: undefined, previewError: '很长的错误 😀'.repeat(8)}),
	input({commitFirst: false}), input({preview: preview({commitCount: 0, commits: [], uncommitted: 0})}), input({preview: preview({commitCount: 0, commits: [], uncommitted: 2}), commitFirst: false}),
	input({preview: preview({commitCount: 1, commits: ['only'], uncommitted: 1, diff: {files: 1, insertions: 0, deletions: 0}})}),
	input({preview: preview({commitCount: 6, commits: ['a', 'b', 'c', 'd', 'e', 'f']})}), input({preview: preview({targetBranch: undefined})}), input({preview: preview({targetIsMain: false, targetRoot: '/srv/me/other'})}),
	input({preview: preview({inProgress: 'merge'})}), input({preview: preview({inProgress: 'conflicts'})}), input({preview: preview({inProgress: 'rebase'})}),
	input({preview: preview({overlap: {committed: ['a.ts', 'b.ts', 'c.ts', 'd.ts'], uncommitted: ['e.ts']}})}), input({preview: preview({overlap: {committed: ['a.ts'], uncommitted: ['b.ts']}}), commitFirst: false}),
	input({tasks: ['ship login']}), input({tasks: ['one', 'two', 'three']}), input({tasks: []}),
	input({openItems: {titles: ['write tests', 'update docs'], send: true}}), input({openItems: {titles: ['write tests'], send: false}}), input({openItems: {titles: [], send: true}}),
	input({error: 'hook failed\n  line two\n\nline three  \nlast'}), input({error: 'e1\ne2\ne3\ne4\ne5\ne6\ne7\ne8\ne9', height: 24}), input({error: 'boom', notes: noteEntries}),
	input({notes: noteEntries, height: 25}), input({notes: noteEntries, height: 22}), input({title: '中文标题 😀 with emoji', commitMessage: 'ünïcode 😀 message that is rather long indeed to force the quote to be cut short', width: 40}),
	...[0, 1, 4, 8, 12, 20, 30, 80, 200].map(width => input({width, notes: noteEntries, error: 'fail'})),
	...[0, 5, 10, 12, 14, 16, 18, 20, 23, 26, 30, 60].map(height => input({height, notes: noteEntries, error: 'fail one\nfail two', tasks: ['t'], openItems: {titles: ['x'], send: true}})),
	input({preview: preview({commits: ['😀'.repeat(30), '中'.repeat(70), 'é'.repeat(5)], commitCount: 40}), width: 30, notes: noteEntries}),
	input({preview: preview({targetRoot: '/srv/very/long/path/with/many/segments/to/compact/app', targetBranch: 'feature/long-branch-name'}), width: 30}),
	input({preview: preview({targetRoot: '/srv/a'}), width: 8}), input({preview: preview({targetRoot: '/srv/me'}), width: 40}),
];
for (const layout of layouts) rec('mergeConfirmLayout', mergeConfirmLayout, [layout]);

// 3. Mutations of the recorded layouts.
for (const args of mutate(seeds('mergeConfirmLayout'), {variants: 3})) rec('mergeConfirmLayout', mergeConfirmLayout, args);
for (const args of mutate(seeds('overlappingFiles'), {variants: 3})) rec('overlappingFiles', overlappingFiles, args);
for (const args of mutate(seeds('surprisingTarget'), {variants: 3})) rec('surprisingTarget', surprisingTarget, args);

// 4. fast-check generated inputs.
const text = arb.text(14);
const strip = (value: object) => Object.fromEntries(Object.entries(value).filter(([, entry]) => entry !== undefined));
const previewArb = fc.record({
	sourceRef: text, targetRoot: fc.array(text, {minLength: 1, maxLength: 5}).map(parts => '/' + parts.join('/')), targetIsMain: fc.boolean(),
	commitCount: fc.nat(30), commits: fc.array(text, {maxLength: 7}), diff: fc.record({files: fc.nat(40), insertions: fc.nat(2000), deletions: fc.nat(2000)}),
	uncommitted: fc.nat(6), overlap: fc.record({committed: fc.array(text, {maxLength: 5}), uncommitted: fc.array(text, {maxLength: 5})}),
	targetBranch: fc.option(text, {nil: undefined}), defaultBranch: fc.option(fc.constantFrom('main', 'master', 'trunk'), {nil: undefined}), inProgress: fc.option(fc.constantFrom('merge', 'rebase', 'cherry-pick', 'revert', 'conflicts'), {nil: undefined}),
}).map(strip);
const inputArb = fc.record({
	title: text, preview: fc.option(previewArb, {nil: undefined}), previewError: fc.option(text, {nil: undefined}), commitFirst: fc.boolean(), commitMessage: text,
	error: fc.option(arb.text(60), {nil: undefined}), notes: fc.array(fc.record({key: fc.constantFrom('w', 's', 'r'), title: text, lines: fc.array(text, {maxLength: 4})}), {maxLength: 3}),
	tasks: fc.option(fc.array(text, {maxLength: 4}), {nil: undefined}), openItems: fc.option(fc.record({titles: fc.array(text, {maxLength: 3}), send: fc.boolean()}), {nil: undefined}),
	width: fc.integer({min: 0, max: 90}), height: fc.integer({min: 0, max: 45}),
}).map(strip);
for (const value of sample(inputArb, 220, 11)) rec('mergeConfirmLayout', mergeConfirmLayout, [value]);
for (const value of sample(previewArb, 80, 12)) {
	rec('surprisingTarget', surprisingTarget, [value]);
	rec('mergeOptions', mergeOptions, [value]);
	rec('overlappingFiles', overlappingFiles, [value, true]);
	rec('overlappingFiles', overlappingFiles, [value, false]);
}

// 5. mergeConfirmRows over recorded layouts (the layout itself is its input) and hand-built ones.
for (const entry of recorder.cases('mergeConfirmLayout')) if ('ret' in entry) rec('mergeConfirmRows', mergeConfirmRows, [decode(entry.ret!)]);
const empty = {title: '', details: [], toggles: [], options: [], hint: '', error: [], notes: []};
rec('mergeConfirmRows', mergeConfirmRows, [empty]);
rec('mergeConfirmRows', mergeConfirmRows, [{...empty, details: [{text: 'a'}], error: [{text: 'e'}], notes: [{text: 'n'}, {text: 'm'}], toggles: ['t']}]);

// 6. conflictView: the five-file cap, counts beyond the list, missing fields and narrow widths.
const files = (count: number) => Array.from({length: count}, (_, index) => `src/file${index}.ts`);
for (const width of [0, 1, 5, 20, 40, 80]) {
	for (const result of [{}, {conflicts: []}, {conflicts: files(1)}, {conflicts: files(CONFLICT_FILES_SHOWN)}, {conflicts: files(CONFLICT_FILES_SHOWN + 1)}, {conflicts: files(3), conflictCount: 12}, {conflicts: files(7), conflictCount: 3}, {conflictCount: 4}, {conflictCount: 1, conflicts: ['中文/文件😀.ts']}, {conflicts: files(5), conflictCount: 5}, {conflicts: files(4), conflictCount: 5}]) rec('conflictView', conflictView, [result, width]);
}
for (const args of mutate(seeds('conflictView'), {variants: 4})) rec('conflictView', conflictView, args);
for (const [count, total, width] of sample(fc.tuple(fc.nat(9), fc.option(fc.nat(30), {nil: undefined}), fc.integer({min: 0, max: 50})), 120, 13)) {
	rec('conflictView', conflictView, [{conflicts: files(count).map((name, index) => index % 3 === 0 ? name + '😀中' : name), ...total === undefined ? {} : {conflictCount: total}}, width]);
}

await recorder.write({prune: true});
