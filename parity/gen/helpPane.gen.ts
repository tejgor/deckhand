/**
 * Fixtures for src/helpPane.tsx pure exports (searchHelp, helpRows). Regenerate with
 *
 *   node --import tsx parity/gen/helpPane.gen.ts
 *
 * HelpView/useHelp/HelpPane are React hooks/components (ported tests). Results holding a lone surrogate are skipped.
 */
import fc from 'fast-check';
import {Recorder, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {HELP_TOPICS} from '../../src/help.ts';
import {helpRows, searchHelp} from '../../src/helpPane.tsx';

installDeterminism();
const recorder = new Recorder('helpPane');

const LONE = /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/;
const hasLone = (v: unknown): boolean => typeof v === 'string' ? LONE.test(v) : Array.isArray(v) ? v.some(hasLone) : v && typeof v === 'object' ? Object.values(v).some(hasLone) : false;
const call = (name: string, fn: Function, args: unknown[]) => {
	try { if (hasLone(fn(...args))) return; } catch { return; }
	recorder.call(name, fn, args);
};
const isLine = (l: any) => l && typeof l === 'object' && (('keys' in l && typeof l.keys === 'string' && typeof l.text === 'string') || ('note' in l && typeof l.note === 'string') || ('heading' in l && typeof l.heading === 'string')) && Object.keys(l).length === ('keys' in l ? 2 : 1);

recorder.addTap(await runTap({tests: ['tests/help.test.ts'], tap: ['helpPane:searchHelp', 'helpPane:helpRows']}), {module: 'helpPane'});

// searchHelp over the real topics.
const words = ['lazygit', 'codex', '  ', '', 'LAZYGIT', ' esc ', 'ctrl+', 'git', 'Sessions', 'sidebar', 'zzz', '😀', '中', 'n', '/', '?', 'handoff', 'Settings', 'a b', '\t', ...HELP_TOPICS.map(t => t.title), ...HELP_TOPICS.map(t => t.title.toUpperCase())];
for (const q of words) call('searchHelp', searchHelp, [q]);
for (const topic of HELP_TOPICS) for (const line of topic.lines.slice(0, 6)) {
	const text = 'keys' in line ? line.keys : 'note' in line ? line.note : line.heading;
	for (const q of [text, text.slice(0, 4), ` ${text.toUpperCase()} `, text.split(' ')[0]!]) call('searchHelp', searchHelp, [q]);
}
const lineArb = fc.oneof(
	fc.record({keys: fc.constantFrom('n', 'Ctrl+]', 'a-very-long-key-name', '😀', '中文', 'exactly15chars!', 'sixteen chars!!!', '', 'Esc'), text: fc.constantFrom('one two three four', '', 'x', 'long long long long long long words here', '中文 text 😀', 'Match Me')}),
	fc.record({note: fc.constantFrom('About this.', '', 'a long note that wraps over several rows at narrow widths', '中文😀', 'match me')}),
	fc.record({heading: fc.constantFrom('More', '', 'Heading 😀', 'match')}),
) as fc.Arbitrary<any>;
const topicsArb = fc.array(fc.record({title: fc.constantFrom('Alpha', 'beta', 'Match', '😀', '中', ''), lines: fc.array(lineArb, {maxLength: 5})}), {maxLength: 4});
for (const [q, topics] of sample(fc.tuple(fc.constantFrom('match', 'MATCH ', 'alpha', 'a', '😀', '中', 'one', '', 'x', 'long', 'beta'), topicsArb), 200, 1)) call('searchHelp', searchHelp, [q, topics]);

// helpRows.
for (const topic of HELP_TOPICS) for (const width of [-1, 0, 1, 10, 20, 30, 40, 60, 80, 120]) call('helpRows', helpRows, [topic.lines, width]);
for (const [lines, width] of sample(fc.tuple(fc.array(lineArb, {maxLength: 8}), fc.oneof(fc.integer({min: -3, max: 60}), fc.constantFrom(10, 11, 12, 20, 80))), 400, 2)) call('helpRows', helpRows, [lines, width]);
call('helpRows', helpRows, [[], 20]);

// Mutations of recorded inputs (valid line shapes only).
for (const args of mutate(recorder.argsOf('helpRows').slice(0, 150), {variants: 2}).filter(([l, w]) => Array.isArray(l) && l.every(isLine) && typeof w === 'number')) call('helpRows', helpRows, args);
for (const args of mutate(recorder.argsOf('searchHelp').slice(0, 150), {variants: 2}).filter(([q, t]) => typeof q === 'string' && (t === undefined || (Array.isArray(t) && t.every((x: any) => typeof x?.title === 'string' && Array.isArray(x.lines) && x.lines.every(isLine)))))) call('searchHelp', searchHelp, args);

await recorder.write({prune: true});
