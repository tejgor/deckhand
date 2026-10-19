/** Regenerate: node --import tsx parity/gen/configDraft.gen.ts (fixtures/parity/configDraft/). Only formatConfigJson is a function; the constants are not recorded. */
import {Recorder, fc, installDeterminism, sample} from '../lib.ts';
import {formatConfigJson} from '../../src/configDraft.ts';

installDeterminism();
const recorder = new Recorder('configDraft');
const call = (text: string) => { try { recorder.call('formatConfigJson', formatConfigJson, [text]); } catch {} };
const fixed = ['{}', '[]', 'null', 'true', '0', '-0', '1e3', '1.5', '1E+2', '12345678901234567890', '0.1', '"x"', '"\\u00e9\\ud83d\\ude00"', '"\\ud800"', '"😀中é"', '', ' ', '{', '}', '[1,', '{"a":1,}', "{'a':1}", 'undefined', 'NaN', '{"a":1}{"b":2}', '  {"a" : [1, 2, {"b": null}]}  \n', '{"a":{}, "b":[], "c":[[]], "d":{"e":{}}}', '{"b":1,"a":2,"1":3,"0":4}', '{"__proto__":{"x":1},"a":1}', '{"a":1,"a":2}', '﻿{}', '{"k":"line\\nbreak\\ttab\\u0000\\u001f\\u007f\\u2028"}', '[1,2,3,[4,[5,[6]]]]', '{"emoji":"👍🏽","cjk":"中文","k\\u00e9y":1}', '{"n":1e400}', '{"n":-1e-400}', '{"n":100000000000000000000000}', '[0.30000000000000004, 1e21, 1e-7, 123456789.123456789]', '"\\/"', '{"a":\n1\n}', 'tru', '[,]', '01', '+1', '.5', '1.', '{"a" 1}', '"unterminated', '"bad\\x"', '{"a":"\t"}', '[1] x', '{"a":1', ' {}'];
for (const text of fixed) call(text);
const json = fc.letrec(tie => ({tree: fc.oneof({depthSize: 'small'}, fc.constant(null), fc.boolean(), fc.integer(), fc.double({noNaN: true, noDefaultInfinity: true}), fc.string({unit: 'binary', maxLength: 6}), fc.array(tie('tree'), {maxLength: 4}), fc.dictionary(fc.string({maxLength: 4}), tie('tree'), {maxKeys: 4}))})).tree;
for (const value of sample(json, 200, 31)) { call(JSON.stringify(value)); call(JSON.stringify(value, null, '\t')); }
for (const text of sample(fc.string({unit: 'binary', maxLength: 10}), 80, 32)) call(text);
await recorder.write({prune: true});
