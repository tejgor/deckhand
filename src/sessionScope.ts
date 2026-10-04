import path from 'node:path';
import type {SessionRecord} from './types.js';

/** True when `child` is `parent` or lies beneath it (lexical; resolve/realpath both sides first as needed). */
export function isPathInside(parent: string, child: string): boolean {
	const relative = path.relative(path.resolve(parent), path.resolve(child));
	return relative === '' || (relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
}

/** Keep the repo overview, but also find sessions launched into this worktree elsewhere. */
export function sessionMatchesScope(session: SessionRecord, repoRoot: string): boolean {
	const root = path.resolve(repoRoot);
	return path.resolve(session.repoRoot) === root
		|| (session.worktree?.path !== undefined && path.resolve(session.worktree.path) === root)
		|| isPathInside(root, session.cwd);
}
