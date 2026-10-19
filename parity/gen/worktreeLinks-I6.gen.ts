/** Fixtures for src/worktreeLinks.ts (unit I6): applyWorktreeLinks as directory cases (launch checkout, new worktree and
 * extra sources all live under the case root). userSlug depends on the OS account (os.userInfo), so it is not recorded;
 * the pure helpers belong to P2. */
import fs from 'node:fs/promises';
import path from 'node:path';
import {installDeterminism, recordDirCase} from '../lib.ts';
import {applyWorktreeLinks, type TemplateVars, type WorktreeSettings} from '../../src/worktreeLinks.ts';

installDeterminism();
const MODULE = 'worktreeLinks-I6';
const put = async (file: string, data = 'x\n') => { await fs.mkdir(path.dirname(file), {recursive: true}); await fs.writeFile(file, data); };

interface Case {
	name: string;
	setup?: (root: string) => Promise<void>;
	settings?: WorktreeSettings | undefined | ((root: string) => WorktreeSettings);
	worktree?: string; // relative to root; default 'wt'
	launch?: string;
	noSettings?: boolean;
}
const L = 'launch', W = 'wt';
const base = async (root: string) => { await fs.mkdir(path.join(root, L), {recursive: true}); await fs.mkdir(path.join(root, W), {recursive: true}); };
const cases: Case[] = [
	{name: 'undefined-settings', noSettings: true},
	{name: 'empty-settings', settings: {}},
	{name: 'empty-lists', settings: {symlink: [], files: {}}},
	{name: 'symlink-dir', setup: r => put(path.join(r, L, 'node_modules/pkg/index.js')), settings: {symlink: ['node_modules']}},
	{name: 'symlink-file', setup: r => put(path.join(r, L, 'secret.txt')), settings: {symlink: ['secret.txt']}},
	{name: 'symlink-nested-creates-parents', setup: r => put(path.join(r, L, 'a/b/c/data.bin')), settings: {symlink: ['a/b/c/data.bin']}},
	{name: 'symlink-multiple-order', setup: async r => { await put(path.join(r, L, 'one')); await put(path.join(r, L, 'two')); await put(path.join(r, L, 'three')); }, settings: {symlink: ['two', 'one', 'three']}},
	{name: 'symlink-duplicate-entries', setup: r => put(path.join(r, L, 'one')), settings: {symlink: ['one', 'one']}},
	{name: 'symlink-source-missing', settings: {symlink: ['missing']}},
	{name: 'symlink-mixed-missing-and-present', setup: r => put(path.join(r, L, 'present')), settings: {symlink: ['missing', 'present', 'missing2']}},
	{name: 'symlink-unicode-names', setup: async r => { await put(path.join(r, L, 'jp/x.txt')); await put(path.join(r, L, 'plain.txt')); await fs.symlink('jp', path.join(r, L, '日本語')); await fs.symlink('plain.txt', path.join(r, L, 'é.txt')); await fs.symlink('plain.txt', path.join(r, L, 'データ😀.txt')); }, settings: {symlink: ['日本語/x.txt', 'é.txt', 'データ😀.txt']}}, // unicode only in symlink names (the scope check cannot read quoted non-ASCII committed paths)
	{name: 'symlink-spaces', setup: r => put(path.join(r, L, 'my dir/my file.txt')), settings: {symlink: ['my dir/my file.txt']}},
	{name: 'symlink-source-is-symlink-resolved', setup: async r => { await put(path.join(r, 'real/target.txt')); await fs.mkdir(path.join(r, L), {recursive: true}); await fs.symlink(path.join(r, 'real/target.txt'), path.join(r, L, 'link.txt')); }, settings: {symlink: ['link.txt']}},
	{name: 'symlink-source-relative-symlink', setup: async r => { await put(path.join(r, L, 'real/target.txt')); await fs.symlink('real', path.join(r, L, 'alias')); }, settings: {symlink: ['alias/target.txt']}},
	{name: 'symlink-dangling-source', setup: async r => { await fs.mkdir(path.join(r, L), {recursive: true}); await fs.symlink('gone', path.join(r, L, 'dangling')); }, settings: {symlink: ['dangling']}},
	{name: 'existing-file-kept', setup: async r => { await put(path.join(r, L, 'cfg'), 'launch\n'); await put(path.join(r, W, 'cfg'), 'worktree\n'); }, settings: {symlink: ['cfg']}},
	{name: 'existing-directory-kept', setup: async r => { await put(path.join(r, L, 'dir/f')); await put(path.join(r, W, 'dir/g')); }, settings: {symlink: ['dir']}},
	{name: 'existing-symlink-replaced', setup: async r => { await put(path.join(r, L, 'cfg'), 'new\n'); await put(path.join(r, 'old/cfg'), 'old\n'); await fs.symlink(path.join(r, 'old/cfg'), path.join(r, W, 'cfg')); }, settings: {symlink: ['cfg']}},
	{name: 'existing-symlink-same-target-replaced', setup: async r => { await put(path.join(r, L, 'cfg')); await fs.symlink(path.join(r, L, 'cfg'), path.join(r, W, 'cfg')); }, settings: {symlink: ['cfg']}},
	{name: 'existing-dangling-symlink-replaced', setup: async r => { await put(path.join(r, L, 'cfg')); await fs.symlink('nowhere', path.join(r, W, 'cfg')); }, settings: {symlink: ['cfg']}},
	{name: 'existing-symlink-to-dir-replaced', setup: async r => { await put(path.join(r, L, 'cache/a')); await fs.mkdir(path.join(r, 'other'), {recursive: true}); await fs.symlink(path.join(r, 'other'), path.join(r, W, 'cache')); }, settings: {symlink: ['cache']}},
	{name: 'source-is-destination-itself', setup: async r => { await fs.rm(path.join(r, W), {recursive: true}); await fs.symlink(path.join(r, L), path.join(r, W)); await put(path.join(r, L, 'x')); }, settings: {symlink: ['x']}},
	{name: 'source-inside-destination', setup: async r => { await put(path.join(r, L, 'a/b')); }, settings: {files: {a: 'a'}}},
	{name: 'destination-contains-source-via-worktree-is-launch', setup: async r => { await put(path.join(r, L, 'sub/x')); }, settings: {symlink: ['sub']}, worktree: L, launch: L},
	{name: 'parent-is-file', setup: async r => { await put(path.join(r, L, 'a/b')); await put(path.join(r, W, 'a'), 'file\n'); }, settings: {symlink: ['a/b']}},
	{name: 'parent-symlink-inside-worktree', setup: async r => { await put(path.join(r, L, 'a/b')); await fs.mkdir(path.join(r, W, 'real'), {recursive: true}); await fs.symlink('real', path.join(r, W, 'a')); }, settings: {symlink: ['a/b']}},
	{name: 'parent-symlink-escapes-worktree', setup: async r => { await put(path.join(r, L, 'a/b')); await fs.mkdir(path.join(r, 'outside'), {recursive: true}); await fs.symlink(path.join(r, 'outside'), path.join(r, W, 'a')); }, settings: {symlink: ['a/b']}},
	{name: 'parent-symlink-escapes-dangling', setup: async r => { await put(path.join(r, L, 'a/b')); await fs.symlink(path.join(r, 'nonexistent'), path.join(r, W, 'a')); }, settings: {symlink: ['a/b']}},
	{name: 'parent-deep-symlink-escape', setup: async r => { await put(path.join(r, L, 'a/b/c')); await fs.mkdir(path.join(r, W, 'a'), {recursive: true}); await fs.mkdir(path.join(r, 'outside'), {recursive: true}); await fs.symlink(path.join(r, 'outside'), path.join(r, W, 'a/b')); }, settings: {symlink: ['a/b/c']}},
	{name: 'invalid-destinations', setup: async r => { await put(path.join(r, L, 'ok')); }, settings: {symlink: ['', '..', '../x', '/abs', 'a/../b', './a', 'a//b', 'a/', '.git', '.git/config', 'a\\b', 'a\0b', 'ok']}},
	{name: 'invalid-destination-too-long', settings: {symlink: ['a'.repeat(4097)]}},
	{name: 'dot-git-prefixed-allowed', setup: r => put(path.join(r, L, '.github/workflows/ci.yml')), settings: {symlink: ['.github/workflows/ci.yml', '.gitignore']}},
	{name: 'dot-segment-hidden-file', setup: r => put(path.join(r, L, '.hidden/x')), settings: {symlink: ['.hidden/x']}},
	{name: 'files-absolute-source', setup: r => put(path.join(r, 'shared/cfg.json')), settings: r => ({files: {'config/cfg.json': path.join(r, 'shared/cfg.json')}})},
	{name: 'files-relative-source', setup: r => put(path.join(r, L, 'tmpl/cfg.json')), settings: {files: {'config/cfg.json': 'tmpl/cfg.json'}}},
	{name: 'files-relative-parent-source', setup: r => put(path.join(r, 'sibling/cfg.json')), settings: {files: {'cfg.json': '../sibling/cfg.json'}}},
	{name: 'files-template-repoRoot', setup: r => put(path.join(r, L, 'secrets/key')), settings: {files: {key: '{repoRoot}/secrets/key'}}},
	{name: 'files-template-repo-repoParent', setup: r => put(path.join(r, 'proj/key')), settings: {files: {key: '{repoParent}/{repo}/key'}}},
	{name: 'files-template-name', setup: r => put(path.join(r, 'by-name/feature x/cfg')), settings: {files: {cfg: '{repoParent}/by-name/{name}/cfg'}}},
	{name: 'files-template-home', setup: r => put(path.join(r, 'home/.rc')), settings: {files: {rc: '{home}/.rc'}}},
	{name: 'files-template-tilde-home', setup: r => put(path.join(r, 'home/.rc')), settings: {files: {rc: '~/.rc'}}},
	{name: 'files-template-tilde-alone', setup: async r => { await fs.mkdir(path.join(r, 'home'), {recursive: true}); }, settings: {files: {home: '~'}}},
	{name: 'files-unknown-placeholder', settings: {files: {x: '{nope}/x'}}},
	{name: 'files-unbalanced-braces', settings: {files: {x: '{name/x', y: 'a}b'}}},
	{name: 'files-empty-source', settings: {files: {x: ''}}},
	{name: 'files-source-missing', settings: {files: {x: 'missing/file'}}},
	{name: 'files-invalid-destination', setup: r => put(path.join(r, L, 'ok')), settings: {files: {'../x': 'ok', '/abs': 'ok', '.git/hooks/x': 'ok', good: 'ok'}}},
	{name: 'files-and-symlink-order', setup: async r => { await put(path.join(r, L, 'a')); await put(path.join(r, L, 'b')); }, settings: {symlink: ['a'], files: {c: 'b', d: 'a'}}},
	{name: 'files-and-symlink-same-destination', setup: async r => { await put(path.join(r, L, 'a')); await put(path.join(r, L, 'b')); }, settings: {symlink: ['a'], files: {a: 'b'}}},
	{name: 'files-unicode', setup: async r => { await put(path.join(r, L, 'data.json')); await fs.symlink('data.json', path.join(r, L, '😀.json')); }, settings: {files: {'設定/😀.json': '😀.json', 'é/ü.json': '😀.json'}}},
	{name: 'files-source-directory', setup: r => put(path.join(r, L, 'dir/x')), settings: {files: {copy: 'dir'}}},
	{name: 'files-source-is-root-dot', settings: {files: {self: '.'}}},
	{name: 'files-source-outside-everything', setup: r => put(path.join(r, 'elsewhere/f')), settings: {files: {f: '../elsewhere/f'}}},
	{name: 'worktree-missing', settings: {symlink: ['a']}, worktree: 'no-such-worktree'},
	{name: 'worktree-missing-no-settings-entries', settings: {}, worktree: 'no-such-worktree'},
	{name: 'worktree-undefined-settings-missing-worktree', noSettings: true, worktree: 'no-such-worktree'},
	{name: 'worktree-is-file', setup: r => put(path.join(r, 'wtfile'), 'x'), settings: {symlink: ['a']}, worktree: 'wtfile'},
	{name: 'worktree-given-as-symlink', setup: async r => { await put(path.join(r, L, 'a')); await fs.mkdir(path.join(r, 'real-wt'), {recursive: true}); await fs.symlink('real-wt', path.join(r, 'wt-link')); }, settings: {symlink: ['a']}, worktree: 'wt-link'},
	{name: 'worktree-unicode-name', setup: async r => { await put(path.join(r, L, 'a')); await fs.mkdir(path.join(r, 'ワーク ツリー'), {recursive: true}); }, settings: {symlink: ['a']}, worktree: 'ワーク ツリー'},
	{name: 'worktree-relative-launch-links', setup: r => put(path.join(r, L, 'a')), settings: {symlink: ['a']}},
	{name: 'many-entries-mixed', setup: async r => { for (const n of ['a', 'b', 'c']) await put(path.join(r, L, n)); await put(path.join(r, W, 'b'), 'keep\n'); }, settings: {symlink: ['a', 'b', 'c', 'd'], files: {'x/y': 'a', 'x/z': 'nope'}}},
	{name: 'link-through-created-parent-twice', setup: async r => { await put(path.join(r, L, 'a/one')); await put(path.join(r, L, 'a/two')); }, settings: {symlink: ['a/one', 'a/two']}},
	{name: 'symlink-source-empty-dir', setup: r => fs.mkdir(path.join(r, L, 'empty'), {recursive: true}), settings: {symlink: ['empty']}},
	{name: 'symlink-source-socket-like-fifo-free-file-mode', setup: async r => { await put(path.join(r, L, 'exe.sh'), '#!/bin/sh\n'); await fs.chmod(path.join(r, L, 'exe.sh'), 0o755); }, settings: {symlink: ['exe.sh']}},
	{name: 'launch-is-worktree-parent', setup: async r => { await put(path.join(r, 'wt/inner/f')); }, settings: {symlink: ['wt/inner']}, launch: '.'},
];

for (const c of cases) {
	await recordDirCase(MODULE, `apply-${c.name}`, {
		setup: async root => { await base(root); await fs.mkdir(path.join(root, 'home'), {recursive: true}); await c.setup?.(root); },
		fn: 'applyWorktreeLinks',
		args: root => {
			const launchRoot = path.join(root, c.launch ?? L), worktreeRoot = path.join(root, c.worktree ?? W);
			const vars: TemplateVars = {name: 'feature x', repo: 'proj', repoParent: root, repoRoot: launchRoot, home: path.join(root, 'home')};
			const settings = c.noSettings ? undefined : typeof c.settings === 'function' ? c.settings(root) : c.settings;
			return [settings, {launchRoot, worktreeRoot, vars}];
		},
		run: applyWorktreeLinks,
	});
}
