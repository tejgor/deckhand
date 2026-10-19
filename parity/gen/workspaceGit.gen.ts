/**
 * Fixtures for src/workspaceGit.ts. Only parseStatus is pure; every other export spawns git or gh (covered by ported
 * tests), and src/changesGit.ts has nothing pure to record (all exports read the file system / spawn git), so no
 * changesGit fixtures exist. Regenerate: node --import tsx parity/gen/workspaceGit.gen.ts
 */
import {Recorder, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {parseStatus} from '../../src/workspaceGit.ts';

const MODULE = 'workspaceGit';
installDeterminism();
const recorder = new Recorder(MODULE);

recorder.addTap(await runTap({tests: ['tests/changesModel.test.ts'], tap: ['workspaceGit:parseStatus']}), {module: 'workspaceGit'});

const OID = 'a'.repeat(40);
const hand: string[] = [
	'',
	'\0',
	'# branch.oid (initial)\0# branch.head main\0',
	`# branch.oid ${OID}\0# branch.head (detached)\0`,
	`# branch.oid ${OID}\0# branch.head feat/x\0# branch.upstream origin/feat/x\0# branch.ab +3 -12\0`,
	'# branch.ab +x -y\0# branch.ab +1 -2 \0# branch.ab\0# branch.head\0# other thing\0#\0',
	'# branch.head my branch with spaces\0',
	'1 .M N... 100644 100644 100644 abc def src/a b.ts\0',
	'1 A. N... 000000 100644 100644 0000 abc 日本語/ファイル😀.ts\0',
	'2 R. N... 100644 100644 100644 abc def R100 new name.ts\0old name.ts\0',
	'2 C. N... 100644 100644 100644 abc def C75 copy.ts\0',
	'u UU N... 100644 100644 100644 100644 a b c conflict.ts\0',
	'? untracked file.txt\0? 😀\0? dir/\0',
	'! ignored.log\0! build/\0',
	'?\0!\0? \0! \0',
	'1\0',
	'1 M\0',
	'2 R. N... 1 2 3 a b R1 p\0',
	'1 .M N... 1 1 1 a a with\nnewline\0? nl\nfile\0',
	'X unknown\0Z\0',
	'1 MM N... 1 1 1 a a s\0\0\0',
	`# branch.oid ${OID}\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +0 -0\0` + '1 M. N... 1 1 1 a a x\0? y\0! z\0',
	'\ud83d\0',
	'? \ud800\0',
];
for (const raw of hand) recorder.call('parseStatus', parseStatus, [raw]);
for (const args of mutate(recorder.argsOf('parseStatus'), {variants: 4})) recorder.call('parseStatus', parseStatus, args);

const chars = fc.constantFrom('a', 'b', ' ', '/', '.', '日', '😀', '́', '\n', '\t');
const pathArb = fc.array(chars, {minLength: 0, maxLength: 8}).map(a => a.join(''));
const record = fc.oneof(
	fc.record({k: fc.constantFrom('1', 'u'), xy: fc.constantFrom('.M', 'M.', 'MM', 'A.', 'UU', 'AA'), p: pathArb}).map(r => {
		const pre = r.k === '1' ? '1 ' + r.xy + ' N... 100644 100644 100644 h1 h2 ' : 'u ' + r.xy + ' N... 100644 100644 100644 100644 h1 h2 h3 ';
		return pre + r.p;
	}),
	fc.record({xy: fc.constantFrom('R.', 'C.', '.R'), p: pathArb, o: pathArb}).map(r => `2 ${r.xy} N... 1 1 1 h1 h2 R100 ${r.p}\0${r.o}`),
	pathArb.map(p => '? ' + p),
	pathArb.map(p => '! ' + p),
	fc.constantFrom('# branch.oid ' + OID, '# branch.oid (initial)', '# branch.head main', '# branch.head (detached)', '# branch.upstream origin/main', '# branch.ab +4 -5', '# branch.ab +a -b', '# branch.head x y'),
);
for (const records of sample(fc.array(record, {maxLength: 8}), 200, 11)) recorder.call('parseStatus', parseStatus, [records.join('\0') + (records.length ? '\0' : '')]);

await recorder.write({prune: true});
