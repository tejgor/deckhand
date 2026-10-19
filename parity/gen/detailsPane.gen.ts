/**
 * Parity fixtures for src/detailsPane.tsx (fixtures/parity/detailsPane/). Regenerate with
 *
 *   node --import tsx parity/gen/detailsPane.gen.ts
 *
 * Pure functions: visibleDetailText, detailLines, detailsViewport, clampScroll, scrollDetails (key is a plain
 * object of booleans). The DetailsPane component is an Ink render and is covered by the ported UI tests.
 */
import {Recorder, arb, fc, installDeterminism, mutate, sample} from '../lib.ts';
import {clampScroll, detailLines, detailsViewport, scrollDetails, visibleDetailText} from '../../src/detailsPane.tsx';

installDeterminism();
const recorder = new Recorder('detailsPane', {cap: 500});

// No existing TS test taps these directly; seed with hand-written edge inputs.
const texts = [
	'', ' ', '\n', '\n\n', 'plain', 'a\tb', '\t', 'tab\tand\nnewline', 'line1\nline2\n', 'x'.repeat(200), '😀'.repeat(7), '中文字符测试', 'é́e', 'é́x',
	'‮bidi‬', '​zero‍width﻿', ' sep ', '؜arabic', '᠎mongol', '⁠⁤⁦⁩',
	'ctl\u0001\u0007\u001B[31mred\u001B[0m', 'del\u007Fx', '\u0080\u009Fc1', 'cr\rlf\r\nend', 'nul\u0000x', ' nbsp', 'trust: run `rm -rf /` ‮',
	'a'.repeat(10) + '\n' + 'b'.repeat(3) + '\n\n' + 'c'.repeat(11),
];
const widths = [-3, 0, 1, 2, 3, 5, 10, 80];
for (const text of texts) for (const width of widths) recorder.call('detailLines', detailLines, [text, width]);
for (const text of texts) recorder.call('visibleDetailText', visibleDetailText, [text]);
for (const text of texts.slice(0, 14)) for (const [width, height] of [[0, 0], [4, 4], [5, 5], [6, 6], [10, 5], [24, 10], [80, 24], [3, 40], [-1, -1]]) recorder.call('detailsViewport', detailsViewport, [text, width!, height!]);

for (const args of mutate(recorder.argsOf('detailLines'), {variants: 2})) recorder.call('detailLines', detailLines, args as [string, number]);
for (const args of mutate(recorder.argsOf('detailsViewport'), {variants: 1})) recorder.call('detailsViewport', detailsViewport, args as [string, number, number]);

for (const [text, width] of sample(fc.tuple(arb.text(40), fc.integer({min: -2, max: 30})), 150, 11)) recorder.call('detailLines', detailLines, [text, width]);
for (const text of sample(arb.text(40), 100, 12)) recorder.call('visibleDetailText', visibleDetailText, [text]);
for (const text of sample(fc.string({maxLength: 40, unit: 'binary'}).map(s => s.replace(/[\uD800-\uDFFF]/g, '?')), 80, 13)) {
	recorder.call('visibleDetailText', visibleDetailText, [text]);
	recorder.call('detailLines', detailLines, [text, 7]);
}
for (const [text, width, height] of sample(fc.tuple(arb.text(60), fc.integer({min: 0, max: 40}), fc.integer({min: 0, max: 30})), 150, 14)) recorder.call('detailsViewport', detailsViewport, [text, width, height]);

for (const scroll of [-5, -1, 0, 1, 3, 10, 1e9, 2.5, -0.5]) for (const max of [0, 1, 3, 10, 7.5]) recorder.call('clampScroll', clampScroll, [scroll, max]);
for (const [scroll, max] of sample(fc.tuple(fc.integer({min: -50, max: 50}), fc.integer({min: 0, max: 40})), 80, 15)) recorder.call('clampScroll', clampScroll, [scroll, max]);

const keys: Record<string, unknown>[] = [
	{}, {home: true}, {end: true}, {pageUp: true}, {pageDown: true}, {upArrow: true}, {downArrow: true}, {ctrl: true}, {meta: true},
	{home: true, end: true}, {pageUp: true, pageDown: true}, {upArrow: true, downArrow: true}, {ctrl: false, meta: false},
	{ctrl: true, upArrow: true}, {leftArrow: true}, {return: true}, {escape: true}, {shift: true},
];
const inputs = ['', 'j', 'k', 'J', 'K', 'x', 'jj', '😀', 'g', 'G'];
const viewports = [{rows: 1, max: 0}, {rows: 1, max: 10}, {rows: 5, max: 3}, {rows: 5, max: 20}, {rows: 10, max: 100}, {rows: 3, max: 2}];
for (const key of keys) for (const input of inputs) for (const viewport of viewports) for (const scroll of [-4, 0, 7]) recorder.call('scrollDetails', scrollDetails, [scroll, input, key, viewport]);
for (const [scroll, input, key, viewport] of sample(fc.tuple(fc.integer({min: -10, max: 60}), fc.constantFrom(...inputs), fc.constantFrom(...keys), fc.record({rows: fc.integer({min: 1, max: 12}), max: fc.integer({min: 0, max: 50})})), 200, 16)) recorder.call('scrollDetails', scrollDetails, [scroll, input, key, viewport]);

await recorder.write({prune: true});
