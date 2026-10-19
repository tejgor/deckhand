/** Fixtures for src/changesPane.tsx (changesLayout, diffLines). Regenerate: node --import tsx parity/gen/changesPane.gen.ts
 *  ChangesPane is an Ink component (rendering), covered by ported tests, not recorded. */
import {Recorder, fc, installDeterminism, sample} from '../lib.ts';
import {changesLayout, diffLines} from '../../src/changesPane.tsx';

const MODULE = 'changesPane';
installDeterminism();
const recorder = new Recorder(MODULE);
const call = (name: string, fn: (...a: any[]) => unknown, args: unknown[]) => { try { recorder.call(name, fn, args); } catch { /* recorded */ } };

// No tap: the existing tests reach these functions only through the built binary (tests/changesUi.test.ts).

// changesLayout: boundaries around SIDE_BY_SIDE_WIDTH (100), clamps, list caps.
const widths = [0, 1, 10, 36, 80, 99, 100, 101, 105, 120, 152, 153, 160, 200, 1000];
const heights = [0, 1, 2, 3, 4, 5, 8, 10, 20, 24, 50];
for (const w of widths) for (const h of heights) for (const l of [0, 1, 2, 5, 12, 100]) call('changesLayout', changesLayout, [w, h, l]);
for (const args of sample(fc.tuple(fc.integer({min: -5, max: 300}), fc.integer({min: -3, max: 80}), fc.integer({min: 0, max: 300})), 200, 5)) call('changesLayout', changesLayout, args);

// diffLines.
const hunk = '@@ -1,3 +1,4 @@\n context\n-old\n+new\n+added';
const full = 'diff --git a/a.ts b/a.ts\nindex 1..2 100644\n--- a/a.ts\n+++ b/a.ts\n' + hunk + '\n\\ No newline at end of file\n';
const texts = ['', ' ', '\n\n', '   \t\n', 'x', full, full + 'more', hunk, '+++ b/x\n--- a/x', '+a\n-b\n c\n@@ h @@', '+中文😀\n-é\n😀', '+tab\there\n-\u0001ctl', 'diff --git a/b b/b\nnew file mode 100644\n', 'Binary files a/x and b/x differ\n', '\r\n+crlf\r\n-x\r\n', '+', '-', ' ', '++++', '--- a\n+++ b\n@@\n'];
for (const t of texts) for (const truncated of [undefined, true, false]) for (const binary of [undefined, true, false]) {
	call('diffLines', diffLines, [{text: t, ...(binary === undefined ? {} : {binary}), ...(truncated === undefined ? {} : {truncated})}]);
}
call('diffLines', diffLines, [undefined]);
call('diffLines', diffLines, [{text: full, truncated: true, binary: true}]);
const lineArb = fc.constantFrom('+a', '-b', ' c', '@@ -1 +1 @@', 'diff --git a/x b/x', 'index 1..2', '--- a/x', '+++ b/x', '\\ No newline', '+中文😀', '', '+++', '---', 'new file mode 100644', 'similarity index 90%');
for (const [lines, truncated, binary] of sample(fc.tuple(fc.array(lineArb, {maxLength: 14}), fc.boolean(), fc.boolean()), 250, 9)) {
	call('diffLines', diffLines, [{text: lines.join('\n'), truncated, binary: binary && lines.length % 7 === 0}]);
}
await recorder.write({prune: true});
