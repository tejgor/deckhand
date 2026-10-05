import fs from 'node:fs/promises';
import {constants} from 'node:fs';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {resolveRepoContext, type RepoContext} from './git.js';
import {PROJECT_CONFIG_FILE, parseProjectConfig, readBoundedUtf8, savedProjectTrust, sha256, trustProjectConfig, type SaveTrust} from './projectConfig.js';
import {MAX_CONFIG_BYTES, MAX_CONFIG_LABEL} from './configDraft.js';
import {getConfigDir, getConfigPath} from './paths.js';
import {loadAppConfig, updateAppConfig} from './storage.js';

export const STARTER_PROJECT_CONFIG = `${JSON.stringify({defaultAgent: 'claude', defaultWorkspace: 'new', actions: {}}, null, 2)}\n`;
export type ConfigTargetKind = 'global' | 'repository';
export interface ProjectConfigDocument {
	kind: ConfigTargetKind;
	root: string;
	path: string;
	raw: string;
	revision: string | null;
	exists: boolean;
}
/** A saved document; a repository file saved with keepTrust says whether it is (still) trusted (see savedProjectTrust). */
export interface SavedConfigDocument extends ProjectConfigDocument {trust?: SaveTrust}
/** C's two targets: the user's global defaults and the repository's deckhand.json (main checkout). */
export interface ConfigTargets {
	global?: ProjectConfigDocument;
	globalError?: string;
	repository?: ProjectConfigDocument;
	repositoryError?: string;
}

const message = (error: unknown) => error instanceof Error ? error.message : String(error);
// Each target stays available when the other cannot be read.
export async function readConfigTargets(cwd: string): Promise<ConfigTargets> {
	const targets: ConfigTargets = {};
	try { targets.global = await readGlobalDefaultsDocument(); } catch (error) { targets.globalError = message(error); }
	try { targets.repository = await readProjectConfigDocument(cwd); } catch (error) { targets.repositoryError = message(error); }
	return targets;
}

async function repositoryContext(cwd: string): Promise<RepoContext & {mainRoot: string}> {
	const context = await resolveRepoContext(cwd);
	if (!context.mainRoot) throw new Error('Bare repositories have no main checkout, so no repository deckhand.json; use global defaults');
	return {...context, mainRoot: context.mainRoot};
}

// Unlike loadProjectConfig, opening a document must allow malformed JSON to be repaired.
async function readDocumentAt(root: string): Promise<{document: ProjectConfigDocument; mode: number}> {
	const file = path.join(root, PROJECT_CONFIG_FILE);
	try {
		const {bytes, text, mode} = await readBoundedUtf8(file, {noFollow: true, max: MAX_CONFIG_BYTES});
		return {document: {kind: 'repository', root, path: file, raw: text, revision: sha256(bytes), exists: true}, mode};
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === 'ENOENT') return {document: {kind: 'repository', root, path: file, raw: STARTER_PROJECT_CONFIG, revision: null, exists: false}, mode: 0o644};
		if ((error as NodeJS.ErrnoException).code === 'ELOOP') throw new Error('Refusing to edit a symlinked deckhand.json; edit its target explicitly');
		throw error;
	}
}
/** The repository's deckhand.json, always in the main checkout (worktree copies are ignored). */
export async function readProjectConfigDocument(cwd: string): Promise<ProjectConfigDocument> {
	return (await readDocumentAt((await repositoryContext(cwd)).mainRoot)).document;
}

// Exclusive creation that never overwrites a newly-created file; copy when the filesystem lacks hard links.
async function createExclusive(temporary: string, file: string): Promise<void> {
	try { await fs.link(temporary, file); }
	catch (error) {
		if (!['EPERM', 'ENOTSUP', 'EOPNOTSUPP', 'EXDEV', 'ENOSYS'].includes((error as NodeJS.ErrnoException).code ?? '')) throw error;
		await fs.copyFile(temporary, file, constants.COPYFILE_EXCL);
	}
}

function checkDraft(raw: string, expectedRevision: string | null): void {
	if (typeof raw !== 'string' || Buffer.byteLength(raw) > MAX_CONFIG_BYTES) throw new Error(`Configuration must be text of at most ${MAX_CONFIG_LABEL}`);
	if (expectedRevision !== null && (typeof expectedRevision !== 'string' || !/^[a-f0-9]{64}$/.test(expectedRevision))) throw new Error('Invalid configuration revision');
}

const writes = new Map<string, Promise<void>>();
/**
 * Writes the repository's deckhand.json if it still has `expectedRevision` (null: must not exist). With `keepTrust`
 * (saves made through Deckhand), the trust decision is made from exactly the replaced bytes inside the same
 * serialized write, and the new bytes' fingerprint is trusted when savedProjectTrust allows it.
 */
export async function saveProjectConfigDocument(cwd: string, raw: string, expectedRevision: string | null, {keepTrust = false}: {keepTrust?: boolean} = {}): Promise<SavedConfigDocument> {
	checkDraft(raw, expectedRevision);
	parseProjectConfig(raw); // Never replace a valid file with an invalid draft.
	const context = await repositoryContext(cwd);
	const root = context.mainRoot;
	const file = path.join(root, PROJECT_CONFIG_FILE);
	const operation = (writes.get(file) ?? Promise.resolve()).then(async () => {
		const check = async () => {
			const current = await readDocumentAt(root);
			if (current.document.revision !== expectedRevision) throw new Error('deckhand.json changed on disk. Your draft is kept; close and reopen the editor before saving.');
			return current;
		};
		const current = await check();
		// Decided from the bytes the revision names (what the user edited from), before anything is written.
		const decision = keepTrust ? await savedProjectTrust(context, {raw: current.document.raw, exists: current.document.exists}, raw, await loadAppConfig()) : undefined;
		const temporary = path.join(root, `.deckhand.json.${randomUUID()}.tmp`);
		try {
			await fs.writeFile(temporary, raw, {encoding: 'utf8', mode: current.mode, flag: 'wx'});
			if (current.document.exists) await fs.chmod(temporary, current.mode);
			await check();
			if (expectedRevision === null) await createExclusive(temporary, file);
			else await fs.rename(temporary, file);
			// The fingerprint covers the bytes written here: anything written over them later needs a review again.
			const fingerprint = decision?.fingerprint;
			if (fingerprint) await updateAppConfig(config => trustProjectConfig({trustRoot: context.trustRoot, fingerprint}, config));
			return {kind: 'repository' as const, root, path: file, raw, revision: sha256(Buffer.from(raw)), exists: true, ...decision ? {trust: decision.trust} : {}};
		} finally { await fs.rm(temporary, {force: true}); }
	});
	const tail = operation.then(() => {}, () => {});
	writes.set(file, tail);
	void tail.then(() => { if (writes.get(file) === tail) writes.delete(file); });
	return operation;
}

// Global defaults live inside config.json, so the document is the pretty-printed `defaults` value and its
// revision hashes that value: a concurrent change to `defaults` rejects a stale draft, other keys do not.
const globalRevision = (value: unknown) => value === undefined ? null : sha256(JSON.stringify(value));
function globalDocument(value: unknown): ProjectConfigDocument {
	return {kind: 'global', root: getConfigDir(), path: getConfigPath(), raw: value === undefined ? STARTER_PROJECT_CONFIG : `${JSON.stringify(value, null, 2)}\n`, revision: globalRevision(value), exists: value !== undefined};
}
export async function readGlobalDefaultsDocument(): Promise<ProjectConfigDocument> {
	return globalDocument((await loadAppConfig()).defaults);
}
export async function saveGlobalDefaultsDocument(raw: string, expectedRevision: string | null): Promise<ProjectConfigDocument> {
	checkDraft(raw, expectedRevision);
	const defaults = parseProjectConfig(raw, 'defaults');
	await updateAppConfig(current => {
		if (globalRevision(current.defaults) !== expectedRevision) throw new Error('Global defaults changed on disk. Your draft is kept; close and reopen the editor before saving.');
		return {...current, defaults};
	});
	return globalDocument(defaults);
}
