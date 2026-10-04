import path from 'node:path';
import type {SessionRecord} from './types.js';

/** Keep the repo overview, but also find sessions launched into this worktree elsewhere. */
export function sessionMatchesScope(session: SessionRecord, repoRoot: string): boolean {
	const root = path.resolve(repoRoot);
	const cwd = path.resolve(session.cwd);
	return path.resolve(session.repoRoot) === root
		|| (session.worktree?.path !== undefined && path.resolve(session.worktree.path) === root)
		|| cwd === root
		|| cwd.startsWith(`${root}${path.sep}`);
}
