// Records V8's JSON.parse SyntaxError messages, which `DhError::json_parse` reproduces (config files are parsed with
// JSON.parse and its message is shown to the user, e.g. `<file>: Unexpected token ...`).
//
//   node crates/core/tests/data/gen_json_errors.mjs > crates/core/tests/data/json_parse_errors.json
//
// Output: [[input, message | null], ...] (null: the input is valid JSON).
import process from 'node:process';

const fixed = [
	'', ' ', '{', '{x', '[1,]', 'abc', '{"a":1}x', '{"a" 1}', 'nul', '"abc', '{"a":1,}', '  ', '[1 2]', 'tru',
	'{"a":1,"b":x}', '{"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": tru1 }', 'xxxxx{', '["a" "b"]', '{"a":1 "b":2}',
	'{"a":"\u0001"}', '{"é":x}', '\n\n  {"a": ]', '[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,x]',
	'[-]', '[01]', '[1.]', '"\\x"', '["😀", x]', '{"a":1}}', '[]]', '{,}', '[,1]', '﻿{}', '"\\u12x4"', '"\\u"', '"\\u12',
	'[1e]', '[1e+]', '[1.5e-x]', '[-x]', '{"a":1,"b"}', '[1,', '{"a":', '{"a"', '[', '[1', '{"a":1', '{"a":1,', '[true,fals]',
	'[nulL]', 'undefined', 'NaN', 'Infinity', '[object Object]', '\t\r\n x', '[1}', '{"a":1]', '{"a":[}', '"a\nb"', '"\\',
	'["a","b","c","d","e","f", z]', '{"k":"vvvvvvvvvvvvvvvvvvvvvv" }}', 'a\nb\nccccccccccccccccccccccccccc', '123abc', '-', '1.', '01',
	'[1]\n\nx', '[1,2]  ,', '{"a":\r\n x}', '{\r\n"a":1,\r\n}', 'tr"', '[1,2,3,4,5,6,7,8,9,10,11,12,13,"x" "y"]', '{"a":1}\n\n\n  ]',
	'0123456789012345678901', '012345678901234567890', '{"😀😀😀😀😀😀😀😀😀😀😀": x}', '["日本語日本語日本語日本語", ?]',
	'{"actions": {"test": "npm test",}}', '{"defaultAgent": "claude"\n"actions": {}}', '{\n  "worktree": {\n    "symlink": [node_modules]\n  }\n}',
	'{}', '[]', '1', '"x"', 'null', 'true', ' {"a": [1, 2.5e-3, -0, "\\u00e9\\n"]} ', '{"a":{"b":{"c":[[]]}}}',
	'[1.5E+10]', '[-0.0]', '"\\/\\b\\f\\n\\r\\t\\"\\\\"', '[1,2', '{"a":true,"b":false,"c":null}', '{"a" : 1 , "b" : [ ] }',
	'[ ]', '[1] ', ' ', '{"a":1}\u0000', '"\u007f"', '[+1]', '[.5]', '[1e5.5]', '[0x10]', "{'a':1}", '{a:1}',
	'{"a":1,\r}', '\r\r]', '[1,\r\n\r\n2,\rx]', '"\\u00e9\\uD83D\\uDE00"', '"\\ud83d"', '["a",😀]', '["aaaaaaaaaaaaaaaaaaaaaaaaa",😀]',
	'["aaaaaaaaaaaaaaaaaaaaaaaaa",é]', '{"a":1}   \n', '\n', '[1,]   ', '[[[[[[[[[[1]]]]]]]]]]]', '{"a":"b"}{"c":1}',
];

// Deterministic mutations of valid documents: deletions, insertions and replacements of one character.
let state = 99;
const random = () => { state = (state + 0x6D2B79F5) | 0; let t = Math.imul(state ^ (state >>> 15), 1 | state); t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t; return ((t ^ (t >>> 14)) >>> 0) / 4294967296; };
const documents = [
	'{"defaultAgent": "claude", "defaultWorkspace": "new", "actions": {"test": "npm test", "lint": "npm run lint"}}',
	'{\n  "worktree": {\n    "location": "{repoParent}/worktrees/{name}",\n    "symlink": ["node_modules", ".env"],\n    "hook": false\n  }\n}',
	'[1, -2.5e+3, true, false, null, "x\\ny", {"k": []}]',
];
const pieces = [...'{}[]:,"\\ -+.0123456789eEtfnulrx\n\t', '😀', 'é', '\u0001'];
const mutated = [];
for (let count = 0; count < 400; count++) {
	const doc = documents[Math.floor(random() * documents.length)];
	const at = Math.floor(random() * doc.length);
	const piece = pieces[Math.floor(random() * pieces.length)];
	const kind = random();
	mutated.push(kind < 0.33 ? doc.slice(0, at) + doc.slice(at + 1) : kind < 0.66 ? doc.slice(0, at) + piece + doc.slice(at) : doc.slice(0, at) + piece + doc.slice(at + 1));
}

// Messages can hold half of a surrogate pair (the token, or the edge of the context): written as UTF-8 it becomes
// U+FFFD, as it does in the Rust port.
const cases = [...new Set([...fixed, ...mutated])].map(text => {
	try { JSON.parse(text); return [text, null]; } catch (error) { return [text, error.message.toWellFormed()]; }
});
process.stdout.write(`${JSON.stringify(cases, null, 0)}\n`);
