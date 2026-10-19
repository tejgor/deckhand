// scripts/verify-unit implementation: checks one port unit and writes a JSON report a fixing agent can act on.
// Plain ESM, no build step. Every child runs with the caller's (validated) DECKHAND_HOME and a fresh temp HOME.
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {parse as parseToml} from 'smol-toml';
import {listExports, listTestTitles} from '../parity/ts-exports.mjs';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const CHECKS = ['scope', 'build', 'placeholders', 'fixtures', 'tests', 'exports', 'ts-tests', 'ts-suites'];
const USAGE = `Usage: DECKHAND_HOME=<temp dir> scripts/verify-unit <unit> [--base rust-port] [--gate-suites] [--report out.json]
       [--only ${CHECKS.join(',')}] [--skip <checks>]
Checks (report: {unit, ok, checks:[{name, ok, gating, details}]}):
  scope         files changed vs --base (minus merged port/<dep> branches) stay within the unit's paths
  build         cargo build --workspace --all-targets; cargo fmt --all --check; cargo clippy -p <crate> -D warnings
  placeholders  no todo!/unimplemented!/#[ignore] in owned rust_paths
  fixtures      manifest fixtures exist and regenerate byte-identically (parity/gen/<module>.gen.ts)
  tests         cargo nextest run -p <crate> passes; every manifest fixture replayed within its allowlist (<=2%)
  exports       every export of ts_sources mapped in exactly one manifest [exports] (to an existing Rust symbol) or dropped
  ts-tests      every test title of ts_tests covered by a passing "// covers: B-<unit>-NN <title>" Rust test or deferred
  ts-suites     ts_suites run twice against target/debug/deckhand (informational unless --gate-suites)`;

// ---------------------------------------------------------------------------------------------------------------
// Isolation
// ---------------------------------------------------------------------------------------------------------------

async function canonical(file) {
	try { return await fsp.realpath(file); }
	catch (error) {
		if (error.code !== 'ENOENT') throw error;
		const stat = await fsp.lstat(file).catch(() => undefined);
		if (stat?.isSymbolicLink()) return canonical(path.resolve(path.dirname(file), await fsp.readlink(file)));
		const parent = path.dirname(file);
		if (parent === file) return file;
		return path.join(await canonical(parent), path.basename(file));
	}
}
/** Same rule as isolatedHome() in scripts/deckhand-dev.mjs: neither equal to, inside, nor containing real state. */
export async function checkIsolatedHome(value) {
	if (!value) throw new Error('verify-unit: DECKHAND_HOME must be set to a temp directory, e.g. DECKHAND_HOME=$(mktemp -d) scripts/verify-unit <unit>');
	const homes = [...new Set([os.homedir(), os.userInfo().homedir])];
	const protectedDirs = await Promise.all(homes.flatMap(home => ['.deckhand', '.deckhand-dev', '.codex', '.claude'].map(name => canonical(path.join(home, name)))));
	const candidate = await canonical(path.resolve(value));
	for (const dir of protectedDirs) {
		if (candidate === dir || candidate.startsWith(`${dir}${path.sep}`) || dir.startsWith(`${candidate}${path.sep}`)) {
			throw new Error(`verify-unit: refusing DECKHAND_HOME=${value} (resolves to ${candidate}, which overlaps ${dir}); use a temp directory`);
		}
	}
	return candidate;
}

// ---------------------------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------------------------

let childEnv = process.env;
function run(command, args, options = {}) {
	return new Promise(resolve => {
		const started = Date.now();
		const child = spawn(command, args, {cwd: options.cwd ?? ROOT, env: {...childEnv, ...options.env}, stdio: ['ignore', 'pipe', 'pipe']});
		let output = '', stdout = '';
		child.stdout.on('data', chunk => { output += chunk; stdout += chunk; });
		child.stderr.on('data', chunk => { output += chunk; });
		const timer = options.timeout ? setTimeout(() => { output += `\n[verify-unit: timed out after ${options.timeout} ms]`; child.kill('SIGKILL'); }, options.timeout) : undefined;
		child.once('error', error => { clearTimeout(timer); resolve({code: 127, output: String(error), stdout: '', ms: Date.now() - started}); });
		child.once('close', code => { clearTimeout(timer); resolve({code: code ?? 1, output, stdout, ms: Date.now() - started}); });
	});
}
const tail = (text, lines = 60) => text.split('\n').slice(-lines).join('\n');
const exists = file => fs.existsSync(path.join(ROOT, file));
const log = message => process.stderr.write(`verify-unit: ${message}\n`);
const moduleOf = file => path.basename(file).replace(/\.(tsx?|mjs|js)$/, '');

function walkFiles(relative, predicate = () => true) {
	const full = path.join(ROOT, relative);
	if (!fs.existsSync(full)) return [];
	if (fs.statSync(full).isFile()) return predicate(relative) ? [relative] : [];
	const out = [];
	for (const name of fs.readdirSync(full).sort()) out.push(...walkFiles(path.posix.join(relative, name), predicate));
	return out;
}

/** cargo diagnostics: {level, message, location} for error/warning lines. */
function cargoDiagnostics(output, limit = 40) {
	const lines = output.split('\n'), out = [];
	for (let index = 0; index < lines.length && out.length < limit; index++) {
		const match = /^(error|warning)(\[[^\]]+\])?: (.*)$/.exec(lines[index]);
		if (!match || /^(warning|error): .*(generated \d+ warning|could not compile|aborting due to)/.test(lines[index])) continue;
		const location = /^\s+--> (.*)$/.exec(lines[index + 1] ?? '')?.[1];
		out.push({level: match[1], message: `${match[2] ?? ''}${match[2] ? ' ' : ''}${match[3]}`.trim(), ...(location ? {location} : {})});
	}
	return out;
}

function loadManifest(unit) {
	const file = path.join(ROOT, 'parity', 'units', `${unit}.toml`);
	if (!fs.existsSync(file)) throw new Error(`verify-unit: no manifest parity/units/${unit}.toml`);
	const manifest = parseToml(fs.readFileSync(file, 'utf8'));
	for (const key of ['ts_sources', 'rust_paths', 'deps', 'ts_tests', 'ts_suites', 'fixtures', 'deferred_tests', 'allowlist']) manifest[key] ??= [];
	manifest.utf16_fields ??= {};
	manifest.exports ??= {};
	return manifest;
}
function allManifests() {
	const dir = path.join(ROOT, 'parity', 'units');
	return fs.readdirSync(dir).filter(name => name.endsWith('.toml')).sort().map(name => {
		try { return {unit: name.slice(0, -5), manifest: parseToml(fs.readFileSync(path.join(dir, name), 'utf8'))}; }
		catch (error) { return {unit: name.slice(0, -5), error: String(error.message ?? error)}; }
	});
}
/** `[exports]` flattened to [{key: "<module>.<export>", value}]. Accepts `module.name = ..` (nested) and `"module.name" = ..`. */
function flattenExports(table) {
	const out = [];
	for (const [key, value] of Object.entries(table ?? {})) {
		if (value && typeof value === 'object' && !Array.isArray(value)) for (const [name, inner] of Object.entries(value)) out.push({key: `${key}.${name}`, value: inner});
		else out.push({key, value});
	}
	return out;
}
function crateName(crate) {
	const file = path.join(ROOT, 'crates', crate, 'Cargo.toml');
	if (!fs.existsSync(file)) return undefined;
	return parseToml(fs.readFileSync(file, 'utf8')).package?.name;
}
function ownedRustFiles(manifest) {
	return manifest.rust_paths.flatMap(entry => walkFiles(entry.replace(/\/$/, ''), file => file.endsWith('.rs')));
}
const escapeRegExp = text => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

// ---------------------------------------------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------------------------------------------

async function git(...args) {
	const result = await run('git', args);
	return {ok: result.code === 0, out: result.stdout};
}

async function checkScope(ctx) {
	const {unit, manifest, base} = ctx;
	if (!(await git('rev-parse', '--verify', '--quiet', `${base}^{commit}`)).ok) return {ok: false, details: {error: `base ${base} does not exist (pass --base <branch>)`}};
	const deps = [];
	for (const dep of manifest.deps) if ((await git('rev-parse', '--verify', '--quiet', `refs/heads/port/${dep}`)).ok) deps.push(`port/${dep}`);
	const committed = (await git('log', '--no-merges', '--name-only', '--format=', 'HEAD', '--not', base, ...deps)).out.split('\n').filter(Boolean);
	const status = (await git('status', '--porcelain=v1', '-z', '--untracked-files=all')).out.split('\0');
	const uncommitted = [];
	for (let index = 0; index < status.length; index++) {
		const entry = status[index];
		if (!entry) continue;
		uncommitted.push(entry.slice(3));
		if (/^[RC]/.test(entry)) uncommitted.push(status[++index]); // rename/copy source
	}
	const modules = [...new Set([...manifest.ts_sources.map(moduleOf), ...manifest.fixtures.map(fixture => fixture.split('/')[0])])];
	const allowed = [
		...manifest.rust_paths.map(entry => (entry.endsWith('.rs') ? entry : `${entry.replace(/\/$/, '')}/**`)),
		...modules.flatMap(module => [`fixtures/parity/${module}/**`, `parity/gen/${module}.gen.ts`]),
		`parity/units/${unit}.toml`, 'Cargo.lock',
	];
	const isAllowed = file => manifest.rust_paths.some(entry => file === entry || file.startsWith(`${entry.replace(/\/$/, '')}/`))
		|| modules.some(module => file.startsWith(`fixtures/parity/${module}/`) || file === `parity/gen/${module}.gen.ts`)
		|| file === `parity/units/${unit}.toml` || file === 'Cargo.lock';
	const outside = [];
	for (const file of new Set(committed)) if (!isAllowed(file)) outside.push({path: file, source: 'committed'});
	for (const file of new Set(uncommitted)) if (!isAllowed(file) && !outside.some(entry => entry.path === file)) outside.push({path: file, source: 'uncommitted'});
	return {ok: outside.length === 0, details: {base, excluded_dep_branches: deps, changed_files: new Set([...committed, ...uncommitted]).size, allowed, outside, ...(outside.length ? {fix: 'revert or move these changes; files owned by other units are frozen (report needed changes instead)'} : {})}};
}

async function checkBuild(ctx) {
	const steps = [
		['cargo', ['build', '--workspace', '--all-targets']],
		['cargo', ['fmt', '--all', '--check']],
		['cargo', ['clippy', '-p', ctx.crate, '--all-targets', '--', '-D', 'warnings']],
	];
	const results = [];
	for (const [command, args] of steps) {
		if (!ctx.crate && args.includes('-p')) { results.push({command: `${command} ${args.join(' ')}`, ok: false, error: `unknown crate ${ctx.manifest.crate}`}); continue; }
		log(`${command} ${args.join(' ')}`);
		const result = await run(command, args, {timeout: 60 * 60_000});
		const entry = {command: `${command} ${args.join(' ')}`, ok: result.code === 0, exit: result.code, seconds: Math.round(result.ms / 1000)};
		if (args[0] === 'fmt') entry.unformatted = [...new Set([...result.output.matchAll(/^Diff in (.*?):\d+:/gm)].map(match => path.relative(ROOT, match[1])))];
		else if (!entry.ok) entry.diagnostics = cargoDiagnostics(result.output);
		if (!entry.ok) entry.output_tail = tail(result.output, 40);
		results.push(entry);
	}
	return {ok: results.every(entry => entry.ok), details: {steps: results}};
}

async function checkPlaceholders(ctx) {
	const files = ownedRustFiles(ctx.manifest), hits = [];
	const pattern = /\btodo!\s*\(|\bunimplemented!\s*\(|#\[\s*ignore\b/;
	for (const file of files) {
		fs.readFileSync(path.join(ROOT, file), 'utf8').split('\n').forEach((text, index) => { if (pattern.test(text)) hits.push({file, line: index + 1, text: text.trim()}); });
	}
	const missing = ctx.manifest.rust_paths.filter(entry => !exists(entry));
	return {ok: hits.length === 0, details: {files_scanned: files.length, hits, ...(missing.length ? {missing_rust_paths: missing} : {})}};
}

function listTree(root) {
	const out = new Map();
	const walk = (dir, prefix) => {
		if (!fs.existsSync(dir)) return;
		for (const name of fs.readdirSync(dir).sort()) {
			const full = path.join(dir, name), relative = prefix ? `${prefix}/${name}` : name;
			const stat = fs.lstatSync(full);
			if (stat.isDirectory()) walk(full, relative);
			else out.set(relative, stat.isSymbolicLink() ? Buffer.from(`symlink:${fs.readlinkSync(full)}`) : fs.readFileSync(full));
		}
	};
	walk(root, '');
	return out;
}
function firstLineDifference(left, right) {
	const a = left.toString('utf8').split('\n'), b = right.toString('utf8').split('\n');
	for (let index = 0; index < Math.max(a.length, b.length); index++) {
		if (a[index] !== b[index]) return {line: index + 1, committed: (a[index] ?? '<eof>').slice(0, 300), regenerated: (b[index] ?? '<eof>').slice(0, 300)};
	}
	return {line: 0};
}

async function checkFixtures(ctx) {
	const {manifest} = ctx;
	if (!manifest.fixtures.length) return {ok: true, details: {note: 'no fixtures listed in the manifest'}};
	const missing = manifest.fixtures.filter(fixture => !exists(`fixtures/parity/${fixture}.jsonl`) && !exists(`fixtures/parity/${fixture}/op.json`));
	const modules = [...new Set(manifest.fixtures.map(fixture => fixture.split('/')[0]))];
	const out = await fsp.mkdtemp(path.join(os.tmpdir(), 'verify-unit-fixtures-'));
	const generated = [];
	try {
		for (const module of modules) {
			const gen = `parity/gen/${module}.gen.ts`;
			if (!exists(gen)) { generated.push({module, gen, ok: false, error: `missing ${gen}`}); continue; }
			log(`regenerating ${gen}`);
			const result = await run(process.execPath, ['--import', 'tsx', gen], {env: {PARITY_FIXTURES_OUT: out}, timeout: 30 * 60_000});
			if (result.code !== 0) { generated.push({module, gen, ok: false, error: `exit ${result.code}`, output_tail: tail(result.output, 40)}); continue; }
			const committed = listTree(path.join(ROOT, 'fixtures', 'parity', module)), regenerated = listTree(path.join(out, module));
			const onlyCommitted = [...committed.keys()].filter(file => !regenerated.has(file)).map(file => `fixtures/parity/${module}/${file}`);
			const onlyRegenerated = [...regenerated.keys()].filter(file => !committed.has(file)).map(file => `fixtures/parity/${module}/${file}`);
			const differing = [...committed].filter(([file, data]) => regenerated.has(file) && !data.equals(regenerated.get(file)))
				.map(([file, data]) => ({file: `fixtures/parity/${module}/${file}`, ...firstLineDifference(data, regenerated.get(file))}));
			const ok = !onlyCommitted.length && !onlyRegenerated.length && !differing.length;
			generated.push({module, gen, ok, ...(ok ? {} : {stale_committed: onlyCommitted, not_committed: onlyRegenerated, differing, fix: `run: node --import tsx ${gen} (twice; output must not change) and commit fixtures/parity/${module}/`})});
		}
	} finally {
		await fsp.rm(out, {recursive: true, force: true});
	}
	return {ok: !missing.length && generated.every(entry => entry.ok), details: {fixtures: manifest.fixtures.length, missing, modules: generated}};
}

/** nextest / libtest status lines -> Map(testName -> {status, binary}). */
function parseTestStatuses(output) {
	const statuses = new Map();
	const set = (name, binary, passed) => {
		const previous = statuses.get(name);
		statuses.set(name, {passed: passed && (previous?.passed ?? true), binary});
	};
	for (const line of output.split('\n')) {
		const nextest = /^\s*(PASS|FAIL|FLAKY|SIGSEGV|SIGABRT|SIGKILL|SIGTERM|SIGBUS|ABORT|TIMEOUT|LEAK-FAIL|LEAK|TRY \d+ (?:PASS|FAIL)|START|SLOW|RETRY|SKIP)\s+\[[^\]]*\]\s+(?:\(\s*\d+\/\s*\d+\)\s+)?(\S+)\s+(\S+)\s*$/.exec(line);
		if (nextest) {
			const status = nextest[1];
			if (['START', 'SLOW', 'RETRY', 'SKIP'].includes(status) || status.startsWith('TRY')) continue;
			set(nextest[3], nextest[2], status === 'PASS' || status === 'LEAK' || status === 'FLAKY');
			continue;
		}
		const libtest = /^test (\S+) \.\.\. (ok|FAILED|ignored)/.exec(line);
		if (libtest && libtest[2] !== 'ignored') set(libtest[1], '', libtest[2] === 'ok');
	}
	return statuses;
}

async function checkTests(ctx) {
	const {unit, manifest} = ctx;
	if (!ctx.crate) return {ok: false, details: {error: `unknown crate ${manifest.crate}`}};
	const results = await fsp.mkdtemp(path.join(os.tmpdir(), 'verify-unit-results-'));
	try {
		const nextest = fs.existsSync(path.join(ROOT, 'target', 'tools', 'bin', 'cargo-nextest'));
		const args = nextest
			? ['nextest', 'run', '-p', ctx.crate, '--no-fail-fast', '--status-level', 'pass', '--final-status-level', 'fail', '--no-tests=pass']
			: ['test', '-p', ctx.crate, '--no-fail-fast'];
		log(`cargo ${args.join(' ')}`);
		const result = await run('cargo', args, {env: {PARITY_RESULTS_DIR: results, PATH: `${path.join(ROOT, 'target', 'tools', 'bin')}${path.delimiter}${childEnv.PATH ?? ''}`}, timeout: 60 * 60_000});
		const statuses = parseTestStatuses(result.output);
		ctx.testStatuses = statuses;
		const failed = [...statuses].filter(([, entry]) => !entry.passed).map(([name, entry]) => `${entry.binary} ${name}`.trim());
		const fixtures = [];
		const resultFiles = [];
		const walk = (dir, prefix) => {
			if (!fs.existsSync(dir)) return;
			for (const name of fs.readdirSync(dir).sort()) {
				const full = path.join(dir, name), relative = prefix ? `${prefix}/${name}` : name;
				if (fs.statSync(full).isDirectory()) walk(full, relative);
				else if (name.endsWith('.json')) resultFiles.push({fixture: relative.slice(0, -5), full});
			}
		};
		walk(path.join(results, unit), '');
		for (const {fixture, full} of resultFiles) {
			const report = JSON.parse(fs.readFileSync(full, 'utf8'));
			const unallowlisted = report.failures.filter(failure => !failure.allowlisted);
			const allowlisted = report.failures.length - unallowlisted.length;
			const limit = Math.floor(report.cases / 50);
			const ok = report.cases > 0 && !report.problems.length && !unallowlisted.length && allowlisted <= limit && manifest.fixtures.includes(fixture);
			fixtures.push({
				fixture, ok, cases: report.cases, failed: report.failures.length, allowlisted, allowlist_limit: limit,
				...(report.problems.length ? {problems: report.problems} : {}),
				...(manifest.fixtures.includes(fixture) ? {} : {problem: 'not listed in the manifest fixtures'}),
				...(unallowlisted.length ? {failing_cases: unallowlisted.slice(0, 25).map(failure => ({index: failure.index, kind: failure.kind, diff: failure.diff, args: failure.args}))} : {}),
				...(report.stale_allowlist?.length ? {stale_allowlist: report.stale_allowlist} : {}),
			});
		}
		const replayed = new Set(fixtures.map(entry => entry.fixture));
		const notReplayed = manifest.fixtures.filter(fixture => !replayed.has(fixture));
		const ok = result.code === 0 && fixtures.every(entry => entry.ok) && !notReplayed.length;
		return {ok, details: {
			runner: nextest ? 'cargo nextest' : 'cargo test', exit: result.code, tests_run: statuses.size,
			passed: [...statuses.values()].filter(entry => entry.passed).length, failed_tests: failed,
			fixtures, not_replayed: notReplayed.map(fixture => ({fixture, fix: `add a test calling deckhand_parity::replay(${JSON.stringify(unit)}, ${JSON.stringify(fixture)}, ..) (or replay_dir for a directory case)`})),
			...(result.code !== 0 ? {diagnostics: cargoDiagnostics(result.output), output_tail: tail(result.output, 80)} : {}),
		}};
	} finally {
		await fsp.rm(results, {recursive: true, force: true});
	}
}

function symbolDefined(source, symbol, first) {
	const name = escapeRegExp(symbol);
	// Also items declared through a local item macro, e.g. `string_union!(/// doc\n Name { .. })` in deckhand-protocol.
	const definition = new RegExp(`\\b(?:fn|struct|enum|trait|type|const|static|mod|union)\\s+${name}\\b|macro_rules!\\s*${name}\\b|\\b[a-z_]+!\\s*[({]\\s*(?:(?:///[^\\n]*|#\\[[^\\]]*\\])\\s*)*${name}\\s*\\{`);
	return definition.test(source) || (!first && new RegExp(`\\b${name}\\b`).test(source));
}
function validateTarget(value) {
	if (typeof value !== 'string') return `expected "<file.rs>::<symbol>" or "drop: <reason>", got ${JSON.stringify(value)}`;
	if (value.startsWith('drop:')) return value.slice(5).trim() ? undefined : 'drop needs a reason: "drop: <reason>"';
	const match = /^(.+?\.rs)::([A-Za-z_][\w]*(?:::[A-Za-z_][\w]*)*)$/.exec(value);
	if (!match) return `expected "<file.rs>::<symbol>" (e.g. "crates/core/src/text_editor.rs::edit_text") or "drop: <reason>"`;
	const [, file, symbols] = match;
	if (!exists(file)) return `${file} does not exist`;
	const source = fs.readFileSync(path.join(ROOT, file), 'utf8');
	const segments = symbols.split('::');
	// The first segment must be defined in the file; later ones (methods, variants, fields) only have to occur in it.
	const undefinedSegment = segments.find((segment, index) => !symbolDefined(source, segment, index === 0));
	return undefinedSegment ? `${undefinedSegment} is not defined in ${file}` : undefined;
}

async function checkExports(ctx) {
	const {unit, manifest} = ctx;
	const others = allManifests();
	const mapping = new Map();
	const broken = others.filter(entry => entry.error).map(entry => ({unit: entry.unit, error: entry.error}));
	for (const entry of others) {
		if (entry.error) continue;
		for (const {key, value} of flattenExports(entry.manifest.exports)) {
			if (!mapping.has(key)) mapping.set(key, []);
			mapping.get(key).push({unit: entry.unit, value});
		}
	}
	const exportsByModule = new Map(), missing = [], duplicates = [], invalid = [], notes = [];
	let mapped = 0, dropped = 0, total = 0;
	for (const source of manifest.ts_sources) {
		if (!exists(source)) { invalid.push({key: source, problem: `ts_source ${source} does not exist`}); continue; }
		const module = moduleOf(source);
		const entries = listExports(path.join(ROOT, source));
		exportsByModule.set(module, new Set(entries.map(entry => entry.name)));
		for (const {name, kind} of entries) {
			if (kind === 'star') { notes.push(`${source}: export * from '${name}' (map the re-exported names in their own module)`); continue; }
			total++;
			const key = `${module}.${name}`, targets = mapping.get(key) ?? [];
			if (!targets.length) { missing.push({key, kind, source, fix: `add under [exports] in parity/units/${unit}.toml: ${module}.${name} = "<crates/.../file.rs>::<symbol>" or "drop: <reason>"`}); continue; }
			if (targets.length > 1) { duplicates.push({key, units: targets.map(target => target.unit)}); continue; }
			const values = Array.isArray(targets[0].value) ? targets[0].value : [targets[0].value];
			for (const value of values) {
				const problem = validateTarget(value);
				if (problem) invalid.push({key, unit: targets[0].unit, value, problem});
			}
			if (values.every(value => typeof value === 'string' && value.startsWith('drop:'))) dropped++; else mapped++;
		}
	}
	// Keys in this manifest that name no export (typos, removed exports).
	const stale = [];
	for (const {key} of flattenExports(manifest.exports)) {
		const [module, ...rest] = key.split('.');
		const name = rest.join('.');
		const known = exportsByModule.get(module);
		if (known && !known.has(name)) stale.push({key, problem: `src/${module} has no export ${name}`});
		else if (!known && !['.ts', '.tsx'].some(extension => exists(`src/${module}${extension}`))) stale.push({key, problem: `no module src/${module}.ts(x)`});
	}
	const ok = !missing.length && !duplicates.length && !invalid.length && !stale.length;
	return {ok, details: {exports: total, mapped, dropped, missing, duplicates, invalid, stale, ...(notes.length ? {notes} : {}), ...(broken.length ? {unparsable_manifests: broken} : {})}};
}

/** `// covers: B-<unit>-NN <title>` comments above test functions in owned Rust files. */
function listCovers(manifest) {
	const out = [];
	for (const file of ownedRustFiles(manifest)) {
		const lines = fs.readFileSync(path.join(ROOT, file), 'utf8').split('\n');
		lines.forEach((text, index) => {
			const match = /^\s*\/\/\s*covers:\s*B-([A-Za-z0-9]+)-(\d+)\s+(.*?)\s*$/.exec(text);
			if (!match) return;
			let fn, isTest = false;
			for (let next = index + 1; next < Math.min(lines.length, index + 12); next++) {
				const line = lines[next].trim();
				if (line.startsWith('#[') && /\btest\b/.test(line)) isTest = true;
				const name = /\bfn\s+([A-Za-z_]\w*)/.exec(line);
				if (name) { fn = name[1]; break; }
				if (!(line.startsWith('#[') || line.startsWith('//') || line === '' || line.startsWith(')]') || line.startsWith('flavor'))) break;
			}
			out.push({file, line: index + 1, id: `B-${match[1]}-${match[2]}`, unit: match[1], title: match[3], fn, isTest});
		});
	}
	return out;
}

async function checkTsTests(ctx) {
	const {unit, manifest} = ctx;
	const titles = [];
	const problems = [];
	for (const file of manifest.ts_tests) {
		if (!exists(file)) { problems.push(`ts_test ${file} does not exist`); continue; }
		for (const entry of listTestTitles(path.join(ROOT, file))) titles.push({file, ...entry});
	}
	const covers = listCovers(manifest);
	const ids = new Map();
	for (const cover of covers) {
		if (cover.unit !== unit) problems.push(`${cover.file}:${cover.line}: ${cover.id} names unit ${cover.unit}, expected B-${unit}-NN`);
		if (!cover.fn || !cover.isTest) problems.push(`${cover.file}:${cover.line}: ${cover.id} is not directly above a #[test] fn`);
		if (ids.has(cover.id)) problems.push(`${cover.file}:${cover.line}: duplicate id ${cover.id} (also ${ids.get(cover.id)})`);
		ids.set(cover.id, `${cover.file}:${cover.line}`);
		if (!titles.some(entry => entry.title === cover.title)) problems.push(`${cover.file}:${cover.line}: ${cover.id} title ${JSON.stringify(cover.title)} matches no test in ts_tests`);
	}
	const statuses = ctx.testStatuses;
	const passed = fn => {
		if (!statuses || !fn) return false;
		const matches = [...statuses].filter(([name]) => name === fn || name.endsWith(`::${fn}`));
		return matches.length > 0 && matches.every(([, entry]) => entry.passed);
	};
	const units = new Set(allManifests().map(entry => entry.unit));
	const deferred = new Map(manifest.deferred_tests.map(entry => [entry.title, entry]));
	for (const entry of manifest.deferred_tests) {
		if (!titles.some(title => title.title === entry.title)) problems.push(`deferred_tests: ${JSON.stringify(entry.title)} matches no test in ts_tests`);
		if (entry.to !== 'drop' && !units.has(entry.to)) problems.push(`deferred_tests: ${JSON.stringify(entry.title)}: to = ${JSON.stringify(entry.to)} is neither "drop" nor a unit`);
		if (!entry.reason?.trim()) problems.push(`deferred_tests: ${JSON.stringify(entry.title)} needs a reason`);
	}
	let covered = 0, deferredCount = 0;
	const missing = [], notPassed = [], dynamic = [];
	for (const entry of titles) {
		const coverers = covers.filter(cover => cover.title === entry.title && cover.unit === unit);
		if (coverers.some(cover => passed(cover.fn))) { covered++; continue; }
		if (deferred.has(entry.title)) { deferredCount++; continue; }
		if (coverers.length) notPassed.push({title: entry.title, file: entry.file, line: entry.line, rust: coverers.map(cover => `${cover.file}:${cover.line} fn ${cover.fn ?? '?'}`), problem: statuses ? 'the covering Rust test did not pass (see the tests check)' : 'the tests check did not run'});
		else missing.push({title: entry.title, file: entry.file, line: entry.line, ...(entry.dynamic ? {dynamic: true} : {})});
		if (entry.dynamic) dynamic.push(entry.title);
	}
	const next = covers.filter(cover => cover.unit === unit).reduce((max, cover) => Math.max(max, Number(cover.id.split('-').at(-1))), 0) + 1;
	const ok = !missing.length && !notPassed.length && !problems.length;
	return {ok, details: {
		titles: titles.length, covered, deferred: deferredCount, missing, not_passed: notPassed, problems,
		...(missing.length ? {fix: `add Rust tests in owned paths with "// covers: B-${unit}-NN <exact title>" directly above #[test] (next NN: ${String(next).padStart(2, '0')}), or deferred_tests entries`} : {}),
	}};
}

async function checkTsSuites(ctx) {
	const {manifest} = ctx;
	if (!manifest.ts_suites.length) return {ok: true, details: {note: 'no ts_suites in the manifest'}};
	const bin = path.join(ROOT, 'target', 'debug', 'deckhand');
	if (!fs.existsSync(bin)) return {ok: false, details: {error: 'target/debug/deckhand is missing (the build check builds it)'}};
	const missingFiles = manifest.ts_suites.filter(file => !exists(file));
	const runs = [];
	for (let attempt = 1; attempt <= 2; attempt++) {
		log(`ts_suites run ${attempt}/2`);
		const result = await run(process.execPath, ['--import', 'tsx', '--test', ...manifest.ts_suites.filter(exists)], {env: {DECKHAND_TEST_BIN: bin}, timeout: 30 * 60_000});
		const count = label => Number(new RegExp(`^ℹ ${label} (\\d+)`, 'm').exec(result.output)?.[1] ?? NaN);
		const failing = [...new Set([...result.output.matchAll(/^\s*✖ (.*?)(?: \([\d.]+m?s\))?$/gm)].map(match => match[1]).filter(title => !/^failing tests:?$/.test(title)))];
		runs.push({run: attempt, exit: result.code, pass: count('pass'), fail: count('fail'), failing: failing.slice(0, 50), ...(result.code ? {output_tail: tail(result.output, 30)} : {})});
	}
	return {ok: !missingFiles.length && runs.every(entry => entry.exit === 0), details: {suites: manifest.ts_suites, ...(missingFiles.length ? {missing_files: missingFiles} : {}), runs}};
}

// ---------------------------------------------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------------------------------------------

function parseArgs(argv) {
	const options = {base: 'rust-port', gateSuites: false, report: undefined, only: undefined, skip: []};
	const positional = [];
	for (let index = 0; index < argv.length; index++) {
		const arg = argv[index];
		const value = () => { const next = argv[++index]; if (next === undefined) throw new Error(`${arg} needs a value`); return next; };
		if (arg === '--help' || arg === '-h') options.help = true;
		else if (arg === '--base') options.base = value();
		else if (arg === '--gate-suites') options.gateSuites = true;
		else if (arg === '--report') options.report = value();
		else if (arg === '--only') options.only = value().split(',');
		else if (arg === '--skip') options.skip = value().split(',');
		else if (arg.startsWith('--')) throw new Error(`unknown option ${arg}`);
		else positional.push(arg);
	}
	for (const name of [...(options.only ?? []), ...options.skip]) if (!CHECKS.includes(name)) throw new Error(`unknown check ${name} (${CHECKS.join(', ')})`);
	if (positional.length !== 1 && !options.help) throw new Error('expected exactly one unit');
	options.unit = positional[0];
	return options;
}

export async function main(argv) {
	let options;
	try { options = parseArgs(argv); }
	catch (error) { console.error(`verify-unit: ${error.message}\n${USAGE}`); return 2; }
	if (options.help) { console.log(USAGE); return 0; }
	let deckhandHome;
	try { deckhandHome = await checkIsolatedHome(process.env.DECKHAND_HOME); }
	catch (error) { console.error(error.message); return 2; }
	await fsp.mkdir(deckhandHome, {recursive: true, mode: 0o700});

	const realHome = os.userInfo().homedir;
	const tempHome = await fsp.mkdtemp(path.join(os.tmpdir(), 'verify-unit-home-'));
	childEnv = {
		...process.env,
		DECKHAND_HOME: deckhandHome,
		HOME: tempHome,
		CARGO_HOME: process.env.CARGO_HOME || path.join(realHome, '.cargo'),
		RUSTUP_HOME: process.env.RUSTUP_HOME || path.join(realHome, '.rustup'),
		GIT_CONFIG_GLOBAL: '/dev/null',
		GIT_CONFIG_NOSYSTEM: '1',
		CARGO_TERM_COLOR: 'never',
		NO_COLOR: '1',
	};
	for (const key of ['DECKHAND_SESSION_ID', 'DECKHAND_LAUNCH_ID', 'DECKHAND_HOOK_TOKEN', 'DECKHAND_ALLOW_PROD_HOME', 'NODE_TEST_CONTEXT', 'PARITY_TAP', 'PARITY_TAP_OUT', 'PARITY_FIXTURES_OUT']) delete childEnv[key];

	const reportPath = path.resolve(options.report ?? path.join(ROOT, 'target', 'verify-unit', `${options.unit}.json`));
	const report = {unit: options.unit, ok: false, base: options.base, head: '', started: new Date().toISOString(), deckhand_home: deckhandHome, checks: []};
	try {
		let manifest;
		try { manifest = loadManifest(options.unit); }
		catch (error) { console.error(error.message); return 2; }
		report.head = (await git('rev-parse', 'HEAD')).out.trim();
		const ctx = {unit: options.unit, manifest, base: options.base, crate: crateName(manifest.crate), testStatuses: undefined};
		const implementations = {scope: checkScope, build: checkBuild, placeholders: checkPlaceholders, fixtures: checkFixtures, tests: checkTests, exports: checkExports, 'ts-tests': checkTsTests, 'ts-suites': checkTsSuites};
		// Cheap checks first; ts-tests needs the statuses from tests.
		const order = ['scope', 'placeholders', 'exports', 'build', 'fixtures', 'tests', 'ts-tests', 'ts-suites'];
		const results = new Map();
		for (const name of order) {
			const gating = name !== 'ts-suites' || options.gateSuites;
			if ((options.only && !options.only.includes(name)) || options.skip.includes(name)) { results.set(name, {name, ok: false, gating: false, skipped: true, details: {}}); continue; }
			log(`check ${name}`);
			let outcome;
			try { outcome = await implementations[name](ctx); }
			catch (error) { outcome = {ok: false, details: {error: error?.stack ?? String(error)}}; }
			results.set(name, {name, ok: outcome.ok, gating, details: outcome.details});
		}
		report.checks = CHECKS.map(name => results.get(name));
		report.skipped = report.checks.filter(check => check.skipped).map(check => check.name);
		report.ok = report.checks.every(check => check.skipped || !check.gating || check.ok) && report.skipped.length === 0;
	} finally {
		report.finished = new Date().toISOString();
		await fsp.mkdir(path.dirname(reportPath), {recursive: true});
		await fsp.writeFile(reportPath, JSON.stringify(report, null, 2) + '\n');
		await fsp.rm(tempHome, {recursive: true, force: true});
	}
	for (const check of report.checks) {
		const mark = check.skipped ? '-' : check.ok ? '✓' : check.gating ? '✗' : '!';
		console.log(`${mark} ${check.name}${check.skipped ? ' (skipped)' : check.gating ? '' : ' (informational)'}`);
	}
	console.log(`${report.ok ? 'OK' : 'FAILED'}: unit ${report.unit} — report ${reportPath}`);
	return report.ok ? 0 : 1;
}
