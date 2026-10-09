import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import {HELP_TOPICS} from '../src/help.js';

// Every letter/symbol key a handler checks (`input === 'x'`) must appear in the ? guide, so the guide cannot drift
// from the code again. Reads the handlers' source: a key added without a help line fails here.

function handledKeys(source: string, from: string, to?: string): string[] {
	const start = source.indexOf(from);
	assert.ok(start >= 0, `handler start not found: ${from}`);
	const end = to ? source.indexOf(to, start) : source.length;
	assert.ok(end > start, `handler end not found: ${to}`);
	const keys = new Set([...source.slice(start, end).matchAll(/input === '(.)'/g)].map(match => match[1]!));
	keys.delete(' ');
	return [...keys].sort();
}

// The key column is space-separated (`x  X`, `[ ]`); descriptions also separate keys with `/`, commas and parentheses.
const keyTokens = (text: string) => new Set(text.split(/\s+/).filter(Boolean));
const words = (text: string) => new Set(text.split(/[\s/,;:()]+/).filter(Boolean));

test('every key the browse view handles has a line in the ? guide', async () => {
	const source = await fs.readFile(new URL('../src/app.tsx', import.meta.url), 'utf8');
	const documented = new Set(HELP_TOPICS.flatMap(topic => topic.lines.flatMap(line => ('keys' in line ? [...keyTokens(line.keys)] : []))));
	const missing = handledKeys(source, "if (mode === 'browse') {", "if (mode === 'pick-program') {").filter(key => !documented.has(key));
	assert.deepEqual(missing, [], `browse keys missing from src/help.ts: ${missing.join(' ')}`);
});

test('every key the Tasks board handles is in the guide\'s Tasks topic', async () => {
	const source = await fs.readFile(new URL('../src/tasksFlow.tsx', import.meta.url), 'utf8');
	const topic = HELP_TOPICS.find(entry => entry.title === 'Tasks')!;
	const documented = new Set(topic.lines.flatMap(line => [...words('keys' in line ? `${line.keys} ${line.text}` : 'note' in line ? line.note : line.heading)]));
	const missing = handledKeys(source, 'const handleInput = ', 'const promote = ').filter(key => !documented.has(key));
	assert.deepEqual(missing, [], `Tasks keys missing from the Tasks help topic: ${missing.join(' ')}`);
});

test('every key the worktree manager handles is in the guide\'s Worktrees topic', async () => {
	const source = await fs.readFile(new URL('../src/worktreesFlow.tsx', import.meta.url), 'utf8');
	const topic = HELP_TOPICS.find(entry => entry.title === 'Worktrees')!;
	const documented = new Set(topic.lines.flatMap(line => [...words('keys' in line ? `${line.keys} ${line.text}` : 'note' in line ? line.note : line.heading)]));
	const missing = handledKeys(source, 'const handleInput = ', 'const render = ').filter(key => !documented.has(key));
	assert.deepEqual(missing, [], `Worktrees keys missing from the Worktrees help topic: ${missing.join(' ')}`);
});
