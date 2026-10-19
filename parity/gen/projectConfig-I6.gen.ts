/** Fixtures for src/projectConfig.ts (unit I6): readBoundedUtf8, loadProjectConfig, savedProjectTrust. Directory cases only
 * (they need real files); the repo context is passed explicitly so no git runs. */
import fs from 'node:fs/promises';
import path from 'node:path';
import {installDeterminism, recordDirCase} from '../lib.ts';
import {loadProjectConfig, projectFingerprint, readBoundedUtf8, savedProjectTrust, sha256} from '../../src/projectConfig.ts';

installDeterminism();
const MODULE = 'projectConfig-I6';
const HOOK = '.claude/scripts/create-worktree.sh';
const write = async (file: string, data: string | Uint8Array, mode?: number) => {
	await fs.mkdir(path.dirname(file), {recursive: true});
	await fs.writeFile(file, data);
	if (mode !== undefined) await fs.chmod(file, mode);
};

// readBoundedUtf8 ---------------------------------------------------------------------------------------------
type Read = {name: string; setup: (root: string) => Promise<void>; opts: (root: string) => {noFollow?: boolean; max: number} | Record<string, unknown>; file?: string};
const reads: Read[] = [
	{name: 'plain', setup: r => write(path.join(r, 'a.json'), '{"x":1}\n'), opts: () => ({max: 1024})},
	{name: 'empty', setup: r => write(path.join(r, 'a.json'), ''), opts: () => ({max: 1024})},
	{name: 'bom-kept', setup: r => write(path.join(r, 'a.json'), '﻿{"x":1}'), opts: () => ({max: 1024})},
	{name: 'unicode', setup: r => write(path.join(r, 'a.json'), '{"名前":"日本語 😀 é é \u{1F468}‍\u{1F469}"}\n'), opts: () => ({max: 1024})},
	{name: 'invalid-utf8', setup: r => write(path.join(r, 'a.json'), Buffer.from([0x7b, 0xff, 0xfe, 0x7d])), opts: () => ({max: 1024})},
	{name: 'truncated-multibyte', setup: r => write(path.join(r, 'a.json'), Buffer.from([0x61, 0xe2, 0x82])), opts: () => ({max: 1024})},
	{name: 'surrogate-bytes', setup: r => write(path.join(r, 'a.json'), Buffer.from([0xed, 0xa0, 0x80])), opts: () => ({max: 1024})},
	{name: 'exact-max', setup: r => write(path.join(r, 'a.json'), 'x'.repeat(1024)), opts: () => ({max: 1024})},
	{name: 'one-over-max', setup: r => write(path.join(r, 'a.json'), 'x'.repeat(1025)), opts: () => ({max: 1024})},
	{name: 'over-max-kib-rounding', setup: r => write(path.join(r, 'a.json'), 'x'.repeat(1600)), opts: () => ({max: 1500})},
	{name: 'over-max-small', setup: r => write(path.join(r, 'a.json'), 'xyz'), opts: () => ({max: 2})},
	{name: 'max-zero-empty', setup: r => write(path.join(r, 'a.json'), ''), opts: () => ({max: 0})},
	{name: 'max-zero-nonempty', setup: r => write(path.join(r, 'a.json'), 'a'), opts: () => ({max: 0})},
	{name: 'mode-0600', setup: r => write(path.join(r, 'a.json'), '{}', 0o600), opts: () => ({max: 1024})},
	{name: 'mode-0755', setup: r => write(path.join(r, 'a.json'), '{}', 0o755), opts: () => ({max: 1024})},
	{name: 'mode-0444', setup: r => write(path.join(r, 'a.json'), '{}', 0o444), opts: () => ({max: 1024})},
	{name: 'missing', setup: async () => {}, opts: () => ({max: 1024}), file: 'nope.json'},
	{name: 'missing-nofollow', setup: async () => {}, opts: () => ({max: 1024, noFollow: true}), file: 'nope.json'},
	{name: 'directory', setup: r => fs.mkdir(path.join(r, 'a.json')), opts: () => ({max: 1024})},
	{name: 'directory-nofollow', setup: r => fs.mkdir(path.join(r, 'a.json')), opts: () => ({max: 1024, noFollow: true})},
	{name: 'symlink-followed', setup: async r => { await write(path.join(r, 'real.json'), '{"y":2}'); await fs.symlink('real.json', path.join(r, 'a.json')); }, opts: () => ({max: 1024})},
	{name: 'symlink-followed-explicit-false', setup: async r => { await write(path.join(r, 'real.json'), '{"y":2}'); await fs.symlink('real.json', path.join(r, 'a.json')); }, opts: () => ({max: 1024, noFollow: false})},
	{name: 'symlink-nofollow', setup: async r => { await write(path.join(r, 'real.json'), '{"y":2}'); await fs.symlink('real.json', path.join(r, 'a.json')); }, opts: () => ({max: 1024, noFollow: true})},
	{name: 'symlink-dangling-nofollow', setup: r => fs.symlink('gone.json', path.join(r, 'a.json')), opts: () => ({max: 1024, noFollow: true})},
	{name: 'symlink-dangling-followed', setup: r => fs.symlink('gone.json', path.join(r, 'a.json')), opts: () => ({max: 1024})},
	{name: 'unicode-name-too-big', setup: async r => { await write(path.join(r, 'big.bin'), 'x'.repeat(3000)); await fs.symlink('big.bin', path.join(r, '设置😀.json')); }, opts: () => ({max: 2048}), file: '设置😀.json'},
	{name: 'unicode-name-directory', setup: r => fs.mkdir(path.join(r, 'é.json')), opts: () => ({max: 2048}), file: 'é.json'},
	{name: 'big-multibyte-fits', setup: r => write(path.join(r, 'a.json'), '😀'.repeat(256)), opts: () => ({max: 1024})},
	{name: 'big-multibyte-over', setup: r => write(path.join(r, 'a.json'), '😀'.repeat(257)), opts: () => ({max: 1024})},
	{name: 'newlines-crlf', setup: r => write(path.join(r, 'a.json'), '{\r\n"a": 1\r\n}\r\n'), opts: () => ({max: 1024})},
	{name: 'nul-bytes', setup: r => write(path.join(r, 'a.json'), 'a\0b'), opts: () => ({max: 1024})},
];
for (const c of reads) {
	await recordDirCase(MODULE, `read-${c.name}`, {setup: c.setup, fn: 'readBoundedUtf8', args: r => [path.join(r, c.file ?? 'a.json'), c.opts(r)], run: readBoundedUtf8});
}

// loadProjectConfig -------------------------------------------------------------------------------------------
interface Load {name: string; setup?: (root: string) => Promise<void>; user?: unknown; bare?: boolean; linked?: boolean; noUser?: boolean}
const cfg = (value: unknown) => JSON.stringify(value, null, 2) + '\n';
const withHook = async (root: string, where = 'main', body = '#!/bin/sh\necho hook\n') => write(path.join(root, where, HOOK), body);
const loads: Load[] = [
	{name: 'no-config'},
	{name: 'no-config-no-user', noUser: true},
	{name: 'valid-full', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({defaultAgent: 'codex', defaultWorkspace: 'existing', devCommand: 'npm run dev', setupCommand: 'npm i', actions: {Test: 'npm test', 'build 1.x': 'make'}, worktree: {location: '~/wt/{name}', symlink: ['node_modules', 'a/b'], files: {'.secrets': '{repoRoot}/.secrets'}, branchFrom: 'origin', branchName: 'u/{name}', hook: true}}))},
	{name: 'valid-empty-object', setup: r => write(path.join(r, 'main/deckhand.json'), '{}')},
	{name: 'valid-bom', setup: r => write(path.join(r, 'main/deckhand.json'), '﻿{"defaultAgent":"pi"}')},
	{name: 'valid-unicode-command', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({devCommand: 'echo 日本語 😀 é', actions: {'Ünï': 'x'}}))},
	{name: 'raw-whitespace-preserved', setup: r => write(path.join(r, 'main/deckhand.json'), '  {\n\t"devCommand" :   "a"\n}\n\n')},
	{name: 'invalid-empty-file', setup: r => write(path.join(r, 'main/deckhand.json'), '')},
	{name: 'invalid-not-object-array', setup: r => write(path.join(r, 'main/deckhand.json'), '[]')},
	{name: 'invalid-not-object-null', setup: r => write(path.join(r, 'main/deckhand.json'), 'null')},
	{name: 'invalid-not-object-number', setup: r => write(path.join(r, 'main/deckhand.json'), '42')},
	{name: 'invalid-unknown-key', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({nope: 1}))},
	{name: 'invalid-agent', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({defaultAgent: 'gpt'}))},
	{name: 'invalid-agent-type', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({defaultAgent: 3}))},
	{name: 'invalid-workspace', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({defaultWorkspace: 'maybe'}))},
	{name: 'invalid-command-blank', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({devCommand: '   '}))},
	{name: 'invalid-command-nul', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({setupCommand: 'a\u0000b'}))},
	{name: 'invalid-command-long', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({setupCommand: 'a'.repeat(8193)}))},
	{name: 'valid-command-8192', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({setupCommand: 'a'.repeat(8192)}))},
	{name: 'invalid-actions-array', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: []}))},
	{name: 'invalid-actions-null', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: null}))},
	{name: 'invalid-action-name', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: {'-bad': 'x'}}))},
	{name: 'invalid-action-name-proto', setup: r => write(path.join(r, 'main/deckhand.json'), '{"actions":{"constructor":"x"}}')},
	{name: 'invalid-action-name-long', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: {['a'.repeat(49)]: 'x'}}))},
	{name: 'valid-action-name-48', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: {['a'.repeat(48)]: 'x'}}))},
	{name: 'invalid-action-command', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: {Go: ''}}))},
	{name: 'invalid-too-many-actions', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: Object.fromEntries(Array.from({length: 31}, (_, i) => [`a${i}`, 'x']))}))},
	{name: 'valid-30-actions', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({actions: Object.fromEntries(Array.from({length: 30}, (_, i) => [`a${i}`, 'x']))}))},
	{name: 'invalid-worktree-type', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({worktree: 'x'}))},
	{name: 'invalid-worktree-key', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({worktree: {foo: 1}}))},
	{name: 'invalid-worktree-location', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({worktree: {location: 'rel/{name}'}}))},
	{name: 'invalid-worktree-symlink', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({worktree: {symlink: ['../x']}}))},
	{name: 'invalid-worktree-branch', setup: r => write(path.join(r, 'main/deckhand.json'), cfg({worktree: {branchName: 'a b/{name}'}}))},
	{name: 'oversized', setup: r => write(path.join(r, 'main/deckhand.json'), ' '.repeat(65 * 1024))},
	{name: 'exactly-max', setup: r => write(path.join(r, 'main/deckhand.json'), '{}' + ' '.repeat(64 * 1024 - 2))},
	{name: 'invalid-utf8', setup: r => write(path.join(r, 'main/deckhand.json'), Buffer.from([0x7b, 0xff, 0x7d]))},
	{name: 'config-is-directory', setup: r => fs.mkdir(path.join(r, 'main/deckhand.json'), {recursive: true})},
	{name: 'config-is-symlink', setup: async r => { await write(path.join(r, 'main/real.json'), '{}'); await fs.symlink('real.json', path.join(r, 'main/deckhand.json')); }},
	{name: 'config-dangling-symlink', setup: async r => { await fs.mkdir(path.join(r, 'main'), {recursive: true}); await fs.symlink('gone.json', path.join(r, 'main/deckhand.json')); }},
	{name: 'worktree-copy-ignored', linked: true, setup: async r => { await write(path.join(r, 'wt/deckhand.json'), cfg({devCommand: 'worktree'})); await write(path.join(r, 'main/deckhand.json'), cfg({devCommand: 'main'})); }},
	{name: 'worktree-copy-only', linked: true, setup: r => write(path.join(r, 'wt/deckhand.json'), cfg({devCommand: 'worktree'}))},
	{name: 'bare-repository', bare: true, setup: r => write(path.join(r, 'wt/deckhand.json'), cfg({devCommand: 'ignored'}))},
	{name: 'bare-repository-with-hook', bare: true, setup: r => withHook(r, 'wt')},
	{name: 'hook-in-main', setup: r => withHook(r)},
	{name: 'hook-in-main-with-config', setup: async r => { await withHook(r); await write(path.join(r, 'main/deckhand.json'), cfg({defaultAgent: 'claude'})); }},
	{name: 'hook-in-worktree', linked: true, setup: r => withHook(r, 'wt')},
	{name: 'hook-in-both-worktree-wins', linked: true, setup: async r => { await withHook(r, 'wt', 'wt\n'); await withHook(r, 'main', 'main\n'); }},
	{name: 'hook-only-main-from-worktree', linked: true, setup: r => withHook(r, 'main', 'main-only\n')},
	{name: 'hook-unicode-bom-content', setup: r => withHook(r, 'main', '﻿#!/bin/sh\necho "日本語 😀"\n')},
	{name: 'hook-empty', setup: r => withHook(r, 'main', '')},
	{name: 'hook-invalid-utf8', setup: r => write(path.join(r, 'main', HOOK), Buffer.from([0xc3, 0x28]))},
	{name: 'hook-oversized', setup: r => withHook(r, 'main', 'x'.repeat(65 * 1024))},
	{name: 'hook-is-directory', setup: r => fs.mkdir(path.join(r, 'main', HOOK), {recursive: true})},
	{name: 'hook-oversized-but-disabled-by-repo', setup: async r => { await withHook(r, 'main', 'x'.repeat(65 * 1024)); await write(path.join(r, 'main/deckhand.json'), cfg({worktree: {hook: false}})); }},
	{name: 'hook-disabled-by-repository', setup: async r => { await withHook(r); await write(path.join(r, 'main/deckhand.json'), cfg({worktree: {hook: false}})); }},
	{name: 'hook-disabled-by-global', user: {defaults: {worktree: {hook: false}}}, setup: r => withHook(r)},
	{name: 'hook-global-false-repo-true', user: {defaults: {worktree: {hook: false}}}, setup: async r => { await withHook(r); await write(path.join(r, 'main/deckhand.json'), cfg({worktree: {hook: true}})); }},
	{name: 'hook-global-false-repo-false', user: {defaults: {worktree: {hook: false}}}, setup: async r => { await withHook(r); await write(path.join(r, 'main/deckhand.json'), cfg({worktree: {hook: false}})); }},
	{name: 'hook-global-true', user: {defaults: {worktree: {hook: true}}}, setup: r => withHook(r)},
	{name: 'hook-global-defaults-garbage', user: {defaults: 'garbage'}, setup: r => withHook(r)},
	{name: 'hook-global-defaults-null', user: {defaults: null}, setup: r => withHook(r)},
	{name: 'hook-global-defaults-array', user: {defaults: []}, setup: r => withHook(r)},
	{name: 'hook-global-worktree-string', user: {defaults: {worktree: 'x'}}, setup: r => withHook(r)},
	{name: 'user-ignored-without-hook', user: {defaults: {worktree: {hook: false}}, dev_command: 'x'}},
	{name: 'bad-config-and-hook', setup: async r => { await withHook(r); await write(path.join(r, 'main/deckhand.json'), cfg({bogus: true})); }},
	{name: 'unicode-root', setup: async r => { await write(path.join(r, 'proj/main/deckhand.json'), cfg({devCommand: 'x'})); await fs.symlink('proj', path.join(r, 'プロジェクト 😀')); }}, // unicode only in a symlink name: the scope check cannot read quoted non-ASCII committed paths
];
for (const c of loads) {
	const dirs = c.name === 'unicode-root' ? ['プロジェクト 😀/main', 'プロジェクト 😀/wt'] : ['main', 'wt'];
	await recordDirCase(MODULE, `load-${c.name}`, {
		setup: async root => { await fs.mkdir(path.join(root, 'main'), {recursive: true}); await fs.mkdir(path.join(root, 'wt'), {recursive: true}); await c.setup?.(root); },
		fn: 'loadProjectConfig',
		args: root => {
			const [main, wt] = dirs.map(d => path.join(root, d)) as [string, string];
			const context = c.bare ? {root: wt, trustRoot: wt} : c.linked ? {root: wt, mainRoot: main, trustRoot: main} : {root: main, mainRoot: main, trustRoot: main};
			return c.noUser ? [main, undefined, context] : [main, c.user ?? {}, context];
		},
		run: loadProjectConfig,
	});
}

// savedProjectTrust -------------------------------------------------------------------------------------------
const HOOK_BODY = '#!/bin/sh\necho hook\n';
const prev = (raw: string, exists = true) => ({raw, exists});
interface Saved {name: string; hook?: boolean; previous: {raw: string; exists: boolean}; next: string; trust?: 'previous' | 'previous-with-hook' | 'next'; user?: unknown; hookDir?: 'main' | 'wt'; hookBody?: string}
const A = cfg({defaultAgent: 'claude'}), B = cfg({defaultAgent: 'pi', devCommand: 'x'}), ON = cfg({worktree: {hook: true}}), OFF = cfg({worktree: {hook: false}});
const saves: Saved[] = [
	{name: 'exists-trusted-no-hook', previous: prev(A), next: B, trust: 'previous'},
	{name: 'exists-untrusted-no-hook', previous: prev(A), next: B},
	{name: 'exists-trusted-other-fingerprint', previous: prev(A), next: B, trust: 'next'},
	{name: 'new-no-hook', previous: prev('', false), next: B},
	{name: 'new-no-hook-garbage-raw', previous: prev('nonsense', false), next: B},
	{name: 'new-no-hook-next-invalid', previous: prev('', false), next: '{"bad":1}'},
	{name: 'exists-next-invalid', previous: prev(A), next: '{"defaultAgent":"x"}', trust: 'previous'},
	{name: 'exists-next-empty-throws', previous: prev(A), next: '', trust: 'previous'},
	{name: 'previous-invalid-json', previous: prev('{oops'), next: B, trust: 'previous'},
	{name: 'previous-invalid-schema', previous: prev('{"bad":1}'), next: B, trust: 'previous'},
	{name: 'previous-invalid-and-next-invalid', previous: prev('{"bad":1}'), next: '{"bad":2}'},
	{name: 'previous-empty-exists', previous: prev(''), next: B},
	{name: 'hook-new-file-hook-untrusted', hook: true, previous: prev('', false), next: B},
	{name: 'hook-new-file-hook-trusted-alone', hook: true, previous: prev('', false), next: B, trust: 'previous-with-hook'},
	{name: 'hook-new-file-hook-trusted-alone-enable-explicit', hook: true, previous: prev('', false), next: ON, trust: 'previous-with-hook'},
	{name: 'hook-new-file-next-disables-hook', hook: true, previous: prev('', false), next: OFF, trust: 'previous-with-hook'},
	{name: 'hook-new-file-global-disabled', hook: true, user: {defaults: {worktree: {hook: false}}}, previous: prev('', false), next: B},
	{name: 'hook-new-file-global-disabled-next-enables', hook: true, user: {defaults: {worktree: {hook: false}}}, previous: prev('', false), next: ON},
	{name: 'hook-exists-trusted-with-hook', hook: true, previous: prev(A), next: B, trust: 'previous-with-hook'},
	{name: 'hook-exists-trusted-without-hook-only', hook: true, previous: prev(A), next: B, trust: 'previous'},
	{name: 'hook-exists-untrusted', hook: true, previous: prev(A), next: B},
	{name: 'hook-exists-next-disables', hook: true, previous: prev(A), next: OFF, trust: 'previous-with-hook'},
	{name: 'hook-exists-prev-disabled-next-enables', hook: true, previous: prev(OFF), next: ON, trust: 'previous'},
	{name: 'hook-exists-prev-disabled-next-enables-default', hook: true, previous: prev(OFF), next: A, trust: 'previous'},
	{name: 'hook-exists-prev-disabled-next-stays-disabled', hook: true, previous: prev(OFF), next: cfg({worktree: {hook: false}, devCommand: 'a'}), trust: 'previous'},
	{name: 'hook-exists-prev-enabled-next-enabled', hook: true, previous: prev(ON), next: A, trust: 'previous-with-hook'},
	{name: 'hook-exists-global-disabled-trusted-without', hook: true, user: {defaults: {worktree: {hook: false}}}, previous: prev(A), next: B, trust: 'previous'},
	{name: 'hook-exists-global-disabled-next-repo-enables', hook: true, user: {defaults: {worktree: {hook: false}}}, previous: prev(A), next: ON, trust: 'previous'},
	{name: 'hook-exists-hook-in-worktree', hook: true, hookDir: 'wt', previous: prev(A), next: B, trust: 'previous-with-hook'},
	{name: 'hook-oversized-throws', hook: true, hookBody: 'x'.repeat(65 * 1024), previous: prev(A), next: B},
	{name: 'hook-unicode', hook: true, hookBody: '#!/bin/sh\necho 日本語 😀\n', previous: prev(A), next: B, trust: 'previous-with-hook'},
	{name: 'unicode-raw', previous: prev(cfg({devCommand: 'echo 日本語 😀'})), next: cfg({devCommand: 'echo 日本語 😀 é'}), trust: 'previous'},
];
for (const c of saves) {
	await recordDirCase(MODULE, `saved-trust-${c.name}`, {
		setup: async root => {
			await fs.mkdir(path.join(root, 'main'), {recursive: true});
			await fs.mkdir(path.join(root, 'wt'), {recursive: true});
			if (c.hook) await write(path.join(root, c.hookDir ?? 'main', HOOK), c.hookBody ?? HOOK_BODY);
		},
		fn: 'savedProjectTrust',
		args: root => {
			const main = path.join(root, 'main'), wt = path.join(root, 'wt');
			const TRUST_ROOT = '/trust/root 日本語 😀'; // a fixed key: object keys are not scrubbed of the temp root
			const context = c.hookDir === 'wt' ? {root: wt, mainRoot: main, trustRoot: TRUST_ROOT} : {root: main, mainRoot: main, trustRoot: TRUST_ROOT};
			const hookFingerprint = sha256(c.hookBody ?? HOOK_BODY);
			const list: string[] = ['0'.repeat(64)];
			if (c.trust === 'previous') list.unshift(projectFingerprint(c.previous.exists ? c.previous.raw : '', undefined));
			if (c.trust === 'previous-with-hook') list.unshift(projectFingerprint(c.previous.exists ? c.previous.raw : '', hookFingerprint));
			if (c.trust === 'next') list.unshift(projectFingerprint(c.next, undefined));
			const user = {...(c.user as object | undefined), ...(c.trust ? {trustedProjects: {[TRUST_ROOT]: list, '/other/root': ['1'.repeat(64)]}} : {})};
			return [context, c.previous, c.next, user];
		},
		run: savedProjectTrust,
	});
}
