/**
 * Parity fixtures for src/configEditorPane.tsx (fixtures/parity/configEditorPane/). Regenerate with
 *
 *   node --import tsx parity/gen/configEditorPane.gen.ts
 *
 * The module exports only the ConfigEditorPane component, so the fixture records whole renders: props and a size in,
 * the screen out (Ink's renderToString at that width, with colors on, as styled lines; the format is described in
 * parity/gen/settingsPane.gen.ts). The pane's scroll position starts at 0 (a fresh render), so the cursor row alone
 * decides what scrolls into view.
 */
import React from 'react';
import {Recorder, installDeterminism, prng} from '../lib.ts';

// Chalk reads the color level when it loads, so ink and the sources are imported after this.
process.env.FORCE_COLOR = '3';
delete process.env.NO_COLOR;
const {renderToString} = await import('ink');
const {ConfigEditorPane} = await import('../../src/configEditorPane.tsx');

installDeterminism();
const recorder = new Recorder('configEditorPane', {cap: 1000});

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

const JSON_TEXTS = [
	'',
	'{}',
	'{\n\t"worktree": {\n\t\t"symlink": ["node_modules", ".venv"]\n\t},\n\t"devCommand": "npm run dev"\n}\n',
	JSON.stringify({actions: {test: 'npm test', lint: 'eslint . --max-warnings 0'}, worktree: {location: '{repoParent}/worktrees/{name}'}}, null, 2),
	JSON.stringify({devCommand: 'x'.repeat(150), setupCommand: 'y'.repeat(60)}, null, 2),
	'line one\r\nline two\ttabbed\nctl\u0001\u0007end sep para\n\n\n',
	'{"é": "naïve café", "中文": "字符测试", "emoji": "😀 ok"}',
	Array.from({length: 60}, (_, index) => `  "key${index}": ${index},`).join('\n'),
	'a'.repeat(40) + '\n' + 'b'.repeat(80) + '\n' + 'c'.repeat(79) + '\n',
];
const PATHS = [
	'/home/me/.deckhand/config.json',
	'/dev/mono/deckhand.json',
	'/home/me/projects/some/very/long/path/that/goes/on/and/on/and/on/and/on/deckhand.json',
	'/dev/ctl\u0001/tab\t/deckhand.json',
];
const ERRORS = ['Unexpected token } in JSON at position 12', 'Save failed: the file changed on disk', 'x'.repeat(200), ''];
const MESSAGES = ['Formatted', 'Nothing to format: invalid JSON', 'm'.repeat(150)];
const widths = [10, 16, 20, 24, 30, 40, 52, 64, 80, 100, 120];
const heights = [7, 8, 10, 12, 16, 20, 24, 30];

// Double-width characters make Ink wrap an editor row that the editor (counting code points) did not, and a wrapped
// row past the pane's height makes Ink's flex layout shrink and overlap the rows, so those only render in wide panes.
const wide = /[\u2E80-\uFFFF\u{10000}-\u{10FFFF}]/u;
const random = prng(0xc0f1);
const pick = <T,>(list: T[]): T => list[Math.floor(random() * list.length)]!;
const chance = (probability: number) => random() < probability;

const render = (input: any): string[] => {
	const {width, height, ...props} = input;
	return renderScreen(React.createElement(ConfigEditorPane, {...props, width, height}), width, height);
};

for (let index = 0; index < 400; index++) {
	const raw = pick(JSON_TEXTS);
	const text = chance(0.6) ? raw : pick(JSON_TEXTS);
	const kind = pick(['global', 'repository']);
	// Cursors at code point boundaries: the Rust editor works on chars.
	const boundaries = [0];
	for (const char of text) boundaries.push(boundaries.at(-1)! + char.length);
	const state: any = {text, cursor: pick(boundaries)};
	if (chance(0.15)) state.selectAll = true;
	if (chance(0.2)) state.message = pick(MESSAGES);
	const input: any = {document: {kind, root: '/r', path: pick(PATHS), raw, revision: chance(0.5) ? 'abc' : null, exists: chance(0.8)}, state, width: wide.test(text) ? pick(widths.filter(width => width >= 100)) : pick(widths), height: pick(heights)};
	if (chance(0.25)) input.error = pick(ERRORS);
	recorder.call('render', render, [input]);
}

await recorder.write({prune: true});
