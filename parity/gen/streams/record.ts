// Records the terminal stream corpus (fixtures/term/streams/<name>.bin + .json). NOT part of regeneration: the
// recorded bytes are the input of parity/gen/term.gen.ts and are committed; re-recording changes them (timing decides
// chunk boundaries). Run only when the corpus should change:
//
//   node --import tsx parity/gen/streams/record.ts                 # everything
//   node --import tsx parity/gen/streams/record.ts sh-sgr lazygit  # some streams
//
// Real sessions run /bin/sh (and lazygit) through node-pty with a temp HOME, XDG dirs and a fixed environment, in
// throwaway directories under $TMPDIR. The agent TUIs (claude, codex, pi) are never run: their streams are synthetic
// (./synthetic.ts).

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import pty from 'node-pty';
import {ensureNodePtyReady} from '../../../src/nodePty.js';
import {StreamBuilder, writeStream} from './format.js';
import {SYNTHETIC_STREAMS} from './synthetic.js';

type Step =
	/** Writes input to the PTY, then waits for `until` (regex over the output since the step began, escapes stripped) and quiet. */
	| {send: string; until?: RegExp; quietMs?: number}
	| {resize: [cols: number, rows: number]; until?: RegExp; quietMs?: number}
	| {checkpoint: string}
	| {wait: number};

interface Session {
	name: string;
	description: string;
	command: string;
	args: string[];
	cols: number;
	rows: number;
	/** Prepares the working directory (inside the sandbox) and returns it with extra environment. */
	prepare: (sandbox: string) => {cwd: string; env?: Record<string, string>};
	/** Output that means the program is ready for the first step. */
	ready: RegExp;
	steps: Step[];
	/** Input that ends the program at the end. */
	exit: string;
	scrollOffsets?: number[];
}

const STRIP = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g;
const PROMPT = /\$ $/;
const STEP_TIMEOUT_MS = 15_000;

const sh = (send: string, quietMs = 150): Step => ({send: `${send}\r`, until: PROMPT, quietMs});

function baseEnv(sandbox: string): Record<string, string> {
	const home = path.join(sandbox, 'home');
	fs.mkdirSync(home, {recursive: true, mode: 0o700});
	return {
		HOME: home,
		PATH: '/usr/bin:/bin:/usr/sbin:/sbin',
		TERM: 'xterm-256color',
		LANG: 'en_US.UTF-8',
		LC_ALL: 'en_US.UTF-8',
		SHELL: '/bin/sh',
		PS1: '$ ',
		PS2: '> ',
		TZ: 'UTC',
		BASH_SILENCE_DEPRECATION_WARNING: '1',
		XDG_CONFIG_HOME: path.join(home, '.config'),
		XDG_STATE_HOME: path.join(home, '.local', 'state'),
		XDG_CACHE_HOME: path.join(home, '.cache'),
		XDG_DATA_HOME: path.join(home, '.local', 'share'),
		GIT_CONFIG_GLOBAL: '/dev/null',
		GIT_CONFIG_NOSYSTEM: '1',
	};
}

const plainDir = (name: string) => (sandbox: string) => {
	const cwd = path.join(sandbox, name);
	fs.mkdirSync(cwd, {recursive: true});
	return {cwd};
};

const SESSIONS: Session[] = [
	{
		name: 'sh-ls-colors', description: '/bin/sh: ls with colours (CLICOLOR_FORCE) over directories, executables, symlinks, hidden and spaced names.',
		command: '/bin/sh', args: ['-i'], cols: 80, rows: 24, ready: PROMPT, exit: 'exit\r',
		prepare: sandbox => {
			const cwd = path.join(sandbox, 'listing');
			fs.mkdirSync(path.join(cwd, 'directory'), {recursive: true});
			fs.mkdirSync(path.join(cwd, 'other dir'), {recursive: true});
			fs.writeFileSync(path.join(cwd, 'file.txt'), 'text\n');
			fs.writeFileSync(path.join(cwd, 'name with spaces.txt'), '');
			fs.writeFileSync(path.join(cwd, 'archive.tar.gz'), '');
			fs.writeFileSync(path.join(cwd, '.hidden'), '');
			fs.writeFileSync(path.join(cwd, 'run.sh'), '#!/bin/sh\n', {mode: 0o755});
			fs.symlinkSync('directory', path.join(cwd, 'link-to-dir'));
			fs.symlinkSync('missing', path.join(cwd, 'broken-link'));
			fs.chmodSync(path.join(cwd, 'other dir'), 0o777);
			return {cwd, env: {CLICOLOR: '1', CLICOLOR_FORCE: '1', LSCOLORS: 'ExGxFxdaCxDaDahbadacec'}};
		},
		steps: [sh('ls -G'), {checkpoint: 'ls'}, sh('ls -G -a -1'), {checkpoint: 'ls-1'}, sh('ls -G -F -C'), {checkpoint: 'ls-F'}, sh('clear'), {checkpoint: 'cleared'}, sh('ls -G -1 | head -3')],
	},
	{
		name: 'sh-sgr', description: '/bin/sh: printf of SGR 16/bright colours, the 256-colour palette, truecolor (semicolon and colon forms), bold/dim/italic/underline variants/blink/inverse/hidden/strike/overline and their resets.',
		command: '/bin/sh', args: ['-i'], cols: 100, rows: 30, ready: PROMPT, exit: 'exit\r', prepare: plainDir('sgr'),
		steps: [
			sh(`for c in 30 31 32 33 34 35 36 37 90 91 92 93 94 95 96 97; do printf '\\033[%sm fg%s \\033[0m' $c $c; done; echo`), {checkpoint: 'fg16'},
			sh(`for c in 40 41 42 43 44 45 46 47 100 101 102 103 104 105 106 107; do printf '\\033[%sm bg%s \\033[0m' $c $c; done; echo`), {checkpoint: 'bg16'},
			sh(`i=0; while [ $i -lt 256 ]; do printf '\\033[38;5;%sm%3s\\033[48;5;%sm \\033[0m' $i $i $i; i=$((i+1)); done; echo`), {checkpoint: 'palette256'},
			sh(`i=0; while [ $i -lt 64 ]; do printf '\\033[48;2;%s;%s;%sm \\033[0m' $((i*4)) $((255-i*4)) 128; i=$((i+1)); done; echo`),
			sh(`printf '\\033[38:2::255:128:0mcolon-rgb\\033[0m \\033[38:5:208mcolon-256\\033[0m \\033[38;2;10;20;30;48;2;200;210;220mboth\\033[39;49m default\\n'`), {checkpoint: 'truecolor'},
			sh(`printf '\\033[1mbold\\033[22m \\033[2mdim\\033[22m \\033[3mitalic\\033[23m \\033[4munder\\033[24m \\033[21mdouble\\033[24m \\033[4:3mcurly\\033[4:0m \\033[58;2;255;0;0m\\033[4mred-ul\\033[59;24m\\n'`),
			sh(`printf '\\033[5mblink\\033[25m \\033[7minverse\\033[27m \\033[8mhidden\\033[28m \\033[9mstrike\\033[29m \\033[53mover\\033[55m \\033[1;3;4;7;31;42mall\\033[0m \\033[1;31mbold-red\\033[0m \\033[1;90mbright\\033[m\\n'`), {checkpoint: 'attributes'},
		],
	},
	{
		name: 'sh-cursor', description: '/bin/sh: cursor movement (CUP/CUU/CUD/CUF/CUB/CHA/VPA), erase in line/display, insert/delete, scroll regions with scrolling, tabs, save/restore, clear.',
		command: '/bin/sh', args: ['-i'], cols: 60, rows: 16, ready: PROMPT, exit: 'exit\r', prepare: plainDir('cursor'),
		steps: [
			sh(`printf '\\033[2J\\033[H0123456789\\033[3;5Habc\\033[2Aup\\033[3Bdown\\033[4Cright\\033[2Dleft\\033[20Gcol20\\033[8dvpa8\\n'`), {checkpoint: 'moves'},
			sh(`printf 'erase-me-to-the-end\\033[9D\\033[K|\\nerase-the-start\\033[6D\\033[1K|\\nwhole-line\\033[2K|\\n'`), {checkpoint: 'erase-line'},
			sh(`printf '\\033[5;1Hfill\\033[5;3H\\033[J'; printf '\\033[2;10H\\033[1J\\n'`), {checkpoint: 'erase-display'},
			sh(`printf '\\033[2J\\033[H'; i=1; while [ $i -le 14 ]; do printf 'row %s\\n' $i; i=$((i+1)); done; printf '\\033[3;8r\\033[8;1H'; i=1; while [ $i -le 4 ]; do printf '\\nregion %s' $i; i=$((i+1)); done; printf '\\033[3;1H\\033M\\033Mrev\\033[r\\033[15;1H'`), {checkpoint: 'scroll-region'},
			sh(`printf 'a\\tb\\tc\\td\\te\\tf\\tg\\th\\n12345678\\t9\\n\\033[3g\\033[5G\\033H\\033[1Gx\\ty\\ty\\n\\033[3g'`), {checkpoint: 'tabs'},
			sh(`printf 'abcdefgh\\033[3D\\033[2@++\\033[2P\\033[3X\\n\\0337\\033[1;1HSAVED\\0338restored \\033[s\\033[2;1Hcsi-saved\\033[urestored-csi\\n'`), {checkpoint: 'edit-and-save'},
			sh(`printf '\\033[2;1H\\033[3L\\033[6;1H\\033[2M'`),
			sh('clear'), {checkpoint: 'cleared'},
		],
	},
	{
		name: 'sh-unicode', description: '/bin/sh: wide CJK, emoji (ZWJ, flags, variation selectors), combining marks, wrapping exactly at the last column and wide chars at the edge, then resizes that reflow the wrapped lines.',
		command: '/bin/sh', args: ['-i'], cols: 40, rows: 12, ready: PROMPT, exit: 'exit\r', prepare: plainDir('unicode'),
		steps: [
			sh(`printf '日本語のテキスト 中文 한국어\\n'`), {checkpoint: 'cjk'},
			sh(`printf '🎉 👩‍💻 👨‍👩‍👧 🇯🇵 ❤️ ☺︎ 1️⃣ end\\n'`), {checkpoint: 'emoji'},
			sh(`printf 'e\\314\\201 a\\314\\210\\314\\243 n\\314\\203 Z\\315\\221\\315\\222\\n'`), {checkpoint: 'combining'},
			sh(`printf '%s\\n' 1234567890123456789012345678901234567890 next`), {checkpoint: 'exact-width'},
			sh(`printf '12345678901234567890123456789012345678日本\\n123456789012345678901234567890123456789日\\n'`), {checkpoint: 'wide-at-edge'},
			sh(`printf 'a long line that wraps around the forty column terminal more than once: 日本語 🎉 done\\n'`), {checkpoint: 'wrapped'},
			{resize: [30, 12], quietMs: 300}, {checkpoint: 'resized-30'},
			{resize: [55, 12], quietMs: 300}, {checkpoint: 'resized-55'},
			sh(`printf 'after resize 日本 🎉\\n'`),
		],
	},
	{
		name: 'sh-modes', description: '/bin/sh: alternate screen (1049, 47, 1047 and tput smcup/rmcup), DEC 2026 synchronized output, bracketed paste and mouse/focus/app-cursor toggles, cursor visibility, OSC 0/2 titles and OSC 8 hyperlinks.',
		command: '/bin/sh', args: ['-i'], cols: 80, rows: 24, ready: PROMPT, exit: 'exit\r', prepare: plainDir('modes'),
		steps: [
			sh(`printf 'before alt\\n'`),
			sh(`printf '\\033[?1049h\\033[H\\033[2Jinside 1049\\033[5;10Hmiddle'`), {checkpoint: 'alt-1049'},
			sh(`printf '\\033[?1049l'`), {checkpoint: 'alt-1049-left'},
			sh(`printf '\\033[?47hinside 47\\033[?47l \\033[?1047hinside 1047\\033[?1047l\\n'`),
			sh('tput smcup; printf "tput alt\\n"'), {checkpoint: 'tput-smcup'},
			sh('tput rmcup'), {checkpoint: 'tput-rmcup'},
			sh(`printf '\\033[?2026hsynchronized\\033[2;1Hframe\\033[?2026l\\n'`), {checkpoint: 'sync'},
			sh(`printf '\\033[?2004h\\033[?1000h\\033[?1006h\\033[?1004h\\033[?1h\\033=\\033[?25lmodes on\\n'`), {checkpoint: 'modes-on'},
			sh(`printf '\\033[?2004l\\033[?1000l\\033[?1006l\\033[?1004l\\033[?1l\\033>\\033[?25hmodes off\\n'`), {checkpoint: 'modes-off'},
			sh(`printf '\\033]0;title zero\\007\\033]2;title two\\033\\\\titled\\n'`), {checkpoint: 'titles'},
			sh(`printf '\\033]8;;https://example.invalid/page\\033\\\\a link\\033]8;;\\033\\\\ and \\033]8;id=x;file:///tmp/f\\007another\\033]8;;\\007\\n'`), {checkpoint: 'hyperlinks'},
		],
	},
	{
		name: 'sh-scrollback', description: '/bin/sh: long outputs filling scrollback (seq, a 200-column line), resizes narrower/wider/shorter while scrollback exists, scroll offsets deep into it.',
		command: '/bin/sh', args: ['-i'], cols: 50, rows: 10, ready: PROMPT, exit: 'exit\r', prepare: plainDir('scrollback'),
		scrollOffsets: [0, 1, 5, 40, 100, 1_000_000, -1],
		steps: [
			sh('seq 1 150'), {checkpoint: 'seq'},
			sh(`printf '%0200d\\n' 7`), {checkpoint: 'long-line'},
			{resize: [30, 10], quietMs: 300}, {checkpoint: 'narrower'},
			{resize: [70, 15], quietMs: 300}, {checkpoint: 'wider-taller'},
			{resize: [70, 5], quietMs: 300}, {checkpoint: 'shorter'},
			sh('seq 1 5'),
		],
	},
	{
		name: 'lazygit', description: 'lazygit in a throwaway repository (temp HOME and XDG dirs, startup popups and update checks off): start, move between panels, open a commit, the keybindings menu, a resize, quit.',
		command: 'lazygit', args: [], cols: 100, rows: 30, ready: /Commits|Status/, exit: 'q',
		prepare: sandbox => {
			const cwd = path.join(sandbox, 'fixture-repo');
			fs.mkdirSync(cwd, {recursive: true});
			const env = baseEnv(sandbox);
			const gitEnv = {...process.env, ...env, GIT_AUTHOR_NAME: 'Fixture', GIT_AUTHOR_EMAIL: 'fixture@example.invalid', GIT_COMMITTER_NAME: 'Fixture', GIT_COMMITTER_EMAIL: 'fixture@example.invalid', GIT_AUTHOR_DATE: '2026-09-01T12:00:00Z', GIT_COMMITTER_DATE: '2026-09-01T12:00:00Z'};
			const git = (...args: string[]) => execFileSync('git', args, {cwd, env: gitEnv, stdio: 'ignore'});
			git('init', '-q', '-b', 'main');
			git('config', 'commit.gpgsign', 'false');
			for (const [index, file] of ['README.md', 'src/app.ts', 'src/util.ts'].entries()) {
				fs.mkdirSync(path.dirname(path.join(cwd, file)), {recursive: true});
				fs.writeFileSync(path.join(cwd, file), `content ${index}\n`);
				git('add', '.');
				git('commit', '-q', '-m', `commit ${index + 1}: add ${file}`);
			}
			git('checkout', '-q', '-b', 'feature');
			fs.writeFileSync(path.join(cwd, 'src/app.ts'), 'changed\n');
			fs.writeFileSync(path.join(cwd, 'untracked.txt'), 'new\n');
			const config = path.join(env.XDG_CONFIG_HOME!, 'lazygit', 'config.yml');
			fs.mkdirSync(path.dirname(config), {recursive: true});
			fs.writeFileSync(config, 'disableStartupPopups: true\nupdate:\n  method: never\ngui:\n  showRandomTip: false\n  nerdFontsVersion: ""\n  showIcons: false\nnotARepository: quit\n');
			return {cwd, env: {LG_CONFIG_FILE: config}};
		},
		steps: [
			{wait: 1500}, {checkpoint: 'started'},
			{send: '2', quietMs: 500}, {send: 'j', quietMs: 400}, {checkpoint: 'files'},
			{send: '4', quietMs: 500}, {send: 'j', quietMs: 400}, {send: '\r', quietMs: 700}, {checkpoint: 'commit-files'},
			{send: '\x1b', quietMs: 500}, {send: '?', quietMs: 700}, {checkpoint: 'keybindings'},
			{send: '\x1b', quietMs: 500},
			{resize: [80, 24], quietMs: 900}, {checkpoint: 'resized'},
		],
	},
];

async function recordSession(session: Session): Promise<void> {
	const sandbox = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'deckhand-term-record-')));
	try {
		const {cwd, env: extra} = session.prepare(sandbox);
		const env = {...baseEnv(sandbox), ...extra};
		const stream = new StreamBuilder(session.name, session.command === 'lazygit' ? 'lazygit' : 'sh', session.description, session.cols, session.rows, session.scrollOffsets);
		// The sandbox PATH is minimal; the program itself is resolved on the recorder's PATH.
		const file = session.command.includes('/') ? session.command : execFileSync('/usr/bin/which', [session.command], {encoding: 'utf8'}).trim();
		const term = pty.spawn(file, session.args, {name: 'xterm-256color', cols: session.cols, rows: session.rows, cwd, env, encoding: null} as pty.IPtyForkOptions);
		let text = '', lastData = Date.now(), exited = false;
		term.onData(data => {
			const bytes = Buffer.isBuffer(data) ? data : Buffer.from(data as unknown as string, 'utf8');
			stream.chunk(bytes);
			text += bytes.toString('utf8');
			lastData = Date.now();
		});
		const ended = new Promise<void>(resolve => term.onExit(() => { exited = true; resolve(); }));
		const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
		const settle = async (until: RegExp | undefined, from: number, quietMs: number) => {
			const deadline = Date.now() + STEP_TIMEOUT_MS;
			while (until && !until.test(text.slice(from).replace(STRIP, ''))) {
				if (Date.now() > deadline) throw new Error(`${session.name}: timed out waiting for ${until} after: ${JSON.stringify(text.slice(from).slice(-300))}`);
				if (exited) throw new Error(`${session.name}: exited early: ${JSON.stringify(text.slice(-500))}`);
				await sleep(20);
			}
			while (Date.now() - lastData < quietMs) await sleep(20);
		};
		await settle(session.ready, 0, 400);
		const done: Step[] = [];
		for (const step of session.steps) {
			if ('checkpoint' in step) { stream.checkpoint(step.checkpoint); done.push(step); continue; }
			if ('wait' in step) { await sleep(step.wait); await settle(undefined, 0, 300); continue; }
			const from = text.length;
			if ('send' in step) term.write(step.send);
			else { stream.resize(step.resize[0], step.resize[1]); term.resize(step.resize[0], step.resize[1]); }
			await settle(step.until, from, step.quietMs ?? 150);
			done.push(step);
		}
		term.write(session.exit);
		await Promise.race([ended, sleep(5000)]);
		if (!exited) term.kill();
		await sleep(100);
		const {meta, bytes} = stream.build({
			command: [session.command, ...session.args].join(' '),
			environment: Object.fromEntries(Object.entries(env).map(([key, value]) => [key, value.startsWith(sandbox) ? `$SANDBOX${value.slice(sandbox.length)}` : value])),
			steps: session.steps.map(step => 'send' in step ? {send: step.send} : 'resize' in step ? {resize: step.resize} : step),
			exit: session.exit,
		});
		writeStream(meta, bytes);
		console.log(`recorded ${session.name}: ${bytes.length} bytes in ${meta.chunks.length} chunks`);
	} finally {
		fs.rmSync(sandbox, {recursive: true, force: true});
	}
}

async function main(): Promise<void> {
	const only = new Set(process.argv.slice(2));
	const wanted = (name: string) => !only.size || only.has(name);
	for (const [name, build] of Object.entries(SYNTHETIC_STREAMS)) {
		if (!wanted(name)) continue;
		const {meta, bytes} = build().build({generator: 'parity/gen/streams/synthetic.ts'});
		writeStream(meta, bytes);
		console.log(`wrote ${name}: ${bytes.length} bytes in ${meta.chunks.length} chunks`);
	}
	await ensureNodePtyReady();
	for (const session of SESSIONS) {
		if (!wanted(session.name)) continue;
		if (session.command === 'lazygit') {
			try { execFileSync('lazygit', ['--version'], {stdio: 'ignore', env: {...process.env, HOME: os.tmpdir()}}); }
			catch { console.log('skipped lazygit: not installed'); continue; }
		}
		try { await recordSession(session); }
		catch (error) {
			if (session.command !== 'lazygit') throw error;
			console.log(`skipped lazygit: ${error instanceof Error ? error.message : String(error)}`);
		}
	}
}

await main();
