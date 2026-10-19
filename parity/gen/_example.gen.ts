/**
 * Example gen script (template for parity/gen/<module>.gen.ts). Regenerate with
 *
 *   node --import tsx parity/gen/_example.gen.ts
 *
 * It writes fixtures/parity/_example/ (or $PARITY_FIXTURES_OUT/_example/) and is deterministic: running it twice gives
 * identical bytes. Replayed by crates/parity/tests/example_replay.rs against a hand-written Rust stand-in.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, arb, fc, installDeterminism, mutate, recordDirCase, registerFn, runTap, sample} from '../lib.ts';
import {scrollTopFor, wordLeft, wordRight} from '../../src/textEditor.ts';

const MODULE = '_example';
installDeterminism(); // fixed clock + seeded randomness for everything recorded in this process
const recorder = new Recorder(MODULE);

// 1. Tap: every call tests/textEditor.test.ts makes to these exports.
const tapped = await runTap({tests: ['tests/textEditor.test.ts'], tap: ['textEditor:wordLeft', 'textEditor:wordRight', 'textEditor:scrollTopFor']});
recorder.addTap(tapped, {module: 'textEditor'});

// 2. Mutations of the tapped inputs. Rust cursors are byte offsets on char boundaries, so only inputs whose offset
//    is a code-point boundary within the text are kept (TS behaviour inside a surrogate pair is not ported).
const onBoundary = ([text, offset]: unknown[]) => {
	if (typeof text !== 'string' || typeof offset !== 'number' || offset < 0 || offset > text.length) return false;
	return !(offset > 0 && offset < text.length && /[\uD800-\uDBFF]/.test(text[offset - 1]!) && /[\uDC00-\uDFFF]/.test(text[offset]!));
};
for (const [name, fn] of [['wordLeft', wordLeft], ['wordRight', wordRight]] as const) {
	for (const args of mutate(recorder.argsOf(name), {variants: 4}).filter(onBoundary)) recorder.call(name, fn, args);
}
for (const args of mutate(recorder.argsOf('scrollTopFor'))) recorder.call('scrollTopFor', scrollTopFor, args);

// 3. fast-check samples with fixed seeds.
for (const [text, cursor] of sample(arb.textWithBoundary(20), 150)) {
	recorder.call('wordLeft', wordLeft, [text, cursor]);
	recorder.call('wordRight', wordRight, [text, cursor]);
}
for (const args of sample(fc.tuple(fc.nat(30), fc.nat(30), fc.integer({min: 1, max: 10}), fc.nat(40)), 120, 7)) recorder.call('scrollTopFor', scrollTopFor, args);

// 4. Tag showcase: exceptions, argument mutation, Map/Set/Date/undefined, function arguments. (Inline oracle
//    functions only for this example; a real gen script records src exports.)
function checkedWidth(width: number): number {
	if (!(width > 0)) throw new Error(`width must be positive: ${width}`);
	return Math.floor(width);
}
for (const width of [80, 2.5, 0, -1, Number.NaN, Infinity]) { try { recorder.call('checkedWidth', checkedWidth, [width]); } catch {} }

function pushUnique(list: string[], item: string): boolean {
	if (list.includes(item)) return false;
	list.push(item);
	return true;
}
for (const args of [[['a'], 'b'], [['a', 'b'], 'a'], [[], 'x']]) recorder.call('pushUnique', pushUnique, args);

function summarize(words: string[]): {counts: Map<string, number>; unique: Set<string>; first?: string; at: Date; missing: undefined[]} {
	const counts = new Map<string, number>();
	for (const word of words) counts.set(word, (counts.get(word) ?? 0) + 1);
	return {counts, unique: new Set(words), first: words[0], at: new Date(), missing: [undefined]};
}
for (const words of [['b', 'a', 'b'], [], ['😀']]) recorder.call('summarize', summarize, [words]);

const double = registerFn('double', (value: number) => value * 2);
const negate = registerFn('negate', (value: number) => -value);
const applyTwice = (fn: (value: number) => number, value: number) => fn(fn(value));
for (const args of [[double, 3], [negate, 5], [double, -0.5]]) recorder.call('applyTwice', applyTwice, args);

await recorder.write({prune: true});

// 5. A file-based operation: before/ -> op -> after/ directory cases.
async function writeSettings(root: string, settings: {name?: string; theme?: string}): Promise<string> {
	if (!settings.name) throw new Error('settings.name is required');
	const directory = path.join(root, 'config');
	await fs.mkdir(directory, {recursive: true, mode: 0o700});
	const file = path.join(directory, 'settings.json');
	await fs.writeFile(file, JSON.stringify({...settings, workspace: path.join(root, 'workspace'), savedAt: new Date().toISOString()}, null, 2) + '\n', {mode: 0o600});
	await fs.rm(path.join(root, 'stale.txt'), {force: true});
	return file;
}
const setup = async (root: string) => {
	await fs.writeFile(path.join(root, 'stale.txt'), 'old\n');
	await fs.mkdir(path.join(root, 'cache'));
	await fs.writeFile(path.join(root, 'keep.json'), '{"keep":true}\n');
};
await recordDirCase(MODULE, 'write-settings', {setup, fn: 'writeSettings', args: root => [root, {name: 'demo', theme: 'dark'}], run: writeSettings});
await recordDirCase(MODULE, 'write-settings-invalid', {setup, fn: 'writeSettings', args: root => [root, {theme: 'dark'}], run: writeSettings});
