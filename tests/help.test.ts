import assert from 'node:assert/strict';
import {test} from 'node:test';
import React from 'react';
import {renderToString} from 'ink';
import {HELP_TOPICS} from '../src/help.js';
import {HelpPane, helpRows, searchHelp} from '../src/helpPane.js';

const plain = (text: string) => text.replace(/\x1b\[[0-9;]*m/g, '');
const pane = (props: Partial<React.ComponentProps<typeof HelpPane>>, width: number, height = 24) => plain(renderToString(React.createElement(HelpPane, {topic: 0, scroll: 0, width, height, ...props}), {columns: width})).split('\n');

test('help topics: keys in one aligned column, descriptions wrap under themselves, a long key gets its own row', () => {
	const rows = helpRows([{note: 'About this.'}, {keys: 'n', text: 'one two three four'}, {keys: 'Ctrl+]', text: 'back'}, {heading: 'More'}, {keys: 'a-very-long-key-name', text: 'x'}], 20).map(row => row.map(part => part.text).join(''));
	assert.deepEqual(rows, ['About this.', '', 'n       one two', '        three four', 'Ctrl+]  back', '', 'More', 'a-very-long-key-name', '        x']);
	// Every topic fits in a short title list and has content.
	for (const topic of HELP_TOPICS) { assert.ok(topic.title.length <= 15, topic.title); assert.ok(topic.lines.length, topic.title); }
});

test('help search matches keys, descriptions and notes across topics; the pane shows a topic list when wide and ◂ topic ▸ when narrow', () => {
	assert.deepEqual(searchHelp('lazygit').map(topic => topic.title), ['Git & PRs']);
	assert.ok(searchHelp('codex').some(topic => topic.title === 'Agent signals'));
	assert.deepEqual(searchHelp('  '), []);
	const wide = pane({topic: 1}, 110);
	assert.match(wide[1]!, /Help · Sessions/);
	assert.ok(wide.some(line => /❯ 2 Sessions +│ x  X +Stop/.test(line)));
	// Every topic fits a 24-row pane at 100 columns, so none needs scrolling there.
	for (const topic of HELP_TOPICS.keys()) assert.doesNotMatch(pane({topic}, 100).join('\n'), /PgDn|↓ \d/, HELP_TOPICS[topic]!.title);
	const narrow = pane({topic: 1}, 56, 14);
	assert.match(narrow[1]!, /Help · ◂ Sessions \(2\/11\) ▸ +↓ \d+\/\d+/);
	assert.ok(narrow.some(line => /PgDn scroll/.test(line)));
	const search = pane({query: 'lazygit', typing: true}, 110).join('\n');
	for (const text of ['Help · search', '/ lazygit', '7  Git & PRs', 'On the Git tab', 'enter done']) assert.ok(search.includes(text), text);
	assert.ok(pane({query: 'zzz'}, 110).join('\n').includes('Nothing matches "zzz".'));
});

test('help: the Sidebar topic explains every row glyph, the gutter marker, the header and the details', () => {
	const topic = HELP_TOPICS.find(entry => entry.title === 'Sidebar')!;
	const text = topic.lines.map(line => 'keys' in line ? `${line.keys} ${line.text}` : 'note' in line ? line.note : line.heading).join('\n');
	for (const glyph of ['⠋', '●', '◌', '○', '!', '?', '◆', '⌛', '▾', '▸', '↳', '⑂', '▶', '▣', '✓', '+N', '✶', 'π', '◇', '↑', '╎', '! N', 'Below the list']) assert.ok(text.includes(glyph), glyph);
});
