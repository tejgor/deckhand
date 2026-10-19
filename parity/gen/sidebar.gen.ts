/**
 * Fixtures for src/sidebar.tsx pure exports (partStyle, moreText). Regenerate with
 *
 *   node --import tsx parity/gen/sidebar.gen.ts
 *
 * Sidebar (the Ink component) is rendering, covered by ported tests, not recorded here.
 * Mutations are restricted to typed values (role/color unions, session shapes a Rust record can hold).
 */
import fc from 'fast-check';
import {Recorder, installDeterminism, mutate, sample} from '../lib.ts';
import {moreText, partStyle} from '../../src/sidebar.tsx';

installDeterminism();
const recorder = new Recorder('sidebar');

const ROLES = ['cursor', 'gutter', 'number', 'tree', 'status', 'title', 'gap', 'dev', 'archived', 'cleanup', 'merged', 'done', 'count', 'outdated', 'agent'];
const COLORS = ['red', 'green', 'yellow', 'cyan', 'gray', 'magenta', 'blue'];
const STATES = ['unknown', 'working', 'needs-input', 'response-ended', 'failed', 'limited'];
const KINDS = ['question', 'plan', 'permission', 'message', 'failure', 'notice'];
const typedPart = (p: any) => ROLES.includes(p?.role) && typeof p.text === 'string';
const typedRow = (r: any) => r && typeof r.selected === 'boolean' && typeof r.dimmed === 'boolean' && typeof r.done === 'boolean' && typeof r.color === 'string' && typeof r.id === 'string' && Array.isArray(r.parts);
const typedSession = (s: any) => s && typeof s.id === 'string' && ['starting', 'running', 'exited'].includes(s.status)
	&& (s.agentStatus === undefined || ['unknown', 'active', 'idle'].includes(s.agentStatus))
	&& (s.archivedAt === undefined || typeof s.archivedAt === 'string')
	&& (s.replyRead === undefined || typeof s.replyRead === 'boolean')
	&& (s.attention === undefined || (STATES.includes(s.attention.state) && typeof s.attention.at === 'string' && typeof s.attention.event === 'string'
		&& (s.attention.reason === undefined || (KINDS.includes(s.attention.reason.kind) && typeof s.attention.reason.text === 'string'))));

// partStyle: every role x selected x dimmed x done x color.
const row = (selected: boolean, dimmed: boolean, done: boolean, color: string) => ({id: 'a', parts: [], selected, dimmed, done, color});
for (const role of ROLES) for (const selected of [false, true]) for (const dimmed of [false, true]) for (const done of [false, true]) for (const color of ['red', 'green', 'gray']) {
	recorder.call('partStyle', partStyle, [{text: 'x', role}, row(selected, dimmed, done, color)]);
}
for (const [role, text, selected, dimmed, done, color] of sample(fc.tuple(fc.constantFrom(...ROLES), fc.constantFrom('', '›', '😀', '中', ' '), fc.boolean(), fc.boolean(), fc.boolean(), fc.constantFrom(...COLORS)), 80, 1)) {
	recorder.call('partStyle', partStyle, [{text, role}, row(selected, dimmed, done, color)]);
}
for (const args of mutate(recorder.argsOf('partStyle').slice(0, 60), {variants: 2}).filter(([p, r]) => typedPart(p) && typedRow(r))) recorder.call('partStyle', partStyle, args);

// moreText.
const stamp = fc.constantFrom('2026-01-01T00:00:00.000Z', '2025-12-31T23:00:00.000Z');
const session = fc.record({
	id: fc.constantFrom('a', 'b', 'c'), title: fc.constantFrom('t', '😀', '中文'), program: fc.constantFrom('claude', 'pi', 'codex'), command: fc.constant('x'), cwd: fc.constant('/r'), repoRoot: fc.constant('/r'),
	status: fc.constantFrom('starting', 'running', 'exited'), createdAt: stamp, updatedAt: stamp,
	agentStatus: fc.option(fc.constantFrom('unknown', 'active', 'idle'), {nil: undefined}),
	archivedAt: fc.option(stamp, {nil: undefined}),
	replyRead: fc.option(fc.boolean(), {nil: undefined}),
	exitReason: fc.option(fc.constantFrom('stopped', 'failed', 'completed', 'interrupted'), {nil: undefined}),
	attention: fc.option(fc.record({state: fc.constantFrom(...STATES), event: fc.constant('Stop'), at: stamp, reason: fc.option(fc.record({kind: fc.constantFrom(...KINDS), text: fc.constantFrom('q?', '😀')}), {nil: undefined})}, {requiredKeys: ['state', 'event', 'at']}), {nil: undefined}),
}, {requiredKeys: ['id', 'title', 'program', 'command', 'cwd', 'repoRoot', 'status', 'createdAt', 'updatedAt']}) as fc.Arbitrary<any>;
for (const arrow of ['↑', '↓']) recorder.call('moreText', moreText, [arrow, []]);
for (const [arrow, hidden] of sample(fc.tuple(fc.constantFrom('↑', '↓', '', '😀', '>'), fc.array(session, {maxLength: 8})), 300, 2)) recorder.call('moreText', moreText, [arrow, hidden]);
for (const args of mutate(recorder.argsOf('moreText').slice(0, 120), {variants: 2}).filter(([a, h]) => typeof a === 'string' && Array.isArray(h) && h.every(typedSession))) recorder.call('moreText', moreText, args);

await recorder.write({prune: true});
