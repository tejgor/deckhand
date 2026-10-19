/**
 * Tap loader: records every call the existing TS tests make to chosen src exports.
 *
 *   PARITY_TAP=textEditor:editText,textEditor:wordLeft,terminalKeys:* PARITY_TAP_OUT=<spool dir> \
 *     node --import tsx --import ./parity/tap.ts --test tests/textEditor.test.ts
 *
 * Each tapped module (src/<module>.ts|tsx) is replaced, for every importer, by a wrapper module that re-exports the
 * original and wraps the tapped functions (see tap-hooks.mjs). Calls are buffered per process and written to
 * `$PARITY_TAP_OUT/<pid>.tap` on exit; `runTap()` in lib.ts drives this and collects the spool. Only cross-module
 * calls are seen (a module calling its own function does not go through the wrapper).
 * PARITY_TAP_DETERMINISM=1 also installs the fixed clock (ticking 1 ms per read) and seeded randomness.
 */
import fs from 'node:fs';
import path from 'node:path';
import {register} from 'node:module';
import {pathToFileURL} from 'node:url';
import {REPO_ROOT, flushTap, installDeterminism} from './lib.ts';
import {listExports} from './ts-exports.mjs';

const spec = process.env.PARITY_TAP;
const spool = process.env.PARITY_TAP_OUT;
if (spec && spool) {
	const wanted = new Map<string, Set<string>>();
	for (const item of spec.split(',').map(part => part.trim()).filter(Boolean)) {
		const [module, fn] = item.split(':');
		if (!module || !fn) throw new Error(`PARITY_TAP: expected module:fn, got "${item}"`);
		let names = wanted.get(module);
		if (!names) wanted.set(module, names = new Set());
		names.add(fn);
	}
	const targets = [...wanted].map(([module, names]) => {
		const file = ['.ts', '.tsx'].map(extension => path.join(REPO_ROOT, 'src', module + extension)).find(candidate => fs.existsSync(candidate));
		if (!file) throw new Error(`PARITY_TAP: no src/${module}.ts(x)`);
		const exports = listExports(file).filter(entry => entry.kind !== 'type' && entry.kind !== 'star');
		const chosen = names.has('*') ? exports : [...names].map(name => {
			const found = exports.find(entry => entry.name === name);
			if (!found) throw new Error(`PARITY_TAP: src/${module} has no runtime export "${name}"`);
			return found;
		});
		return {module, url: pathToFileURL(file).href, exports: chosen};
	});
	register('./tap-hooks.mjs', import.meta.url, {data: {targets, lib: pathToFileURL(path.join(REPO_ROOT, 'parity', 'lib.ts')).href}});
	if (process.env.PARITY_TAP_DETERMINISM === '1') installDeterminism({tickMs: 1});
	process.on('exit', () => flushTap(spool));
}
