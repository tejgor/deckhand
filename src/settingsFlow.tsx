import React, {useRef, useState} from 'react';
import type {Key} from 'ink';
import type {LiveClient} from './client.js';
import {ConfigEditorPane} from './configEditorPane.js';
import {DetailsPane} from './detailsPane.js';
import {formatConfigJson} from './configDraft.js';
import {editText, type EditorState} from './textEditor.js';
import type {ConfigTargetKind, ProjectConfigDocument, SavedConfigDocument, SettingsInfo} from './types.js';
import {updateAppConfig} from './storage.js';
import {ACTION_COMMAND_EXAMPLES, ACTION_COMMAND_HELP, ACTION_NAME_EXAMPLES, ACTION_NAME_RULES, APP_FLAG, SETTINGS, isAppFlag, type AppFlagId, actionNameCheck, actionNameProblem, actionSavedMessage, applyChange, columnProblem, commandProblem, choiceChange, choiceOptions, currentChoice, infoLayer, inheritedHint, initialColumn, initialLinks, initialText, layerActions, layerSets, linkSelection, otherTarget, ownValue, previewLocation, savedMessage, settingPath, targetName, textChange, trustNote, type SettingChange} from './settingsModel.js';
import {SettingsPane, type LinksState, type Notice, type SettingsEdit, type SettingsView} from './settingsPane.js';
import type {DetailLine} from './menu.js';
import {THEME, errorMessage} from './ui.js';
import {expandBranchName} from './worktreeLinks.js';

export type SettingsFlowMode = 'settings' | 'edit-project' | 'discard-project';
export function isSettingsFlowMode(mode: string): mode is SettingsFlowMode {
	return mode === 'settings' || mode === 'edit-project' || mode === 'discard-project';
}

interface SettingsFlowOptions {
	client?: LiveClient;
	mode: string;
	setMode: (mode: SettingsFlowMode | 'browse') => void;
	setBusy: (busy: boolean) => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
	/** T: the inline trust review for `cwd`; `back` reopens Settings when it closes. */
	onReview: (cwd: string, back: () => void) => void;
}
export interface SettingsFlow {
	/** C: load the settings of `cwd`'s repository and show them. Nothing is written. */
	open(cwd: string): void;
	/** Input while a SettingsFlowMode is active. The caller busy-gates it. */
	handleInput(input: string, key: Partial<Key>): void;
	/** Whether a first Ctrl+C should cancel (like Esc) instead of quitting: an open edit or a JSON draft. */
	cancelable(): boolean;
	render(width: number, height: number): React.ReactNode;
}

const SIZE_CHUNK = 8;
const MAX_TEXT: Partial<Record<string, number>> = {'worktree.branchName': 200, 'worktree.location': 4096};
const display = (value: unknown) => Array.isArray(value) ? value.join(', ') || 'none' : typeof value === 'boolean' ? (value ? 'on' : 'off') : String(value);
const capitalize = (text: string) => `${text[0]!.toUpperCase()}${text.slice(1)}`;

// C → Settings: a grid of every setting with one column per layer; the cursor is a cell (↑↓ setting, ←→ or Tab
// layer) and edits save immediately into that cell's layer through save-config's revision-checked writes. e opens the
// raw JSON of the selected column's file and returns here; T reviews trust and returns here. Saving never runs
// anything; a repository file stays trusted when the version it replaced was (see savedProjectTrust).
export function useSettingsFlow({client, mode, setMode, setBusy, setError, setStatusMessage, onReview}: SettingsFlowOptions): SettingsFlow {
	const [info, setInfo] = useState<SettingsInfo>();
	const [cwd, setCwd] = useState('');
	const [view, setView] = useState<SettingsView>('main');
	const [row, setRow] = useState(0);
	const [actionRow, setActionRow] = useState(0);
	// The selected column (layer); it stays put while the row changes (global-only rows select Global; see cellColumn).
	const [column, setColumn] = useState<ConfigTargetKind>('repository');
	const [edit, setEdit] = useState<SettingsEdit>();
	const [links, setLinks] = useState<LinksState>();
	const [sizes, setSizes] = useState<Record<string, number | null>>({});
	const [notice, setNotice] = useState<Notice>();
	const [document, setDocument] = useState<ProjectConfigDocument>();
	const [editor, setEditor] = useState<EditorState>({text: '', cursor: 0});
	const [editorError, setEditorError] = useState<string>();
	// Each link picker gets a token; size results for a closed or reopened picker are dropped and stop the loop.
	const token = useRef(0);

	const load = (targetCwd: string, after?: (next: SettingsInfo) => void, fresh = false) => {
		if (!client) return;
		setBusy(true);
		// Each result releases busy in the same render that shows it, so keys typed right after it are not dropped.
		void client.settingsInfo(targetCwd).then(next => {
			setBusy(false); token.current++;
			setInfo(next); setCwd(targetCwd); setEdit(undefined); setLinks(undefined);
			setView(current => current === 'links' ? 'main' : current);
			setColumn(current => fresh || columnProblem(next, current) ? initialColumn(next) : current);
			setActionRow(current => Math.min(current, layerActions(next, column).own.length));
			setMode('settings');
			after?.(next);
		}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
	};
	const open = (targetCwd: string) => {
		setError(undefined); setStatusMessage(undefined); setNotice(undefined); setView('main'); setRow(0); setActionRow(0);
		load(targetCwd, undefined, true);
	};
	const close = () => { token.current++; setEdit(undefined); setLinks(undefined); setMode('browse'); };

	const actions = info ? layerActions(info, column).own : [];
	const def = SETTINGS[Math.min(row, SETTINGS.length - 1)]!;
	// The cell the cursor is on: a global-only row has nothing to edit in This repo, so it always selects Global,
	// while `column` keeps the picked column for the rows around it.
	const cellColumn: ConfigTargetKind = def.globalOnly ? 'global' : column;
	const action = view === 'actions' ? actions[actionRow] : undefined;

	/** Writes `change` into `layer` and reloads; a concurrent change reloads too and says what was not saved. */
	const save = (layer: ConfigTargetKind, change: SettingChange, label: string, {cleared = false, extra = '', message, fail, after}: {cleared?: boolean; extra?: string; /** The status once saved, instead of savedMessage's. */ message?: (trust: SavedConfigDocument['trust']) => string; fail?: (message: string) => void; after?: (next: SettingsInfo) => void} = {}) => {
		if (!client || !info) return;
		const report = fail ?? ((text: string) => setNotice({text, error: true}));
		const document = info.targets[layer];
		if (!document) { report((layer === 'global' ? info.targets.globalError : info.targets.repositoryError) ?? `${capitalize(targetName(layer))} is unavailable`); return; }
		let raw: string;
		try { raw = applyChange(document, change); } catch (error) { report(`${label} was not saved: ${errorMessage(error)}`); return; }
		setBusy(true);
		void client.saveConfig(layer, cwd, raw, document.revision).then(saved => {
			setBusy(false); setNotice(undefined);
			// The status appears with the reloaded screen, once it accepts keys again.
			load(cwd, next => { setStatusMessage(`${message?.(saved.trust) ?? savedMessage(label, layer, {cleared, trust: saved.trust})}${extra}`); after?.(next); });
		}, error => {
			setBusy(false);
			const message = errorMessage(error);
			const attempt = cleared ? 'clearing it' : `wanted ${display(change.value)}`;
			const text = /changed on disk/.test(message) ? `${capitalize(targetName(layer))} changed on disk, so ${label} was not saved (${attempt}). Reloaded; try again.` : `${label} was not saved (${attempt}): ${message}`;
			load(cwd, () => setNotice({text, error: true}));
		});
	};
	/** A global-only setting is stored in config.json itself (like the attach scroll setting), not in a layer. */
	const saveFlag = (id: AppFlagId, label: string, value: boolean) => {
		setBusy(true);
		void updateAppConfig({[APP_FLAG[id]]: value}).then(() => {
			setBusy(false); setNotice(undefined);
			// The Codex hint (if any) shows under the row, which stays selected.
			load(cwd, () => setStatusMessage(`${label} ${value ? 'on' : 'off'} (global)${value ? ' · applies to new and restarted sessions' : ''}`));
		}, error => { setBusy(false); setNotice({text: `${label} was not saved: ${errorMessage(error)}`, error: true}); });
	};
	const editJson = (kind: ConfigTargetKind) => {
		if (!client) return;
		setBusy(true);
		// Reload so the editor opens the current bytes and revision.
		void client.settingsInfo(cwd).then(next => {
			setBusy(false); setInfo(next);
			const opened = next.targets[kind];
			if (!opened) { setNotice({text: (kind === 'global' ? next.targets.globalError : next.targets.repositoryError) ?? `${capitalize(targetName(kind))} is unavailable`, error: true}); return; }
			token.current++; setEdit(undefined); setLinks(undefined); setView(current => current === 'links' ? 'main' : current);
			setDocument(opened); setEditor({text: opened.raw, cursor: 0}); setEditorError(undefined); setMode('edit-project');
		}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
	};
	const loadSizes = async (live: LiveClient, data: LinksState['data'], current: number) => {
		const paths = data.candidates.filter(candidate => !candidate.missing && candidate.kind !== 'symlink').map(candidate => candidate.path);
		for (let index = 0; index < paths.length && token.current === current; index += SIZE_CHUNK) {
			const chunk = paths.slice(index, index + SIZE_CHUNK);
			const result = await live.worktreeCandidateSizes(cwd, chunk).catch(() => Object.fromEntries(chunk.map(file => [file, null])));
			if (token.current !== current) return;
			setSizes(previous => ({...previous, ...result}));
		}
	};
	const openLinks = (layer: ConfigTargetKind) => {
		if (!client) return;
		const current = ++token.current;
		setBusy(true);
		void client.worktreeCandidates(cwd).then(data => {
			setBusy(false);
			if (token.current !== current) return;
			const selected = initialLinks(data.candidates);
			setLinks({data, links: selected, initial: selected, target: layer, row: 0}); setSizes({}); setView('links'); setNotice(undefined);
			void loadSizes(client, data, current);
		}).catch(error => setNotice({text: errorMessage(error), error: true})).finally(() => setBusy(false));
	};

	const linksDirty = Boolean(links && JSON.stringify(links.links) !== JSON.stringify(links.initial));
	const cancelable = () => mode === 'edit-project' || (mode === 'settings' && (Boolean(edit) || (view === 'links' && linksDirty)));

	const handleEdit = (current: SettingsEdit, input: string, key: Partial<Key>) => {
		if (!info) return;
		if (current.kind === 'discard-links') {
			if (key.escape) setEdit(undefined);
			else if (key.return) { token.current++; setEdit(undefined); setLinks(undefined); setView('main'); }
			return;
		}
		if (key.escape) { setEdit(undefined); return; }
		if (current.kind === 'clear') {
			if (key.return) save(current.target, {path: settingPath(current.id, current.entry), value: undefined}, current.label, {cleared: true});
			return;
		}
		if (current.kind === 'choice') {
			const count = current.options.length;
			if (key.upArrow || input === 'k') setEdit({...current, index: (current.index - 1 + count) % count});
			else if (key.downArrow || input === 'j') setEdit({...current, index: (current.index + 1) % count});
			else if (key.return) {
				const option = current.options[current.index]!;
				if (option.custom) {
					const text = typeof option.value === 'string' ? option.value : '';
					setEdit({kind: 'text', id: current.id, label: current.label, target: current.target, state: {text, cursor: text.length}, placeholder: 'a template with {name}, e.g. ~/worktrees/{repo}/{name}'});
					return;
				}
				const change = choiceChange(info, current.id, current.target, option);
				if (!change) { setEdit(undefined); setNotice({text: `${current.label} unchanged: ${isAppFlag(current.id) ? 'it is already' : `${targetName(current.target)} already has it`}`}); return; }
				if (isAppFlag(current.id)) { saveFlag(current.id, current.label, option.value === true); return; }
				save(current.target, change, current.label, {cleared: change.value === undefined});
			}
			return;
		}
		if (key.return) {
			const text = current.state.text;
			if (current.step === 'name') {
				// An invalid name keeps the step open; the live check already shows why (an empty name says so now).
				const problem = actionNameCheck(info, current.target, text).error ?? actionNameProblem(text);
				if (problem) { if (!text) setEdit({...current, error: problem}); return; }
				const command = initialText(info, 'actions', current.target, text);
				const placeholder = inheritedHint(info, 'actions', current.target, text);
				setEdit({kind: 'text', id: 'actions', label: `Action ${text}`, target: current.target, entry: text, step: 'command', state: {text: command, cursor: command.length}, ...placeholder ? {placeholder} : {}});
				return;
			}
			const result = textChange(current.id, text, current.entry);
			if ('error' in result) { setEdit({...current, error: result.error}); return; }
			// Enter on the layer's own value unchanged saves nothing.
			if (ownValue(infoLayer(info, current.target), current.id, current.entry) === text) { setEdit(undefined); setNotice({text: `${current.label} unchanged`}); return; }
			const name = current.entry;
			save(current.target, result.change, current.label, {fail: message => setEdit({...current, error: message}), ...name !== undefined ? {message: trust => actionSavedMessage(name, current.target, trust), after: next => setActionRow(Math.max(0, layerActions(next, current.target).own.findIndex(entry => entry.name === name)))} : {}});
			return;
		}
		if (key.tab || key.upArrow || key.downArrow) return;
		// One-line fields: Ctrl+A / Ctrl+E are line start / end, as in notes and tasks (select-all is the JSON editor's).
		const next = editText(current.state, input, key, {selectAll: false});
		const limit = current.step === 'name' ? 48 : MAX_TEXT[current.id] ?? 8192;
		const stripped = next.text.replace(/\n/g, '');
		setEdit({...current, state: {text: stripped.slice(0, limit), cursor: Math.min(next.cursor, stripped.length, limit), ...next.selectAll ? {selectAll: true} : {}}, error: undefined});
	};

	const handleInput = (input: string, key: Partial<Key>) => {
		if (mode === 'edit-project') {
			if (!document) { load(cwd); return; }
			if (key.escape) { if (editor.text !== document.raw) setMode('discard-project'); else load(cwd); return; }
			if (key.ctrl && input === 's' && client) {
				setBusy(true); setEditorError(undefined);
				void client.saveConfig(document.kind, cwd, editor.text, document.revision).then(saved => {
					setDocument(saved); setBusy(false);
					load(cwd, () => setStatusMessage(saved.kind === 'global' ? 'Saved global defaults.' : `Saved deckhand.json · ${trustNote(saved.trust)}. Nothing ran.`));
				}, error => { setBusy(false); setEditorError(errorMessage(error)); });
				return;
			}
			if (key.ctrl && input === 'f') {
				try {
					const text = formatConfigJson(editor.text);
					setEditor(state => ({text, cursor: Math.min(state.cursor, text.length)})); setEditorError(undefined);
				} catch (error) { setEditorError(errorMessage(error)); }
				return;
			}
			setEditorError(undefined); setEditor(state => editText(state, input, key));
			return;
		}
		if (mode === 'discard-project') {
			if (key.escape) setMode('edit-project');
			if (key.return) { setEditorError(undefined); load(cwd); }
			return;
		}
		if (!info) { close(); return; }
		if (edit) { handleEdit(edit, input, key); return; }
		const move = (current: number, last: number): number | undefined => {
			if (key.upArrow || input === 'k') return Math.max(0, current - 1);
			if (key.downArrow || input === 'j') return Math.min(last, current + 1);
			if (key.pageUp || key.pageDown) return Math.max(0, Math.min(last, current + (key.pageUp ? -5 : 5)));
			if (key.home || key.end) return key.home ? 0 : last;
			return undefined;
		};
		// ←/→ (Tab toggles): the column; a layer that is unavailable here says why.
		const pickColumn = (kind: ConfigTargetKind) => {
			const problem = columnProblem(info, kind);
			if (problem) setNotice({text: problem, error: true});
			else { setColumn(kind); setNotice(undefined); }
		};

		if (view === 'links' && links) {
			const moved = move(links.row, Math.max(0, links.data.candidates.length - 1));
			if (moved !== undefined) { setLinks({...links, row: moved}); setNotice(undefined); return; }
			if (key.escape) { if (linksDirty) setEdit({kind: 'discard-links'}); else { token.current++; setLinks(undefined); setView('main'); } return; }
			const candidate = links.data.candidates[links.row];
			if (input === ' ' && candidate) {
				if (candidate.configured === 'files') setNotice({text: `${candidate.path} comes from worktree.files (→ ${candidate.source}); e edits it in the raw JSON.`});
				else { setLinks({...links, links: {...links.links, [candidate.path]: !links.links[candidate.path]}}); setNotice(undefined); }
				return;
			}
			if (input === 'e') {
				if (linksDirty) { setNotice({text: 'Unsaved link changes: Enter saves them (Esc discards) before e opens the raw JSON.', error: true}); return; }
				editJson(links.target); return;
			}
			if (key.return) {
				if (links.data.candidatesError) { setNotice({text: 'Nothing to save: the candidates could not be listed.', error: true}); return; }
				const other: ConfigTargetKind = links.target === 'global' ? 'repository' : 'global';
				const before = ownValue(infoLayer(info, links.target), 'worktree.links');
				const selection = linkSelection(infoLayer(info, links.target), infoLayer(info, other), links.data.candidates, links.links);
				if (JSON.stringify(selection.symlink) === JSON.stringify(Array.isArray(before) ? before : [])) {
					token.current++; setLinks(undefined); setView('main');
					setNotice({text: `Linked items unchanged${selection.stillLinked.length ? `; still linked by ${targetName(other)}: ${selection.stillLinked.join(', ')} (edit them in that column)`: ''}`, ...selection.stillLinked.length ? {error: true} : {}});
					return;
				}
				save(links.target, {path: settingPath('worktree.links'), value: selection.symlink.length ? selection.symlink : undefined}, 'Linked items', {extra: selection.stillLinked.length ? ` · still linked by ${targetName(other)}: ${selection.stillLinked.join(', ')}` : ''});
			}
			return;
		}

		if (view === 'actions') {
			const moved = move(actionRow, actions.length);
			if (moved !== undefined) { setActionRow(moved); setNotice(undefined); return; }
			if (key.escape) { setView('main'); setNotice(undefined); return; }
			if (input === 'e') { editJson(column); return; }
			if (input === 'T') { onReview(cwd, () => load(cwd)); return; }
			setNotice(undefined);
			if (input === 'a' || (key.return && !action)) { setEdit({kind: 'text', id: 'actions', label: 'New action', target: column, step: 'name', state: {text: '', cursor: 0}}); return; }
			if (key.return && action) {
				const text = initialText(info, 'actions', column, action.name);
				setEdit({kind: 'text', id: 'actions', label: `Action ${action.name}`, target: column, entry: action.name, state: {text, cursor: text.length}});
				return;
			}
			if (input === 'x' && action) setEdit({kind: 'clear', id: 'actions', label: `action ${action.name}`, target: column, entry: action.name});
			return;
		}

		const moved = move(row, SETTINGS.length - 1);
		if (moved !== undefined) { setRow(moved); setNotice(undefined); return; }
		if (key.escape) { close(); return; }
		if (key.leftArrow || key.rightArrow || key.tab) { pickColumn(key.tab ? otherTarget(column) : key.leftArrow ? 'global' : 'repository'); return; }
		if (input === 'e') { editJson(cellColumn); return; }
		if (input === 'T') { onReview(cwd, () => load(cwd)); return; }
		setNotice(undefined);
		if (input !== 'x' && !key.return) return;
		if (isAppFlag(def.id)) {
			if (input === 'x') setNotice({text: `${def.label}: Enter switches it on or off`});
			else { const options = choiceOptions(info, def.id, 'global'); setEdit({kind: 'choice', id: def.id, label: def.label, target: 'global', options, ...currentChoice(info, def.id, 'global', options)}); }
			return;
		}
		const problem = columnProblem(info, column);
		if (problem) { setNotice({text: problem, error: true}); return; }
		if (input === 'x') {
			const layer = infoLayer(info, column);
			if (def.id === 'worktree.links' ? ownValue(layer, 'worktree.links') === undefined : !layerSets(layer, def.id)) {
				setNotice({text: `${def.label} is not set in ${targetName(column)}; nothing to clear.${def.id === 'worktree.links' && layerSets(layer, def.id) ? ' worktree.files entries are edited in the raw JSON (e).' : ''}`, error: true});
				return;
			}
			setEdit({kind: 'clear', id: def.id, label: def.label, target: column});
			return;
		}
		if (def.control === 'choice') {
			const options = choiceOptions(info, def.id, column);
			setEdit({kind: 'choice', id: def.id, label: def.label, target: column, options, ...currentChoice(info, def.id, column, options)});
		} else if (def.control === 'text') {
			const text = initialText(info, def.id, column);
			const placeholder = inheritedHint(info, def.id, column);
			setEdit({kind: 'text', id: def.id, label: def.label, target: column, state: {text, cursor: text.length}, ...placeholder ? {placeholder} : {}});
		} else if (def.control === 'actions') { setView('actions'); setActionRow(0); }
		else if (!info.vars) setNotice({text: 'Linked items need a Git repository.', error: true});
		else openLinks(column);
	};

	// The edit control's help lines: a live check (red when invalid) and what the value accepts, with examples; a live
	// preview for templates.
	const editHelp = (): DetailLine[] | undefined => {
		if (!edit || edit.kind !== 'text' || !info) return undefined;
		const text = edit.state.text;
		const line = (value: string, extra: Partial<DetailLine> = {}): DetailLine => ({text: value, ...extra});
		if (edit.step === 'name') {
			const check = actionNameCheck(info, edit.target, text);
			const live = check.error && check.error !== edit.error ? [line(check.error, {color: THEME.error})] : check.note ? [line(check.note, {color: THEME.active})] : [];
			return [...live, line(ACTION_NAME_RULES), line(ACTION_NAME_EXAMPLES)];
		}
		if (edit.id === 'actions') {
			const problem = text.trim() ? commandProblem(text) : undefined;
			return [...problem ? [line(problem, {color: THEME.error})] : [], line(ACTION_COMMAND_HELP), line(ACTION_COMMAND_EXAMPLES)];
		}
		if (edit.id === 'worktree.branchName' && !text.trim()) return [line('A template with {name}; {user} is your user name')];
		if (edit.id === 'worktree.branchName') { try { return [line(`→ ${expandBranchName(text, {name: 'my-task', user: info.user})} for a session named my-task`, {nowrap: true})]; } catch (error) { return [line(errorMessage(error))]; } }
		if (edit.id === 'worktree.location') return [text.trim() ? line(`→ ${previewLocation(text, info.vars)}`, {nowrap: true}) : line('A template with {name}: {repo} {repoParent} {repoRoot} {home}, ~/')];
		if (edit.id === 'devCommand') return [line('Runs in the Dev pane (d), with your shell in the session\'s worktree')];
		return [line('Runs in a new worktree before the agent starts')];
	};

	const render = (width: number, height: number): React.ReactNode => {
		if (mode === 'edit-project' && document) return <ConfigEditorPane document={document} state={editor} error={editorError} width={width} height={height} />;
		if (mode === 'discard-project') return <DetailsPane title="Discard unsaved configuration?" text={`${document?.path ?? ''}\n\nYour draft has unsaved changes. Discarding closes this draft without writing it and returns to Settings; earlier saves are not undone.\n\nEnter discards the draft. Escape returns to editing.`} footer="enter discard · esc keep editing" width={width} height={height} />;
		if (mode !== 'settings' || !info) return null;
		return <SettingsPane info={info} view={view} row={row} actionRow={actionRow} column={cellColumn} edit={edit} editHelp={editHelp()} links={links} sizes={sizes} notice={notice} width={width} height={height} />;
	};

	return {open, handleInput, cancelable, render};
}
