import {randomUUID} from 'node:crypto';
import {codexResumeFromOutput} from './agentSignals.js';
import type {AgentSessionRef, ProgramKey, SessionRecord} from './types.js';

// One table of what each agent can do and how Deckhand launches it. The daemon decides *which* launch a session
// needs (relaunchPlan); this table only turns a plan into argv and reads the agent's exit screen.

/** One agent launch: a new conversation (create, a clean child, S), resuming one (s), or forking a parent's. */
export type LaunchPlan =
	| {kind: 'new'; ref?: AgentSessionRef; name: string}
	| {kind: 'resume'; ref: AgentSessionRef}
	| {kind: 'fork'; parent: AgentSessionRef; ref?: AgentSessionRef; name: string};

export interface AgentSpec {
	label: string;
	/** Deckhand picks the conversation ID at launch; otherwise it is captured later (SessionStart hook or exit hint). */
	idAtLaunch: boolean;
	/** A parent's conversation can be forked into a sub-session. */
	forks: boolean;
	/** Forking by ID works from another directory, so a forked child may run in a different worktree. */
	forksAcrossDirectories: boolean;
	args(plan: LaunchPlan): string[];
	/** The conversation the agent's exit screen says to resume, if it prints one. */
	exitRef?(output: string): AgentSessionRef | undefined;
	/** The ID the agent reported it has no saved conversation for (it exits instead of starting). */
	missingConversation?(output: string): string | undefined;
	/** The agent's exit screen says a fork found no parent conversation to copy. */
	forkFailed?(output: string): boolean;
	/** Versions: `<command> --version` (parsed to x.y.z by parseVersion), the latest from npm, and the agent's own updater. */
	version: AgentVersionSource;
}

export interface AgentVersionSource {
	/** npm package whose `latest` dist-tag is the newest release (`npm view <package> version`). */
	npmPackage: string;
	/** Arguments of the agent's own non-interactive update command. */
	updateArgs: string[];
}

const plain = (output: string) => output.replace(/\x1b\[[0-9;?]*[ -/]*[@-~]/g, '');
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export const AGENTS: Record<ProgramKey, AgentSpec> = {
	// claude --session-id <uuid> --name <label> · --resume <uuid|legacy name> · --resume <parent> --fork-session --session-id <child>.
	// Never --session-id with a plain --resume: Claude rejects it unless --fork-session is given, and refuses an ID in use.
	// --resume also accepts a name, so legacy parents with a name ref fork the same way.
	claude: {
		label: 'Claude',
		idAtLaunch: true,
		forks: true,
		forksAcrossDirectories: true, // Verified: the child is stored under the child's cwd project.
		args(plan) {
			if (plan.kind === 'resume') return ['--resume', plan.ref.value];
			if (plan.kind === 'fork') return ['--resume', plan.parent.value, '--fork-session', ...(plan.ref ? ['--session-id', plan.ref.value] : []), '--name', plan.name];
			if (plan.ref?.kind === 'id') return ['--session-id', plan.ref.value, '--name', plan.name];
			return plan.ref?.kind === 'name' ? ['--name', plan.ref.value] : [];
		},
		exitRef(output) {
			const match = plain(output).match(/(?:^|\n)\s*claude\s+--resume(?:=|\s+)(?:"([^"]+)"|'([^']+)'|(\S+))/i);
			const value = (match?.[1] ?? match?.[2] ?? match?.[3])?.trim();
			return value ? {provider: 'claude', kind: UUID_PATTERN.test(value) ? 'id' : 'name', value} : undefined;
		},
		missingConversation: output => plain(output).match(/No conversation found with session ID:\s*([0-9a-f-]{36})/i)?.[1],
		// `claude --version` prints `2.1.287 (Claude Code)`. The native builds are published to npm under the same
		// numbers; `latest` (not `stable`) is the channel `claude update` follows by default.
		version: {npmPackage: '@anthropic-ai/claude-code', updateArgs: ['update']},
	},
	// pi --session-id <uuid> --name <label> · --session-id <uuid> (opens it, or creates it if absent) · --fork <parent> --session-id <child>.
	// Legacy path refs use --session <path>. --fork copies the parent before the TUI starts, from any directory.
	pi: {
		label: 'Pi',
		idAtLaunch: true,
		forks: true,
		// --fork <id> falls back to a search of every project's sessions and writes the child into the cwd's project.
		forksAcrossDirectories: true,
		args(plan) {
			if (plan.kind === 'fork') return ['--fork', plan.parent.value, ...(plan.ref ? ['--session-id', plan.ref.value] : []), '--name', plan.name];
			const ref = plan.ref;
			if (ref?.kind === 'path') return ['--session', ref.value];
			if (ref?.kind !== 'id') return [];
			return plan.kind === 'new' ? ['--session-id', ref.value, '--name', plan.name] : ['--session-id', ref.value];
		},
		forkFailed: output => /No session found matching/.test(output),
		// `pi --version` prints `1.0.2`; `pi update --self` updates pi only (not its packages or model catalogs).
		version: {npmPackage: '@earendil-works/pi-coding-agent', updateArgs: ['update', '--self']},
	},
	// codex · codex resume <id> · codex fork <parent id>. Codex picks every ID itself (also a fork's): Deckhand learns it
	// from an authenticated SessionStart hook or the `codex resume <id>` exit hint.
	codex: {
		label: 'Codex',
		idAtLaunch: false,
		forks: true,
		// Codex finds a session by ID anywhere, but resuming/forking one recorded in another directory asks whether to
		// use the session's directory or the current one (tui cwd prompt), so the child could run outside its worktree.
		forksAcrossDirectories: false,
		args(plan) {
			if (plan.kind === 'fork') return ['fork', plan.parent.value];
			return plan.kind === 'resume' && plan.ref.kind === 'id' ? ['resume', plan.ref.value] : [];
		},
		exitRef: codexResumeFromOutput,
		// `codex --version` prints `codex-cli 0.157.0`.
		version: {npmPackage: '@openai/codex', updateArgs: ['update']},
	},
};

export function agentSpec(program: ProgramKey): AgentSpec {
	return AGENTS[program];
}

/** A new conversation reference: an exact ID for agents that take one at launch, none for agents that report their own. */
export function newAgentRef(program: ProgramKey): AgentSessionRef | undefined {
	return AGENTS[program].idAtLaunch ? {provider: program, kind: 'id', value: randomUUID()} : undefined;
}

export function launchArgs(program: ProgramKey, plan: LaunchPlan): string[] {
	return AGENTS[program].args(plan);
}

export function sameAgentSessionRef(left: AgentSessionRef | undefined, right: AgentSessionRef | undefined): boolean {
	return Boolean(left && right && left.provider === right.provider && left.kind === right.kind && left.value === right.value);
}

type RelaunchSession = Pick<SessionRecord, 'program' | 'agentSessionRef' | 'subSessionKind' | 'forkedFromAgentSessionRef'>;

/**
 * A forked child without a conversation of its own forks its parent again: it never launched, its fork failed (the
 * exit handling then stores the parent's ref), it never reported its own ID (Codex), or it is a legacy child that
 * still holds the parent's ref.
 */
export function forksParentAgain(session: RelaunchSession, neverStarted: boolean): boolean {
	const parent = session.subSessionKind === 'forked' ? session.forkedFromAgentSessionRef : undefined;
	return Boolean(parent) && (neverStarted || !session.agentSessionRef || sameAgentSessionRef(session.agentSessionRef, parent));
}

/**
 * How to relaunch an exited session. `fresh` (S) starts a new conversation; otherwise a fork without its own
 * conversation forks its parent again, a session that never started launches its assigned conversation, and anything
 * else resumes its own reference. Undefined: nothing to resume (the caller refuses rather than starting fresh).
 */
export function relaunchPlan(session: RelaunchSession, mode: 'resume' | 'fresh', neverStarted: boolean, name: string): LaunchPlan | undefined {
	if (mode === 'fresh') return {kind: 'new', ref: newAgentRef(session.program), name};
	if (forksParentAgain(session, neverStarted)) return {kind: 'fork', parent: session.forkedFromAgentSessionRef!, ref: newAgentRef(session.program), name};
	const ref = session.agentSessionRef;
	if (neverStarted) return {kind: 'new', ref: ref ?? newAgentRef(session.program), name};
	return ref ? {kind: 'resume', ref} : undefined;
}
