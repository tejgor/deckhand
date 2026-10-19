// Generates text-vectors.json: Ink's text measuring, truncation and wrapping (string-width, cli-truncate as
// wrap="truncate*" calls it, wrap-ansi as wrap="wrap" calls it), replayed by crates/ui/tests/text_vectors.rs against
// deckhand_ui::primitives.
//
//   node crates/ui/tests/data/gen-text-vectors.mjs > crates/ui/tests/data/text-vectors.json
const modules = new URL('../../../../node_modules/', import.meta.url);
const {default: stringWidth} = await import(new URL('string-width/index.js', modules));
const {default: cliTruncate} = await import(new URL('cli-truncate/index.js', modules));
const {default: wrapAnsi} = await import(new URL('wrap-ansi/index.js', modules));

const corpus = [
	'',
	'a',
	'hello world',
	'deckhand · DEV (isolated)',
	'❯ selected row',
	'● running',
	'○ stopped',
	'✓ exit 0',
	'⠋ starting',
	'↑ 12 more · ! 1',
	'╭─╮│╰╯',
	'日本語のテキスト',
	'mixed 日本 text',
	'😀 emoji',
	'👍🏽 thumbs',
	'👨‍👩‍👧 family',
	'🇺🇸 flag',
	'1️⃣ keycap',
	'❤️ heart',
	'❤ heart text',
	'é combining',
	'tab\there',
	'zero​width',
	'ﾃｽﾄ halfwidth',
	'ｶﾞ dakuten',
	'Ａ fullwidth',
	'…',
	'ā long line of text that keeps going past any reasonable pane width for sure',
	'/Users/someone/Developer/a/very/long/path/to/a/worktree/directory',
	'word another word yet another word',
	'   leading and trailing   ',
	'averyveryverylongwordwithoutspaces and more',
	'two  spaces  between',
	'line one\nline two',
	'crlf\r\nline',
	'☐ open item · ☑ done',
	'⑂ Fork parent',
	'◈ p:Preview  ⌁ t:Terminal  ⑂ g:Git  ▹ d:Dev  ✎ a:Notes',
	'中文中文中文中文中文',
	'a中b文c',
];
const widths = [0, 1, 2, 3, 4, 5, 7, 8, 10, 13, 20, 40];

const out = {
	stringWidth: corpus.map(text => ({text, width: stringWidth(text)})),
	truncate: corpus.flatMap(text => widths.flatMap(columns => ['end', 'start', 'middle'].map(position => ({text, columns, position, result: cliTruncate(text, columns, {position})})))),
	wrap: corpus.flatMap(text => widths.filter(columns => columns > 0).map(columns => ({text, columns, result: wrapAnsi(text, columns, {trim: false, hard: true})}))),
};
process.stdout.write(`${JSON.stringify(out, null, '\t')}\n`);
