import React, {useState} from 'react';
import type {Key} from 'ink';
import type {LiveClient} from './client.js';
import {AGENTS} from './agents.js';
import {AGENT_PROGRAMS, agentUpdateStatus} from './agentVersions.js';
import {MenuPane, type DetailLine, type MenuItem} from './menu.js';
import {formatAge} from './sidebarModel.js';
import type {AgentUpdateResult, AgentVersionInfo, AgentVersions, ProgramKey} from './types.js';
import {THEME, compactPath, errorMessage, plainTerminalText} from './ui.js';

// U → Agents: each agent's installed and latest version, and its own updater (enter). Updating never touches
// sessions: running ones keep their version until restarted (↑ in the sidebar). r looks the latest up again.

export type AgentUpdateOutcome = AgentUpdateResult | {program: ProgramKey; error: string};
/** The last lines of an update's output shown in the details box. */
const OUTPUT_LINES = 8;

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? '' : 's'}`;

function rowStatus(info: AgentVersionInfo | undefined, working: boolean): {text: string; color: string} {
	if (working) return {text: 'updating…', color: THEME.warn};
	const status = agentUpdateStatus(info);
	return {text: status, color: status === 'update available' ? THEME.warn : status === 'up to date' ? THEME.success : THEME.muted};
}

function rowDescription(info: AgentVersionInfo | undefined): string {
	const installed = (info?.installed ?? (info?.path ? 'unknown' : '—')).padEnd(10);
	const latest = `latest ${info?.latest ?? '?'}`.padEnd(16);
	return `${installed}${latest}${info?.outdated ? `${plural(info.outdated, 'session')} on an older version` : ''}`.trimEnd();
}

function outcomeLines(outcome: AgentUpdateOutcome): DetailLine[] {
	if ('error' in outcome) return [{text: `Update failed: ${outcome.error}`, color: THEME.error}];
	const output = plainTerminalText(outcome.output).split('\n').map(line => line.trimEnd()).filter(Boolean).slice(-OUTPUT_LINES);
	const head: DetailLine = !outcome.ok
		? {text: `${outcome.command} failed (exit ${outcome.exitCode ?? 'none'}):`, color: THEME.error}
		: outcome.after && outcome.before !== outcome.after
			? {text: `Updated ${outcome.before ?? '?'} → ${outcome.after}. Running sessions keep their version until restarted.`, color: THEME.success}
			: {text: `Nothing new installed (${outcome.after ?? 'version unknown'}).`, color: THEME.success};
	return [head, ...output.map(text => ({text, nowrap: true, color: outcome.ok ? THEME.muted : undefined}))];
}

function detailLines(info: AgentVersionInfo | undefined, program: ProgramKey, working: boolean, outcome: AgentUpdateOutcome | undefined, now: number): DetailLine[] {
	const spec = AGENTS[program];
	const age = info?.checkedAt ? formatAge(now - Date.parse(info.checkedAt)) : undefined;
	const checked = age ? ` · checked ${age === 'now' ? 'just now' : `${age} ago`}` : '';
	// A running update or its result first: the box is cut from the bottom when the pane is short.
	const lines: DetailLine[] = working ? [{text: `Running ${program} ${spec.version.updateArgs.join(' ')}… sessions keep running.`, color: THEME.warn}]
		: outcome ? outcomeLines(outcome) : !info?.path ? [{text: 'Not installed. Run `deckhand setup` to install it.', color: THEME.muted}] : [];
	lines.push(
		{text: `Installed  ${info?.installed ?? (info?.path ? 'version unknown' : 'not installed')}${info?.path ? `  ${compactPath(info.path, 60)}` : ''}`, nowrap: true},
		{text: `Latest     ${info?.latest ?? 'unknown'}  npm ${spec.version.npmPackage}${checked}`, nowrap: true},
		{text: `Update     ${program} ${spec.version.updateArgs.join(' ')}`, nowrap: true},
		{text: `Sessions   ${info?.running ? `${plural(info.running, 'running session')}${info.outdated ? ` · ${info.outdated} on an older version (restart to update)` : ''}` : 'none running'}`, nowrap: true},
	);
	if (info?.error) lines.push({text: info.error, color: THEME.muted});
	return lines;
}

export interface AgentsPaneProps {
	versions?: AgentVersions;
	selected: number;
	checking?: boolean;
	/** Agents whose update is running (this UI's requests or the daemon's `updating`). */
	working?: ReadonlySet<ProgramKey>;
	outcomes?: Partial<Record<ProgramKey, AgentUpdateOutcome>>;
	/** The agent whose update waits for confirmation (it has running sessions). */
	confirming?: ProgramKey;
	width: number;
	height: number;
	now?: number;
}

export function AgentsPane({versions, selected, checking, working = new Set(), outcomes = {}, confirming, width, height, now = Date.now()}: AgentsPaneProps) {
	const items: MenuItem[] = AGENT_PROGRAMS.map(program => {
		const info = versions?.[program];
		return {key: program, label: AGENTS[program].label, description: rowDescription(info), status: rowStatus(info, working.has(program) || Boolean(info?.updating))};
	});
	const program = AGENT_PROGRAMS[selected] ?? AGENT_PROGRAMS[0]!;
	const info = versions?.[program];
	const label = AGENTS[program].label;
	const details = confirming
		? {title: `Update ${AGENTS[confirming].label}?`, lines: [
			{text: `${plural(versions?.[confirming]?.running ?? 0, `running ${AGENTS[confirming].label} session`)} keep their current version until restarted.`},
			{text: `Runs ${confirming} ${AGENTS[confirming].version.updateArgs.join(' ')}.`, color: THEME.muted},
		]}
		: {title: `${label} · ${rowStatus(info, working.has(program) || Boolean(info?.updating)).text}`, lines: detailLines(info, program, working.has(program) || Boolean(info?.updating), outcomes[program], now)};
	return <MenuPane
		title="Agents"
		subtitle={[checking ? {text: 'Checking the latest releases…', color: THEME.warn} : {text: 'Running sessions keep their version until restarted (↑ in the sidebar).'}]}
		items={items}
		selected={Math.min(selected, items.length - 1)}
		labelWidth={8}
		details={details}
		hint={confirming ? ['enter update', 'esc cancel'] : [{text: 'j/k choose', drop: 1}, 'enter update', {text: 'r re-check', drop: 2}, 'esc back']}
		width={width}
		height={height}
	/>;
}

interface AgentsFlowOptions {
	client?: LiveClient;
	versions?: AgentVersions;
	setVersions: (versions: AgentVersions) => void;
	setMode: (mode: 'agents' | 'browse') => void;
	setStatusMessage: (message: string | undefined) => void;
}
export interface AgentsFlow {
	/** U: show the screen and look the latest releases up again. */
	open(): void;
	handleInput(input: string, key: Partial<Key>): void;
	render(width: number, height: number): React.ReactNode;
}

export function useAgentsFlow({client, versions, setVersions, setMode, setStatusMessage}: AgentsFlowOptions): AgentsFlow {
	const [selected, setSelected] = useState(0);
	const [checking, setChecking] = useState(false);
	const [confirming, setConfirming] = useState<ProgramKey>();
	const [working, setWorking] = useState<ReadonlySet<ProgramKey>>(new Set());
	const [outcomes, setOutcomes] = useState<Partial<Record<ProgramKey, AgentUpdateOutcome>>>({});

	const check = () => {
		if (!client) return;
		setChecking(true);
		// Offline or npm missing only leaves the latest unknown; nothing to report.
		void client.agentVersions(true).then(setVersions).catch(() => {}).finally(() => setChecking(false));
	};
	const update = (program: ProgramKey) => {
		if (!client) return;
		const label = AGENTS[program].label;
		setWorking(current => new Set([...current, program]));
		setOutcomes(({[program]: _dropped, ...rest}) => rest);
		void client.updateAgent(program).then(result => {
			setVersions(result.versions);
			setOutcomes(current => ({...current, [program]: result}));
			setStatusMessage(!result.ok ? `${label} update failed (exit ${result.exitCode ?? 'none'}); U shows the output`
				: result.after && result.after !== result.before ? `${label} updated to ${result.after}` : `${label}: nothing new installed`);
		}).catch(error => {
			setOutcomes(current => ({...current, [program]: {program, error: errorMessage(error)}}));
			setStatusMessage(`${label} update failed; U shows why`);
		}).finally(() => setWorking(current => new Set([...current].filter(item => item !== program))));
	};

	return {
		open() {
			setConfirming(undefined);
			setMode('agents');
			check();
		},
		handleInput(input, key) {
			const program = AGENT_PROGRAMS[selected]!;
			if (confirming) {
				if (key.return || input === 'y') { setConfirming(undefined); update(confirming); }
				else if (key.escape || input === 'n') setConfirming(undefined);
				return;
			}
			if (key.escape) { setMode('browse'); return; }
			if (key.upArrow || input === 'k') { setSelected(index => Math.max(0, index - 1)); return; }
			if (key.downArrow || input === 'j') { setSelected(index => Math.min(AGENT_PROGRAMS.length - 1, index + 1)); return; }
			if (input === 'r') { check(); return; }
			if (key.return) {
				const info = versions?.[program];
				// Installing is deckhand setup's job (the details say so); one update at a time per agent.
				if (!info?.path || working.has(program) || info.updating) return;
				if (info.running > 0) setConfirming(program);
				else update(program);
			}
		},
		render(width, height) {
			return <AgentsPane versions={versions} selected={selected} checking={checking} working={working} outcomes={outcomes} confirming={confirming} width={width} height={height} />;
		},
	};
}
