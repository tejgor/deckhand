// Generates crates/core/src/jsstr/collation.txt: per-code-point collation weights measured from Node's default
// Intl collator (ICU root/en, sensitivity variant, non-ignorable punctuation), which `jsstr::locale_compare` replays.
//
//   node crates/core/tests/data/gen_collation.mjs > crates/core/src/jsstr/collation.txt
//
// The model is UCA's three levels, applied after canonical decomposition and reordering (NFD), with one collation
// element per character plus expansions and two-character contractions:
//   P<start>[-<end>] <primary> <extra secondary> <tertiary>   primary-bearing characters (primary +1 per code point in a run)
//   M<cp> <secondary>                                         combining marks (secondary only)
//   I<start>[-<end>]                                          completely ignorable characters
//   D<cp> <cp> <cp>...                                        canonical decompositions (NFD)
//   X<cp> <extra secondary> <first tertiary|-> <cp> <cp>...   expansions (NFKD, or two Latin letters such as æ, ß, œ)
//   K<cp> <first> <second>                                    contractions: <first><second> sorts as <cp> (й = и + ̆)
//   C<cp> <rank>                                              canonical combining class, as a rank (NFD reordering)
// Weights are ranks, not ICU's numbers: only their order matters. Characters outside the table get implicit weights
// (src/jsstr.rs), after every table primary, by code point.
import process from 'node:process';

const base = new Intl.Collator('en-US', {sensitivity: 'base'});
const accent = new Intl.Collator('en-US', {sensitivity: 'accent'});
const variant = new Intl.Collator('en-US');

const RANGES = [
	[0x0000, 0x1fff], [0x2000, 0x2dff], [0x2e00, 0x2e7f], [0x3000, 0x33ff], [0x3400, 0x4dbf], [0x4e00, 0x9fff],
	[0xa640, 0xa69f], [0xa720, 0xa7ff], [0xab30, 0xab6f], [0xf900, 0xfaff], [0xfb00, 0xfb4f], [0xfe00, 0xfe0f],
	[0xfe30, 0xfe6f], [0xff00, 0xffef], [0x1f000, 0x1faff],
];
const usable = cp => !/[\p{Cn}\p{Cs}\p{Co}]/u.test(String.fromCodePoint(cp));
const hex = cp => cp.toString(16);
const chars = text => [...text].map(ch => ch.codePointAt(0));
const str = cp => String.fromCodePoint(cp);

const decompositions = new Map();
const atomic = new Set();
for (const [start, end] of RANGES) {
	for (let cp = start; cp <= end; cp++) {
		if (!usable(cp)) continue;
		const text = str(cp);
		const nfd = text.normalize('NFD');
		if (nfd !== text) {
			decompositions.set(cp, chars(nfd));
			for (const part of chars(nfd)) atomic.add(part);
		} else atomic.add(cp);
	}
}

// Contractions: a precomposed letter whose primary differs from its base letter's (Cyrillic й) is a letter of its own;
// its decomposed spelling sorts as it.
const contractions = new Map();
for (const [cp, parts] of decompositions) {
	if (parts.length !== 2 || base.compare(str(cp), str(parts[0])) === 0) continue;
	if (base.compare(str(cp), str(parts[0]) + str(parts[1])) !== 0) continue;
	contractions.set(cp, parts);
}
for (const cp of contractions.keys()) decompositions.delete(cp);
for (const cp of decompositions.keys()) atomic.delete(cp);

// Canonical combining classes as ranks: two marks swap under NFD when the first's class is higher.
const swaps = (a, b) => `x${str(a)}${str(b)}`.normalize('NFD') === `x${str(b)}${str(a)}`;
const combining = [...atomic].filter(cp => /\p{M}/u.test(str(cp)));
const references = [0x0301, 0x0316];
const nonzero = combining.filter(cp => references.some(ref => swaps(cp, ref) || swaps(ref, cp)) || references.includes(cp));
const byClass = nonzero.slice().sort((a, b) => (swaps(a, b) ? 1 : swaps(b, a) ? -1 : 0) || 0);
// Insertion into class groups: equal classes never swap either way.
const classGroups = [];
for (const cp of byClass) {
	const group = classGroups.find(members => !swaps(members[0], cp) && !swaps(cp, members[0]));
	if (group) group.push(cp); else classGroups.push([cp]);
}
classGroups.sort((a, b) => (swaps(a[0], b[0]) ? 1 : -1));
const combiningRank = new Map();
classGroups.forEach((group, index) => { for (const cp of group) combiningRank.set(cp, index + 1); });

const ignorable = [];
const marks = [];
const bearing = [];
for (const cp of atomic) {
	const text = str(cp);
	if (base.compare(text, '') === 0) {
		if (accent.compare(text, '') !== 0) marks.push(cp); else ignorable.push(cp);
	} else bearing.push(cp);
}
for (const cp of contractions.keys()) bearing.push(cp);

// Expansions: compatibility decompositions of several characters equal at the primary level, and Latin letters
// that sort as two letters (æ, ß, œ, ꜩ, ...).
const expansions = new Map();
const LATIN = cp => (cp >= 0xc0 && cp <= 0x24f) || (cp >= 0x1e00 && cp <= 0x1eff) || (cp >= 0x2c60 && cp <= 0x2c7f)
	|| (cp >= 0xa720 && cp <= 0xa7ff) || (cp >= 0xab30 && cp <= 0xab6f) || (cp >= 0xfb00 && cp <= 0xfb06);
const letters = 'abcdefghijklmnopqrstuvwxyz';
for (const cp of bearing) {
	if (contractions.has(cp)) continue;
	const text = str(cp);
	const nfkd = text.normalize('NFKD');
	if (chars(nfkd).length > 1 && base.compare(text, nfkd) === 0) { expansions.set(cp, chars(nfkd)); continue; }
	if (!LATIN(cp)) continue;
	search: for (const x of letters) for (const y of letters) {
		// An uppercase letter expands to uppercase letters (Æ → AE), so case still orders it after its lowercase.
		if (base.compare(text, x + y) === 0) { expansions.set(cp, chars(text.toLowerCase() !== text ? (x + y).toUpperCase() : x + y)); break search; }
	}
}
const single = bearing.filter(cp => !expansions.has(cp));

// Primary ranks: classes of characters equal at the primary level, in order.
const byBase = single.slice().sort((a, b) => base.compare(str(a), str(b)) || a - b);
const primary = new Map();
const classes = [];
for (const cp of byBase) {
	const last = classes.at(-1);
	if (last && base.compare(str(last[0]), str(cp)) === 0) last.push(cp);
	else classes.push([cp]);
	primary.set(cp, classes.length);
}

// Mark secondaries: classes of marks on `a`, ranked; weight = rank * 256 (room for extra secondaries between).
const markText = cp => `a${str(cp)}`;
const sortedMarks = marks.slice().sort((a, b) => accent.compare(markText(a), markText(b)) || a - b);
const markGroups = [];
for (const cp of sortedMarks) {
	const last = markGroups.at(-1);
	if (last && accent.compare(markText(last[0]), markText(cp)) === 0) last.push(cp);
	else markGroups.push([cp]);
}
const markWeight = new Map();
markGroups.forEach((group, index) => { for (const cp of group) markWeight.set(cp, (index + 1) * 256); });

// The extra secondary of `text` against `plain` + mark (`insert` puts the mark into the plain form): 0 when equal at
// the accent level, a mark's weight when it equals plain + that mark, else a slot between two marks.
const slots = new Map();
function extraSecondary(text, plain, insert) {
	if (accent.compare(text, plain) === 0) return 0;
	let index = 0;
	for (; index < markGroups.length; index++) {
		const order = accent.compare(text, insert(str(markGroups[index][0])));
		if (order === 0) return (index + 1) * 256;
		if (order < 0) break;
	}
	const slot = index;
	slots.set(slot, [...slots.get(slot) ?? [], text]);
	return {slot, text};
}

const extra = new Map();
const pending = [];
for (const group of classes) {
	const sorted = group.slice().sort((a, b) => variant.compare(str(a), str(b)) || a - b);
	const plain = str(sorted[0]);
	for (const cp of sorted) {
		const value = extraSecondary(str(cp), plain, mark => plain + mark);
		if (typeof value === 'number') extra.set(cp, value); else pending.push([cp, value]);
	}
}
for (const [cp, components] of expansions) {
	const parts = components.map(str);
	const value = extraSecondary(str(cp), parts.join(''), mark => parts[0] + mark + parts.slice(1).join(''));
	if (typeof value === 'number') extra.set(cp, value); else pending.push([cp, value]);
}
// Slot weights: slot k lies between mark group k-1 and k: k * 256 + 1.. in accent order.
for (const [slot, texts] of slots) {
	const ordered = [...new Set(texts)].sort((a, b) => accent.compare(a, b));
	const ranks = new Map();
	let rank = 0;
	ordered.forEach((text, index) => {
		if (index === 0 || accent.compare(ordered[index - 1], text) !== 0) rank++;
		ranks.set(text, rank);
	});
	for (const [cp, value] of pending) if (value.slot === slot) extra.set(cp, slot * 256 + Math.min(255, ranks.get(value.text)));
}

// Tertiary ranks: within a primary class and extra secondary, the variant order, as 2, 4, 6, ... (odd values are the
// slots between, for expansions). Strings equal at the first two levels have the same classes at the same positions,
// so tertiary weights are only ever compared within a class: ranks suffice.
const tertiary = new Map();
const tertiaryGroups = new Map();
for (const group of classes) {
	const bySecondary = new Map();
	for (const cp of group) bySecondary.set(extra.get(cp), [...bySecondary.get(extra.get(cp)) ?? [], cp]);
	for (const members of bySecondary.values()) {
		const sorted = members.slice().sort((a, b) => variant.compare(str(a), str(b)) || a - b);
		let rank = 0;
		sorted.forEach((cp, index) => {
			if (index > 0 && variant.compare(str(sorted[index - 1]), str(cp)) !== 0) rank++;
			tertiary.set(cp, 2 + rank * 2);
			tertiaryGroups.set(cp, sorted);
		});
	}
}
// An expansion's first element: its tertiary slot among the variants of its first part (ﬃ sorts after ffi but
// before FFI). `-`: keep the part's own (the expansion already differs at the accent level, like æ).
const expansionTertiary = new Map();
for (const [cp, parts] of expansions) {
	const text = str(cp), rest = parts.slice(1).map(str).join('');
	const variants = tertiaryGroups.get(parts[0]);
	if (!variants || accent.compare(text, parts.map(str).join('')) !== 0) { expansionTertiary.set(cp, '-'); continue; }
	let slot = tertiary.get(variants.at(-1)) + 1;
	for (const member of variants) {
		const order = variant.compare(text, str(member) + rest);
		if (order === 0) { slot = tertiary.get(member); break; }
		if (order < 0) { slot = tertiary.get(member) - 1; break; }
	}
	expansionTertiary.set(cp, slot);
}

const lines = [];
const runs = [...primary.keys()].sort((a, b) => a - b);
for (let index = 0; index < runs.length;) {
	const start = runs[index];
	let end = index;
	while (end + 1 < runs.length && runs[end + 1] === runs[end] + 1 && primary.get(runs[end + 1]) === primary.get(runs[end]) + 1
		&& extra.get(runs[end + 1]) === extra.get(start) && tertiary.get(runs[end + 1]) === tertiary.get(start)) end++;
	const last = runs[end];
	lines.push(`P${hex(start)}${last !== start ? `-${hex(last)}` : ''} ${primary.get(start)} ${extra.get(start)} ${tertiary.get(start)}`);
	index = end + 1;
}
for (const cp of [...markWeight.keys()].sort((a, b) => a - b)) lines.push(`M${hex(cp)} ${markWeight.get(cp)}`);
const quiet = ignorable.slice().sort((a, b) => a - b);
for (let index = 0; index < quiet.length;) {
	let end = index;
	while (end + 1 < quiet.length && quiet[end + 1] === quiet[end] + 1) end++;
	lines.push(`I${hex(quiet[index])}${end !== index ? `-${hex(quiet[end])}` : ''}`);
	index = end + 1;
}
for (const [cp, parts] of [...decompositions].sort((a, b) => a[0] - b[0])) lines.push(`D${hex(cp)} ${parts.map(hex).join(' ')}`);
for (const [cp, parts] of [...expansions].sort((a, b) => a[0] - b[0])) {
	lines.push(`X${hex(cp)} ${extra.get(cp) ?? 0} ${expansionTertiary.get(cp)} ${parts.map(hex).join(' ')}`);
}
for (const [cp, [first, second]] of [...contractions].sort((a, b) => a[0] - b[0])) lines.push(`K${hex(cp)} ${hex(first)} ${hex(second)}`);
for (const [cp, rank] of [...combiningRank].sort((a, b) => a[0] - b[0])) lines.push(`C${hex(cp)} ${rank}`);
process.stdout.write(`# Generated by crates/core/tests/data/gen_collation.mjs from Node ${process.version} (ICU ${process.versions.icu}, CLDR ${process.versions.cldr}). Do not edit.\n${lines.join('\n')}\n`);
