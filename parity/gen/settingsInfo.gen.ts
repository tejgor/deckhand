/** Fixtures for src/settingsInfo.ts (unit I7): the pure helpers and readCandidateSizes' argument validation. */
import {Recorder, arb, fc, installDeterminism, sample} from '../lib.ts';
import {classifyCandidate, codexHookCommands, codexHookState, readCandidateSizes} from '../../src/settingsInfo.ts';

installDeterminism();
const recorder = new Recorder('settingsInfo');

// codexHookCommands(hooksJson, configToml)
const hooks = (...commands: string[]) => JSON.stringify({hooks: {Stop: [{hooks: commands.map(command => ({type: 'command', command}))}]}});
const toml = {
	basic: '[[hooks.Stop]]\ncommand = "deckhand hook"\n',
	single: "command = 'node /x/cli.js hook'\n",
	escaped: 'command = "a \\"quoted\\" \\u00e9 \\\\ b"\n',
	indented: '  command   =   "indent hook"\n',
	bad: 'command = "bad \\q escape"\n',
	unterminated: 'command = "open\n',
	notCommand: 'xcommand = "no"\nmycommand = "no"\n',
	multi: 'command = "one"\ncommand = \'two\'\n# command = "comment"\ncommand = "three"\n',
	unicode: 'command = "中文 😀 hook"\n',
	trailing: 'command = "x" # trailing comment\n',
	empty: 'command = ""\n',
};
const jsons: (string | undefined)[] = [
	undefined, '', 'not json', '{}', '[]', 'null', '"str"', '{"command":"top"}', '{"command":5}', '{"command":null}',
	hooks('deckhand hook'), hooks('a', 'b'), hooks('/usr/bin/node /x/dist/cli.js hook'),
	JSON.stringify([{command: 'in array'}, [{command: 'nested'}]]),
	JSON.stringify({a: {command: 'x', b: {command: 'y', command2: 'z'}}, command: 'w'}),
	JSON.stringify({command: '中 😀 \ud83d'}), '{"command":"bad', ' {"command": "ws"} ',
];
const tomls: (string | undefined)[] = [undefined, '', ...Object.values(toml)];
for (const json of jsons) for (const config of [undefined, '', toml.basic, toml.single]) recorder.call('codexHookCommands', codexHookCommands, [json, config]);
for (const config of tomls) for (const json of [undefined, hooks('from json')]) recorder.call('codexHookCommands', codexHookCommands, [json, config]);
for (const [text] of sample(arb.textWithOffset(30), 40, 11)) {
	recorder.call('codexHookCommands', codexHookCommands, [undefined, `command = "${text.replaceAll('\n', ' ').replaceAll('"', '').replaceAll('\\', '')}"\n`]);
	recorder.call('codexHookCommands', codexHookCommands, [JSON.stringify({command: text}), undefined]);
	recorder.call('codexHookCommands', codexHookCommands, [undefined, `command = '${text.replaceAll("'", '')}'\n`]);
	recorder.call('codexHookCommands', codexHookCommands, [undefined, text]);
}

// codexHookState(commands, expected)
const expected = "'/usr/local/bin/node' '/x/dist/cli.js' 'hook'";
const stateCommands: string[][] = [
	[], [expected], ['unrelated'], ['deckhand hook'], ['Deckhand hook'], ["node /a/deckhand/cli.js 'hook'"], ["node /a/deckhand/cli.js hook'"],
	["node /a/deckhand/cli.js hook   "], ['deckhand hooks codex'], ['deckhand hook codex'], ['deckhand-hook'], ['DECKHAND\thook'], ['hook'],
	['deckhand hook\n'], ['deckhand hook\nmore'], ['deckhand hookx'], ['my-deckhand hook', 'other'], ['x', expected], ['x', 'deckhand hook'],
	['/Users/中/deckhand/dist/cli.js hook'], ["deckhand hook'"], ["deckhand 'hook"],
];
for (const commands of stateCommands) for (const want of [expected, 'deckhand hook', '']) recorder.call('codexHookState', codexHookState, [commands, want]);
for (const [text] of sample(arb.textWithOffset(16), 60, 12)) {
	recorder.call('codexHookState', codexHookState, [[text], expected]);
	recorder.call('codexHookState', codexHookState, [[`deckhand ${text} hook`, text], text]);
	recorder.call('codexHookState', codexHookState, [[`deckhand ${text}hook${text.slice(0, 2)}`], expected]);
}

// classifyCandidate(relative)
const candidates = [
	'', '/', 'node_modules', 'a/node_modules/b', '.venv', 'venv/lib', 'vendor', '.vercel/output', 'packages/x/node_modules',
	'.env', '.env.local', '.env.', 'prod.env', 'a/.env.production', 'env', 'x.environment', '.envrc', 'a.env/b',
	'dist', 'build/out', 'out', 'coverage/lcov', '.next/cache', '.turbo', '.cache', '__pycache__/x.pyc', '.pytest_cache', '.DS_Store', 'a/.DS_Store',
	'debug.log', 'a/b/c.log', 'logs', 'log', '.log', 'x.log/y', 'a.log.txt', 'src/main.rs', 'README.md', 'toString', 'constructor', '__proto__', 'hasOwnProperty',
	'node_modules/dist', 'dist/node_modules', 'dist/.env', '.env/dist', 'build.log', 'coverage.log', 'a//b', 'a/', 'a/b/',
	'中文/node_modules', 'é.env', '😀/.cache', 'dist/中', '日本.log', 'ＤＩＳＴ', 'Dist', 'NODE_MODULES',
];
for (const candidate of candidates) recorder.call('classifyCandidate', classifyCandidate, [candidate]);
const segments = ['node_modules', '.venv', 'venv', 'vendor', '.vercel', '.env', '.env.x', 'a.env', 'dist', 'build', 'out', 'coverage', '.next', '.turbo', '.cache', '__pycache__', '.pytest_cache', '.DS_Store', 'x.log', 'src', '中', '😀', 'toString', ''];
for (const parts of sample(fc.array(fc.constantFrom(...segments), {minLength: 1, maxLength: 4}), 150, 13)) recorder.call('classifyCandidate', classifyCandidate, [parts.join('/')]);
for (const [text] of sample(arb.textWithOffset(20), 60, 14)) recorder.call('classifyCandidate', classifyCandidate, [text]);

// readCandidateSizes: only the argument validation (throws before any git or du); real sizes need a repository.
const tooMany = Array.from({length: 25}, (_, index) => `f${index}`);
const sizeArgs: unknown[][] = [
	['/nonexistent', tooMany], ['/nonexistent', [...tooMany, '../x']], ['/nonexistent', ['/abs']], ['/nonexistent', ['../up']], ['/nonexistent', ['a/../b']],
	['/nonexistent', ['']], ['/nonexistent', ['.']], ['/nonexistent', ['ok', 5]], ['/nonexistent', 'not-array'], ['/nonexistent', null], ['/nonexistent', {length: 0}],
	['/nonexistent', ['../中\n']],['/nonexistent', ['a\0b']], ['/nonexistent', ['.git/config']], ['/nonexistent', ['a\\b']], ['/nonexistent', ['a/./b']], ['/nonexistent', ['a//b']], ['/nonexistent', ['a/']],
];
await recorder.callAll('readCandidateSizes', readCandidateSizes, sizeArgs);

await recorder.write({prune: true});
