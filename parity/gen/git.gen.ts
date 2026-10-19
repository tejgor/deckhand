/**
 * Gen script for src/git.ts. Regenerate with `node --import tsx parity/gen/git.gen.ts`.
 * Records only the pure / file-system exports (sanitizeWorktreeName, operationProblem, resolveCreateScript);
 * everything else spawns git and is covered by the ported tests/merge.test.ts.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, arb, installDeterminism, recordDirCase, sample} from '../lib.ts';
import {operationProblem, resolveCreateScript, sanitizeWorktreeName} from '../../src/git.ts';

const MODULE = 'git';
installDeterminism();
const recorder = new Recorder(MODULE);

const titles = [
	'', ' ', 'Fix login bug', 'fix/login Bug', '///a///b///', '__a__', '---', '-_-a-_-', 'UPPER_case-Mix', 'a b  c',
	'日本語のタイトル', 'emoji 😀 title', 'café Ü', 'áb', 'tab\tnew\nline', '!!!', 'a/../b', 'a//b', 'x'.repeat(96), 'x'.repeat(97),
	'x'.repeat(95) + '-y', 'x'.repeat(95) + '/y', 'x'.repeat(94) + '__z', 'a'.repeat(200), 'ab '.repeat(60), '/' + 'q'.repeat(100),
	'\uD83D', 'a\uD83Db', 'İstanbul', 'ß', 'feature: add (thing) [wip]',
];
for (const title of titles) recorder.call('sanitizeWorktreeName', sanitizeWorktreeName, [title]);
for (const title of sample(arb.text(), 150)) recorder.call('sanitizeWorktreeName', sanitizeWorktreeName, [title]);
for (const title of sample(arb.text(), 40, 11)) recorder.call('sanitizeWorktreeName', sanitizeWorktreeName, [title.repeat(30)]);

for (const operation of ['merge', 'rebase', 'cherry-pick', 'revert', 'conflicts']) recorder.call('operationProblem', operationProblem, [operation]);

await recorder.write({prune: true});

// resolveCreateScript: current root first, then main root; only regular files count.
const script = path.join('.claude', 'scripts', 'create-worktree.sh');
const put = async (root: string, rel: string, text = '#!/bin/bash\n') => {
	await fs.mkdir(path.dirname(path.join(root, rel)), {recursive: true});
	await fs.writeFile(path.join(root, rel), text);
};
const both = (r: string) => [`${r}/cur`, `${r}/main`];
const cases: Array<[string, (root: string) => Promise<void>, (root: string) => unknown[]]> = [
	['resolve-script-in-current', async r => { await put(r, `cur/${script}`); }, both],
	['resolve-script-in-main-only', async r => { await put(r, `main/${script}`); await fs.mkdir(`${r}/cur`); }, both],
	['resolve-script-prefers-current', async r => { await put(r, `cur/${script}`); await put(r, `main/${script}`, 'echo main\n'); }, both],
	['resolve-script-none', async r => { await fs.mkdir(`${r}/cur`); await fs.mkdir(`${r}/main`); }, both],
	['resolve-script-no-main', async r => { await put(r, `cur/${script}`); }, r => [`${r}/cur`]],
	['resolve-script-same-root', async r => { await put(r, `cur/${script}`); }, r => [`${r}/cur`, `${r}/cur/../cur`]],
	['resolve-script-is-directory', async r => { await fs.mkdir(`${r}/cur/${script}`, {recursive: true}); await put(r, `main/${script}`); }, both],
	['resolve-script-missing-roots', async r => { await fs.mkdir(r, {recursive: true}); }, r => [`${r}/nope`, `${r}/nada`]],
	['resolve-script-symlink', async r => { await put(r, 'real.sh'); await fs.mkdir(`${r}/cur/.claude/scripts`, {recursive: true}); await fs.symlink(`${r}/real.sh`, `${r}/cur/${script}`); }, r => [`${r}/cur`]],
	// Non-ASCII stays in the arguments only: committed fixture paths must be ASCII (git C-quotes others in the
	// verify-unit scope check). A script found under a non-ASCII directory is a runtime test in crates/sys/src/git.rs.
	['resolve-script-unicode-missing-current', async r => { await put(r, `main/${script}`); }, r => [`${r}/日本 😀`, `${r}/main`]],
	['resolve-script-special-dir', async r => { await put(r, `a b & 'c' $d (e) #f/${script}`); }, r => [`${r}/a b & 'c' $d (e) #f`]],
];
for (const [name, setup, args] of cases) await recordDirCase(MODULE, name, {setup, fn: 'resolveCreateScript', args, run: resolveCreateScript});
