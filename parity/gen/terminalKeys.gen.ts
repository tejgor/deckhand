/** Regenerate: node --import tsx parity/gen/terminalKeys.gen.ts (fixtures/parity/terminalKeys/). */
import {Recorder, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {normalizeTerminalKey} from '../../src/terminalKeys.ts';

installDeterminism();
const recorder = new Recorder('terminalKeys');
recorder.addTap(await runTap({tests: ['tests/terminalKeys.test.ts', 'tests/textEditor.test.ts'], tap: ['terminalKeys:*']}), {module: 'terminalKeys'});
const call = (args: unknown[]) => recorder.call('normalizeTerminalKey', normalizeTerminalKey as any, args);
for (const args of mutate(recorder.argsOf('normalizeTerminalKey'), {variants: 4})) call(args);

const sequences = ['', 'a', '\x7f', '\b', '\x1b\x7f', '\x1b\b', '\x1b', '\x1b[3~', '\x1b[3;2~', '\x1b[127u', '\x1b[8u', '\x1b[8;1u', '\x1b[127;1:2u', '\x1b[127;1:3u', '\x1b[127;1:4u', '\x1b[127;5;65u', '\x1b[127;5;65:66u', '\x1b[127;u', '\x1b[127;1:u', '\x1b[9u', '\x1b[127', '\x1b[127uX', 'X\x1b[127u', '\x1b[1127u', '\x1b[8;12;1:2u', '\x7f\x7f', '\x1b[;1u', '\x1b[127;１u', '\x1b[127;1\nu', '😀'];
const keys: any[] = [{}, {delete: true}, {backspace: true}, {delete: true, shift: true}, {ctrl: true, delete: true}, {meta: false}, {meta: true, backspace: false}, {leftArrow: true}, {backspace: true, delete: true, meta: false}, {return: true, delete: false}];
for (const sequence of sequences) for (const key of keys) call([key, sequence]);
const flags = ['ctrl', 'meta', 'shift', 'delete', 'backspace', 'return', 'tab', 'escape', 'upArrow', 'pageUp'];
const keyArb = fc.subarray(flags, {maxLength: 4}).map(set => Object.fromEntries(set.map(flag => [flag, true])));
for (const [key, sequence] of sample(fc.tuple(keyArb, fc.oneof(fc.constantFrom(...sequences), fc.stringMatching(/^\x1b\[[0-9;:]{0,12}u?$/), fc.string({maxLength: 4}))), 300, 21)) call([key, sequence]);
await recorder.write({prune: true});
