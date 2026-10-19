/** Fixtures for src/projectConfigDocument.ts (unit I6): readGlobalDefaultsDocument and saveGlobalDefaultsDocument as
 * directory cases (DECKHAND_HOME is the case root; the Rust replay must point its config dir at root), plus the
 * saveProjectConfigDocument drafts that are rejected before any git or file access. readConfigTargets,
 * readProjectConfigDocument and the accepted paths of saveProjectConfigDocument resolve the repository through git
 * (resolveRepoContext) and are left to ported tests. */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, installDeterminism, recordDirCase} from '../lib.ts';
import {saveGlobalDefaultsDocument, readGlobalDefaultsDocument, saveProjectConfigDocument} from '../../src/projectConfigDocument.ts';
import {sha256} from '../../src/projectConfig.ts';

installDeterminism();
const MODULE = 'projectConfigDocument-I6';
const configJson = (value: unknown) => JSON.stringify(value, null, 2) + '\n';
const revision = (defaults: unknown) => sha256(JSON.stringify(defaults));
const withHome = (root: string) => { process.env.DECKHAND_HOME = root; return root; };

const DEFAULTS = {defaultAgent: 'claude', defaultWorkspace: 'new', devCommand: 'npm run dev', actions: {Test: 'npm test'}, worktree: {location: '~/wt/{name}', symlink: ['node_modules'], branchFrom: 'default', hook: false}};
const UNICODE_DEFAULTS = {devCommand: 'echo 日本語 😀 é é', actions: {Unicode: 'echo \u{1F468}‍\u{1F469}'}};
const OTHER_KEYS = {dev_command: 'legacy', attach_scroll_sensitivity: 3, trustedProjects: {'/repo': ['a'.repeat(64)]}, agent_hooks: true, notifications: true, custom_key: {nested: [1, 2]}};

interface Config {name: string; config?: string | object | undefined}
const configs: Config[] = [
	{name: 'no-config'},
	{name: 'empty-object', config: {}},
	{name: 'defaults-full', config: {defaults: DEFAULTS}},
	{name: 'defaults-unicode', config: {defaults: UNICODE_DEFAULTS}},
	{name: 'defaults-empty-object', config: {defaults: {}}},
	{name: 'defaults-with-other-keys', config: {...OTHER_KEYS, defaults: DEFAULTS}},
	{name: 'other-keys-only', config: OTHER_KEYS},
	{name: 'defaults-null', config: {defaults: null}},
	{name: 'defaults-invalid-shape', config: {defaults: {bogus: 1}}},
	{name: 'defaults-string', config: {defaults: 'text'}},
	{name: 'defaults-key-order', config: {defaults: {worktree: {hook: true}, actions: {B: 'b', A: 'a'}, devCommand: 'z'}}},
	{name: 'config-array', config: '[]'},
	{name: 'config-null', config: 'null'},
	{name: 'config-empty-file', config: ''},
	{name: 'config-compact', config: '{"defaults":{"devCommand":"x"}}'},
];
const textOf = (c: Config) => c.config === undefined ? undefined : typeof c.config === 'string' ? c.config : configJson(c.config);
const place = (c: Config) => async (root: string) => { withHome(root); const text = textOf(c); if (text !== undefined) await fs.writeFile(path.join(root, 'config.json'), text, {mode: 0o600}); };

for (const c of configs) await recordDirCase(MODULE, `read-global-${c.name}`, {setup: place(c), fn: 'readGlobalDefaultsDocument', args: () => [], run: readGlobalDefaultsDocument});
await recordDirCase(MODULE, 'read-global-config-is-directory', {setup: async root => { withHome(root); await fs.mkdir(path.join(root, 'config.json')); }, fn: 'readGlobalDefaultsDocument', args: () => [], run: readGlobalDefaultsDocument});
await recordDirCase(MODULE, 'read-global-permissive-home', {setup: async root => { withHome(root); await fs.chmod(root, 0o755); await fs.writeFile(path.join(root, 'config.json'), configJson({defaults: DEFAULTS})); }, fn: 'readGlobalDefaultsDocument', args: () => [], run: readGlobalDefaultsDocument});

// saveGlobalDefaultsDocument ----------------------------------------------------------------------------------
const draft = (value: unknown) => JSON.stringify(value, null, 2) + '\n';
interface Save {name: string; config?: Config['config']; raw: string | ((root: string) => string); expected: null | string | ((config: Config['config']) => string | null)}
const rev = (config: Config['config']) => { const text = typeof config === 'string' ? config : config ? JSON.stringify(config) : undefined; if (text === undefined) return null; try { const d = (JSON.parse(text) as {defaults?: unknown}).defaults; return d === undefined ? null : revision(d); } catch { return null; } };
const saves: Save[] = [
	{name: 'new-file', raw: draft({defaultAgent: 'pi'}), expected: null},
	{name: 'new-file-empty-object', raw: '{}', expected: null},
	{name: 'new-file-unicode', raw: draft(UNICODE_DEFAULTS), expected: null},
	{name: 'new-file-no-trailing-newline', raw: '{"devCommand":"x"}', expected: null},
	{name: 'new-file-bom', raw: '﻿{"devCommand":"x"}', expected: null},
	{name: 'new-file-full', raw: draft(DEFAULTS), expected: null},
	{name: 'existing-matching-revision', config: {defaults: DEFAULTS}, raw: draft({defaultAgent: 'codex'}), expected: c => rev(c)},
	{name: 'existing-matching-revision-keeps-other-keys', config: {...OTHER_KEYS, defaults: DEFAULTS}, raw: draft({devCommand: 'make'}), expected: c => rev(c)},
	{name: 'existing-other-keys-no-defaults', config: OTHER_KEYS, raw: draft({devCommand: 'make'}), expected: null},
	{name: 'existing-empty-object-config', config: {}, raw: draft({devCommand: 'make'}), expected: null},
	{name: 'existing-defaults-replaced-by-empty', config: {defaults: DEFAULTS}, raw: '{}', expected: c => rev(c)},
	{name: 'existing-same-content', config: {defaults: DEFAULTS}, raw: draft(DEFAULTS), expected: c => rev(c)},
	{name: 'existing-unicode', config: {defaults: UNICODE_DEFAULTS}, raw: draft({...UNICODE_DEFAULTS, setupCommand: '日本語'}), expected: c => rev(c)},
	{name: 'stale-revision', config: {defaults: DEFAULTS}, raw: draft({defaultAgent: 'codex'}), expected: 'f'.repeat(64)},
	{name: 'stale-revision-null-but-exists', config: {defaults: DEFAULTS}, raw: draft({defaultAgent: 'codex'}), expected: null},
	{name: 'stale-revision-defaults-removed', config: OTHER_KEYS, raw: draft({defaultAgent: 'codex'}), expected: 'a'.repeat(64)},
	{name: 'stale-revision-no-config', raw: draft({defaultAgent: 'codex'}), expected: 'b'.repeat(64)},
	{name: 'revision-uppercase-hex', config: {defaults: DEFAULTS}, raw: '{}', expected: c => rev(c)!.toUpperCase()},
	{name: 'revision-too-short', config: {defaults: DEFAULTS}, raw: '{}', expected: 'abc'},
	{name: 'revision-not-hex', config: {defaults: DEFAULTS}, raw: '{}', expected: 'g'.repeat(64)},
	{name: 'revision-empty-string', raw: '{}', expected: ''},
	{name: 'invalid-draft-not-object', raw: '[]', expected: null},
	{name: 'invalid-draft-null', raw: 'null', expected: null},
	{name: 'invalid-draft-unknown-key', raw: '{"nope":1}', expected: null},
	{name: 'invalid-draft-agent', raw: '{"defaultAgent":"x"}', expected: null},
	{name: 'invalid-draft-worktree', raw: '{"worktree":{"location":"rel/{name}"}}', expected: null},
	{name: 'invalid-draft-unicode-action-name', raw: draft({actions: {'Ünï': 'x'}}), expected: null},
	{name: 'invalid-draft-empty', raw: '', expected: null},
	{name: 'invalid-draft-over-limit', raw: `{"devCommand":"x"}${' '.repeat(64 * 1024)}`, expected: null},
	{name: 'draft-exactly-limit', raw: `{"devCommand":"x"}${' '.repeat(64 * 1024 - 18)}`, expected: null},
	{name: 'draft-multibyte-over-limit', raw: `{"devCommand":"${'😀'.repeat(16384)}"}`, expected: null},
	{name: 'draft-multibyte-under-limit', raw: `{"devCommand":"${'😀'.repeat(8000)}"}`, expected: null},
	{name: 'invalid-draft-with-stale-revision', config: {defaults: DEFAULTS}, raw: '{"nope":1}', expected: 'f'.repeat(64)},
	{name: 'invalid-revision-and-draft', raw: '{"nope":1}', expected: 'zz'},
	{name: 'existing-config-is-array', config: '[]', raw: '{}', expected: null},
];
for (const c of saves) {
	await recordDirCase(MODULE, `save-global-${c.name}`, {
		setup: place({name: c.name, config: c.config}),
		fn: 'saveGlobalDefaultsDocument',
		args: () => [typeof c.raw === 'function' ? c.raw('') : c.raw, typeof c.expected === 'function' ? c.expected(c.config) : c.expected],
		run: saveGlobalDefaultsDocument,
	});
}

// saveProjectConfigDocument: drafts rejected before the repository is resolved (cwd is never touched) -----------
const recorder = new Recorder(MODULE);
const NOWHERE = '/parity/no-such-checkout';
const REV = 'a'.repeat(64);
const rejected: Array<[string, string | null]> = [
	['', null], ['[]', null], ['null', null], ['42', null], ['"text"', null], ['{"nope":1}', null], ['{"defaultAgent":"x"}', REV], ['{"defaultWorkspace":3}', null],
	['{"devCommand":""}', null], ['{"devCommand":"a\\u0000b"}', null], ['{"actions":{"-x":"y"}}', null], ['{"actions":[]}', null], ['{"worktree":{"hook":"yes"}}', null],
	['{"worktree":{"location":"rel/{name}"}}', null], ['{"worktree":{"symlink":["../a"]}}', null], ['{"worktree":{"branchName":"x"}}', null], ['{"worktree":{"branchFrom":"tip"}}', null],
	['{"devCommand":"x"}', 'abc'], ['{"devCommand":"x"}', REV.toUpperCase()], ['{"devCommand":"x"}', 'g'.repeat(64)], ['{"devCommand":"x"}', ''], ['{"devCommand":"x"}', REV + '0'],
	[`{"devCommand":"x"}${' '.repeat(64 * 1024)}`, null], [`{"devCommand":"${'😀'.repeat(16384)}"}`, REV], ['{"名前":1}', null], ['﻿{"名前":1}', REV],
];
for (const [raw, expected] of rejected) {
	await recorder.callAll('saveProjectConfigDocument', saveProjectConfigDocument, [[NOWHERE, raw, expected], [NOWHERE, raw, expected, {keepTrust: true}]]);
}
await recorder.write({prune: true});
