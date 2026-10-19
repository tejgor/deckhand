/**
 * Fixtures for unit P10 (pure parts of src/projectConfig.ts). Regenerate with
 *
 *   node --import tsx parity/gen/projectConfig-P10.gen.ts
 *
 * Skipped on purpose: loadProjectConfig, savedProjectTrust, readBoundedUtf8 (spawn git / touch the file system, owned
 * by I6 or covered by ported tests); PROJECT_CONFIG_FILE and SUGGESTION_KEYS are constants.
 * DECKHAND_HOME is pinned so error text and built-in worktree locations that embed the config dir are stable.
 */
process.env.DECKHAND_HOME = '/deckhand-home';
import os from 'node:os';
import {Recorder, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {explainSettings, globalDefaults, isProjectTrusted, parseProjectConfig, projectFingerprint, projectNeedsReview, resolveDevCommand, resolveSettings, resolveSetupCommand, sha256, trustProjectConfig, untrustedLayer, validateProjectConfig} from '../../src/projectConfig.ts';

const MODULE = 'projectConfig-P10';
installDeterminism();
const recorder = new Recorder(MODULE);
const rc = (fn: string, impl: Function, args: unknown[]) => { try { recorder.call(fn, impl, args); } catch {} };
const FNS: Record<string, Function> = {sha256, validateProjectConfig, parseProjectConfig, projectFingerprint, isProjectTrusted, projectNeedsReview, trustProjectConfig, globalDefaults, untrustedLayer, resolveSettings, resolveDevCommand, resolveSetupCommand, explainSettings};

// 1. Tap the existing tests, keeping only calls whose arguments carry no temp paths (those vary per run).
// sha256 is not tapped: the concurrent-save test hashes whichever draft wins the race, so its calls vary per run
// (sha256 is covered by the direct calls below).
const tmp = [os.tmpdir(), '/private/tmp', '/var/folders', '/tmp/'];
const tapped = (await runTap({tests: ['tests/projectConfig.test.ts', 'tests/projectConfigDocument.test.ts'], tap: Object.keys(FNS).filter(fn => fn !== 'sha256').map(fn => `projectConfig:${fn}`)}))
	.filter(record => !tmp.some(prefix => record.line.includes(prefix)) && record.fn in FNS);
recorder.addTap(tapped, {module: 'projectConfig'});
for (const fn of ['validateProjectConfig', 'parseProjectConfig', 'resolveSettings', 'resolveDevCommand', 'resolveSetupCommand', 'explainSettings', 'trustProjectConfig', 'isProjectTrusted']) {
	for (const args of mutate(recorder.argsOf(fn), {variants: 3})) rc(fn, FNS[fn]!, args);
}

// 2. sha256 / projectFingerprint
const texts = ['', 'a', 'hello world', '{}', '﻿{}', '中文😀', 'é', 'line1\nline2\r\n', '\0', 'x'.repeat(5000), 'config\0{}\0creation-hook\0'];
for (const text of texts) rc('sha256', sha256, [text]);
for (const text of sample(fc.string({maxLength: 20}), 40, 11)) rc('sha256', sha256, [text]);
rc('sha256', sha256, [new Uint8Array([0, 1, 2, 255])]);
rc('sha256', sha256, [new Uint8Array(0)]);
for (const raw of texts) {
	rc('projectFingerprint', projectFingerprint, [raw]);
	rc('projectFingerprint', projectFingerprint, [raw, 'hookhash']);
	rc('projectFingerprint', projectFingerprint, [raw, '']);
}

// 3. validateProjectConfig / parseProjectConfig
const W = {location: '{repoParent}/wt/{name}', symlink: ['node_modules', '.venv'], files: {'a/b.cfg': '{repoParent}/x.cfg'}, branchFrom: 'default', branchName: '{user}/{name}', hook: false};
const validInputs: unknown[] = [
	{}, {defaultAgent: 'claude'}, {defaultAgent: 'pi', defaultWorkspace: 'none'}, {defaultAgent: 'codex', defaultWorkspace: 'existing'},
	{devCommand: 'npm run dev', setupCommand: 'npm ci'}, {devCommand: ' x '}, {devCommand: 'x'.repeat(8192)}, {actions: {}}, {actions: {test: 'npm test', 'my action': 'echo hi', 'a.b-c_d': 'x', '0': 'zero'}},
	{actions: {['a'.repeat(48)]: 'x'}}, {worktree: W}, {worktree: {}}, {defaultAgent: 'claude', defaultWorkspace: 'new', devCommand: 'd', setupCommand: 's', actions: {a: 'b'}, worktree: W},
	{actions: {'中文': 'x'}}, {actions: {a: '😀'}}, {actions: {'😀': 'x'}},
];
const invalidInputs: unknown[] = [
	null, undefined, 1, 'str', true, [], [1], {unknown: true}, {'😀': 1}, {defaultAgent: ['pi']}, {defaultAgent: 'gpt'}, {defaultAgent: ''}, {defaultAgent: null}, {defaultAgent: 1},
	{defaultWorkspace: null}, {defaultWorkspace: 'fresh'}, {defaultWorkspace: 'New'}, {devCommand: ''}, {devCommand: '   '}, {devCommand: 5}, {devCommand: 'x'.repeat(8193)}, {devCommand: 'a\0b'},
	{setupCommand: ''}, {setupCommand: null}, {setupCommand: '\t\n'}, {actions: null}, {actions: []}, {actions: 'x'}, {actions: {'bad/name': 'echo ok'}}, {actions: {'': 'x'}}, {actions: {' lead': 'x'}},
	{actions: {['a'.repeat(49)]: 'x'}}, {actions: {a: ''}}, {actions: {a: 1}}, {actions: {a: 'x\0'}}, {actions: {a: 'x'.repeat(8193)}},
	{actions: Object.fromEntries(Array.from({length: 31}, (_, i) => [`a${i}`, 'x']))}, {actions: Object.fromEntries(Array.from({length: 30}, (_, i) => [`a${i}`, 'x']))},
	JSON.parse('{"actions":{"__proto__":"echo bad"}}'), {actions: {constructor: 'x'}}, {actions: {prototype: 'x'}},
	{worktree: null}, {worktree: []}, {worktree: 'x'}, {worktree: {unknown: 1}}, {worktree: {branchFrom: 'nope'}}, {worktree: {hook: 'yes'}}, {worktree: {branchName: 'no-template'}}, {worktree: {branchName: 5}},
	{worktree: {location: 'relative/{name}'}}, {worktree: {location: '/abs'}}, {worktree: {location: 5}}, {worktree: {symlink: 'x'}}, {worktree: {symlink: ['/abs']}}, {worktree: {symlink: ['../up']}}, {worktree: {files: []}}, {worktree: {files: {'../x': 'y'}}},
];
// An own "__proto__" key cannot survive the fixture encoding, so it is exercised through parseProjectConfig only.
for (const value of [...validInputs, ...invalidInputs].filter(value => !JSON.stringify(value)?.includes("__proto__"))) {
	rc('validateProjectConfig', validateProjectConfig, [value]);
	rc('validateProjectConfig', validateProjectConfig, [value, 'defaults']);
}
rc('validateProjectConfig', validateProjectConfig, [{bad: 1}, '😀 label']);
const rawInputs = [...validInputs.map(value => JSON.stringify(value)), ...invalidInputs.filter(value => value !== undefined).map(value => JSON.stringify(value)),
	'', ' ', '{', '}', '{"a":', 'null', 'tru', '[]', '{"defaultAgent":"pi",}', "{'a':1}", '﻿{}', '﻿{"defaultAgent":"pi"}', '﻿﻿{}', ' ﻿{}', '{"devCommand":"中文😀"}', '{"actions":{"a":"\\ud83d\\ude00"}}', '{"actions":{"a":"\\ud83d"}}', '{"defaultAgent":"pi"} x', '{"a":1}{"b":2}', '{"defaultAgent":"pi","defaultAgent":"codex"}'];
// JSON syntax errors carry V8-specific text the port cannot reproduce: only inputs that are valid JSON are recorded.
const isJson = (raw: string) => { try { JSON.parse(raw.replace(/^\uFEFF/, "")); return true; } catch { return false; } };
for (const raw of rawInputs.filter(isJson)) {
	rc('parseProjectConfig', parseProjectConfig, [raw]);
	rc('parseProjectConfig', parseProjectConfig, [raw, 'defaults']);
}
const configArb = fc.record({
	defaultAgent: fc.constantFrom('claude', 'pi', 'codex', 'x', 1, null),
	defaultWorkspace: fc.constantFrom('none', 'new', 'existing', 'x', 1, null),
	devCommand: fc.constantFrom('dev', ' npm start ', '', '  ', 'a\0b', 'cd 中 && 😀', 5),
	setupCommand: fc.constantFrom('npm ci', '', 'make', null),
	actions: fc.oneof(fc.constantFrom({}, null, []), fc.dictionary(fc.constantFrom('test', 'lint', 'a b', 'bad/x', '', '__proto__x', 'constructor', '😀', 'x_1'), fc.constantFrom('go', '', 'x\0', 'echo 中'), {maxKeys: 4})),
	worktree: fc.constantFrom(undefined, null, {}, W, {hook: true}, {hook: false}, {location: '/w/{name}'}, {location: '/w'}, {branchFrom: 'origin', symlink: ['a', 'a']}, {files: {'x/y': '{home}/z'}}, {zzz: 1}),
}, {requiredKeys: []});
for (const config of sample(configArb, 250, 21)) {
	rc('validateProjectConfig', validateProjectConfig, [config]);
	rc('parseProjectConfig', parseProjectConfig, [JSON.stringify(config)]);
}

// 4. trust
const fps = ['f', 'g', '', 'a'.repeat(64)];
const trustCfgs: unknown[] = [{}, {trustedProjects: {}}, {trustedProjects: {'/r': ['f']}}, {trustedProjects: {'/r': ['g', 'f']}}, {trustedProjects: {'/r': []}}, {trustedProjects: {'/r': 'f'}}, {trustedProjects: {'/r': null}}, {trustedProjects: {'/other': ['f']}}, {trustedProjects: {'/中😀': ['f']}}, {trustedProjects: {'/r': [1, 'f']}}, {trustedProjects: null}, {trustedProjects: {'/r': Array.from({length: 25}, (_, i) => `h${i}`)}}, {trustedProjects: {'/r': [...Array.from({length: 19}, (_, i) => `h${i}`), 'f']}}, {other: 1, trustedProjects: {'/r': ['f'], '/s': ['q']}}];
for (const trustRoot of ['/r', '/中😀', '']) for (const fingerprint of fps) for (const config of trustCfgs) {
	rc('isProjectTrusted', isProjectTrusted, [{trustRoot, fingerprint}, config]);
	rc('trustProjectConfig', trustProjectConfig, [{trustRoot, fingerprint}, config]);
}
const mkProject = (over: Record<string, unknown> = {}): Record<string, unknown> => ({root: '/r', trustRoot: '/r', path: '/r/deckhand.json', exists: true, fingerprint: 'f', config: {}, ...over});
const hookEnabled = {file: '/r/.claude/scripts/create-worktree.sh', fingerprint: 'h', script: 'x'};
for (const project of [mkProject(), mkProject({exists: false}), mkProject({exists: false, creationHook: hookEnabled}), mkProject({creationHook: hookEnabled}), mkProject({disabledHook: {file: '/r/x.sh', by: 'global'}, exists: false}), mkProject({fingerprint: 'g'}), mkProject({path: undefined, exists: false})]) {
	for (const config of trustCfgs) rc('projectNeedsReview', projectNeedsReview, [project, config]);
}

// 5. settings resolution
const cfgs: Record<string, unknown>[] = [
	{}, {defaultAgent: 'pi'}, {defaultWorkspace: 'new'}, {defaultAgent: 'pi', defaultWorkspace: 'existing', devCommand: 'repo dev', setupCommand: 'repo setup', actions: {test: 'repo test', lint: 'repo lint'}},
	{devCommand: '  '}, {devCommand: 'dev 中😀', actions: {'😀': 'x'}}, {worktree: {hook: false}}, {worktree: {hook: true, location: '/r/{name}'}}, {worktree: W},
	{worktree: {symlink: ['a', 'b'], files: {'x': '/s/x'}}, setupCommand: 's'}, {actions: {}},
];
const users: Record<string, unknown>[] = [
	{}, {dev_command: 'legacy'}, {dev_command: '  '}, {dev_command: ' leg '}, {defaults: {}}, {defaults: {defaultAgent: 'codex', defaultWorkspace: 'new', setupCommand: 'npm ci', actions: {test: 'global test', fmt: 'global fmt'}}, dev_command: 'legacy'},
	{defaults: {devCommand: 'npm run dev'}, dev_command: 'legacy'}, {defaults: {setupCommand: ''}}, {defaults: 'x'}, {defaults: null}, {defaults: []}, {defaults: {bogus: 1}}, {defaults: {worktree: {hook: false}}},
	{defaults: {worktree: {branchName: '{user}/{name}', symlink: ['node_modules', 'a'], files: {'x': '/g/x', 'y': '/g/y'}, location: '/g/{name}'}, actions: {lint: 'g lint'}}},
	{defaults: {devCommand: 'd'}, trustedProjects: {'/r': ['f']}}, {trustedProjects: {'/r': ['f']}, dev_command: 'legacy'}, {defaults: {setupCommand: 'gs'}, trustedProjects: {'/r': ['f']}},
	{defaults: {devCommand: '', actions: {a: 'b'}}}, {dev_command: '中 legacy 😀', defaults: {defaultAgent: 'claude'}},
];
const projects: (Record<string, unknown> | undefined)[] = [undefined];
for (const config of cfgs) for (const over of [{}, {fingerprint: 'g'}, {exists: false}]) projects.push(mkProject({config, ...over}));
projects.push(mkProject({config: {worktree: {hook: false}}, creationHook: hookEnabled}), mkProject({config: {setupCommand: 'x'}, disabledHook: {file: '/r/h.sh', by: 'repository'}}));
for (const project of projects) for (const user of users) {
	rc('resolveSettings', resolveSettings, [project, user]);
	rc('resolveDevCommand', resolveDevCommand, [project, user]);
	rc('resolveSetupCommand', resolveSetupCommand, [project, user]);
	if (project) for (const reviewed of ['f', 'g', '']) rc('resolveSetupCommand', resolveSetupCommand, [project, user, reviewed]);
}
for (const user of users) rc('globalDefaults', globalDefaults, [user]);
for (const config of cfgs) rc('untrustedLayer', untrustedLayer, [config]);
rc('untrustedLayer', untrustedLayer, [{defaultAgent: undefined, defaultWorkspace: 'none', devCommand: 'x'}]);

// 6. explainSettings
const vars = {name: '<name>', repo: 'mono', repoParent: '/dev', repoRoot: '/dev/mono', home: '/home/me'};
const contexts: Record<string, unknown>[] = [{}, {vars, user: 'tejas', configDir: '/state'}, {vars: {...vars, name: 'feat-中😀'}, user: 'u'}, {hookFile: '/r/.claude/scripts/create-worktree.sh'}, {configDir: '/cfg 中'}, {vars, hookFile: '/x.sh'}];
const explainConfigs: Record<string, unknown>[] = [
	...cfgs,
	{defaultWorkspace: 'new', devCommand: 'cd frontend && npm run dev', actions: {test: 'pytest', lint: 'repo lint'}, worktree: {location: '{repoParent}/worktrees/{name}', hook: false, symlink: ['frontend/node_modules', 'node_modules'], files: {'backend/local.cfg': '{repoParent}/backend.cfg'}}},
	{devCommand: 'dev', actions: {lint: 'npm run lint'}}, {worktree: {branchName: 'bad', branchFrom: 'origin'}}, {worktree: {location: '/w/{unknown}/{name}', files: {a: '{bogus}'}}},
	{worktree: {hook: true, branchName: 'x/{name}'}},
];
const explainUsers: Record<string, unknown>[] = [
	...users,
	{dev_command: 'legacy dev', defaults: {defaultAgent: 'codex', actions: {lint: 'npm run lint'}, worktree: {branchName: '{user}/{name}', symlink: ['node_modules'], files: {'backend/local.cfg': '/g/cfg'}}}},
];
let n = 0;
for (const config of explainConfigs) for (const user of explainUsers) {
	const base = mkProject({config, root: '/dev/mono', trustRoot: '/dev/mono', path: '/dev/mono/deckhand.json', ...n % 5 === 0 ? {creationHook: hookEnabled} : {}});
	const context = contexts[n % contexts.length];
	n++;
	rc('explainSettings', explainSettings, [base, user, context]);
	const trusted = trustProjectConfig(base as any, user as any);
	rc('explainSettings', explainSettings, [base, trusted, context]);
	if (n % 3 === 0) rc('explainSettings', explainSettings, [undefined, user, context]);
	if (n % 4 === 0) rc('explainSettings', explainSettings, [base, user]);
}
for (const context of contexts) for (const user of [{}, ...explainUsers.slice(0, 4)]) rc('explainSettings', explainSettings, [undefined, user, context]);
rc('explainSettings', explainSettings, [undefined, {}]);
rc('explainSettings', explainSettings, [mkProject({config: {devCommand: 'dev'}}), {}, {vars, user: 'tejas', configDir: '/state'}]);
rc('explainSettings', explainSettings, [mkProject({config: {worktree: {symlink: ['a/b', 'c']}}, path: undefined, exists: false}), {defaults: {worktree: {symlink: ['c', 'd']}}}, {vars}]);

await recorder.write({prune: true});
