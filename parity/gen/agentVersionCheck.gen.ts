/** Regenerate with: node --import tsx parity/gen/agentVersionCheck.gen.ts  (writes fixtures/parity/agentVersionCheck/). */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, installDeterminism, recordDirCase} from '../lib.ts';
import {findOnPath, npmLatest, runBounded} from '../../src/agentVersionCheck.ts';

// AgentVersionChecker (class methods) is not wrapped by the tap and needs real binaries; it is covered by ported
// tests. npmLatest is recorded only through the DECKHAND_AGENT_LATEST override (the real path runs npm).
installDeterminism();
const MODULE = 'agentVersionCheck';
const recorder = new Recorder(MODULE);

// args: [DECKHAND_AGENT_LATEST value, program, npmPackage]
const OVERRIDE_KEY = 'DECKHAND_AGENT_LATEST';
async function npmLatestWithOverride(override: string, program: string, npmPackage: string) {
	const previous = process['env'][OVERRIDE_KEY];
	process['env'][OVERRIDE_KEY] = override;
	try { return await npmLatest(program as never, npmPackage); } finally { if (previous === undefined) delete process['env'][OVERRIDE_KEY]; else process['env'][OVERRIDE_KEY] = previous; }
}
const overrides = [
	'{"claude":"1.2.3"}', '{"claude":"v2.0.10"}', '{"claude":"Claude Code 3.4.5 (build)"}', '{"codex":"0.1.0"}', '', '{}', '{"claude":"nope"}',
	'{"claude":42}', '{"claude":null}', '{"claude":""}', 'not json', '[]', 'null', '{"claude":"1.2"}', '{"claude":"01.002.0003"}',
	'{"claude":"1.2.3-beta.1"}', '{"claude":"9999999999.0.0"}', '{"pi":"0.5.9","claude":"1.0.0"}', '{"claude":"版本 1.2.3 😀"}', '{"claude":"1.2.3.4"}',
];
const programs = ['claude', 'codex', 'pi'];
for (const override of overrides) for (const program of programs) {
	await recorder.callAll('npmLatest', npmLatestWithOverride, [[override, program, `@x/${program}`]]);
}

const sh = '/bin/sh';
const runs: [string, string[], {timeout: number; limit: number}][] = [
	[sh, ['-c', 'printf hello'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'printf 0123456789abcdef'], {timeout: 5000, limit: 5}],
	[sh, ['-c', 'printf abc'], {timeout: 5000, limit: 0}],
	[sh, ['-c', 'printf abc'], {timeout: 5000, limit: 3}],
	[sh, ['-c', 'printf abc'], {timeout: 5000, limit: 2}],
	[sh, ['-c', 'exit 3'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'printf out; exit 1'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'printf "line1\\nline2\\n"'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'printf "\\344\\270\\255\\360\\237\\230\\200"'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'cat; printf done'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'echo err 1>&2; exit 2'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'sleep 30'], {timeout: 150, limit: 100}],
	[sh, ['-c', 'printf before; sleep 30'], {timeout: 150, limit: 100}],
	[sh, ['-c', 'kill -9 $$'], {timeout: 5000, limit: 100}],
	[sh, ['-c', 'printf "$0 $1"', 'zero', 'one'], {timeout: 5000, limit: 100}],
	['/no/such/binary', [], {timeout: 5000, limit: 100}],
	['/no/such/binary', ['--version'], {timeout: 5000, limit: 100}],
	['/', [], {timeout: 5000, limit: 100}],
];
await recorder.callAll('runBounded', runBounded, runs);

await recorder.write({prune: true});

// findOnPath: first executable regular file on a PATH value.
const delimiter = path.delimiter;
const exe = async (root: string, rel: string, mode = 0o755, text = '#!/bin/sh\n') => {
	await fs.mkdir(path.dirname(path.join(root, rel)), {recursive: true});
	await fs.writeFile(path.join(root, rel), text, {mode});
	await fs.chmod(path.join(root, rel), mode);
};
const cases: {name: string; setup: (root: string) => Promise<void>; args: (root: string) => unknown[]}[] = [
	{name: 'find-first-of-two', setup: async r => { await exe(r, 'a/tool'); await exe(r, 'b/tool'); }, args: r => ['tool', `${r}/a${delimiter}${r}/b`]},
	{name: 'find-skips-non-executable', setup: async r => { await exe(r, 'a/tool', 0o644); await exe(r, 'b/tool'); }, args: r => ['tool', `${r}/a${delimiter}${r}/b`]},
	{name: 'find-skips-directory', setup: async r => { await fs.mkdir(path.join(r, 'a/tool'), {recursive: true}); await exe(r, 'b/tool'); }, args: r => ['tool', `${r}/a${delimiter}${r}/b`]},
	{name: 'find-not-found', setup: async r => { await exe(r, 'a/other'); }, args: r => ['tool', `${r}/a${delimiter}${r}/missing`]},
	{name: 'find-empty-path-entries', setup: async r => { await exe(r, 'b/tool'); }, args: r => ['tool', `${delimiter}${delimiter}${r}/b${delimiter}`]},
	{name: 'find-empty-path', setup: async r => { await exe(r, 'a/tool'); }, args: () => ['tool', '']},
	{name: 'find-relative-entry', setup: async () => {}, args: () => ['tool', 'relative/dir']},
	{name: 'find-symlink-to-executable', setup: async r => { await exe(r, 'real/tool'); await fs.mkdir(path.join(r, 'a')); await fs.symlink(path.join(r, 'real/tool'), path.join(r, 'a/tool')); }, args: r => ['tool', `${r}/a`]},
	{name: 'find-broken-symlink', setup: async r => { await fs.mkdir(path.join(r, 'a')); await fs.symlink(path.join(r, 'gone'), path.join(r, 'a/tool')); await exe(r, 'b/tool'); }, args: r => ['tool', `${r}/a${delimiter}${r}/b`]},
	{name: 'find-unicode-name', setup: async r => { await exe(r, 'a/工具😀'); }, args: r => ['工具😀', `${r}/a`]},
	{name: 'find-nested-program', setup: async r => { await exe(r, 'a/sub/tool'); }, args: r => ['sub/tool', `${r}/a`]},
	{name: 'find-trailing-slash-entry', setup: async r => { await exe(r, 'a/tool'); }, args: r => ['tool', `${r}/a/`]},
];
for (const {name, setup, args} of cases) await recordDirCase(MODULE, name, {setup, fn: 'findOnPath', args, run: findOnPath});
