// Records how Node's String.prototype.localeCompare (default locale, default options) orders the kind of strings
// deckhand compares (session titles, names, branches, paths, ISO timestamps, IDs), plus emoji, CJK and other scripts.
//
//   node crates/core/tests/data/gen_locale_compare.mjs > crates/core/tests/data/locale_compare.json
//
// Output: {strings, sorted, pairs}: `sorted` is `strings` sorted by localeCompare (a stable sort), `pairs` are
// [i, j, sign] of localeCompare(strings[i], strings[j]) for every adjacent pair of `sorted` and pseudo-random pairs.
// Han beyond the BMP (CJK Extension B and later) is left out on purpose: jsstr's table stops at the BMP, so those sort
// after all other Han there (ICU interleaves them by radical and stroke); see jsstr's known_divergences test.
import process from 'node:process';

const fixed = [
	'', ' ', '  ', 'a', 'A', 'b', 'B', 'z', 'Z', 'aa', 'aA', 'Aa', 'AA', 'ab', 'aB', 'Ab', 'AB', 'abc', 'abd', 'ABC',
	'a b', 'a-b', 'a_b', 'a.b', 'a/b', 'a\\b', 'a:b', 'a,b', 'a;b', 'a!b', 'a?b', 'a\'b', 'a"b', 'a(b', 'a)b', 'a[b', 'a]b',
	'a{b', 'a}b', 'a@b', 'a*b', 'a&b', 'a#b', 'a%b', 'a`b', 'a^b', 'a+b', 'a<b', 'a=b', 'a>b', 'a|b', 'a~b', 'a$b', 'a\tb',
	'ab ', ' ab', 'a  b', 'item 1', 'item 2', 'item 9', 'item 10', 'item 100', 'Item 10', 'item10', 'item-10', 'item_10',
	'v1.2.3', 'v1.10.0', 'v1.9.9', 'V1.2.3', '1', '01', '10', '2', '9', '100', '0', '-1', '+1', '1.5', '1,5', '½', '²', '₂',
	'fix bug', 'Fix bug', 'Fix Bug', 'fix-bug', 'fix_bug', 'fixbug', 'fix bugs', 'Fix: bug', 'fix: bug', '[wip] fix bug',
	'feature/login', 'feature/Login', 'feature-login', 'feature/login-v2', 'fix/login', 'main', 'master', 'Main', 'HEAD',
	'deckhand/feature-x', 'tejas/fix-123', 'release/2026.10', 'release/2026.9', 'dependabot/npm_and_yarn/foo-1.2.3',
	'/Users/tejas/dev/deckhand', '/Users/tejas/dev/Deckhand', '/Users/tejas/dev/deckhand-2', '/Users/tejas/dev/deckhand_2',
	'/Users/tejas/dev/deckhand/src', '/tmp/x', '/tmp/X', '/tmp/x y', '~/code', '~/Code', './src', '../src', 'src/app.tsx',
	'src/App.tsx', 'src/app.ts', 'src/app-test.ts', 'src/app_test.ts', 'src/app.test.ts', 'README.md', 'readme.md',
	'.gitignore', '.env', '.env.local', 'node_modules', 'Cargo.toml', 'cargo.lock', '__init__.py', '_private', '-flag', '--flag',
	'2026-10-09T12:34:56.789Z', '2026-10-09T12:34:56.790Z', '2026-10-09T12:35:00.000Z', '2026-10-10T00:00:00.000Z',
	'2026-09-30T23:59:59.999Z', '2025-12-31T23:59:59.999Z', '2026-10-09', '2026-10-09T12:34:56Z',
	'3fa9c1d2', '3FA9C1D2', '3fa9c1d3', 'a1b2c3d4', 'A1B2C3D4', '0f1e2d3c', 'ffffffff', '00000000',
	'f47ac10b-58cc-4372-a567-0e02b2c3d479', 'F47AC10B-58CC-4372-A567-0E02B2C3D479', 'f47ac10b-58cc-4372-a567-0e02b2c3d47a',
	'é', 'e', 'E', 'É', 'è', 'ê', 'ë', 'ē', 'ė', 'ę', 'é', 'café', 'cafe', 'Café', 'CAFE', 'cafés', 'caff', 'cafd',
	'résumé', 'resume', 'Résumé', 'naïve', 'naive', 'über', 'uber', 'Über', 'ünder', 'zoë', 'zoe', 'Ångström', 'angstrom',
	'ß', 'ss', 'SS', 'straße', 'strasse', 'Straße', 'æ', 'ae', 'Æ', 'AE', 'œuvre', 'oeuvre', 'ø', 'o', 'Ø', 'ł', 'l', 'đ', 'd',
	'ñ', 'n', 'Ñ', 'piñata', 'pinata', 'ç', 'c', 'Ç', 'façade', 'facade', 'ı', 'i', 'İ', 'I', 'ǅ', 'Ǆ', 'ǆ', 'dž', 'ﬁ', 'fi',
	'ａ', 'Ａ', 'ａｂｃ', 'abc', 'ª', 'º', '…', '...', '..', '.', '—', '–', '-', '‐', '‒', '―', '·', '•', '°', '§', '¶', '©', '®', '™', 'TM',
	'😀', '😁', '😀a', 'a😀', '👍', '👍🏽', '🚀', '🎉', '✨', '🔥', '❤️', '❤', '🇺🇸', '👨‍👩‍👧', '⭐', '🌟', '🙂', '🙃', '😀😀',
	'日本', '日本語', '中文', '中', '一', '二', '三', '汉字', '漢字', '你好', 'こんにちは', 'カタカナ', 'ひらがな',
	'あ', 'ア', 'ｱ', 'い', 'イ', 'か', 'が', 'カ', 'ガ', 'ー', '한국어', '한', '가', '나', 'ㄱ', 'α', 'Α', 'β', 'ω', 'Ω', 'λόγος',
	'я', 'Я', 'а', 'б', 'привет', 'Привет', 'ё', 'е', 'עברית', 'שלום', 'العربية', 'مرحبا', 'ไทย', 'हिन्दी',
	'☐ 2 open', '☑ 3 done', '☐', '☑', '✓', '✗', '↗', '↳', '⑂', '⎇ main', '⎇ feature', '◆', '○', '●', '◌', '▶', '▣', '╎', '›', '❯',
	'✶', 'π', '◇', '?', '!', '⌛', '↑', '↓', '←', '→', '★', '☆', '♠', '♥', '€', '$', '£', '¥', '¢', '₹', '%', '‰', '@', '#',
	'Session 1', 'session 1', 'Session 1 / child', 'Session 1/child', 'session-1', 'Claude', 'claude', 'Codex', 'codex', 'Pi', 'pi',
	'test', 'Test', 'TEST', 'tests', 'testing', 'tést', 'tèst', 'tëst', 'tęst', 'te st', 'te-st', 'te_st', 'te.st', 'te​st',
	'a\u0000b', 'a\u0001b', 'ab\u0007', 'a\nb', 'a\rb', ' a', '　a', 'à', 'à', 'à́', 'á̀',
];

// A deterministic generator (no Math.random) so the file is reproducible.
let seed = 42;
const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const pick = list => list[Math.floor(random() * list.length)];
const alphabet = [...'aAbBcCeEéÉzZ019 -_./:', 'ß', 'æ', 'ø', '😀', '👍🏽', '日', 'あ', 'ア', 'я', 'α', '☐', '✓', 'ü', 'Ü', 'ﬁ', '…'];
const generated = [];
while (generated.length < 220) {
	const length = 1 + Math.floor(random() * 6);
	let text = '';
	for (let index = 0; index < length; index++) text += pick(alphabet);
	generated.push(text);
}
const strings = [...new Set([...fixed, ...generated])];
const sorted = strings.slice().sort((a, b) => a.localeCompare(b));
const index = new Map(strings.map((text, at) => [text, at]));
const pairs = [];
for (let at = 1; at < sorted.length; at++) {
	const i = index.get(sorted[at - 1]), j = index.get(sorted[at]);
	pairs.push([i, j, Math.sign(strings[i].localeCompare(strings[j]))]);
}
for (let count = 0; count < 3000; count++) {
	const i = Math.floor(random() * strings.length), j = Math.floor(random() * strings.length);
	pairs.push([i, j, Math.sign(strings[i].localeCompare(strings[j]))]);
}
process.stdout.write(`${JSON.stringify({node: process.version, icu: process.versions.icu, strings, sorted, pairs})}\n`);
