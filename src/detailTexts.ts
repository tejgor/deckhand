import type {ProjectInfo, SessionWorktreeRecord, WorkspaceSummary} from './types.js';

// Pure text builders for the scrollable details panes. Rendering (wrapping and
// visible escapes for invisible characters) happens in detailsPane.tsx.

/** The inline review. `purpose` says what is about to run (or be created) from the repository. */
export function trustReviewText(project: ProjectInfo, purpose?: string): string {
	const status = project.needsReview ? 'Not trusted: until you trust it, its commands, creation hook and worktree settings do not apply (your global defaults do; its agent/workspace suggestions only preselect the new-session picker).' : project.trusted ? 'Trusted: these exact bytes are in effect on top of your global defaults.' : 'Nothing to trust: no repository config or creation hook. Global defaults apply.';
	const config = project.path
		? project.exists ? `${project.path}\n\n${JSON.stringify(project.config, null, 2)}` : `${project.path}\n\n(no deckhand.json in the main checkout)`
		: 'Bare repository: no main checkout, so no repository deckhand.json.';
	const hook = project.creationHook
		? `\n\nCreation hook: ${project.creationHook.file}\nSHA-256: ${project.creationHook.fingerprint}\n\n${project.creationHook.content}`
		: project.disabledHook
			? `\n\nCreation hook ${project.disabledHook.file} is switched off by worktree.hook: false (${project.disabledHook.by === 'global' ? 'global defaults' : 'deckhand.json'}): it is not reviewed and never runs.`
			: '\n\nNo custom creation hook. Git fallback will be used.';
	return `${purpose ? `${purpose}\n\n` : ''}${status}\n\nTrusting lets this repository's commands (setup, Dev, actions, creation hook) execute as you. Changed bytes need another review, except edits you save in Deckhand while it is trusted.\n\n${config}${hook}\n\nTrust covers these bytes, not everything commands may execute.`;
}

export function cleanupOverrideText(reasons: string[] | undefined): string {
	return `This can permanently erase local work and commits.\n\n${reasons?.length ? reasons.join('\n') : 'Safety could not be verified'}\n\nMain/current/shared worktree protections cannot be overridden.`;
}

/** One entry of the e list. Untrusted repository actions are listed too, marked, and reviewed when chosen. */
export interface ProjectAction {name: string; command: string; needsTrust: boolean; /** The global command run instead when the review is skipped. */ fallback?: string}
export function projectActions(project: ProjectInfo | undefined): ProjectAction[] {
	if (!project) return [];
	const effective = project.effective.actions ?? {}, pending = project.needsReview ? project.config.actions ?? {} : {};
	return [...new Set([...Object.keys(effective), ...Object.keys(pending)])].map(name => Object.hasOwn(pending, name)
		? {name, command: pending[name]!, needsTrust: true, ...Object.hasOwn(effective, name) ? {fallback: effective[name]!} : {}}
		: {name, command: effective[name]!, needsTrust: false});
}

export function workspaceSummaryText(summary: WorkspaceSummary | undefined, prLoading = false, options: {creatingPr?: boolean; links?: SessionWorktreeRecord['links']} = {}): string {
	if (!summary) return 'Loading workspace information…';
	const pr = options.creatingPr
		? `Pushing ${summary.branch} to ${summary.pushRemote ?? 'its remote'} and opening GitHub…`
		: prLoading
		? 'Requesting PR/check status…'
		: summary.pr
			? `PR #${summary.pr.number}: ${summary.pr.state}, checks ${summary.pr.checks}\n${summary.pr.url}`
			: summary.prError ?? 'PR lookup is opt-in: press P';
	return [
		`Workspace: ${summary.cwd}`,
		`Branch: ${summary.branch}`,
		`HEAD: ${summary.head || '(no commits)'}`,
		`Base: ${summary.baseRef ?? 'unknown'}`,
		'',
		`Uncommitted: ${summary.changedFiles} files, +${summary.additions} -${summary.deletions}`,
		`Untracked: ${summary.untrackedFiles} files`,
		`Committed changes beyond base: ${summary.commitsAheadOfBase ?? 'unknown'}`,
		`Locally cached upstream: ahead ${summary.ahead ?? '?'}, behind ${summary.behind ?? '?'}`,
		...(options.links ? ['', `Worktree links: ${options.links.linked.length} created${options.links.notes.length ? `; ${options.links.notes.length} skipped:` : ''}`, ...options.links.notes.map(note => `  ${note}`)] : []),
		'',
		pr,
		'',
		'Changes belong to this workspace, not necessarily this agent. No automatic fetch; c pushes only after confirmation.',
	].join('\n');
}

export function createPrConfirmText(summary: WorkspaceSummary): string {
	const remote = summary.pushRemote ?? '(no remote: the push will fail)';
	return [
		`Push ${summary.branch} to ${remote} and open GitHub's new-PR form in your browser?`,
		'',
		`Runs: git push -u ${remote} ${summary.branch}  (never forced)`,
		'Then: gh pr create --web (or gh pr view --web when a PR is already open).',
		'',
		'Pushing publishes the branch\'s commits to the remote. Uncommitted changes are not pushed.',
		`Uncommitted: ${summary.changedFiles} files · untracked: ${summary.untrackedFiles} files`,
	].join('\n');
}
