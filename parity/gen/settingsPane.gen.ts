/**
 * Parity fixtures for src/settingsPane.tsx (fixtures/parity/settingsPane/). Regenerate with
 *
 *   node --import tsx parity/gen/settingsPane.gen.ts
 *
 * The module exports only the SettingsPane component, so the fixture records whole renders: props and a size in,
 * the screen out (Ink's renderToString at that width, with colors on, as styled lines). A styled line is its text
 * with a `⟦style⟧` marker wherever the style changes: the foreground color name plus `bold` `dim` `italic`
 * `inverse` ("" = plain). A space shows only its inverse (otherwise it continues the style before it) and trailing
 * spaces without inverse are cut; the Rust side normalizes its buffer the same way. Infos come from the recorded settingsModel inputs.
 */
import fs from 'node:fs';
import path from 'node:path';
import React from 'react';
import {REPO_ROOT, Recorder, installDeterminism, prng} from '../lib.ts';

// Chalk reads the color level when it loads, so ink and the sources are imported after this.
process.env.FORCE_COLOR = '3';
delete process.env.NO_COLOR;
const {renderToString} = await import('ink');
const {SettingsPane} = await import('../../src/settingsPane.tsx');
const {SETTINGS} = await import('../../src/settingsModel.ts');

installDeterminism();
const recorder = new Recorder('settingsPane', {cap: 1000});

// ANSI -> styled lines ---------------------------------------------------------------------------------------------

const FOREGROUND: Record<number, string> = {30: 'black', 31: 'red', 32: 'green', 33: 'yellow', 34: 'blue', 35: 'magenta', 36: 'cyan', 37: 'white', 90: 'gray', 91: 'redBright', 92: 'greenBright', 93: 'yellowBright', 94: 'blueBright', 95: 'magentaBright', 96: 'cyanBright', 97: 'whiteBright'};
interface Cell {char: string; style: string}
function styledLine(line: string): string {
	let fg = '', bold = false, dim = false, italic = false, inverse = false;
	const cells: Cell[] = [];
	const push = (text: string) => {
		for (const char of text) {
			// A space shows only its inverse; otherwise it continues the style before it (so words are not split).
			const visible = [fg, bold ? 'bold' : '', dim ? 'dim' : '', italic ? 'italic' : '', inverse ? 'inverse' : ''].filter(Boolean).join(' ');
			const previous = cells.at(-1)?.style ?? '';
			cells.push({char, style: char !== ' ' ? visible : inverse ? 'inverse' : previous.endsWith('inverse') ? '' : previous});
		}
	};
	let position = 0;
	for (const match of line.matchAll(/\u001B\[([0-9;]*)m/g)) {
		push(line.slice(position, match.index));
		position = match.index + match[0].length;
		for (const code of (match[1] || '0').split(';').map(Number)) {
			if (code === 0) { fg = ''; bold = dim = italic = inverse = false; }
			else if (code === 1) bold = true;
			else if (code === 2) dim = true;
			else if (code === 3) italic = true;
			else if (code === 7) inverse = true;
			else if (code === 22) { bold = false; dim = false; }
			else if (code === 23) italic = false;
			else if (code === 27) inverse = false;
			else if (code === 39) fg = '';
			else if (FOREGROUND[code]) fg = FOREGROUND[code]!;
			else throw new Error(`unhandled SGR code ${code}`);
		}
	}
	push(line.slice(position));
	while (cells.length && cells.at(-1)!.char === ' ' && !cells.at(-1)!.style.endsWith('inverse')) cells.pop();
	let previous = '', out = '';
	for (const cell of cells) {
		if (cell.style !== previous) out += `⟦${cell.style}⟧`;
		previous = cell.style;
		out += cell.char;
	}
	return out;
}
function renderScreen(element: React.ReactElement, width: number, height: number): string[] {
	const lines = renderToString(element, {columns: width}).split('\n').map(styledLine);
	while (lines.length > height) lines.pop();
	return lines;
}

// Scenarios -----------------------------------------------------------------------------------------------------------

const read = (file: string) => fs.readFileSync(path.join(REPO_ROOT, 'fixtures', 'parity', 'settingsModel', file), 'utf8').split('\n').filter(Boolean).map(line => JSON.parse(line));

// Distinct infos from the recorded settingsModel inputs, one per (state, review, layers present, actions, codex hook)
// combination, so every header and cell variety shows up.
const seen = new Map<string, any>();
for (const file of fs.readdirSync(path.join(REPO_ROOT, 'fixtures', 'parity', 'settingsModel')).sort()) {
	if (!file.endsWith('.jsonl')) continue;
	for (const entry of read(file)) {
		for (const arg of entry.args) {
			if (!arg || typeof arg !== 'object' || !arg.repository || !arg.targets || !arg.vars?.home) continue;
			const actions = (arg.rows as any[]).filter(row => row.key === 'actions').length;
			const key = [arg.repository.state, arg.needsReview, Boolean(arg.targets.global), Boolean(arg.targets.repository), Math.min(actions, 3), arg.codexHooks?.state ?? '-', Boolean(arg.rows.some((row: any) => row.pending)), arg.agentHooks, arg.notifications, Boolean(arg.globalError)].join('|');
			if (!seen.has(key)) seen.set(key, arg);
		}
	}
}
const infos: any[] = [...seen.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)).map(([, info]) => info);
// Hand-made variants: a repository file that is invalid, a global file that cannot be read, long names.
for (const base of infos.slice(0, 3)) {
	infos.push({...base, repository: {state: 'invalid', path: base.repository.path, error: 'Unexpected token } in JSON at position 12'}});
	infos.push({...base, globalError: 'Cannot read /home/me/.deckhand/config.json: Unexpected end of JSON input. Repair it with e or delete the file.'});
	infos.push({...base, globalError: 'Config is invalid'});
	infos.push({...base, repo: 'a-very-long-repository-name-that-needs-cutting-in-the-title', repository: {state: 'invalid', path: base.repository.path}});
}

const choiceCases = read('choiceOptions.jsonl').filter(entry => Array.isArray(entry.ret));
const optionsFor = (id: string, target: string, random: () => number) => {
	const pool = choiceCases.filter(entry => entry.args[1] === id && entry.args[2] === target);
	return pool.length ? pool[Math.floor(random() * pool.length)].ret : undefined;
};

const CANDIDATES: any[] = [
	{path: 'node_modules', kind: 'dir', ignored: true, suggestion: 'link', reason: 'dependencies'},
	{path: '.env', kind: 'file', ignored: true, suggestion: 'link', reason: 'env file'},
	{path: 'dist', kind: 'dir', ignored: true, suggestion: 'skip', reason: 'build output'},
	{path: 'packages/web/.next', kind: 'dir', ignored: true, suggestion: 'skip', reason: 'build cache'},
	{path: 'notes.md', kind: 'file', ignored: false, suggestion: 'skip', reason: ''},
	{path: 'vendor', kind: 'dir', ignored: true, suggestion: 'link', reason: 'dependencies', configured: 'symlink', layers: ['global'], repos: ['mono', 'api']},
	{path: '.venv', kind: 'dir', ignored: true, suggestion: 'link', reason: 'dependencies', configured: 'symlink', layers: ['repository'], repos: ['mono']},
	{path: 'both-layers', kind: 'dir', ignored: true, suggestion: 'skip', reason: '', configured: 'symlink', layers: ['global', 'repository'], repos: ['mono', 'api', 'web']},
	{path: 'config/local.json', kind: 'file', ignored: true, suggestion: 'skip', reason: '', configured: 'files', source: 'config/local.example.json', layers: ['repository']},
	{path: 'current', kind: 'symlink', ignored: false, suggestion: 'skip', reason: '', target: '/home/me/projects/mono/releases/2026-01-01/very/long/target/path/to/a/place'},
	{path: 'broken', kind: 'symlink', ignored: false, suggestion: 'skip', reason: ''},
	{path: 'gone', kind: 'dir', ignored: true, suggestion: 'skip', reason: '', configured: 'symlink', layers: ['repository'], missing: true},
	{path: 'fresh-entry', kind: 'dir', ignored: false, suggestion: 'skip', reason: '', added: true, repos: ['mono']},
	{path: 'deeply/nested/path/with/a/very/long/name/that/will/need/cutting/in/the/picker', kind: 'dir', ignored: true, suggestion: 'link', reason: 'dependencies'},
	{path: 'shared-cache', kind: 'dir', ignored: true, suggestion: 'link', reason: 'dependencies', repos: ['mono', 'api']},
	{path: 'tab\tname\u0007', kind: 'file', ignored: false, suggestion: 'skip', reason: 'weird name'},
];
const orderLinked = (list: any[]) => [...list.filter(c => c.configured || c.added), ...list.filter(c => !(c.configured || c.added))];

const widths = [10, 16, 20, 24, 30, 40, 52, 63, 64, 70, 80, 100, 120];
const heights = [8, 10, 12, 14, 16, 20, 24, 30, 40];
const TEXTS = ['', 'npm run dev', 'x'.repeat(70), 'pnpm --filter web dev && echo "ready ✓"', 'tab\there\u0001ctl', 'feat/<name>'];
const HELP: any[][] = [
	[],
	[{text: 'Runs in the new worktree after creation · e.g. npm install · pnpm i'}],
	[{text: 'Rules: lowercase letters, digits, - and _ · no spaces · at most 48 characters · must not be a reserved name'}, {text: '/very/long/path/preview/that/gets/cut/at/the/edge/of/the/box', nowrap: true}],
	[{text: 'Preview: feat/my-task', color: 'green'}, {text: 'Heads up: this is long enough to wrap onto a second line when the box is narrow', color: 'yellow'}],
];
const NOTICES: any[] = [
	{text: 'Saved Default agent in global defaults.'},
	{text: 'Could not save: the file changed on disk. Reload with r and try again.', error: true},
	{text: 'Trust needed'},
];

const random = prng(0x5e77);
const pick = <T,>(list: T[]): T => list[Math.floor(random() * list.length)]!;
const chance = (probability: number) => random() < probability;

function scenario(view: 'main' | 'actions' | 'links'): any {
	const info = pick(infos);
	const column = pick(['global', 'repository']);
	const out: any = {info, view, row: Math.floor(random() * 14), actionRow: Math.floor(random() * 5), column, sizes: {}, width: pick(widths), height: pick(heights)};
	if (chance(0.25)) out.notice = pick(NOTICES);
	if (view === 'links') {
		const global = chance(0.5);
		const candidates = orderLinked(CANDIDATES.filter(() => chance(0.7))).map(candidate => (global ? candidate : {...candidate, repos: undefined})).filter(Boolean);
		const links: Record<string, boolean> = {};
		for (const candidate of candidates) if (chance(0.5)) links[candidate.path] = chance(0.5);
		const initial = chance(0.5) ? {...links} : Object.fromEntries(Object.entries(links).filter(() => chance(0.7)));
		const sizes: Record<string, number | null> = {};
		for (const candidate of candidates) if (chance(0.6)) sizes[candidate.path] = chance(0.2) ? null : pick([0, 3, 900, 1023, 1024, 1280, 5120, 10239, 10240, 60000, 1048575, 1048576, 1572864, 5 * 1048576, 3000000]);
		out.sizes = sizes;
		out.links = {data: {checkout: '/dev/mono', candidates, moreCandidates: chance(0.3) ? 7 : 0, ...global ? {repos: ['mono', 'api', 'web'].slice(0, 1 + Math.floor(random() * 3))} : {}, ...chance(0.1) ? {candidatesError: 'git status failed'} : {}}, links, initial, target: pick(['global', 'repository']), row: Math.floor(random() * Math.max(1, candidates.length))};
		if (!candidates.length) out.links.row = 0;
	}
	if (chance(0.35)) {
		const kind = pick(['choice', 'choice', 'text', 'text', 'actions', 'clear', 'discard']);
		const target = pick(['global', 'repository']);
		if (kind === 'choice') {
			const def = pick(SETTINGS.filter(def => def.control === 'choice'));
			const options = optionsFor(def.id, target, random);
			if (options) out.edit = {kind: 'choice', id: def.id, label: def.label, target, options, index: Math.floor(random() * options.length), current: Math.floor(random() * (options.length + 1)) - 1};
		} else if (kind === 'text' || kind === 'actions') {
			const text = pick(TEXTS);
			const edit: any = {kind: 'text', id: kind === 'actions' ? 'actions' : pick(['devCommand', 'setupCommand', 'worktree.branchName']), label: kind === 'actions' ? 'Actions' : pick(['Dev command', 'Setup command', 'Branch name']), target, state: {text, cursor: Math.floor(random() * (text.length + 1))}};
			if (chance(0.2)) edit.state.selectAll = true;
			if (chance(0.3)) edit.error = pick(['Name must not be empty', 'A name can use lowercase letters, digits, - and _ only, and is at most 48 characters long, so please change it.']);
			if (chance(0.4)) edit.placeholder = pick(['dev', 'runs after creation', 'p'.repeat(60)]);
			if (kind === 'actions') { edit.entry = pick(['test', 'lint frontend', 'db.migrate.with.a.very.long.name.that.needs.cutting']); edit.step = pick(['name', 'command', undefined]); }
			out.edit = edit;
			if (chance(0.7)) out.editHelp = pick(HELP);
		} else if (kind === 'clear') {
			const def = pick(SETTINGS);
			out.edit = {kind: 'clear', id: def.id, label: def.label, target};
			if (chance(0.5)) out.editHelp = [{text: 'Heads up: nothing else sets it'}];
		} else {
			out.edit = {kind: 'discard-links'};
			if (chance(0.5)) out.editHelp = [{text: 'two changes', color: 'red'}];
		}
	}
	return out;
}

const render = (input: any): string[] => {
	const {width, height, ...props} = input;
	return renderScreen(React.createElement(SettingsPane, {...props, width, height}), width, height);
};

for (let index = 0; index < 700; index++) {
	const view = index % 10 < 5 ? 'main' : index % 10 < 8 ? 'actions' : 'links';
	recorder.call('render', render, [scenario(view)]);
}

await recorder.write({prune: true});
