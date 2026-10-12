import assert from 'node:assert/strict';
import {test} from 'node:test';
import fs from 'node:fs/promises';
import path from 'node:path';
import {tempDir} from './helpers.js';
import {claudeProjectKey, latestClaudeConversation} from '../src/claudeTranscripts.js';

const line = (record: object) => `${JSON.stringify(record)}\n`;
const at = (minutes: number) => new Date(Date.UTC(2026, 0, 1, 12, minutes)).toISOString();
/** A transcript as Claude writes it: the launch title first (unless none), then messages; `from` is the parent of the first. */
function transcript(title: string | undefined, start: number, end: number, prefix: string, from: string | null = null): string {
	return (title ? line({type: 'custom-title', customTitle: title}) : '')
		+ line({type: 'user', uuid: `${prefix}-1`, parentUuid: from, timestamp: at(start)})
		+ line({type: 'assistant', uuid: `${prefix}-2`, parentUuid: `${prefix}-1`, timestamp: at(end)});
}

test('restart follows the conversation a Claude session continued in, from its transcripts', async t => {
	const projects = await tempDir(t, 'claude-projects-');
	const cwd = '/work/repo';
	const dir = path.join(projects, claudeProjectKey(cwd));
	await fs.mkdir(dir, {recursive: true});
	const write = (id: string, text: string) => fs.writeFile(path.join(dir, `${id}.jsonl`), text);
	const latest = (id: string, exclude: string[] = []) => latestClaudeConversation(id, {projectsDir: projects, cwdHints: [cwd], exclude: new Set(exclude)});
	assert.equal(claudeProjectKey('/Users/me/.deckhand/worktrees/a_b'), '-Users-me--deckhand-worktrees-a-b');

	// Unknown conversation, or no successor: nothing to change.
	assert.equal(await latest('missing'), undefined);
	await write('plan', transcript('dh-task-1234', 0, 10, 'plan'));
	assert.equal(await latest('plan'), undefined);

	// Accepting the plan with "clear context": same title, first message continues the plan. Then accepted again.
	await write('impl', transcript('dh-task-1234', 11, 30, 'impl', 'plan-2'));
	assert.equal(await latest('plan'), 'impl');
	await write('impl2', transcript('dh-task-1234', 31, 40, 'impl2', 'impl-2'));
	assert.equal(await latest('plan'), 'impl2');
	assert.equal(await latest('impl'), 'impl2');
	assert.equal(await latest('impl2'), undefined); // Never back to an earlier conversation.

	// Linked by the parent message alone (the title was renamed), transitively; unrelated conversations are not followed.
	await write('other', transcript('dh-other-9999', 41, 50, 'other'));
	await write('a', transcript(undefined, 0, 5, 'a'));
	await write('b', transcript('renamed', 6, 8, 'b', 'a-2'));
	await write('c', transcript(undefined, 9, 12, 'c', 'b-2'));
	assert.equal(await latest('a'), 'c');

	// A conversation resumed and worked in after its successor stays the one to resume (latest message wins).
	await write('plan', transcript('dh-task-1234', 0, 55, 'plan'));
	assert.equal(await latest('plan'), undefined);

	// Another session's conversation (a Deckhand fork with the copied title) is never followed.
	await write('x', transcript('dh-x-0000', 0, 5, 'x'));
	await write('fork', transcript('dh-x-0000', 6, 20, 'fork', 'x-2'));
	assert.equal(await latest('x', ['fork']), undefined);
	assert.equal(await latest('x'), 'fork');

	// Found in another project when the cwd hint does not match (e.g. a path Claude shortened).
	assert.equal(await latestClaudeConversation('impl', {projectsDir: projects, cwdHints: ['/elsewhere']}), 'impl2');
	assert.equal(await latestClaudeConversation('a', {projectsDir: projects}), 'c');
});
