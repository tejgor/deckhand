/**
 * Fixtures for src/notesVim.ts. Regenerate with `node --import tsx parity/gen/notesVim.gen.ts`.
 *
 * NOTES_VIM_SCRIPT (the exact vimscript text) and writeNotesVimScript (directory cases; DECKHAND_HOME is the case root).
 * The script's behaviour in vim is not recorded: tests/notesVim.test.ts drives a real vim and is ported instead.
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, installDeterminism, recordDirCase} from '../lib.ts';
import {NOTES_VIM_SCRIPT, writeNotesVimScript} from '../../src/notesVim.ts';

const MODULE = 'notesVim';
installDeterminism();
const recorder = new Recorder(MODULE);
const HOME_VAR = 'DECKHAND_HOME';
const environment = process['env'];

recorder.call('NOTES_VIM_SCRIPT', () => NOTES_VIM_SCRIPT, []);
await recorder.write({prune: true});

async function write(root: string): Promise<string> {
	const previous = environment[HOME_VAR];
	environment[HOME_VAR] = root;
	try { return await writeNotesVimScript(); } finally { if (previous === undefined) delete environment[HOME_VAR]; else environment[HOME_VAR] = previous; }
}
const none = async () => {};
await recordDirCase(MODULE, 'write-fresh-home', {setup: none, fn: 'writeNotesVimScript', args: root => [root], run: write});
await recordDirCase(MODULE, 'write-overwrites-stale-script', {
	setup: async root => { await fs.writeFile(path.join(root, 'notes.vim'), '" old version\n', {mode: 0o644}); await fs.writeFile(path.join(root, 'keep.txt'), 'x'); },
	fn: 'writeNotesVimScript', args: root => [root], run: write,
});
await recordDirCase(MODULE, 'write-shorter-than-existing', {
	setup: async root => { await fs.writeFile(path.join(root, 'notes.vim'), 'x'.repeat(100_000), {mode: 0o600}); },
	fn: 'writeNotesVimScript', args: root => [root], run: write,
});
