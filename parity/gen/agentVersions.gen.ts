/** Records src/agentVersions.ts. Regenerate: node --import tsx parity/gen/agentVersions.gen.ts */
import {Recorder, arb, fc, installDeterminism, mutate, runTap, sample} from '../lib.ts';
import {agentUpdateStatus, compareVersions, installedVersions, parseVersion, sessionOutdated, updateHint} from '../../src/agentVersions.ts';

installDeterminism();
const recorder = new Recorder('agentVersions');

recorder.addTap(await runTap({tests: ['tests/agentVersions.test.ts'], tap: ['agentVersions:*']}), {module: 'agentVersions'});

const PROGRAMS = ['claude', 'pi', 'codex'] as const;
const texts = [
	undefined, '', '2.1.287 (Claude Code)', 'codex-cli 0.157.0', '1.0.2', 'v1.2.3', 'V1.2.3', '1.2', '1.2.3.4', '1.2.3-alpha.1', '01.002.0003', '0.0.0',
	'x1.2.3', '.1.2.3', '1.2.3 and 4.5.6', 'version\n3.4.5\n', '١.٢.٣', '１.２.３', '中 1.2.3 中', '😀1.2.3', 'v', 'v.1.2.3',
	'1..2.3', '1.2.a', ' 10.20.30 ', '1.2.3', '-1.2.3', '1.2.3\u0000', 'a1.2.3', '1_2_3', '2.1.287 (Claude Code) 3.0.0',
];
for (const text of texts) recorder.call('parseVersion', parseVersion, [text]);
const part = fc.oneof(fc.nat(30), fc.constantFrom(0, 1, 9, 10, 99, 100, 287, 1000));
const versionArb = fc.tuple(part, part, part).map(parts => parts.join('.'));
const noisy = fc.tuple(fc.constantFrom('', 'v', 'codex-cli ', 'x', '.', ' ', '😀', '中'), versionArb, fc.constantFrom('', ' (Claude Code)', '-alpha.1', '.4', '\n', 'é'));
for (const [prefix, version, suffix] of sample(noisy, 200)) recorder.call('parseVersion', parseVersion, [`${prefix}${version}${suffix}`]);
for (const text of sample(arb.text(14, ['1', '2', '.', 'v', ' ', '0', '-', 'a', '😀', '中', '\n']), 150, 3)) recorder.call('parseVersion', parseVersion, [text]);
for (const args of mutate(recorder.argsOf('parseVersion').filter(a => typeof a[0] === 'string').slice(0, 80), {variants: 2})) recorder.call('parseVersion', parseVersion, args);

const pairs: Array<[string, string]> = [];
for (const left of texts.filter((t): t is string => typeof t === 'string')) for (const right of ['1.2.3', '1.2.4', '2.0.0', 'junk', '', 'codex-cli 0.157.0']) pairs.push([left, right], [right, left]);
for (const pair of pairs) recorder.call('compareVersions', compareVersions, pair);
for (const [left, right] of sample(fc.tuple(versionArb, versionArb), 250)) recorder.call('compareVersions', compareVersions, [left, right]);
for (const [text, right] of sample(fc.tuple(noisy, versionArb), 80, 5)) recorder.call('compareVersions', compareVersions, [text.join(''), right]);
for (const args of mutate(recorder.argsOf('compareVersions').slice(0, 80), {variants: 2})) recorder.call('compareVersions', compareVersions, args);

const infos: any[] = [undefined, {}, {program: 'claude'}];
const sv = [undefined, '', '1.2.3', '1.2.4', '2.0.0', 'junk', '1.2.3-alpha.1', 'v1.2.3', 'codex-cli 0.157.0'];
for (const installed of sv) for (const latest of sv) infos.push({program: 'claude', installed, latest, path: '/usr/bin/claude'});
for (const info of infos) recorder.call('agentUpdateStatus', agentUpdateStatus, [info]);
for (const [installed, latest] of sample(fc.tuple(fc.option(versionArb, {nil: undefined}), fc.option(versionArb, {nil: undefined})), 100)) recorder.call('agentUpdateStatus', agentUpdateStatus, [{program: 'pi', installed, latest}]);

const statuses = ['running', 'exited', 'starting', 'stopped', undefined];
for (const status of statuses) for (const agentVersion of [undefined, '', '1.2.3', '1.2.4', 'junk']) for (const installed of [undefined, '', '1.2.3', '1.3.0', 'junk', '1.2.3-alpha.1']) {
	recorder.call('sessionOutdated', sessionOutdated, [{status, ...agentVersion === undefined ? {} : {agentVersion}}, installed]);
}
for (const [status, agentVersion, installed] of sample(fc.tuple(fc.constantFrom('running', 'exited'), fc.option(versionArb, {nil: undefined}), fc.option(versionArb, {nil: undefined})), 150)) {
	recorder.call('sessionOutdated', sessionOutdated, [{status, agentVersion}, installed]);
}

const infoArb = (program: string) => fc.record({program: fc.constant(program), installed: fc.option(fc.oneof(versionArb, fc.constant('')), {nil: undefined}), latest: fc.option(versionArb, {nil: undefined})}, {requiredKeys: ['program']});
const versionsArb = fc.record({claude: infoArb('claude'), pi: infoArb('pi'), codex: infoArb('codex')});
recorder.call('installedVersions', installedVersions, [undefined]);
recorder.call('installedVersions', installedVersions, [{}]);
recorder.call('installedVersions', installedVersions, [{claude: {program: 'claude', installed: '1.0.0'}}]);
for (const versions of sample(versionsArb, 120)) {
	recorder.call('installedVersions', installedVersions, [versions]);
	recorder.call('updateHint', updateHint, [versions]);
}
recorder.call('updateHint', updateHint, [undefined]);
recorder.call('updateHint', updateHint, [{}]);
const info = (program: string, installed?: string, latest?: string) => ({program, ...installed ? {installed} : {}, ...latest ? {latest} : {}});
for (const a of [['1.0.0', '1.0.1'], ['1.0.0', '1.0.0'], [undefined, '1.0.0'], ['1.0.0', undefined]] as const) {
	for (const b of [['2.0.0', '3.0.0'], ['2.0.0', '2.0.0']] as const) for (const c of [['0.1.0', '0.2.0'], ['junk', '9.9.9'], [undefined, undefined]] as const) {
		recorder.call('updateHint', updateHint, [{claude: info('claude', ...a), pi: info('pi', ...b), codex: info('codex', ...c)}]);
	}
}
for (const args of mutate(recorder.argsOf('updateHint').slice(0, 60), {variants: 2})) recorder.call('updateHint', updateHint, args);

await recorder.write({prune: true});
