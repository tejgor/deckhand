/**
 * Parity fixtures for src/cleanupView.tsx (fixtures/parity/cleanupView/). Regenerate with
 *
 *   node --import tsx parity/gen/cleanupView.gen.ts
 *
 * Records lostFileCount, fitPath, lostFileRows, fileRowsLeft and cleanupLossLines. cleanupLossLines returns Ink
 * elements, so the recorded call is a wrapper that flattens each element to {key, color, wrap, children} (text
 * children stay strings, nested Text elements recurse). CLEANUP_FILE_MARKS / CLEANUP_FILE_LEGEND are constants
 * covered by the detailTexts fixtures. fitPath's width is a UTF-16 length; cases that would cut inside a surrogate
 * pair are not recorded.
 */
import React from 'react';
import {Recorder, arb, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {cleanupLossLines, fileRowsLeft, fitPath, lostFileCount, lostFileRows} from '../../src/cleanupView.tsx';

installDeterminism();
const recorder = new Recorder('cleanupView', {cap: 500});

recorder.addTap(await runTap({tests: ['tests/cleanupView.test.ts'], tap: ['cleanupView:*']}), {module: 'cleanupView'});

type State = 'changed' | 'untracked' | 'ignored';
const states: State[] = ['changed', 'untracked', 'ignored'];
interface Insp {safe: boolean; reasons: string[]; dirtyFiles: number; untrackedFiles: number; ignoredFiles: number; files?: {path: string; state: State}[]}
const insp = (files: {path: string; state: State}[] | undefined, counts: [number, number, number], reasons: string[] = []): Insp => {
	const record: Insp = {safe: false, reasons, dirtyFiles: counts[0], untrackedFiles: counts[1], ignoredFiles: counts[2]};
	if (files) record.files = files;
	return record;
};
const mk = (n: number) => Array.from({length: n}, (_, i) => ({path: `dir/${i}.md`, state: states[i % 3]!}));

const inspections: Insp[] = [
	insp(undefined, [0, 0, 0]), insp([], [0, 0, 0]), insp(undefined, [2, 3, 4], ['2 changed']), insp(mk(1), [1, 0, 0], ['one']),
	insp(mk(3), [1, 1, 1], ['a', 'b']), insp(mk(10), [3, 4, 3], ['ten files']), insp(mk(10), [50, 100, 100], ['cut list', '😀 emoji reason', '中文 reason that is rather long indeed']),
	insp(mk(5), [0, 0, 0]), insp([{path: 'src/中文/文件名.tsx', state: 'changed'}, {path: '😀/😀😀.md', state: 'untracked'}, {path: 'x', state: 'ignored'}], [1, 1, 1], ['unicode']),
	insp([{path: '', state: 'changed'}, {path: 'a'.repeat(120), state: 'ignored'}], [1, 0, 1], []),
];
for (const i of inspections) recorder.call('lostFileCount', lostFileCount, [i]);
for (const counts of [[0, 0, 0], [1, 2, 3], [100, 0, 0]] as [number, number, number][]) for (const n of [0, 1, 5, 200]) recorder.call('lostFileCount', lostFileCount, [{files: n ? mk(n) : [], dirtyFiles: counts[0], untrackedFiles: counts[1], ignoredFiles: counts[2]}]);
for (const [n, a, b, c] of sample(fc.tuple(fc.integer({min: 0, max: 30}), fc.nat(40), fc.nat(40), fc.nat(40)), 60, 21)) recorder.call('lostFileCount', lostFileCount, [insp(n % 7 === 0 ? undefined : mk(n), [a, b, c])]);

for (const i of inspections) for (const rows of [-1, 0, 1, 2, 3, 4, 5, 10, 11, 100]) recorder.call('lostFileRows', lostFileRows, [i, rows]);
for (const [n, a, b, c, rows] of sample(fc.tuple(fc.integer({min: 0, max: 30}), fc.nat(15), fc.nat(15), fc.nat(15), fc.integer({min: 0, max: 35})), 150, 22)) recorder.call('lostFileRows', lostFileRows, [insp(mk(n), [a, b, c]), rows]);

// fitPath: UTF-16 width. Skip cuts inside a surrogate pair.
const safeCut = (file: string, width: number): boolean => {
	if (file.length <= width) return true;
	const start = file.length - Math.max(1, width - 1);
	return !(start > 0 && /[\uDC00-\uDFFF]/.test(file[start]!));
};
const paths = ['', 'a', 'a.txt', 'src/components/deeply/nested/Button.tsx', '中文/路径/文件.ts', 'dir/😀.md', '😀😀😀', 'é'.repeat(10), 'áb́ć', 'x'.repeat(50), 'with space/and\ttab', '../../up/file'];
for (const file of paths) for (const width of [-5, 0, 1, 2, 3, 4, 5, 6, 10, 20, 40, 50, 51, 100]) if (safeCut(file, width)) recorder.call('fitPath', fitPath, [file, width]);
for (const [file, width] of sample(fc.tuple(arb.text(40), fc.integer({min: -2, max: 45})), 200, 23)) if (safeCut(file, width)) recorder.call('fitPath', fitPath, [file, width]);
for (const args of mutate(recorder.argsOf('fitPath'), {variants: 2}).filter(([f, w]) => typeof f === 'string' && typeof w === 'number' && safeCut(f, w))) recorder.call('fitPath', fitPath, args as [string, number]);

for (const height of [-1, 0, 5, 10, 24, 40]) for (const used of [-3, 0, 5, 10, 20, 30, 50]) recorder.call('fileRowsLeft', fileRowsLeft, [height, used]);
for (const [h, u] of sample(fc.tuple(fc.integer({min: 0, max: 80}), fc.integer({min: 0, max: 80})), 60, 24)) recorder.call('fileRowsLeft', fileRowsLeft, [h, u]);

// cleanupLossLines, flattened.
const flat = (node: React.ReactNode): unknown => {
	if (Array.isArray(node)) return node.map(flat);
	if (node === null || node === undefined || typeof node === 'boolean') return node ?? null;
	if (typeof node === 'string' || typeof node === 'number') return node;
	const element = node as React.ReactElement<{children?: React.ReactNode} & Record<string, unknown>>;
	const {children, ...props} = element.props;
	return {key: element.key, ...props, children: flat(children ?? null)};
};
const lossLines = (inspection: Insp, width: number, fileRows: number) => flat(cleanupLossLines(inspection as never, width, fileRows));
for (const i of inspections) for (const [width, rows] of [[80, 5], [20, 3], [6, 0], [0, -2], [10, 1], [40, 12], [5, 4]]) recorder.call('cleanupLossLines', lossLines, [i, width!, rows!]);
for (const [n, a, b, c, width, rows, reasons] of sample(fc.tuple(fc.integer({min: 0, max: 12}), fc.nat(6), fc.nat(6), fc.nat(6), fc.integer({min: 0, max: 60}), fc.integer({min: -1, max: 15}), fc.array(arb.text(20), {maxLength: 3})), 120, 25)) {
	const files = mk(n).map((f, i) => ({...f, path: i % 4 === 0 ? `${f.path}-${'中é'.repeat(i)}` : f.path}));
	recorder.call('cleanupLossLines', lossLines, [insp(n % 5 === 4 ? undefined : files, [a, b, c], reasons), width, rows]);
}

await recorder.write({prune: true});
