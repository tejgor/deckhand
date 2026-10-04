import type {ProjectInfo, SessionWorktreeRecord, WorkspaceSummary} from './types.js';

// Pure text builders for the scrollable details panes. Rendering (wrapping and
// visible escapes for invisible characters) happens in detailsPane.tsx.

export function trustReviewText(project: ProjectInfo): string {
	const status = project.needsReview ? 'Not trusted: until you trust it, only your global defaults apply.' : project.trusted ? 'Trusted: these exact bytes are in effect on top of your global defaults.' : 'Nothing to trust: no repository config or creation hook. Global defaults apply.';
	const config = project.path
		? project.exists ? `${project.path}\n\n${JSON.stringify(project.config, null, 2)}` : `${project.path}\n\n(no deckhand.json in the main checkout)`
		: 'Bare repository: no main checkout, so no repository deckhand.json.';
	const hook = project.creationHook
		? `\n\nCreation hook: ${project.creationHook.file}\nSHA-256: ${project.creationHook.fingerprint}\n\n${project.creationHook.content}`
		: project.disabledHook
			? `\n\nCreation hook ${project.disabledHook.file} is switched off by worktree.hook: false (${project.disabledHook.by === 'global' ? 'global defaults' : 'deckhand.json'}): it is not reviewed and never runs.`
			: '\n\nNo custom creation hook. Git fallback will be used.';
	return `${status}\n\nTrusting lets this repository's commands (setup, Dev, actions, creation hook) execute as you. Changed bytes need another review.\n\n${config}${hook}\n\nTrust covers these bytes, not everything commands may execute.`;
}

export function cleanupOverrideText(reasons: string[] | undefined): string {
	return `This can permanently erase local work and commits.\n\n${reasons?.length ? reasons.join('\n') : 'Safety could not be verified'}\n\nMain/current/shared worktree protections cannot be overridden.`;
}

export function projectActionNames(project: ProjectInfo | undefined): string[] {
	return Object.keys(project?.effective.actions ?? {});
}

export function actionPickerText(project: ProjectInfo | undefined, selectedIndex: number): string {
	const actions = project?.effective.actions ?? {};
	const names = projectActionNames(project);
	const list = names.map((action, index) => `${index === selectedIndex ? '›' : ' '} ${action}`).join('\n') || 'No actions configured (C edits global defaults or deckhand.json)';
	const selected = names[selectedIndex];
	return `${project?.needsReview ? 'Global defaults only: the repository config is not trusted' : 'Global defaults and repository actions'}\n\n${list}\n\nSelected command:\n${selected ? actions[selected] : ''}\n\nActions share the Dev pane; stop the previous command before running another.`;
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
