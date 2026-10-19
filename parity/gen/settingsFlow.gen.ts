/** Regenerate: node --import tsx parity/gen/settingsFlow.gen.ts. Only isSettingsFlowMode is pure; the hook is covered by ported tests. */
import {Recorder, arb, installDeterminism, sample} from '../lib.ts';
import {isSettingsFlowMode} from '../../src/settingsFlow.tsx';

installDeterminism();
const recorder = new Recorder('settingsFlow');
const fixed = ['settings', 'edit-project', 'discard-project', 'browse', '', 'Settings', 'settings ', ' settings', 'edit_project', 'discard-project\n', 'edit-projectx', 'discard', '設定', '😀', 'settings​', 'SETTINGS', 'edit-project\0', '\uD800'];
for (const mode of fixed) recorder.call('isSettingsFlowMode', isSettingsFlowMode, [mode]);
for (const text of sample(arb.text(), 60)) recorder.call('isSettingsFlowMode', isSettingsFlowMode, [text]);
await recorder.write({prune: true});
