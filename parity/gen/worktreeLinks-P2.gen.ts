/**
 * Parity fixtures for the pure parts of src/worktreeLinks.ts (unit P2; userSlug and applyWorktreeLinks belong to I6 and
 * touch the OS / file system). Regenerate with
 *   node --import tsx parity/gen/worktreeLinks-P2.gen.ts
 * writes fixtures/parity/worktreeLinks-P2/.
 */
import {Recorder, arb, fc, installDeterminism, mutate, sample} from '../lib.ts';
import {branchNameProblem, expandBranchName, expandWorktreeTemplate, mergeWorktreeSettings, validRelativePath, validateWorktreeSettings, worktreeLocation} from '../../src/worktreeLinks.ts';

const MODULE = 'worktreeLinks-P2';
installDeterminism();
const recorder = new Recorder(MODULE);
const rc = (name: string, fn: Function, args: unknown[]): void => { try { recorder.call(name, fn, args); } catch {} };

const vars = {name: 'feat', repo: 'repo', repoParent: '/work', repoRoot: '/work/repo', home: '/home/me'};
const unicodeVars = {name: '功能😀', repo: 'répo', repoParent: '/work/中文', repoRoot: '/work/中文/répo', home: '/home/émile'};
const trailing = {name: 'n', repo: 'r', repoParent: '/work/', repoRoot: '/work/r/', home: '/home/me/'};
const allVars = [vars, unicodeVars, trailing, {...vars, name: ''}, {...vars, home: '/'}, {...vars, name: '../escape'}, {...vars, home: ''}];

const TEMPLATES = [
	'', '~', '~/', '~/wt/{name}', '~/{name}', '{home}/wt/{name}', '{repoParent}/{repo}-{name}', '{repoRoot}/../{name}', '{repoRoot}/.worktrees/{name}', '/abs/{name}', '/abs/{name}/', '/abs//{name}/./x', 'relative/{name}', './{name}', '{name}', '{name}{name}',
	'{', '}', '{}', '{{name}}', '{name', 'name}', '{name}}', '{{name}', '{unknown}', '{Name}', '{ name }', '{name}{', '~user/x', '~x', '{home}', '{repoRoot}', 'a{name}b{repo}c', '/tmp/{name}-{repoParent}', '/w/{__proto__}', '/w/{constructor}', '/w/{toString}', '/日本/{name}/😀',
	'~/{home}', '{home}{home}', '/', '//', '/{name}/..', '/a/{name}/../../..', '/..', '~/..', '~/../x', '~/\u00e9/{name}', '~/e\u0301/{name}', '/a\nb/{name}', '/a b/{name}',
];
for (const template of TEMPLATES) for (const v of allVars) {
	rc('expandWorktreeTemplate', expandWorktreeTemplate, [template, v]);
	rc('worktreeLocation', worktreeLocation, [template, v]); // relative expansions throw before path.resolve, so no cwd dependence
}
const piece = fc.constantFrom('{name}', '{repo}', '{repoParent}', '{repoRoot}', '{home}', '{x}', '{', '}', '/', '~', '~/', '/a', '..', '.', 'é', '中', '😀', ' ', '-', '{user}');
const template = fc.array(piece, {maxLength: 6}).map(parts => parts.join(''));
const varsArb = fc.constantFrom(...allVars);
for (const [t, v] of sample(fc.tuple(template, varsArb), 300, 51)) {
	rc('expandWorktreeTemplate', expandWorktreeTemplate, [t, v]);
	rc('worktreeLocation', worktreeLocation, [t, v]);
}
for (const [t, v] of sample(fc.tuple(arb.text(16), varsArb), 100, 52)) {
	rc('expandWorktreeTemplate', expandWorktreeTemplate, [t, v]);
}

// expandBranchName / branchNameProblem
const bvars = [{name: 'feat', user: 'me'}, {name: '功能😀', user: 'émile'}, {name: '', user: ''}, {name: 'a b', user: 'x'}, {name: 'x/y', user: 'u'}, {name: '..', user: 'u'}];
const BRANCH_TEMPLATES = ['{name}', '{user}/{name}', 'feature/{name}', '{name}-{user}', '{user}', '', '{', '}', '{}', '{x}', '{repo}', '{{name}}', '{name', 'a{name}b{user}c{name}', '{Name}', '{__proto__}', '{constructor}', '{toString}', '用户/{name}', 'é/{name}'];
for (const t of BRANCH_TEMPLATES) for (const v of bvars) rc('expandBranchName', expandBranchName, [t, v]);
const bpiece = fc.constantFrom('{name}', '{user}', '{x}', '{', '}', '/', '-', '.', 'a', 'b', 'é', '中', '😀', ' ', '@', '~', ':');
for (const [t, v] of sample(fc.tuple(fc.array(bpiece, {maxLength: 6}).map(parts => parts.join('')), fc.constantFrom(...bvars)), 250, 53)) rc('expandBranchName', expandBranchName, [t, v]);

const BRANCHES = ['', 'main', 'feature/x', 'feat/a-b_c.d', 'a'.repeat(200), 'a'.repeat(201), '@', '@{', 'a@{b', 'a@b', 'a..b', 'a//b', '/a', 'a/', '.a', 'a.', '-a', 'a-', 'a.lock', 'a.lock/b', 'a/b.lock', 'a/.b', 'a/.', 'a b', 'a\tb', 'a\nb', 'a\x7fb', 'a\x00b', 'a\x1fb', 'a~b', 'a^b', 'a:b', 'a?b', 'a*b', 'a[b', 'a\\b', 'a]b', 'a{b', 'a}b', '功能', '😀', 'é', 'e\u0301', '用户/功能-😀', 'a\u0080b', 'a\u00a0b', 'a\u2028b', 'a.lockx', '.lock', 'lock', 'a/.lock', 'a//.b', '...', '..', '.', '/', '-', 'a/-b', 'A/B', 'a/b/c/d/e'];
for (const name of BRANCHES) rc('branchNameProblem', branchNameProblem, [name]);
for (const [name] of sample(fc.tuple(fc.array(fc.constantFrom('a', 'b', '.', '/', '-', '@', '{', '~', '^', ':', '?', '*', '[', '\\', ' ', '\x00', '\x1f', '\x7f', '.lock', '..', '//', '@{', 'é', '中', '😀'), {maxLength: 7}).map(parts => parts.join(''))), 400, 54)) rc('branchNameProblem', branchNameProblem, [name]);
for (const args of mutate(recorder.argsOf('branchNameProblem'), {variants: 3})) rc('branchNameProblem', branchNameProblem, args);

// validRelativePath(value: unknown)
const PATH_VALUES: unknown[] = [
	'', 'a', 'a/b', 'a/b/c', 'a/', '/a', '/', '.', '..', './a', 'a/.', 'a/./b', 'a/../b', '../a', 'a/..', 'a//b', '//', 'a\\b', 'a\0b', '.git', '.git/config', 'a/.git', '.gitignore', '.git/', '.github/x', 'node_modules', 'node_modules/.cache', 'dir/file.txt', '中文/路径', '😀/e\u0301', 'é', ' a', 'a ', 'a b/c d', 'a\nb', 'x'.repeat(4096), 'x'.repeat(4097), `${'a/'.repeat(2047)}a`, 'a/b/', '~/a', '~', 'C:\\a', 'C:/a', '.a/.b', '...', 'a/...', 'a/..b',
	undefined, null, 0, 1, true, false, {}, [], ['a'], {a: 1}, Number.NaN,
];
for (const value of PATH_VALUES) rc('validRelativePath', validRelativePath, [value]);
for (const [value] of sample(fc.tuple(fc.array(fc.constantFrom('a', 'b', '.', '..', '', '.git', 'x y', '中', '😀', 'é', '\\', '\0'), {maxLength: 5}).map(parts => parts.join('/'))), 400, 55)) rc('validRelativePath', validRelativePath, [value]);
for (const args of mutate(recorder.argsOf('validRelativePath'), {variants: 2})) rc('validRelativePath', validRelativePath, args);

// validateWorktreeSettings(value: unknown)
const manyFiles = Object.fromEntries(Array.from({length: 51}, (_v, index) => [`f${index}`, '{repoRoot}/x']));
const exactFiles = Object.fromEntries(Array.from({length: 50}, (_v, index) => [`f${index}`, '{repoRoot}/x']));
const SETTINGS: unknown[] = [
	undefined, null, 0, 'x', true, [], [{}], {}, {location: '~/wt/{name}'}, {location: '/abs/{name}'}, {location: '{repoParent}/{name}'}, {location: '{repoRoot}/.wt/{name}'}, {location: '{home}/{name}'}, {location: '/abs'}, {location: 'rel/{name}'}, {location: '{name}'}, {location: ''}, {location: '   '},
	{location: 5}, {location: null}, {location: '{unknown}/{name}'}, {location: '/a/{name'}, {location: '/a/{name}}'}, {location: `/${'a'.repeat(4096)}/{name}`}, {location: '/a\0/{name}'}, {location: '~/{name}/中文/😀'}, {location: '~'}, {location: '{repo}/{name}'},
	{symlink: []}, {symlink: ['node_modules']}, {symlink: ['a', 'a', 'b']}, {symlink: 'node_modules'}, {symlink: [1]}, {symlink: ['/abs']}, {symlink: ['../x']}, {symlink: ['.git']}, {symlink: ['a/']}, {symlink: ['']}, {symlink: [null]}, {symlink: ['中文/😀']}, {symlink: Array.from({length: 50}, (_v, index) => `d${index}`)}, {symlink: Array.from({length: 51}, (_v, index) => `d${index}`)}, {symlink: null}, {symlink: {}},
	{files: {}}, {files: {'.env': '{repoRoot}/.env'}}, {files: {'.env': '/abs/.env', 'a/b': 'rel/x'}}, {files: {'../x': 'a'}}, {files: {'/x': 'a'}}, {files: {'.git/x': 'a'}}, {files: {__proto__x: 'a'}}, {files: JSON.parse('{"__proto__": "a"}')}, {files: {constructor: 'a'}}, {files: {prototype: 'a'}}, {files: {a: ''}}, {files: {a: '  '}}, {files: {a: 1}}, {files: {a: '{bad}'}}, {files: {a: '{'}}, {files: {a: 'x\0y'}}, {files: {a: 'x'.repeat(4097)}}, {files: []}, {files: null}, {files: 'x'}, {files: manyFiles}, {files: exactFiles}, {files: {'中文': '😀'}}, {files: {a: '~/x'}},
	{branchFrom: 'current'}, {branchFrom: 'default'}, {branchFrom: 'origin'}, {branchFrom: 'other'}, {branchFrom: 1}, {branchFrom: null}, {branchFrom: ''},
	{branchName: '{name}'}, {branchName: '{user}/{name}'}, {branchName: 'fixed'}, {branchName: ''}, {branchName: 5}, {branchName: '{name'}, {branchName: '{name}{x}'}, {branchName: '-{name}'}, {branchName: '{name}.lock'}, {branchName: '{name}//x'}, {branchName: '{name} x'}, {branchName: '{name}..{user}'}, {branchName: '/{name}'}, {branchName: '{name}/'}, {branchName: '功能/{name}'}, {branchName: `${'a'.repeat(190)}{name}`}, {branchName: `${'a'.repeat(200)}{name}`}, {branchName: '@{name}'}, {branchName: '{user}@{{name}'}, {branchName: '{name}~'}, {branchName: null},
	{hook: true}, {hook: false}, {hook: 'yes'}, {hook: 0}, {hook: null},
	{unknown: 1}, {location: '/a/{name}', extra: true}, {__proto__: null}, JSON.parse('{"__proto__": {"location": "x"}}'),
	{location: '~/wt/{name}', symlink: ['node_modules', '.venv'], files: {'.env': '{repoRoot}/.env'}, branchFrom: 'origin', branchName: '{user}/{name}', hook: false},
	{location: '~/wt/{name}', symlink: ['a'], files: {x: 'y'}, branchFrom: 'bogus', branchName: '{x}', hook: 'no'},
	{hook: 'no', branchFrom: 'bogus'}, {branchFrom: 'bogus', branchName: 'zzz'}, {branchName: 'zzz', location: 'bad'}, {location: 'bad', symlink: 'bad'}, {symlink: 'bad', files: 'bad'},
];
for (const value of SETTINGS) rc('validateWorktreeSettings', validateWorktreeSettings, [value]);
for (const args of mutate(recorder.argsOf('validateWorktreeSettings').filter(([value]) => value !== undefined), {variants: 3, seed: 56})) rc('validateWorktreeSettings', validateWorktreeSettings, args);
const setting = fc.record({
	location: fc.option(template, {nil: undefined}), symlink: fc.option(fc.array(fc.constantFrom('a', 'a/b', '..', '.git', '', 'x\\y', '中文', '/a'), {maxLength: 4}), {nil: undefined}),
	files: fc.option(fc.dictionary(fc.constantFrom('a', 'a/b', '..', '.git', '', '__proto__x', 'constructor', '中文'), fc.constantFrom('x', '{repoRoot}/x', '{bad}', '', '/a')), {nil: undefined}),
	branchFrom: fc.option(fc.constantFrom('current', 'default', 'origin', 'x'), {nil: undefined}), branchName: fc.option(fc.array(bpiece, {maxLength: 5}).map(parts => parts.join('')), {nil: undefined}), hook: fc.option(fc.boolean(), {nil: undefined}),
}, {requiredKeys: []});
for (const [value] of sample(fc.tuple(setting), 400, 57)) rc('validateWorktreeSettings', validateWorktreeSettings, [value]);
for (const [value] of sample(fc.tuple(fc.record({location: fc.constantFrom('~/w/{name}', '/abs/{name}', '{repoParent}/x-{name}', '{home}/{name}/中'), symlink: fc.array(fc.constantFrom('a', 'b/c', 'node_modules', '中文'), {maxLength: 3}), files: fc.dictionary(fc.constantFrom('a', 'b/c', '.env'), fc.constantFrom('x', '{repoRoot}/x', '/abs')), branchFrom: fc.constantFrom('current', 'default', 'origin'), branchName: fc.constantFrom('{name}', '{user}/{name}', 'p-{name}'), hook: fc.boolean()}, {requiredKeys: []})), 200, 58)) rc('validateWorktreeSettings', validateWorktreeSettings, [value]);

// mergeWorktreeSettings(global, repository)
const S: unknown[] = [
	undefined, {}, {location: '~/g/{name}'}, {location: '~/r/{name}'}, {location: ''}, {branchFrom: 'origin'}, {branchFrom: 'default'}, {branchName: 'g/{name}'}, {branchName: ''}, {hook: false}, {hook: true}, {symlink: ['a', 'b']}, {symlink: ['b', 'c']}, {symlink: []}, {files: {a: '1', b: '2'}}, {files: {b: '3', c: '4'}}, {files: {}},
	{location: '~/g/{name}', symlink: ['a', 'a'], files: {a: 'g'}, branchFrom: 'current', branchName: '{user}/{name}', hook: true}, {location: '~/r/{name}', symlink: ['z'], files: {a: 'r', z: 'r'}, branchFrom: 'origin', branchName: 'r/{name}', hook: false},
	{symlink: ['中文', '😀', 'e\u0301']}, {symlink: ['e\u0301', 'é']}, {files: {'中文': 'x'}}, {location: '', branchName: ''}, {hook: false, location: ''}, {symlink: undefined, files: undefined}, {extra: 1},
];
for (const first of S) for (const second of S) rc('mergeWorktreeSettings', mergeWorktreeSettings, [first, second]);
const valid = fc.record({location: fc.constantFrom('~/g/{name}', '/abs/{name}', ''), symlink: fc.array(fc.constantFrom('a', 'b', 'c', 'd/e', '中'), {maxLength: 4}), files: fc.dictionary(fc.constantFrom('a', 'b', 'c', 'd/e', '中'), fc.constantFrom('x', 'y', '{home}/z')), branchFrom: fc.constantFrom('current', 'default', 'origin'), branchName: fc.constantFrom('{name}', 'a/{name}', ''), hook: fc.boolean()}, {requiredKeys: []});
for (const [first, second] of sample(fc.tuple(fc.option(valid, {nil: undefined}), fc.option(valid, {nil: undefined})), 400, 59)) rc('mergeWorktreeSettings', mergeWorktreeSettings, [first, second]);

await recorder.write({prune: true});
