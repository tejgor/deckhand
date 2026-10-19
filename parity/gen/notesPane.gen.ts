/** Fixtures for src/notesPane.tsx (notesLayout). Regenerate: node --import tsx parity/gen/notesPane.gen.ts
 *  NotesRows / NotesMessage are Ink components (rendering), covered by ported tests, not recorded. */
import {Recorder, fc, installDeterminism, mutate, registerFn, runTap, sample} from '../lib.ts';
import {notesLayout} from '../../src/notesPane.tsx';

const MODULE = 'notesPane';
installDeterminism();
const recorder = new Recorder(MODULE);

const linkOpen = registerFn('linkOpen', () => 'open' as const);
const linkDone = registerFn('linkDone', () => 'done' as const);
const linkGone = registerFn('linkGone', () => 'gone' as const);
const linkById = registerFn('linkById', (id: string) => (/^[0-9]/.test(id) ? 'done' : /^[a-m]/.test(id) ? 'open' : 'gone') as 'open' | 'done' | 'gone');
const lookups = [linkOpen, linkDone, linkGone, linkById, undefined];
const known = new Set(['linkOpen', 'linkDone', 'linkGone', 'linkById']);

const boundary = (text: string, cursor: number) => !(cursor > 0 && cursor < text.length && /[\uD800-\uDBFF]/.test(text[cursor - 1]!) && /[\uDC00-\uDFFF]/.test(text[cursor]!));
const ok = ([input]: any[]) => {
	for (const s of [input?.shared, input?.session]) {
		if (s === undefined) continue;
		if (typeof s.text !== 'string' || !Array.isArray(s.titles)) return false;
		if (s.editing && !(Number.isInteger(s.editing.cursor) && s.editing.cursor >= 0 && s.editing.cursor <= s.text.length && boundary(s.text, s.editing.cursor))) return false;
	}
	return typeof input?.width === 'number' && typeof input?.height === 'number';
};
const call = (args: unknown[]) => { if (!ok(args)) return; try { recorder.call('notesLayout', notesLayout, args); } catch { /* throws recorded */ } };

// 1. Tap.
const tapped = (await runTap({tests: ['tests/notes.test.ts', 'tests/notesUi.test.ts'], tap: ['notesPane:notesLayout'], allowFailures: true}))
	.filter(entry => [...JSON.stringify(JSON.parse(entry.line).args).matchAll(/"\$fn":"([^"]*)"/g)].every(m => known.has(m[1]!)));
recorder.addTap(tapped, {module: MODULE});

// 2. Mutations (only on-boundary, function-free).
for (const args of mutate(recorder.argsOf('notesLayout'), {variants: 6})) call(args);

// 3. Hand-written edges.
const TEXTS = ['', '   \n', 'plain', '# Heading\n\nbody text that is long enough to wrap around a narrow pane width',
	'- [ ] open item\n- [x] done item\n- [ ] 中文任务😀 with a long tail that wraps and wraps',
	'- [ ] linked ↗ task:abc123 title\n- [x] ↗ task:9zz done\n- [ ] ↗ task:zzz gone',
	'😀😀😀😀😀😀\n中文中文中文中文中文\nééé', 'a\tb\u0001c\n\n\n\nz', 'x'.repeat(200), Array.from({length: 30}, (_, i) => `- [${i % 3 ? ' ' : 'x'}] item ${i}`).join('\n')];
const sec = (text: string, extra: object = {}) => ({titles: ['Worktree note (shared by sessions)', 'Worktree note', 'Shared'], text, empty: 'Empty. Press E to write.', ...extra});
let n = 0;
for (const text of TEXTS) for (const width of [0, 1, 5, 12, 40, 100]) for (const height of [0, 1, 2, 3, 8, 20]) {
	const link = lookups[n++ % lookups.length];
	const shared = sec(text), session = sec(TEXTS[(n + 3) % TEXTS.length]!, {titles: ['This session', 'Session']});
	const base = {width, height, ...(link ? {links: link} : {})};
	call([{...base, shared}]);
	call([{...base, session}]);
	call([{...base, shared, session}]);
	call([{...base, shared, session, focus: 'shared'}]);
	call([{...base, shared, session, focus: 'session'}]);
	call([{...base, shared: {...shared, flag: 'read-only · too large'}, session: {...session, active: true}, focus: n % 2 ? 'shared' : 'session'}]);
}
call([{width: 40, height: 10}]);
call([{width: 40, height: 10, shared: sec(''), session: sec('', {titles: ['S']})}]);
for (const text of TEXTS) for (const cursor of [0, 1, 2, 7, 20, text.length, text.length - 1]) for (const scrollTop of [0, 1, 5, 99, -2]) {
	for (const [width, height, focus] of [[40, 10, 'session'], [12, 4, 'shared'], [30, 6, undefined], [1, 3, 'session']] as const) {
		const editing = {cursor: Math.max(0, cursor), scrollTop};
		call([{shared: sec(text), session: sec(text, {editing}), width, height, ...(focus ? {focus} : {})}]);
		call([{shared: sec(text, {editing}), session: sec('- [ ] other'), width, height, ...(focus ? {focus} : {})}]);
		call([{shared: sec(text, {editing}), width, height, ...(focus ? {focus} : {})}]);
	}
}

// 4. fast-check.
const textArb = fc.array(fc.constantFrom('- [ ] ', '- [x] ', '# ', '↗ task:a1 ', 'word ', '中文', '😀', '\n', '\n\n', 'é', '\t', 'long-token-without-breaks-'), {maxLength: 14}).map(p => p.join(''));
const secArb = fc.record({titles: fc.constantFrom(['A long title here', 'Title', 'T'], ['x']), text: textArb, empty: fc.constantFrom('Empty', '中文空', ''), flag: fc.option(fc.constantFrom('read-only', 'too large'), {nil: undefined}), active: fc.option(fc.boolean(), {nil: undefined})}, {requiredKeys: ['titles', 'text', 'empty']});
const edit = (s: any, c: number, t: number) => ({...s, editing: {cursor: [...s.text].slice(0, c).join('').length, scrollTop: t}});
for (const [shared, session, w, h, focus, ed, c, t, l] of sample(fc.tuple(fc.option(secArb, {nil: undefined}), fc.option(secArb, {nil: undefined}), fc.integer({min: 0, max: 60}), fc.integer({min: 0, max: 24}), fc.constantFrom(undefined, 'shared', 'session'), fc.constantFrom(0, 1, 2), fc.nat(40), fc.integer({min: -1, max: 12}), fc.nat(4)), 400, 21)) {
	const sh = shared && ed === 1 ? edit(shared, c, t) : shared;
	const se = session && ed === 2 ? edit(session, c, t) : session;
	const link = lookups[l];
	call([{...(sh ? {shared: sh} : {}), ...(se ? {session: se} : {}), width: w, height: h, ...(focus ? {focus} : {}), ...(link ? {links: link} : {})}]);
}

await recorder.write({prune: true});
