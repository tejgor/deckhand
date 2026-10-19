/**
 * Parity fixtures for the pure helpers of src/terminalPane.tsx (fixtures/parity/terminalPane/). Regenerate with
 *
 *   node --import tsx parity/gen/terminalPane.gen.ts
 *
 * Recorded: hasAction, actionStatus. The Ink components (TerminalPane) are rendering, covered by the UI goldens.
 * Other U1 modules (menu: recorded by P12; tabs, preview, devPane, useTerminalInput) are components/hooks only.
 */
import {Recorder, fc, installDeterminism, mutate, sample} from '../lib.ts';
import * as pane from '../../src/terminalPane.tsx';

installDeterminism();
const recorder = new Recorder('terminalPane');

const sessions = [undefined, {id: 's1'}, {id: ''}, {id: 's1', title: 'x'}, {id: '😀'}];
const actions: Record<string, unknown>[] = [];
for (const sessionId of [undefined, 's1', 's2', '', '😀'])
	for (const workspace of [undefined, '', '/w/a'])
		for (const command of [undefined, '', 'npm test', 'echo 中文'])
			actions.push({sessionId, workspace, command, content: '', live: false});
for (const session of sessions) for (const action of actions) recorder.call('hasAction', pane.hasAction as Function, [session, action]);

const exitCodes = [undefined, null, 0, 1, 2, 127, -1];
const signals = [undefined, null, 0, 9, 15];
for (const live of [true, false]) for (const exitCode of exitCodes) for (const exitSignal of signals) {
	const action: Record<string, unknown> = {content: '', live};
	if (exitCode !== undefined) action.exitCode = exitCode;
	if (exitSignal !== undefined) action.exitSignal = exitSignal;
	recorder.call('actionStatus', pane.actionStatus as Function, [action]);
}
for (const [exitCode, exitSignal, live] of sample(fc.tuple(fc.integer({min: -300, max: 300}), fc.integer({min: 0, max: 64}), fc.boolean()), 80, 61))
	recorder.call('actionStatus', pane.actionStatus as Function, [{content: 'out', live, command: 'x', exitCode, exitSignal}]);
for (const args of mutate(recorder.argsOf('hasAction').slice(0, 40))) recorder.call('hasAction', pane.hasAction as Function, args);
for (const args of mutate(recorder.argsOf('actionStatus').slice(0, 40))) recorder.call('actionStatus', pane.actionStatus as Function, args);

await recorder.write({prune: true});
