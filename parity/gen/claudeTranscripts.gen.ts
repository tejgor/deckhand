/**
 * Fixtures for src/claudeTranscripts.ts: claudeProjectKey / claudeProjectsDir as direct calls, and
 * latestClaudeConversation as directory cases (the projects dir is part of the tree; file mtimes are real "now",
 * always later than the 2026 timestamps in the transcripts, so the successor filter is deterministic).
 *   node --import tsx parity/gen/claudeTranscripts.gen.ts
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, installDeterminism, recordDirCase} from '../lib.ts';
import {claudeProjectKey, claudeProjectsDir, latestClaudeConversation} from '../../src/claudeTranscripts.ts';

const MODULE = 'claudeTranscripts';
installDeterminism();
const recorder = new Recorder(MODULE);

for (const cwd of [
	'/work/repo', '/Users/me/.deckhand/worktrees/a_b', '/', '/a/b/../c', '/a//b/./c/', '/with space/日本語/😀', '/é/é/ß',
	'/Users/me/a-b_c.d', '/tab\there', '/new\nline', '/UPPER/lower/0123456789', '/x/' + 'long'.repeat(80),
]) recorder.call('claudeProjectKey', claudeProjectKey, [cwd]);

for (const env of [
	{CLAUDE_CONFIG_DIR: '/cfg/claude'}, {CLAUDE_CONFIG_DIR: '/cfg/claude/'}, {CLAUDE_CONFIG_DIR: '/cfg/../other//claude'},
	{CLAUDE_CONFIG_DIR: '/日本語/😀'}, {CLAUDE_CONFIG_DIR: '/', HOME: '/h'}, {CLAUDE_CONFIG_DIR: '/a', OTHER: 'x'},
]) recorder.call('claudeProjectsDir', claudeProjectsDir, [env]);
await recorder.write({prune: true});

const line = (record: object) => `${JSON.stringify(record)}\n`;
const at = (minutes: number) => new Date(Date.UTC(2026, 0, 1, 12, minutes)).toISOString();
function transcript(title: string | undefined, start: number, end: number, prefix: string, from: string | null = null): string {
	return (title ? line({type: 'custom-title', customTitle: title}) : '')
		+ line({type: 'user', uuid: `${prefix}-1`, parentUuid: from, timestamp: at(start)})
		+ line({type: 'assistant', uuid: `${prefix}-2`, parentUuid: `${prefix}-1`, timestamp: at(end)});
}

const CWD = '/work/repo';
const KEY = claudeProjectKey(CWD);
type File = [string, string]; // "<project dir>/<name>" -> text, written in order
const f = (id: string, text: string, key = KEY): File => [`${key}/${id}.jsonl`, text];

async function dirCase(name: string, files: File[], id: string, opts: {hints?: string[] | null; exclude?: string[]; noProjects?: boolean} = {}) {
	await recordDirCase(MODULE, name, {
		setup: async root => {
			const projects = path.join(root, 'projects');
			if (!opts.noProjects) await fs.mkdir(projects, {recursive: true});
			for (const [rel, text] of files) {
				await fs.mkdir(path.dirname(path.join(projects, rel)), {recursive: true});
				await fs.writeFile(path.join(projects, rel), text);
				await new Promise(resolve => setTimeout(resolve, 5)); // distinct, ordered mtimes
			}
		},
		fn: 'latestClaudeConversation',
		args: root => [id, {
			projectsDir: path.join(root, 'projects'),
			...(opts.hints === null ? {} : {cwdHints: opts.hints ?? [CWD]}),
			...(opts.exclude ? {exclude: new Set(opts.exclude)} : {}),
		}],
		run: latestClaudeConversation,
	});
}

const T = 'dh-task-1234';
await dirCase('missing-id', [f('plan', transcript(T, 0, 10, 'plan'))], 'missing');
await dirCase('no-projects-dir', [], 'plan', {noProjects: true});
await dirCase('no-successor', [f('plan', transcript(T, 0, 10, 'plan'))], 'plan');
await dirCase('plan-accepted', [f('plan', transcript(T, 0, 10, 'plan')), f('impl', transcript(T, 11, 30, 'impl', 'plan-2'))], 'plan');
const chain = [f('plan', transcript(T, 0, 10, 'plan')), f('impl', transcript(T, 11, 30, 'impl', 'plan-2')), f('impl2', transcript(T, 31, 40, 'impl2', 'impl-2'))];
await dirCase('plan-chain-from-plan', chain, 'plan');
await dirCase('plan-chain-from-impl', chain, 'impl');
await dirCase('plan-chain-from-latest', chain, 'impl2');
const renamed = [
	f('other', transcript('dh-other-9999', 41, 50, 'other')), f('a', transcript(undefined, 0, 5, 'a')),
	f('b', transcript('renamed', 6, 8, 'b', 'a-2')), f('c', transcript(undefined, 9, 12, 'c', 'b-2')),
];
await dirCase('parent-link-transitive', renamed, 'a');
await dirCase('parent-link-unrelated-not-followed', renamed, 'other');
await dirCase('resumed-earlier-wins', [f('plan', transcript(T, 0, 55, 'plan')), f('impl', transcript(T, 11, 30, 'impl', 'plan-2'))], 'plan');
const fork = [f('x', transcript('dh-x-0000', 0, 5, 'x')), f('fork', transcript('dh-x-0000', 6, 20, 'fork', 'x-2'))];
await dirCase('fork-excluded', fork, 'x', {exclude: ['fork']});
await dirCase('fork-followed', fork, 'x');
await dirCase('exclude-unrelated', fork, 'x', {exclude: ['nope']});
await dirCase('hint-mismatch-found-in-other-project', chain, 'impl', {hints: ['/elsewhere']});
await dirCase('no-hints', chain, 'plan', {hints: null});
await dirCase('empty-hints', chain, 'plan', {hints: []});
await dirCase('other-project-dir-searched-in-order', [f('plan', transcript(T, 0, 10, 'plan'), 'zzz'), f('impl', transcript(T, 11, 30, 'impl', 'plan-2'), 'zzz'), f('plan', transcript(T, 0, 10, 'plan'), 'aaa')], 'plan', {hints: null});
await dirCase('different-title-no-parent', [f('plan', transcript(T, 0, 10, 'plan')), f('impl', transcript('other', 11, 30, 'impl'))], 'plan');
await dirCase('untitled-not-matched-by-title', [f('a', transcript(undefined, 0, 10, 'a')), f('b', transcript(undefined, 11, 30, 'b'))], 'a');
await dirCase('successor-started-before-not-followed', [f('plan', transcript(T, 20, 30, 'plan')), f('old', transcript(T, 0, 10, 'old'))], 'plan');
await dirCase('successor-same-start-not-followed', [f('plan', transcript(T, 5, 30, 'plan')), f('same', transcript(T, 5, 40, 'same'))], 'plan');
await dirCase('successor-quiet-older-activity', [f('plan', transcript(T, 0, 30, 'plan')), f('impl', transcript(T, 11, 20, 'impl', 'plan-2'))], 'plan');
await dirCase('stored-without-timestamps', [f('plan', line({type: 'custom-title', customTitle: T}) + line({type: 'user', uuid: 'plan-1'})), f('impl', transcript(T, 11, 30, 'impl', 'plan-1'))], 'plan');
await dirCase('title-first-wins', [f('plan', line({type: 'custom-title', customTitle: T}) + line({type: 'custom-title', customTitle: 'second'}) + line({type: 'user', uuid: 'plan-1', timestamp: at(0)})), f('impl', transcript('second', 11, 30, 'impl')), f('impl2', transcript(T, 12, 31, 'impl2'))], 'plan');
await dirCase('non-string-title-and-parent', [f('plan', line({type: 'custom-title', customTitle: 5}) + line({type: 'user', uuid: 'plan-1', timestamp: at(0)})), f('impl', line({type: 'user', uuid: 'i-1', parentUuid: 7, timestamp: at(11)}))], 'plan');
await dirCase('bad-lines-and-non-objects', [f('plan', 'not json\n42\nnull\n"str"\n[1]\n' + transcript(T, 0, 10, 'plan')), f('impl', '{broken\n' + transcript(T, 11, 30, 'impl', 'plan-2') + '\n\n')], 'plan');
await dirCase('bad-timestamp', [f('plan', transcript(T, 0, 10, 'plan')), f('impl', line({type: 'user', uuid: 'i-1', parentUuid: 'plan-2', timestamp: 'garbage'}))], 'plan');
await dirCase('metadata-after-messages-ignored', [f('plan', transcript(T, 0, 10, 'plan') + line({type: 'custom-title', customTitle: T, timestamp: at(90)})), f('impl', transcript(T, 11, 30, 'impl', 'plan-2'))], 'plan');
await dirCase('non-jsonl-and-dirs-ignored', [f('plan', transcript(T, 0, 10, 'plan')), [`${KEY}/notes.txt`, transcript(T, 11, 30, 'n')], [`${KEY}/sub.jsonl/inner`, 'x'], f('impl', transcript(T, 11, 30, 'impl', 'plan-2'))], 'plan');
await dirCase('unicode-title-and-ids', [f('plán-😀', transcript('タスク 😀', 0, 10, 'p')), f('继续', transcript('タスク 😀', 11, 30, 'q', 'p-2'))], 'plán-😀');
await dirCase('uuid-substring-not-matched', [f('a', transcript(undefined, 0, 5, 'a')), f('b', transcript(undefined, 6, 8, 'b', 'a-'))], 'a');
await dirCase('large-records-beyond-64k', [f('plan', transcript(T, 0, 10, 'plan') + line({type: 'padding', pad: 'x'.repeat(70 * 1024)})), f('impl', transcript(T, 11, 30, 'impl', 'plan-2') + line({type: 'padding', pad: 'y'.repeat(70 * 1024)}))], 'plan');
await dirCase('first-message-beyond-head-window', [f('plan', line({type: 'padding', pad: 'x'.repeat(70 * 1024)}) + transcript(T, 0, 10, 'plan')), f('impl', transcript(T, 11, 30, 'impl', 'plan-2'))], 'plan');
