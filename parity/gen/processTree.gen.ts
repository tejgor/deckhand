/**
 * Fixtures for src/processTree.ts. snapshotDescendants and the live half of survivors/killSurvivors read the real
 * process table, so only the deterministic shapes are recorded (empty snapshots, PIDs that cannot exist); the live
 * behaviour is covered by the ported tests.
 *   node --import tsx parity/gen/processTree.gen.ts
 */
import {Recorder, installDeterminism} from '../lib.ts';
import {killSurvivors, survivors} from '../../src/processTree.ts';

installDeterminism();
const recorder = new Recorder('processTree');
const snapshots: Array<Map<number, string>> = [
	new Map(),
	new Map([[2_000_000_000, 'Thu Jan  1 00:00:00 1970']]),
	new Map([[2_000_000_001, 'Mon Jan  5 10:00:00 2026'], [2_000_000_002, '中 😀']]),
	new Map([[2_000_000_003, '']]),
];
for (const snapshot of snapshots) {
	recorder.call('survivors', survivors, [snapshot]);
	recorder.call('killSurvivors', killSurvivors, [snapshot]);
}
await recorder.write({prune: true});
