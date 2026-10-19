/** Regenerate with: node --import tsx parity/gen/desktop.gen.ts  (writes fixtures/parity/desktop/). */
import {Recorder, arb, fc, installDeterminism, runTap, sample} from '../lib.ts';
import {isVimFamily, resolveTerminalEditor, terminalEditorArgs} from '../../src/desktop.ts';

// openInEditor / openUrl spawn detached GUI helpers and are not recorded (covered by ported tests).
// resolveTerminalEditor's PATH fallback depends on the machine, so only explicit editors are recorded.
installDeterminism();
const recorder = new Recorder('desktop');

recorder.addTap(await runTap({tests: ['tests/terminalEditor.test.ts'], tap: ['desktop:terminalEditorArgs']}), {module: 'desktop'});

const stable: Record<string, string>[] = [
	{VISUAL: 'sh -x', EDITOR: 'nope'},
	{EDITOR: '  /bin/sh  '},
	{EDITOR: 'deckhand-no-such-editor'},
	{VISUAL: 'deckhand-no-such-editor', EDITOR: 'sh'},
	{VISUAL: '/bin/sh -c  -x   y', EDITOR: ''},
	{VISUAL: '/bin/sh', EDITOR: 'nope'},
	{EDITOR: '/bin/sh\t-e'},
	{EDITOR: 'sh 中 😀'},
	{EDITOR: '/no/such/dir/vim --clean'},
	{EDITOR: '/bin/sh/'},
	{EDITOR: '"sh"'},
	{EDITOR: 'sh;echo'},
];
for (const vars of stable) recorder.call('resolveTerminalEditor', resolveTerminalEditor, [vars]);

const editors = [
	{command: '/usr/bin/vim', args: [], label: 'vim'},
	{command: 'nvim', args: ['--clean'], label: 'nvim'},
	{command: 'vi', args: [], label: 'vi'},
	{command: 'vim', args: [], label: 'vim.basic'},
	{command: 'nano', args: ['-l'], label: 'nano'},
	{command: 'gvim', args: [], label: 'gvim'},
	{command: 'view', args: ['+1', '-n'], label: 'vi'},
	{command: 'x', args: [], label: ''},
	{command: 'x', args: [], label: 'nvim\n'},
	{command: 'x', args: [], label: 'ex'},
	{command: 'x', args: ['--中'], label: 'Vim'},
];
for (const editor of editors) {
	recorder.call('isVimFamily', isVimFamily, [editor]);
	for (const [file, script] of [['n.md', 's.vim'], ['n.md', undefined], ['n.md', ''], ['/notes/日本 😀.md', '/dh/notes.vim'], ['', 'x']] as const) {
		recorder.call('terminalEditorArgs', terminalEditorArgs, [editor, file, script]);
	}
}
for (const [label, file, script] of sample(fc.tuple(fc.constantFrom('vim', 'nvim', 'vi', 'nano', 'code', 'vimx', 'nvi'), arb.text(12), fc.option(arb.text(12), {nil: undefined})), 120, 11)) {
	const editor = {command: label, args: ['-a'], label};
	recorder.call('isVimFamily', isVimFamily, [editor]);
	recorder.call('terminalEditorArgs', terminalEditorArgs, [editor, file, script]);
}

await recorder.write({prune: true});
