import type {AgentVersionInfo, AgentVersions, ProgramKey, SessionRecord} from './types.js';

// Agent versions as pure data: parsing `--version` output, comparing, and what the UI says about them (the Agents
// screen U, the sidebar's ↑ marker, the header hint). The daemon's I/O lives in src/agentVersionCheck.ts.

export const AGENT_PROGRAMS: readonly ProgramKey[] = ['claude', 'pi', 'codex'];

/** The first x.y.z in `text` (`2.1.287 (Claude Code)`, `codex-cli 0.157.0`, `1.0.2`); suffixes such as `-alpha.1` are ignored. */
export function parseVersion(text: string | undefined): string | undefined {
	const match = text?.match(/(?:^|[^\d.])v?(\d+)\.(\d+)\.(\d+)/);
	return match ? `${Number(match[1])}.${Number(match[2])}.${Number(match[3])}` : undefined;
}

/** Numeric x.y.z comparison (negative: `left` is older). Unparsable versions compare equal to everything. */
export function compareVersions(left: string, right: string): number {
	const a = parseVersion(left), b = parseVersion(right);
	if (!a || !b) return 0;
	const [x, y] = [a.split('.').map(Number), b.split('.').map(Number)];
	for (let index = 0; index < 3; index++) if (x[index] !== y[index]) return x[index]! - y[index]!;
	return 0;
}

export type AgentUpdateStatus = 'up to date' | 'update available' | 'not installed' | 'latest unknown';
export function agentUpdateStatus(info: AgentVersionInfo | undefined): AgentUpdateStatus {
	if (!info?.installed) return 'not installed';
	if (!info.latest) return 'latest unknown';
	return compareVersions(info.installed, info.latest) < 0 ? 'update available' : 'up to date';
}

/** A running session whose agent launched with an older version than the one installed now (a restart would pick it up). */
export function sessionOutdated(session: Pick<SessionRecord, 'status' | 'agentVersion'>, installed: string | undefined): boolean {
	return session.status === 'running' && Boolean(session.agentVersion && installed) && compareVersions(session.agentVersion!, installed!) < 0;
}

/** Installed versions by agent, for the sidebar. */
export function installedVersions(versions: AgentVersions | undefined): Partial<Record<ProgramKey, string>> {
	return Object.fromEntries(AGENT_PROGRAMS.flatMap(program => versions?.[program]?.installed ? [[program, versions[program].installed]] : []));
}

/** The app header's quiet hint: `codex update · U`, `2 agent updates · U`, or nothing (all current, or latest unknown). */
export function updateHint(versions: AgentVersions | undefined): string {
	const available = AGENT_PROGRAMS.filter(program => agentUpdateStatus(versions?.[program]) === 'update available');
	if (!available.length) return '';
	return `${available.length === 1 ? `${available[0]} update` : `${available.length} agent updates`} · U`;
}
