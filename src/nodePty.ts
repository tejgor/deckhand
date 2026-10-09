import fs from 'node:fs/promises';
import path from 'node:path';
import {execFile} from 'node:child_process';
import {createRequire} from 'node:module';
import {randomUUID} from 'node:crypto';
import {promisify} from 'node:util';

const execFileAsync = promisify(execFile);

interface DarwinNodePtyPaths {
	prebuildDir: string;
	helperPath: string;
	nativePath: string;
}

function getDarwinNodePtyPaths(): DarwinNodePtyPaths | undefined {
	if (process.platform !== 'darwin') {
		return undefined;
	}
	const require = createRequire(import.meta.url);
	const packageJsonPath = require.resolve('node-pty/package.json');
	const packageDir = path.dirname(packageJsonPath);
	const prebuildDir = path.join(packageDir, 'prebuilds', `darwin-${process.arch}`);
	return {
		prebuildDir,
		helperPath: path.join(prebuildDir, 'spawn-helper'),
		nativePath: path.join(prebuildDir, 'pty.node'),
	};
}

async function ensureExecutable(filePath: string): Promise<void> {
	const stat = await fs.stat(filePath);
	const nextMode = stat.mode | 0o111;
	if (nextMode !== stat.mode) {
		await fs.chmod(filePath, nextMode);
	}
}

async function tryExecFile(file: string, args: string[]): Promise<void> {
	try {
		await execFileAsync(file, args);
	} catch {
		// best-effort only
	}
}

async function validSignature(file: string): Promise<boolean> {
	try { await execFileAsync('codesign', ['--verify', file]); return true; } catch { return false; }
}

/**
 * Ad-hoc signs `file` when its signature is missing or invalid. Never in place: a copy is signed and renamed over it,
 * so a process reading or running it sees the old file or the new one, never a half-written one. Signing in place on
 * every start (the daemon and each worker) once let two at the same moment corrupt spawn-helper, and then no agent,
 * shell or pane could start.
 */
export async function signIfNeeded(file: string): Promise<void> {
	if (await validSignature(file)) return;
	const temporary = path.join(path.dirname(file), `.${path.basename(file)}.${process.pid}.${randomUUID().slice(0, 8)}.tmp`);
	try {
		await fs.copyFile(file, temporary);
		await fs.chmod(temporary, (await fs.stat(file)).mode);
		await execFileAsync('codesign', ['--force', '--sign', '-', temporary]);
		await fs.rename(temporary, file);
	} catch {
		// best-effort only: node-pty reports what is still wrong when it spawns
	} finally {
		await fs.rm(temporary, {force: true});
	}
}

export async function ensureNodePtyReady(): Promise<void> {
	const paths = getDarwinNodePtyPaths();
	if (!paths) {
		return;
	}

	await ensureExecutable(paths.helperPath);
	await tryExecFile('xattr', ['-dr', 'com.apple.quarantine', paths.prebuildDir]);
	await signIfNeeded(paths.helperPath);
	await signIfNeeded(paths.nativePath);
}
