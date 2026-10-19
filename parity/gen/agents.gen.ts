/** Records src/agents.ts. Regenerate: node --import tsx parity/gen/agents.gen.ts */
import {Recorder, arb, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {agentSpec, forksParentAgain, launchArgs, newAgentRef, relaunchPlan, sameAgentSessionRef, type LaunchPlan} from '../../src/agents.ts';
import type {AgentSessionRef, ProgramKey} from '../../src/types.ts';

installDeterminism();
const recorder = new Recorder('agents');
const PROGRAMS: ProgramKey[] = ['claude', 'pi', 'codex'];
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// The tests' calls (launchArgs; the AGENTS table is read directly in them).
recorder.addTap(await runTap({tests: ['tests/agents.test.ts'], tap: ['agents:launchArgs']}), {module: 'agents'});

// relaunchPlan / newAgentRef pick a random UUID for agents that take an ID at launch (the Rust port generates its
// own). Recorded wrappers replace a freshly generated UUID (one not present in the input) with "<new-uuid>".
const NEW = '<new-uuid>';
function scrubPlan(plan: LaunchPlan | undefined, known: string[]): unknown {
	if (!plan) return plan;
	const copy: any = JSON.parse(JSON.stringify(plan));
	if (copy.ref && UUID.test(copy.ref.value) && !known.includes(copy.ref.value)) copy.ref.value = NEW;
	return copy;
}
const knownValues = (...refs: Array<AgentSessionRef | undefined>) => refs.flatMap(ref => ref ? [ref.value] : []);
const newAgentRefScrubbed = (program: ProgramKey) => {
	const ref = newAgentRef(program);
	return ref && UUID.test(ref.value) ? {...ref, value: NEW} : ref;
};
const relaunchPlanScrubbed = (session: any, mode: 'resume' | 'fresh', neverStarted: boolean, name: string) =>
	scrubPlan(relaunchPlan(session, mode, neverStarted, name), knownValues(session.agentSessionRef, session.forkedFromAgentSessionRef));

// Spec data: the function-valued members are exercised through the wrappers below.
const specData = (program: ProgramKey) => {
	const spec = agentSpec(program);
	const {label, idAtLaunch, forks, forksAcrossDirectories, version} = spec;
	return {label, idAtLaunch, forks, forksAcrossDirectories, version, hasExitRef: Boolean(spec.exitRef), hasMissingConversation: Boolean(spec.missingConversation), hasForkFailed: Boolean(spec.forkFailed)};
};
for (const program of PROGRAMS) recorder.call('agentSpec', specData, [program]);
for (const program of PROGRAMS) recorder.call('newAgentRef', newAgentRefScrubbed, [program]);

const exitRef = (program: ProgramKey, output: string) => agentSpec(program).exitRef?.(output);
const missingConversation = (program: ProgramKey, output: string) => agentSpec(program).missingConversation?.(output);
const forkFailed = (program: ProgramKey, output: string) => agentSpec(program).forkFailed?.(output);

const U1 = '0f8fad5b-d9cb-469f-a716-446655440000', U2 = 'A1B2C3D4-0000-4000-8000-000000000001';
const refs = (provider: ProgramKey): AgentSessionRef[] => [
	{provider, kind: 'id', value: U1}, {provider, kind: 'id', value: 'plain-id_1'}, {provider, kind: 'name', value: 'my session 😀'},
	{provider, kind: 'name', value: '中文 name'}, {provider, kind: 'path', value: '/tmp/a b/session.jsonl'}, {provider, kind: 'id', value: ''},
];
const allRefs = PROGRAMS.flatMap(refs);
const refArb = fc.record({provider: fc.constantFrom(...PROGRAMS), kind: fc.constantFrom('id', 'name', 'path'), value: fc.oneof(arb.text(12), fc.constantFrom(U1, U2, 'x'))});
const names = ['', 'work', 'Fix: ünï 😀', '中文', 'a\nb', '--flag', "it's"];

// launchArgs
const plans: LaunchPlan[] = [];
for (const name of names) {
	plans.push({kind: 'new', name});
	for (const ref of allRefs) { plans.push({kind: 'new', ref, name}); plans.push({kind: 'fork', parent: ref, name}); plans.push({kind: 'fork', parent: ref, ref: allRefs[0]!, name}); }
}
for (const ref of allRefs) plans.push({kind: 'resume', ref});
for (const program of PROGRAMS) for (const plan of plans) recorder.call('launchArgs', launchArgs, [program, plan]);
for (const [program, ref, parent, name] of sample(fc.tuple(fc.constantFrom(...PROGRAMS), refArb, refArb, arb.text(10)), 150)) {
	recorder.call('launchArgs', launchArgs, [program, {kind: 'new', ref, name}]);
	recorder.call('launchArgs', launchArgs, [program, {kind: 'fork', parent, ref, name}]);
	recorder.call('launchArgs', launchArgs, [program, {kind: 'resume', ref: parent}]);
}
for (const args of mutate(recorder.argsOf('launchArgs').slice(0, 60), {variants: 2}).filter(a => PROGRAMS.includes(a[0] as ProgramKey))) recorder.call('launchArgs', launchArgs, args);

// sameAgentSessionRef
const maybe = [undefined, ...allRefs.slice(0, 7), {provider: 'claude', kind: 'id', value: U1}, {provider: 'codex', kind: 'id', value: U1}, {provider: 'claude', kind: 'name', value: U1}];
for (const left of maybe) for (const right of maybe) recorder.call('sameAgentSessionRef', sameAgentSessionRef, [left, right]);
for (const [left, right] of sample(fc.tuple(fc.option(refArb, {nil: undefined}), fc.option(refArb, {nil: undefined})), 100)) recorder.call('sameAgentSessionRef', sameAgentSessionRef, [left, right]);

// forksParentAgain / relaunchPlan
const sessions: any[] = [];
for (const program of PROGRAMS) for (const subSessionKind of [undefined, 'forked', 'clean']) {
	const parent = {provider: program, kind: 'id', value: U1} as AgentSessionRef;
	for (const agentSessionRef of [undefined, parent, {provider: program, kind: 'id', value: 'own-1'} as AgentSessionRef, {provider: program, kind: 'name', value: U1} as AgentSessionRef]) {
		for (const forkedFromAgentSessionRef of [undefined, parent]) sessions.push({program, agentSessionRef, subSessionKind, forkedFromAgentSessionRef});
	}
}
for (const session of sessions) for (const neverStarted of [false, true]) {
	recorder.call('forksParentAgain', forksParentAgain, [session, neverStarted]);
	for (const mode of ['resume', 'fresh'] as const) for (const name of ['child', '😀 中']) recorder.call('relaunchPlan', relaunchPlanScrubbed, [session, mode, neverStarted, name]);
}
for (const [program, kind, own, parent, neverStarted, mode, name] of sample(fc.tuple(fc.constantFrom(...PROGRAMS), fc.constantFrom(undefined, 'forked', 'clean'), fc.option(refArb, {nil: undefined}), fc.option(refArb, {nil: undefined}), fc.boolean(), fc.constantFrom('resume', 'fresh'), arb.text(8)), 200)) {
	const session = {program, agentSessionRef: own, subSessionKind: kind, forkedFromAgentSessionRef: parent};
	recorder.call('forksParentAgain', forksParentAgain, [session, neverStarted]);
	recorder.call('relaunchPlan', relaunchPlanScrubbed, [session, mode, neverStarted, name]);
}

// exit-screen readers
const ansi = '\x1b[1m', reset = '\x1b[0m';
const outputs = [
	'', 'claude --resume ' + U1, `\n  claude --resume ${U1}\n`, `${ansi}claude --resume${reset} ${U1}`, 'Resume this session with:\nclaude --resume "my session"',
	"claude --resume 'quoted name'", 'claude --resume=eq-name', 'CLAUDE  --RESUME  ABC', 'x claude --resume nope', 'claude --resume ""', 'claude --resume   ', 'claude --resume 中文 more',
	`No conversation found with session ID: ${U1}`, 'no conversation found with session id:   ' + U1.toUpperCase(), `${ansi}No conversation found with session ID: ${U1}${reset}`, 'No conversation found with session ID: short',
	'To continue this conversation, run codex resume 019a1b2c-3d4e', 'codex resume abcdefg', 'codex resume abcdefgh', 'codex   resume   abcdefgh_-12 trailing', `${ansi}codex resume abcdefgh${reset}`, 'codex resume -abcdefgh', 'codex resume abcdefgh!',
	'xcodex resume abcdefgh', 'codex resume ' + 'a'.repeat(128), 'codex resume ' + 'a'.repeat(129), 'No session found matching abc', 'error: No session found matching "x"', 'no session found matching',
	'\x1b[31mred\x1b[0m\x1b[?25h', '😀 claude --resume ' + U2, 'codex resume 😀😀😀😀😀😀😀😀',
];
for (const program of PROGRAMS) for (const output of outputs) {
	recorder.call('exitRef', exitRef, [program, output]);
	recorder.call('missingConversation', missingConversation, [program, output]);
	recorder.call('forkFailed', forkFailed, [program, output]);
}
for (const args of mutate(recorder.argsOf('exitRef'), {variants: 2}).filter(a => PROGRAMS.includes(a[0] as ProgramKey))) {
	recorder.call('exitRef', exitRef, args); recorder.call('missingConversation', missingConversation, args); recorder.call('forkFailed', forkFailed, args);
}
for (const program of PROGRAMS) for (const text of sample(arb.text(30), 40, 11)) recorder.call('exitRef', exitRef, [program, `claude --resume ${text}\ncodex resume ${text}`]);

await recorder.write({prune: true});
