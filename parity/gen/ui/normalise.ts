// Deterministic normalisation of volatile screen content. Rules apply in order to each screen row; a replaced span
// takes the style of its first cell, so style runs stay aligned with the normalised text. The same table is listed
// in fixtures/ui/README.md: a replaying harness applies it to its own screen before comparing.

export type Style = {fg: number | string | null; bg: number | string | null; bold: boolean; dim: boolean; italic: boolean; underline: boolean; inverse: boolean};
export type Glyph = {ch: string; width: number; style: Style};

export type Rule = {name: string; pattern: RegExp; replace: string | ((match: string) => string); describe?: string};

/** The fixed rules; the per-run temp directory rules come first (see `rulesFor`). */
export const RULES: Rule[] = [
	{name: 'temp dir name', pattern: /dh-ui-[A-Za-z0-9]{6}/g, replace: '<TMP>'},
	{name: 'uuid', pattern: /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/g, replace: '<UUID>'},
	// Git object names differ per run (commit dates are part of the hash); words made of a-f only are never touched.
	{name: 'git sha', pattern: /\b(?=[0-9a-f]*\d)(?=[0-9a-f]*[a-f])[0-9a-f]{7,40}\b/g, replace: '<SHA>'},
	{name: 'spinner frame', pattern: /[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]/g, replace: '⠋'},
	{name: 'iso timestamp', pattern: /\b\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z?\b/g, replace: '<TIME>'},
	{name: 'date', pattern: /\b\d{4}-\d\d-\d\d\b/g, replace: '<DATE>'},
	{name: 'clock time', pattern: /\b\d{1,2}:\d\d:\d\d\b/g, replace: '<CLOCK>'},
	{name: 'pid', pattern: /\bpid:? ?\d+\b/gi, replace: 'pid <PID>'},
	{name: 'duration', pattern: /\b\d+(?:\.\d+)?(?:ms|s)\b/g, replace: '<DUR>'},
	{name: 'relative age', pattern: /\b\d+ (?:seconds?|minutes?|hours?|days?) ago\b/g, replace: '<AGE> ago'},
];

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/**
 * The rules for one run whose temp directories are `bases` (realpath, and the same without /private): the full path
 * becomes `<TMP>`; a path the app cut from the front shows only the end of the base (`…Ab12/home/...`, always the same
 * number of characters whatever the base's length), which becomes as many `*`, keeping the row's layout exact.
 */
export function rulesFor(bases: string[]): Rule[] {
	const suffixes = [...new Set(bases.flatMap(base => Array.from({length: base.length}, (_, index) => base.slice(index))))].sort((a, b) => b.length - a.length);
	return [
		...bases.map(base => ({name: 'temp path', pattern: new RegExp(escape(base), 'g'), replace: '<TMP>'})),
		{name: 'truncated temp path', pattern: new RegExp(`(?<=…)(?:${suffixes.map(escape).join('|')})(?=/)`, 'g'), replace: (match: string) => '*'.repeat(match.length)},
		...RULES,
	];
}

/**
 * Keeps the row's display width after a replacement that changed it by `delta` (removed minus inserted columns): the
 * difference goes into (or comes out of) the whitespace right after the word the replacement ends in, so right-aligned
 * text and box borders stay put whatever the length of the original (a temp path's length varies by machine). At least
 * one space is left between words.
 */
function keepWidth(glyphs: Glyph[], from: number, delta: number): Glyph[] {
	if (delta === 0) return glyphs;
	let at = from;
	while (at < glyphs.length && glyphs[at]!.ch !== ' ') at++;
	if (delta > 0) {
		const style = (glyphs[at] ?? glyphs[at - 1])!.style;
		return [...glyphs.slice(0, at), ...Array.from({length: delta}, () => ({ch: ' ', width: 1, style})), ...glyphs.slice(at)];
	}
	let end = at;
	while (end < glyphs.length && glyphs[end]!.ch === ' ') end++;
	const removable = end === glyphs.length ? end - at : Math.max(0, end - at - 1);
	const remove = Math.min(removable, -delta);
	return [...glyphs.slice(0, at), ...glyphs.slice(at + remove)];
}

const replacementOf = (rule: Rule, match: string) => (typeof rule.replace === 'string' ? rule.replace : rule.replace(match));

export function normaliseRow(row: Glyph[], bases: string[]): Glyph[] {
	let glyphs = row;
	for (const rule of rulesFor(bases)) {
		// Re-scan after each replacement: offsets into the joined string move.
		for (let guard = 0; guard < 200; guard++) {
			const text = glyphs.map(glyph => glyph.ch).join('');
			rule.pattern.lastIndex = 0;
			let match = rule.pattern.exec(text);
			// Skip matches that already are their replacement (the spinner rule maps ⠋ to itself).
			while (match && match[0] === replacementOf(rule, match[0])) match = rule.pattern.exec(text);
			if (!match) break;
			const owner: number[] = [];
			glyphs.forEach((glyph, index) => { for (let unit = 0; unit < glyph.ch.length; unit++) owner.push(index); });
			const first = owner[match.index]!, last = owner[match.index + match[0].length - 1]!;
			const style = glyphs[first]!.style;
			const replacement = [...replacementOf(rule, match[0])].map(ch => ({ch, width: 1, style}));
			const removed = glyphs.slice(first, last + 1).reduce((sum, glyph) => sum + glyph.width, 0);
			glyphs = [...glyphs.slice(0, first), ...replacement, ...glyphs.slice(last + 1)];
			glyphs = keepWidth(glyphs, first + replacement.length, removed - replacement.length);
		}
	}
	return glyphs;
}

export function normaliseText(text: string, bases: string[]): string {
	let out = text;
	for (const rule of rulesFor(bases)) out = out.replace(rule.pattern, match => replacementOf(rule, match));
	return out;
}
