import React, {useRef, useState} from 'react';
import path from 'node:path';
import {Box, Text, type Key} from 'ink';
import type {LiveClient} from './client.js';
import type {ConfigTargetKind, WorktreeSetupInfo} from './types.js';
import {editText, type EditorState} from './textEditor.js';
import {DetailsPane} from './detailsPane.js';
import {BRANCH_FROM, type BranchFrom} from './worktreeLinks.js';
import {LOCATION_PRESETS, applyWorktreeSection, branchNameExample, initialSetupModel, previewLocation, setupModelDirty, worktreeSection, type LocationPreset, type WorktreeCandidate, type WorktreeSetupModel} from './worktreeSetup.js';
import {DISPLAY_CONTROL_PATTERN, THEME, compactPath, errorMessage, truncate} from './ui.js';

export type WorktreeSetupMode = 'worktree-setup' | 'discard-worktree-setup';
export function isWorktreeSetupMode(mode: string): mode is WorktreeSetupMode { return mode === 'worktree-setup' || mode === 'discard-worktree-setup'; }

interface WorktreeSetupFlowOptions {
	client?: LiveClient;
	mode: string;
	setMode: (mode: WorktreeSetupMode | 'browse') => void;
	setBusy: (busy: boolean) => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
	/** e: open the regular JSON editor for the same target. */
	onEditJson: (cwd: string, kind: ConfigTargetKind) => void;
}
export interface WorktreeSetupFlow {
	open(cwd: string): void;
	handleInput(input: string, key: Partial<Key>): void;
	render(width: number, height: number): React.ReactNode;
}
type Row = {kind: 'hook'} | {kind: 'location'} | {kind: 'branchFrom'} | {kind: 'branchName'} | {kind: 'candidate'; candidate: WorktreeCandidate};
const SIZE_CHUNK = 8;
const visible = (text: string) => text.replace(DISPLAY_CONTROL_PATTERN, '?').replace(/[\n\t]/g, ' ');
const locationOptions = (model: WorktreeSetupModel): LocationPreset[] => ['next', 'default', 'inside', ...model.customLocation ? ['custom' as const] : []];
const cycle = <T,>(options: readonly T[], current: T, delta: number): T => options[(options.indexOf(current) + delta + options.length) % options.length]!;
function rowsFor(info: WorktreeSetupInfo): Row[] {
	return [...info.hook ? [{kind: 'hook' as const}] : [], {kind: 'location'}, {kind: 'branchFrom'}, {kind: 'branchName'}, ...info.candidates.map(candidate => ({kind: 'candidate' as const, candidate}))];
}
// Home as ~, then the path's tail (the distinctive part) when it does not fit.
function tailPath(value: string, width: number): string {
	const home = process.env.HOME;
	const display = home && (value === home || value.startsWith(`${home}/`)) ? `~${value.slice(home.length)}` : value;
	return display.length <= width ? display : `…${display.slice(display.length - Math.max(1, width - 1))}`;
}
export function formatSize(kib: number | null | undefined, kind?: WorktreeCandidate['kind']): string {
	if (kind === 'symlink') return '';
	if (kib === undefined) return '…';
	if (kib === null) return '?';
	if (kib < 1024) return `${kib} KB`;
	const mib = kib / 1024;
	return mib < 1024 ? `${mib < 10 ? mib.toFixed(1) : Math.round(mib)} MB` : `${(mib / 1024).toFixed(1)} GB`;
}

// C → Worktree setup: edits the `worktree` section (location, branch base/name, hook, which untracked/ignored
// entries of the main checkout are linked) and saves it to deckhand.json or global defaults via save-config.
export function useWorktreeSetupFlow({client, mode, setMode, setBusy, setError, setStatusMessage, onEditJson}: WorktreeSetupFlowOptions): WorktreeSetupFlow {
	const [info, setInfo] = useState<WorktreeSetupInfo>();
	const [model, setModel] = useState<WorktreeSetupModel>();
	const [initial, setInitial] = useState<WorktreeSetupModel>();
	const [row, setRow] = useState(0);
	const [editing, setEditing] = useState<EditorState>();
	const [sizes, setSizes] = useState<Record<string, number | null>>({});
	const [notice, setNotice] = useState<{text: string; error?: boolean}>();
	// Each open gets a token; size results for a closed or reopened screen are dropped and stop the loop.
	const token = useRef(0);

	const close = () => { token.current++; setEditing(undefined); setMode('browse'); };
	const loadSizes = async (live: LiveClient, next: WorktreeSetupInfo, current: number) => {
		const paths = next.candidates.filter(candidate => !candidate.missing && candidate.kind !== 'symlink').map(candidate => candidate.path);
		for (let index = 0; index < paths.length && token.current === current; index += SIZE_CHUNK) {
			const chunk = paths.slice(index, index + SIZE_CHUNK);
			const result = await live.worktreeCandidateSizes(next.cwd, chunk).catch(() => Object.fromEntries(chunk.map(file => [file, null])));
			if (token.current !== current) return;
			setSizes(previous => ({...previous, ...result}));
		}
	};
	const open = (cwd: string) => {
		if (!client) return;
		const current = ++token.current;
		setBusy(true); setError(undefined); setStatusMessage(undefined);
		void client.worktreeSetupInfo(cwd).then(next => {
			if (token.current !== current) return;
			const start = initialSetupModel(next);
			setInfo(next); setModel(start); setInitial(start); setRow(0); setEditing(undefined); setSizes({}); setNotice(undefined); setMode('worktree-setup');
			void loadSizes(client, next, current);
		}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
	};

	const save = (current: WorktreeSetupInfo, draft: WorktreeSetupModel) => {
		if (!client) return;
		const document = current.targets[draft.target];
		if (!document) { setNotice({text: (draft.target === 'global' ? current.targets.globalError : current.targets.repositoryError) ?? 'Target unavailable', error: true}); return; }
		let raw: string;
		try { raw = applyWorktreeSection(document, worktreeSection(draft, current)); }
		catch (error) { setNotice({text: errorMessage(error), error: true}); return; }
		const other: ConfigTargetKind = draft.target === 'global' ? 'repository' : 'global';
		const stillLinked = current.candidates.filter(candidate => candidate.configured === 'symlink' && !draft.links[candidate.path] && candidate.layers?.includes(other)).map(candidate => candidate.path);
		setBusy(true);
		void client.saveConfig(draft.target, current.cwd, raw, document.revision).then(() => {
			close();
			setStatusMessage(`${draft.target === 'global' ? 'Saved worktree setup to global defaults. Nothing ran.' : 'Saved worktree setup to deckhand.json. Nothing ran or was trusted; the next n asks you to review it if needed.'}${stillLinked.length ? ` Still linked by ${other === 'global' ? 'global defaults' : 'deckhand.json'}: ${stillLinked.join(', ')}.` : ''}`);
		}).catch(error => setNotice({text: errorMessage(error), error: true})).finally(() => setBusy(false));
	};

	const handleInput = (input: string, key: Partial<Key>) => {
		if (mode === 'discard-worktree-setup') {
			if (key.escape) setMode('worktree-setup');
			if (key.return) close();
			return;
		}
		if (!info || !model || !initial) { close(); return; }
		const update = (patch: Partial<WorktreeSetupModel>) => { setModel({...model, ...patch}); setNotice(undefined); };
		if (editing) {
			if (key.escape) { setEditing(undefined); setNotice(undefined); return; }
			if (key.return) {
				const check = branchNameExample(editing.text, info.user);
				if (check.error) { setNotice({text: check.error, error: true}); return; }
				update({branchName: editing.text}); setEditing(undefined); return;
			}
			const next = editText(editing, input, key);
			setEditing({text: next.text.replace(/\n/g, '').slice(0, 200), cursor: Math.min(next.cursor, 200)});
			return;
		}
		const rows = rowsFor(info);
		const current = rows[Math.min(row, rows.length - 1)]!;
		const dirty = setupModelDirty(model, initial);
		const toggleHook = () => {
			if (!info.hook) { setNotice({text: 'No .claude/scripts/create-worktree.sh detected; Deckhand creates worktrees itself.'}); return; }
			update({hook: !model.hook});
		};
		if (key.escape) { if (dirty) setMode('discard-worktree-setup'); else close(); return; }
		if (key.ctrl && input === 's') { save(info, model); return; }
		if (key.upArrow || input === 'k') { setRow(Math.max(0, row - 1)); return; }
		if (key.downArrow || input === 'j') { setRow(Math.min(rows.length - 1, row + 1)); return; }
		if (key.pageUp || key.pageDown) { setRow(Math.max(0, Math.min(rows.length - 1, row + (key.pageUp ? -10 : 10)))); return; }
		if (key.home || key.end) { setRow(key.home ? 0 : rows.length - 1); return; }
		if (input === 't') {
			if (!info.targets.repository) { setNotice({text: `Only global defaults can be saved here: ${info.targets.repositoryError ?? 'no repository deckhand.json'}`, error: true}); return; }
			update({target: model.target === 'repository' ? 'global' : 'repository'}); return;
		}
		if (input === 'h') { toggleHook(); return; }
		if (input === 'e') {
			if (dirty) { setNotice({text: 'Unsaved changes: Ctrl+S saves them (Esc discards) before e opens the JSON.', error: true}); return; }
			close(); onEditJson(info.cwd, model.target); return;
		}
		if (!(key.leftArrow || key.rightArrow || key.return || input === ' ')) return;
		const delta = key.leftArrow ? -1 : 1;
		if (current.kind === 'hook') toggleHook();
		else if (current.kind === 'location') update({location: cycle(locationOptions(model), model.location, delta)});
		else if (current.kind === 'branchFrom') update({branchFrom: cycle<BranchFrom>(BRANCH_FROM, model.branchFrom, delta)});
		else if (current.kind === 'branchName') { if (key.return || input === ' ') { setEditing({text: model.branchName, cursor: model.branchName.length}); setNotice(undefined); } }
		else if (current.kind === 'candidate') {
			const {candidate} = current;
			if (candidate.configured === 'files') setNotice({text: `${candidate.path} comes from worktree.files (→ ${candidate.source}); press e to edit it in JSON.`});
			else update({links: {...model.links, [candidate.path]: !model.links[candidate.path]}});
		}
	};

	const render = (width: number, height: number): React.ReactNode => {
		if (mode === 'discard-worktree-setup') return <DetailsPane title="Discard unsaved worktree setup?" text={'Your worktree setup has unsaved changes. Discarding closes the screen without writing anything; earlier saves are not undone.\n\nEnter discards. Escape returns to the setup screen.'} footer="enter discard · esc keep editing" width={width} height={height} />;
		if (mode !== 'worktree-setup' || !info || !model || !initial) return null;
		return <WorktreeSetupPane info={info} model={model} dirty={setupModelDirty(model, initial)} row={row} editing={editing} sizes={sizes} notice={notice} width={width} height={height} />;
	};

	return {open, handleInput, render};
}

interface Line {text: string; row?: number; color?: string; bold?: boolean; dim?: boolean}
function setupLines(info: WorktreeSetupInfo, model: WorktreeSetupModel, row: number, editing: EditorState | undefined, sizes: Record<string, number | null>, inner: number): Line[] {
	const lines: Line[] = [];
	const mark = (index: number) => index === row ? '›' : ' ';
	const radio = (on: boolean) => on ? '◉' : '○';
	const hookActive = Boolean(info.hook && model.hook);
	const ignored = hookActive ? '  (ignored: hook decides)' : '';
	let index = 0;
	if (info.hook) {
		const file = path.relative(info.checkout, info.hook.file) || info.hook.file;
		const state = model.hook
			? `Creates worktrees itself: Location/Branch ignored, links still apply.${info.hook.trusted ? '' : ' Runs once trusted.'}`
			: `Off: Deckhand creates worktrees with the settings below.${info.hook.by === 'repository' && model.target === 'global' ? ' (deckhand.json switches it off.)' : ''}`;
		lines.push({text: `${mark(index)} Hook: ${file} detected · ${model.hook ? 'on' : 'off'}   (h: use hook on/off)`, row: index, color: model.hook ? THEME.warn : undefined});
		lines.push({text: `         ${state}`, row: index, color: THEME.muted});
		index++;
	}
	const global = info.layers.global?.location;
	// [label, preview path, suffix]: the path is shortened from the left so the suffix stays visible.
	const labels: Record<LocationPreset, [string, string, string]> = {
		next: ['next to repo', previewLocation(LOCATION_PRESETS.next, info.vars), ''],
		default: model.target === 'repository' && global ? ['global default', previewLocation(global, info.vars), ''] : ['Deckhand default', info.defaultLocation, ''],
		inside: ['inside repo', previewLocation(LOCATION_PRESETS.inside, info.vars), info.insideIgnored ? '' : '   ⚠ .worktrees/ is not gitignored'],
		custom: ['custom (from config)', model.customLocation ? previewLocation(model.customLocation, info.vars) : '', `   (${model.customLocation ?? ''})`],
	};
	locationOptions(model).forEach((option, position) => {
		const [label, preview, suffix] = labels[option];
		const tail = `${suffix}${position === 0 ? ignored : ''}`;
		lines.push({text: `${position === 0 ? `${mark(index)} Location:    ` : '               '}${radio(model.location === option)} ${label.padEnd(18)} ${tailPath(preview, Math.max(10, inner - 36 - tail.length))}${tail}`, row: index, dim: hookActive});
	});
	index++;
	const fromLabels: Record<BranchFrom, string> = {
		current: 'current checkout',
		default: `default branch (${info.defaultBranch ?? 'none found'})`,
		origin: info.originBranch ? `fetch origin/${info.originBranch} first` : 'fetch origin (no remote)',
	};
	lines.push({text: `${mark(index)} Branch from: ${BRANCH_FROM.map(option => `${radio(model.branchFrom === option)} ${fromLabels[option]}`).join('   ')}${ignored}`, row: index, dim: hookActive});
	index++;
	const template = editing?.text ?? model.branchName;
	const check = branchNameExample(template, info.user);
	const shown = editing ? `${template.slice(0, editing.cursor)}▏${template.slice(editing.cursor)}` : template;
	lines.push({text: `${mark(index)} Branch name: ${shown}   ${check.error ? `⚠ ${check.error}` : `example: ${check.example}`}${editing ? '' : '   (enter edits)'}${ignored}`, row: index, dim: hookActive && !editing, color: check.error ? THEME.error : editing ? THEME.accent : undefined});
	index++;
	lines.push({text: ' '});
	const pathWidth = Math.max(16, Math.min(60, inner - 50));
	lines.push({text: `  ${'Missing from new worktrees'.padEnd(pathWidth + 7)} ${'size'.padStart(8)}   action`, bold: true});
	if (info.candidatesError) lines.push({text: `  Could not list untracked/ignored files: ${info.candidatesError}`, color: THEME.error});
	else if (!info.candidates.length) lines.push({text: '  No untracked or ignored entries in the main checkout.', color: THEME.muted});
	for (const candidate of info.candidates) {
		const linked = candidate.configured === 'files' || model.links[candidate.path];
		const name = `${candidate.path}${candidate.kind === 'dir' ? '/' : ''}${candidate.target ? ` → ${tailPath(candidate.target, 40)}` : candidate.kind === 'symlink' ? ' → (broken link)' : ''}`;
		const action = candidate.missing ? 'missing in checkout'
			: candidate.configured === 'files' ? `files → ${candidate.source}`
			: candidate.configured ? `configured${candidate.layers?.length ? ` (${candidate.layers.map(layer => layer === 'global' ? 'global' : 'repo').join(', ')})` : ''}`
			: candidate.reason ? `${candidate.suggestion === 'link' ? 'suggested: ' : ''}${candidate.reason}` : '';
		lines.push({text: `${mark(index)} [${linked ? 'link' : 'skip'}] ${truncate(name, pathWidth).padEnd(pathWidth)} ${formatSize(sizes[candidate.path] ?? (candidate.missing ? null : undefined), candidate.kind).padStart(8)}   ${action}`, row: index, color: linked ? THEME.success : THEME.muted});
		index++;
	}
	if (info.moreCandidates) lines.push({text: `  +${info.moreCandidates} more (edit JSON)`, color: THEME.muted});
	return lines;
}

function WorktreeSetupPane({info, model, dirty, row, editing, sizes, notice, width, height}: {info: WorktreeSetupInfo; model: WorktreeSetupModel; dirty: boolean; row: number; editing?: EditorState; sizes: Record<string, number | null>; notice?: {text: string; error?: boolean}; width: number; height: number}) {
	const inner = Math.max(1, width - 4), bodyRows = Math.max(1, height - 6);
	const lines = setupLines(info, model, row, editing, sizes, inner);
	const scroll = useRef(0);
	const first = lines.findIndex(line => line.row === row), last = first < 0 ? -1 : first + lines.slice(first).filter(line => line.row === row).length - 1;
	if (first >= 0 && first < scroll.current) scroll.current = first;
	else if (last >= scroll.current + bodyRows) scroll.current = last - bodyRows + 1;
	scroll.current = Math.max(0, Math.min(scroll.current, Math.max(0, lines.length - bodyRows)));
	const document = info.targets[model.target];
	const target = model.target === 'repository' ? '[this repo] / global' : 'this repo / [global]';
	const trustNote = model.target === 'repository' && info.repositoryNeedsReview ? ' · not trusted yet: applies once trusted' : '';
	const hint = editing ? 'type the template ({name} required, {user}) · enter keep · esc revert' : '↑↓ move · space link/skip · ←→ change option · t save target · h hook · ctrl+s save · e edit JSON · esc cancel';
	return <Box flexDirection="column" width={width} height={height} borderStyle="round" borderColor={THEME.borderActive} paddingX={1}>
		<Text color={THEME.accent} bold>{truncate(visible(`Worktree setup · ${info.repo}${dirty ? ' · unsaved' : ''}    save to: ${target}   (t toggles)`), inner)}</Text>
		<Text color={THEME.muted}>{truncate(visible(`${document ? `${compactPath(document.path, Math.max(10, inner - 50))}${model.target === 'global' ? ' ("defaults")' : ''}${document.exists ? '' : ' · new'}` : 'unavailable'}${trustNote}`), inner)}</Text>
		{Array.from({length: bodyRows}, (_, index) => {
			const line = lines[scroll.current + index];
			return <Text key={index} color={line?.row === row ? THEME.accent : line?.color} bold={line?.bold || line?.row === row} dimColor={line?.dim && line.row !== row}>{line ? truncate(visible(line.text), inner) : ' '}</Text>;
		})}
		<Text color={notice?.error ? THEME.error : THEME.muted}>{truncate(visible(notice?.text ?? 'Saving writes only the worktree section; it never runs, trusts or commits anything.'), inner)}</Text>
		<Text color={THEME.muted}>{truncate(hint, inner)}</Text>
	</Box>;
}
