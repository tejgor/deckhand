import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

// Claude moves to a new conversation ID without exiting: accepting a plan with "clear context", /clear, /resume, /branch.
// Hooks report the new ID while Deckhand runs (acceptedNativeRef), but a hook can be missed (hooks off, the daemon down
// at that moment, a Claude version that sends no hook for the switch: plan acceptance sent none in 2.1.x). Restart then
// reads Claude's transcripts to find the conversation the session continued in.
//
// Transcripts are `<config>/projects/<encoded cwd>/<session id>.jsonl`, one JSON record per line. Observed in 2.1.296:
// a conversation started by accepting a plan keeps the planning conversation's `custom-title` record (Deckhand's
// `--name`), and its first message's `parentUuid` is a record of the planning conversation.

const HEAD_BYTES = 64 * 1024;
const TAIL_BYTES = 64 * 1024;
const MAX_CANDIDATES = 200;

export function claudeProjectsDir(env: NodeJS.ProcessEnv = process.env): string {
	return path.join(env.CLAUDE_CONFIG_DIR ? path.resolve(env.CLAUDE_CONFIG_DIR) : path.join(os.homedir(), '.claude'), 'projects');
}

/** Claude's project directory name for a cwd (every character but a letter or a digit becomes `-`). */
export function claudeProjectKey(cwd: string): string {
	return path.resolve(cwd).replace(/[^a-zA-Z0-9]/g, '-');
}

interface TranscriptHead {
	/** The first `custom-title` (the launch `--name`). */
	title?: string;
	/** `parentUuid` of the first message: set when the conversation continues another one. */
	parentUuid?: string;
	startedAt?: number;
}

async function readRange(file: string, start: number, length: number): Promise<string> {
	const handle = await fs.open(file, 'r');
	try {
		const buffer = Buffer.alloc(length);
		const {bytesRead} = await handle.read(buffer, 0, length, start);
		return buffer.subarray(0, bytesRead).toString('utf8');
	} finally { await handle.close(); }
}

function records(text: string): Array<Record<string, unknown>> {
	const out: Array<Record<string, unknown>> = [];
	for (const line of text.split('\n')) {
		try { const value: unknown = JSON.parse(line); if (value && typeof value === 'object') out.push(value as Record<string, unknown>); } catch { /* A line cut by the byte range. */ }
	}
	return out;
}

function timeOf(record: Record<string, unknown>): number | undefined {
	const at = typeof record.timestamp === 'string' ? Date.parse(record.timestamp) : NaN;
	return Number.isFinite(at) ? at : undefined;
}

async function readHead(file: string): Promise<TranscriptHead> {
	const head: TranscriptHead = {};
	let sawMessage = false;
	for (const record of records(await readRange(file, 0, HEAD_BYTES))) {
		if (record.type === 'custom-title' && head.title === undefined && typeof record.customTitle === 'string') head.title = record.customTitle;
		if ((record.type === 'user' || record.type === 'assistant') && !sawMessage) {
			sawMessage = true;
			if (typeof record.parentUuid === 'string') head.parentUuid = record.parentUuid;
			head.startedAt = timeOf(record);
		}
	}
	return head;
}

/** When the conversation last had a message (not metadata, which Claude re-appends to idle transcripts). */
async function lastActivity(file: string, size: number): Promise<number> {
	const found = records(await readRange(file, Math.max(0, size - TAIL_BYTES), TAIL_BYTES))
		.filter(record => record.type === 'user' || record.type === 'assistant')
		.map(timeOf).filter((at): at is number => at !== undefined);
	return found.length ? Math.max(...found) : 0;
}

async function findTranscript(projectsDir: string, id: string, cwdHints: string[]): Promise<string | undefined> {
	const exists = (file: string) => fs.stat(file).then(stat => stat.isFile(), () => false);
	for (const cwd of cwdHints) {
		const file = path.join(projectsDir, claudeProjectKey(cwd), `${id}.jsonl`);
		if (await exists(file)) return file;
	}
	// Long paths are shortened in the key, and a session may have moved: look in every project.
	let dirs: string[];
	try { dirs = await fs.readdir(projectsDir); } catch { return undefined; }
	for (const dir of dirs) {
		const file = path.join(projectsDir, dir, `${id}.jsonl`);
		if (await exists(file)) return file;
	}
	return undefined;
}

export interface LatestConversationOptions {
	projectsDir?: string;
	/** Directories the conversation may be stored under, tried before every project. */
	cwdHints?: string[];
	/** Conversation IDs that belong to other sessions (e.g. a fork's): never followed. */
	exclude?: ReadonlySet<string>;
}

/**
 * The conversation that `id` continued in, when it differs from `id`: among `id` and the conversations started after it
 * in the same project that carry its title or continue one of them (transitively), the one with the latest message.
 * The latest message wins, not the latest start, so resuming the earlier conversation and working there keeps it.
 */
export async function latestClaudeConversation(id: string, options: LatestConversationOptions = {}): Promise<string | undefined> {
	const projectsDir = options.projectsDir ?? claudeProjectsDir();
	const stored = await findTranscript(projectsDir, id, options.cwdHints ?? []);
	if (!stored) return undefined;
	const dir = path.dirname(stored);
	const storedStat = await fs.stat(stored);
	const storedHead = await readHead(stored);
	const since = storedHead.startedAt ?? storedStat.birthtimeMs;
	let names: string[];
	try { names = await fs.readdir(dir); } catch { return undefined; }
	const candidates: Array<{id: string; file: string; size: number; mtimeMs: number}> = [];
	for (const name of names) {
		const other = name.endsWith('.jsonl') ? name.slice(0, -'.jsonl'.length) : undefined;
		if (!other || other === id || options.exclude?.has(other)) continue;
		const file = path.join(dir, name);
		const stat = await fs.stat(file).catch(() => undefined);
		// A successor is written after the stored conversation began.
		if (stat?.isFile() && stat.mtimeMs >= since) candidates.push({id: other, file, size: stat.size, mtimeMs: stat.mtimeMs});
	}
	candidates.sort((a, b) => b.mtimeMs - a.mtimeMs).splice(MAX_CANDIDATES);
	const heads = await Promise.all(candidates.map(async candidate => ({...candidate, head: await readHead(candidate.file).catch((): TranscriptHead => ({}))})));
	// Oldest first: a conversation's parent was started before it, so one pass follows a chain of plan acceptances.
	const successors = heads.filter(item => item.head.startedAt !== undefined && item.head.startedAt > since).sort((a, b) => a.head.startedAt! - b.head.startedAt!);
	const family = [{id, file: stored, size: storedStat.size}];
	const familyText = new Map<string, string>();
	const contains = async (member: {file: string}, uuid: string) => {
		if (!familyText.has(member.file)) familyText.set(member.file, await fs.readFile(member.file, 'utf8').catch(() => ''));
		return familyText.get(member.file)!.includes(`"uuid":"${uuid}"`);
	};
	for (const item of successors) {
		const sameTitle = Boolean(storedHead.title) && item.head.title === storedHead.title;
		let continues = false;
		if (!sameTitle && item.head.parentUuid) for (const member of family) if (await contains(member, item.head.parentUuid)) { continues = true; break; }
		if (sameTitle || continues) family.push(item);
	}
	if (family.length === 1) return undefined;
	let latest = family[0]!, latestAt = await lastActivity(latest.file, latest.size).catch(() => 0);
	for (const member of family.slice(1)) {
		const at = await lastActivity(member.file, member.size).catch(() => 0);
		if (at > latestAt) { latest = member; latestAt = at; }
	}
	return latest.id === id ? undefined : latest.id;
}
