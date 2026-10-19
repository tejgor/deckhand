/**
 * Fixtures for src/agentSignals.ts (unit I7: hookCommand, hookSettings, integrationArgs; the rest is P1's).
 * hookCommand/hookSettings embed machine paths: node's path is `$EXEC`, the CLI entry `$CLI`, the tsx loader `$TSX`.
 * integrationArgs runs `<command> --help`, so it is recorded as directory cases with fake programs (shell scripts).
 */
import fs from 'node:fs/promises';
import path from 'node:path';
import {Recorder, installDeterminism, recordDirCase} from '../lib.ts';
import {getCliEntryPath, getTsxLoaderPath} from '../../src/paths.ts';
import {hookCommand, hookSettings, integrationArgs} from '../../src/agentSignals.ts';

const MODULE = 'agentSignals-I7';
installDeterminism();
delete process.env.DECKHAND_CHANNEL;
const withEnv = <T>(env: Record<string, string | undefined>, run: () => T): T => {
	const saved = Object.fromEntries(Object.keys(env).map(key => [key, process.env[key]]));
	for (const [key, value] of Object.entries(env)) { if (value === undefined) delete process.env[key]; else process.env[key] = value; }
	try { return run(); } finally { for (const [key, value] of Object.entries(saved)) { if (value === undefined) delete process.env[key]; else process.env[key] = value; } }
};
const prod = {DECKHAND_DEV: undefined}, dev = {DECKHAND_DEV: '1'};
const scrubbers: [string, string][] = [];
for (const env of [prod, dev]) withEnv(env, () => { scrubbers.push([getCliEntryPath(), '$CLI'], [getTsxLoaderPath(), '$TSX']); });
scrubbers.push([process.execPath, '$EXEC']);
const scrubText = (text: string) => scrubbers.reduce((out, [from, to]) => out.split(from).join(to), text);
const scrubArgs = (promise: Promise<string[]>) => promise.then(args => args.map(scrubText));
const recorder = new Recorder(MODULE, {scrub: line => scrubbers.reduce((text, [from, to]) => text.split(JSON.stringify(from).slice(1, -1)).join(to), line)});

// hookCommand(): production runs `node dist/cli.js hook`, dev adds the tsx loader.
recorder.call('hookCommand', () => withEnv(prod, hookCommand), []);
recorder.call('hookCommandDev', () => withEnv(dev, hookCommand), []);
// hookSettings(options?)
for (const options of [undefined, {}, {async: true}, {async: false}, {async: undefined}, {async: 1 as unknown as boolean}, {async: 0 as unknown as boolean}, {extra: 1} as {async?: boolean}]) {
	recorder.call('hookSettings', (...args: any[]) => withEnv(prod, () => (hookSettings as any)(...args)), options === undefined ? [] : [options]);
	recorder.call('hookSettingsDev', (...args: any[]) => withEnv(dev, () => (hookSettings as any)(...args)), options === undefined ? [] : [options]);
}
await recorder.write({prune: true});

// integrationArgs(program, command, enabled): `command` is a fake program under $ROOT.
const helps: Record<string, string> = {
	both: 'usage: x --settings <file> --no-daemon\n',
	settings: 'usage: x --settings <file>\n',
	daemon: 'usage: x --no-daemon\n',
	none: 'usage: x [prompt]\n',
	empty: '',
	unicode: '用法 😀 --settings --no-daemon é\n',
	loose: 'prefix--settingsXYZ and a--no-daemon.\n',
};
const script = (body: string) => `#!/bin/sh\n${body}\n`;
const programs: Record<string, string> = {
	...Object.fromEntries(Object.entries(helps).map(([name, text]) => [name, script(`if [ "$1" = "--help" ]; then printf '%s' ${JSON.stringify(text).replaceAll('$', '\\$').replaceAll('`', '\\`')}; exit 0; fi\nexit 3`)])),
	failing: script('echo "--settings --no-daemon"; exit 1'),
	stderrOnly: script('echo "--settings --no-daemon" >&2; exit 0'),
	nonexec: '#!/bin/sh\necho "--settings"\n',
};
const channel = (value: string | undefined) => ({DECKHAND_CHANNEL: value});
let count = 0;
for (const [name, body] of Object.entries(programs)) {
	for (const program of ['claude', 'codex', 'pi', 'other'] as const) {
		for (const enabled of [true, false]) {
			for (const env of [undefined, 'dev']) {
				// Dev-channel isolation only matters for codex (and enabled=false); other programs get one dev case.
				if (env === 'dev' && program !== 'codex' && !(name === 'both' && enabled)) continue;
				const caseName = `integration-args-${name}-${program}-${enabled ? 'on' : 'off'}${env ? '-dev-channel' : ''}`;
				await recordDirCase(MODULE, caseName, {
					setup: async root => {
						await fs.writeFile(path.join(root, 'agent'), body, {mode: name === 'nonexec' ? 0o644 : 0o755});
					},
					fn: env ? 'integrationArgsDevChannel' : 'integrationArgs',
					args: root => [program, path.join(root, 'agent'), enabled],
					run: (...args: [any, string, boolean]) => scrubArgs(withEnv(channel(env === 'dev' ? 'dev' : undefined), () => integrationArgs(...args))),
				});
				count++;
			}
		}
	}
}
// A command that does not exist and a different channel value.
for (const [program, env] of [['claude', undefined], ['codex', 'dev'], ['codex', 'prod'], ['codex', undefined]] as const) {
	await recordDirCase(MODULE, `integration-args-missing-${program}-${env ?? 'unset'}`, {
		setup: async () => {},
		fn: env === 'dev' ? 'integrationArgsDevChannel' : 'integrationArgs',
		args: root => [program, path.join(root, 'nope'), true],
		run: (...args: [any, string, boolean]) => scrubArgs(withEnv(channel(env), () => integrationArgs(...args))),
	});
	count++;
}
// Unknown channel with codex and a disabled agent.
await recordDirCase(MODULE, 'integration-args-codex-off-channel-prod', {
	setup: async root => { await fs.writeFile(path.join(root, 'agent'), programs.both!, {mode: 0o755}); },
	fn: 'integrationArgs',
	args: root => ['codex', path.join(root, 'agent'), false],
	run: (...args: [any, string, boolean]) => scrubArgs(withEnv(channel('prod'), () => integrationArgs(...args))),
});
console.error(`agentSignals-I7: ${count + 1} integrationArgs cases`);
