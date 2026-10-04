#!/usr/bin/env node
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawn, execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {randomUUID} from 'node:crypto';
const exec = promisify(execFile);
const project = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

async function canonical(file) {
	try { return await fs.realpath(file); }
	catch (error) {
		if (error.code !== 'ENOENT') throw error;
		const stat = await fs.lstat(file).catch(() => undefined);
		if (stat?.isSymbolicLink()) return canonical(path.resolve(path.dirname(file), await fs.readlink(file)));
		const parent = path.dirname(file);
		if (parent === file) return file;
		return path.join(await canonical(parent), path.basename(file));
	}
}
export async function isolatedHome(candidate = process.env.DECKHAND_DEV_HOME || path.join(os.homedir(), '.deckhand-dev')) {
	// Production state is ~/.deckhand, plus DECKHAND_HOME when this shell points stable Deckhand elsewhere
	// (inside an isolated dev session DECKHAND_HOME is the dev home itself).
	const configured = process.env.DECKHAND_CHANNEL === 'dev' ? undefined : process.env.DECKHAND_HOME;
	const production = await Promise.all([path.join(os.homedir(), '.deckhand'), ...(configured ? [path.resolve(configured)] : [])].map(canonical));
	const isolated = await canonical(path.resolve(candidate));
	for (const home of production) if (isolated === home || isolated.startsWith(`${home}${path.sep}`) || home.startsWith(`${isolated}${path.sep}`)) throw new Error('The dev state directory must be separate from ~/.deckhand and DECKHAND_HOME (including symlinks)');
	return isolated;
}
export async function createSandbox(home) {
	const cwd = path.join(home, 'sandbox');
	try { await fs.access(path.join(cwd, '.git')); return cwd; } catch {}
	try { await fs.access(cwd); throw new Error(`Refusing to overwrite existing sandbox directory: ${cwd}`); }
	catch (error) { if (error.code !== 'ENOENT') throw error; }
	// Build aside and rename into place so an interrupted first run never leaves a half-made sandbox behind.
	const temporary = path.join(home, `sandbox.tmp-${randomUUID()}`);
	try {
		await populateSandbox(temporary, cwd);
		await fs.rename(temporary, cwd);
	} catch (error) {
		await fs.rm(temporary, {recursive: true, force: true});
		try { await fs.access(path.join(cwd, '.git')); return cwd; } catch {} // A concurrent launch won the race.
		throw error;
	}
	return cwd;
}
async function populateSandbox(cwd, final) {
	await fs.mkdir(cwd, {recursive: true});
	await fs.writeFile(path.join(cwd, 'package.json'), JSON.stringify({name: 'deckhand-sandbox', private: true, type: 'module', scripts: {test: 'node --test', dev: 'node app.js'}}, null, 2) + '\n');
	await fs.writeFile(path.join(cwd, 'app.js'), "import http from 'node:http';\nhttp.createServer((request, response) => response.end('Hello from the Deckhand dev sandbox!')).listen(4319, '127.0.0.1');\nconsole.log('Sandbox server: http://127.0.0.1:4319');\n");
	await fs.writeFile(path.join(cwd, 'app.test.js'), "import {test} from 'node:test';\nimport assert from 'node:assert/strict';\ntest('example', () => assert.equal(2 + 2, 4));\n");
	await fs.writeFile(path.join(cwd, 'deckhand.json'), JSON.stringify({defaultWorkspace: 'new', devCommand: 'npm run dev', actions: {test: 'npm test'}}, null, 2) + '\n');
	await fs.writeFile(path.join(cwd, 'README.md'), '# Deckhand dev sandbox\n\nA disposable repository for testing the isolated workbench. No dependencies to install.\n');
	for (const args of [['init', '-b', 'main'], ['config', 'user.name', 'Deckhand Sandbox'], ['config', 'user.email', 'sandbox@deckhand.invalid'], ['config', 'commit.gpgsign', 'false'], ['config', 'core.hooksPath', path.join(final, '.git', 'no-hooks')], ['add', '.'], ['commit', '-m', 'Initialize disposable sandbox']]) await exec('git', ['-C', cwd, ...args]);
}
export async function main(args = process.argv.slice(2)) {
	if (args.includes('--help')) {
		console.log('Usage: node scripts/deckhand-dev.mjs [--source] [--sandbox] [status|stop]\n\nBuilt CLI by default; --source uses tsx. State: ~/.deckhand-dev, override with DECKHAND_DEV_HOME.\n--sandbox opens a disposable Git repo. No npm link/global install required.');
		return;
	}
	const home = await isolatedHome();
	await fs.mkdir(home, {recursive: true, mode: 0o700});
	const source = args.includes('--source');
	const cwd = args.includes('--sandbox') ? await createSandbox(home) : process.cwd();
	const forwarded = args.filter(arg => !['--source', '--sandbox'].includes(arg));
	if (['setup', 'doctor'].includes(forwarded[0]) && !forwarded.includes('--check')) throw new Error('The isolated launcher does not install global agent tools. Use setup --check for read-only inspection.');
	const entry = path.join(project, source ? 'src/cli.ts' : 'dist/cli.js');
	try { await fs.access(entry); } catch { throw new Error('Build this checkout first: npm run build'); }
	const env = {...process.env, DECKHAND_HOME: home, DECKHAND_CHANNEL: 'dev', DECKHAND_DEV: source ? '1' : '0'};
	for (const key of ['DECKHAND_SESSION_ID', 'DECKHAND_LAUNCH_ID', 'DECKHAND_HOOK_TOKEN']) delete env[key];
	console.error(`Deckhand dev · isolated state: ${home}\nWorkspace: ${cwd}`);
	const child = spawn(process.execPath, [...(source ? ['--import', import.meta.resolve('tsx')] : []), entry, ...forwarded], {cwd, env, stdio: 'inherit'});
	for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => child.kill(signal));
	const code = await new Promise((resolve, reject) => { child.once('error', reject); child.once('exit', code => resolve(code ?? 1)); });
	process.exitCode = code;
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main().catch(error => { console.error(error.message); process.exitCode = 1; });
