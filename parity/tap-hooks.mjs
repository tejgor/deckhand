// Module customization hooks for parity/tap.ts (runs on the loader thread; plain JS on purpose).
// Loading a tapped src module yields a wrapper that re-exports `<url>?parity-orig` (the real module, loaded through
// the remaining hooks, i.e. tsx) and redefines the tapped exports as recording wrappers.
let targets = new Map();
let lib = '';

export async function initialize(data) {
	targets = new Map(data.targets.map(target => [target.url, target]));
	lib = data.lib;
}

function wrapperSource(target, query) {
	const original = JSON.stringify(`${target.url}?parity-orig${query ? `&${query}` : ''}`);
	const module = JSON.stringify(target.module);
	const lines = [
		`import * as __parityOriginal from ${original};`,
		`import {tapCall as __parityCall, tapWrap as __parityWrap} from ${JSON.stringify(lib)};`,
		`export * from ${original};`,
	];
	for (const {name, kind} of target.exports) {
		const key = JSON.stringify(name);
		// Function declarations stay hoisted (initialised before any import cycle runs); everything else is a const.
		if (kind === 'function') lines.push(`export function ${name}(...args) { return __parityCall(${module}, ${key}, __parityOriginal[${key}], this, args); }`);
		else if (kind === 'default' || kind === 'default-function') lines.push(`export default __parityWrap(${module}, "default", __parityOriginal.default);`);
		else if (kind === 'class' || kind === 'enum') continue; // re-exported unchanged by export *
		else lines.push(`export const ${name} = __parityWrap(${module}, ${key}, __parityOriginal[${key}]);`);
	}
	return lines.join('\n') + '\n';
}

export async function load(url, context, nextLoad) {
	const at = url.indexOf('?');
	const base = at < 0 ? url : url.slice(0, at), query = at < 0 ? '' : url.slice(at + 1);
	const target = targets.get(base);
	if (!target || query.split('&').includes('parity-orig')) return nextLoad(url, context);
	return {format: 'module', source: wrapperSource(target, query), shortCircuit: true};
}
