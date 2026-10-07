import assert from 'node:assert/strict';
import {execFileSync, spawn} from 'node:child_process';
import {existsSync} from 'node:fs';
import {test} from 'node:test';
import pty from 'node-pty';
import {killSurvivors, snapshotDescendants} from '../src/processTree.js';

const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

function isAlive(pid: number): boolean {
	try { process.kill(pid, 0); return true; } catch { return false; }
}

function commandOf(pid: number): string {
	try { return execFileSync('ps', ['-o', 'command=', '-p', String(pid)], {encoding: 'utf8'}).trim(); } catch { return ''; }
}

test('kills a SIGHUP-ignoring job that an interactive shell moved to its own process group', {skip: !existsSync('/bin/zsh') && 'zsh not installed'}, async () => {
	// Compound command, so zsh cannot exec it and runs it as a job in a separate process group.
	const term = pty.spawn('/bin/zsh', ['-f', '-ic', `sh -c 'trap "" HUP; exec sleep 60'; echo done`], {cols: 80, rows: 24, cwd: '/tmp'});
	try {
		await sleep(500);
		const descendants = snapshotDescendants(term.pid);
		const sleeper = [...descendants.keys()].find(pid => commandOf(pid) === 'sleep 60');
		assert.ok(sleeper, 'expected to find the sleep descendant');

		process.kill(-term.pid, 'SIGHUP');
		await sleep(500);
		assert.equal(isAlive(sleeper), true, 'the job should survive the group SIGHUP');
		try { process.kill(-term.pid, 'SIGKILL'); } catch {}
		await sleep(200);
		assert.equal(isAlive(sleeper), true, 'the group SIGKILL should not reach the job');

		killSurvivors(descendants);
		await sleep(200);
		assert.equal(isAlive(sleeper), false);
	} finally {
		try { term.kill('SIGKILL'); } catch {}
	}
});

test('does not kill a PID whose start time no longer matches', async () => {
	const child = spawn('sleep', ['60'], {stdio: 'ignore'});
	try {
		await sleep(100);
		killSurvivors(new Map([[child.pid!, 'Thu Jan  1 00:00:00 1970']]));
		await sleep(100);
		assert.equal(isAlive(child.pid!), true);
	} finally {
		child.kill('SIGKILL');
	}
});
