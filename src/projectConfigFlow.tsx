import React, {useState} from 'react';
import type {Key} from 'ink';
import type {LiveClient} from './client.js';
import {ConfigEditorPane, ConfigTargetPane, EffectiveSettingsPane, settingLayer} from './configEditorPane.js';
import {DetailsPane} from './detailsPane.js';
import {editText, type EditorState} from './textEditor.js';
import {formatConfigJson} from './configDraft.js';
import type {ConfigTargetKind, ConfigTargets, EffectiveSettingsInfo, ProjectConfigDocument} from './types.js';
import {errorMessage} from './ui.js';
import {isWorktreeSetupMode, useWorktreeSetupFlow, type WorktreeSetupMode} from './worktreeSetupFlow.js';

export type ConfigFlowMode = 'pick-config' | 'edit-project' | 'discard-project' | 'effective-settings' | WorktreeSetupMode;

export function isConfigFlowMode(mode: string): mode is ConfigFlowMode {
	return mode === 'pick-config' || mode === 'edit-project' || mode === 'discard-project' || mode === 'effective-settings' || isWorktreeSetupMode(mode);
}
// Global defaults, Repository, Worktree setup, Effective settings.
const TARGET_COUNT = 4;

interface ProjectConfigFlowOptions {
	client?: LiveClient;
	mode: string;
	setMode: (mode: ConfigFlowMode | 'browse') => void;
	setBusy: (busy: boolean) => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
	/** Effective settings' T: the inline trust review for `cwd`; `back` reopens the view when it closes. */
	onReview: (cwd: string, back: () => void) => void;
}

export interface ProjectConfigFlow {
	// C: load the global/repository targets and show the picker. Nothing is written.
	open(cwd: string): void;
	// Input for pick-config/edit-project/discard-project. The caller busy-gates it.
	handleInput(input: string, key: Partial<Key>): void;
	render(width: number, height: number): React.ReactNode;
}

// The config editor flow: target picker (global defaults / repository / worktree setup / effective settings) →
// editor (or the worktree setup screen, or the read-only effective settings view) → discard confirmation.
// Opening/cancelling never creates a file; only Ctrl+S writes.
export function useProjectConfigFlow({client, mode, setMode, setBusy, setError, setStatusMessage, onReview}: ProjectConfigFlowOptions): ProjectConfigFlow {
	const [targets, setTargets] = useState<ConfigTargets>();
	const [cwd, setCwd] = useState('');
	const [targetIndex, setTargetIndex] = useState(0);
	const [document, setDocument] = useState<ProjectConfigDocument>();
	const [editor, setEditor] = useState<EditorState>({text: '', cursor: 0});
	const [editorError, setEditorError] = useState<string>();
	const [effective, setEffective] = useState<EffectiveSettingsInfo>();
	const [effectiveRow, setEffectiveRow] = useState(0);
	const editDocument = (next: ProjectConfigDocument) => { setDocument(next); setEditor({text: next.raw, cursor: 0}); setEditorError(undefined); setMode('edit-project'); };
	// Worktree setup's e: reload the targets so the editor opens the current bytes and revision.
	const editJson = (targetCwd: string, kind: ConfigTargetKind) => {
		if (!client) return;
		setBusy(true);
		void client.configTargets(targetCwd).then(nextTargets => {
			const next = nextTargets[kind];
			if (!next) throw new Error((kind === 'global' ? nextTargets.globalError : nextTargets.repositoryError) ?? 'Target unavailable');
			setTargets(nextTargets); setCwd(targetCwd); editDocument(next);
		}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
	};
	const worktreeSetup = useWorktreeSetupFlow({client, mode, setMode, setBusy, setError, setStatusMessage, onEditJson: editJson});
	// Effective settings: reloaded on every open (and after T), keeping the selected row where possible.
	const openEffective = (targetCwd: string, keepRow = false) => {
		if (!client) return;
		setBusy(true); setError(undefined);
		void client.effectiveSettings(targetCwd).then(next => {
			setEffective(next); setCwd(targetCwd); setEffectiveRow(row => keepRow ? Math.min(row, Math.max(0, next.rows.length - 1)) : 0); setMode('effective-settings');
		}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
	};

	const open = (cwd: string) => {
		if (!client) return;
		setBusy(true); setError(undefined); setStatusMessage(undefined);
		void client.configTargets(cwd).then(nextTargets => {
			setTargets(nextTargets); setCwd(cwd); setTargetIndex(0); setEditorError(undefined); setMode('pick-config');
		}).catch(error => setError(errorMessage(error))).finally(() => setBusy(false));
	};

	const handleInput = (input: string, key: Partial<Key>) => {
		if (isWorktreeSetupMode(mode)) { worktreeSetup.handleInput(input, key); return; }
		if (mode === 'pick-config') {
			if (key.escape) { setMode('browse'); return; }
			if (!targets) return;
			if (key.upArrow || input === 'k') setTargetIndex(index => Math.max(0, index - 1));
			if (key.downArrow || input === 'j') setTargetIndex(index => Math.min(TARGET_COUNT - 1, index + 1));
			if (key.return) {
				if (targetIndex === 2) { worktreeSetup.open(cwd); return; }
				if (targetIndex === 3) { openEffective(cwd); return; }
				const next = targetIndex === 0 ? targets.global : targets.repository;
				if (next) editDocument(next);
			}
			return;
		}
		if (mode === 'effective-settings') {
			if (key.escape) { setMode(targets ? 'pick-config' : 'browse'); return; }
			if (!effective) { setMode('browse'); return; }
			const last = Math.max(0, effective.rows.length - 1);
			if (key.upArrow || input === 'k') setEffectiveRow(row => Math.max(0, row - 1));
			else if (key.downArrow || input === 'j') setEffectiveRow(row => Math.min(last, row + 1));
			else if (key.pageUp || key.pageDown) setEffectiveRow(row => Math.max(0, Math.min(last, row + (key.pageUp ? -10 : 10))));
			else if (key.home || key.end) setEffectiveRow(key.home ? 0 : last);
			else if (input === 'T') { const viewCwd = cwd; onReview(viewCwd, () => openEffective(viewCwd, true)); }
			else if (key.return || input === 'e') { const row = effective.rows[effectiveRow]; editJson(cwd, row ? settingLayer(row) : 'global'); }
			return;
		}
		if (mode === 'edit-project') {
			if (!document) { setMode('browse'); return; }
			if (key.escape) { setMode(editor.text !== document.raw ? 'discard-project' : 'browse'); return; }
			if (key.ctrl && input === 's' && client) {
				setBusy(true); setEditorError(undefined);
				void client.saveConfig(document.kind, cwd, editor.text, document.revision).then(saved => {
					setDocument(saved); setMode('browse');
					setStatusMessage(saved.kind === 'global' ? 'Saved global defaults.' : 'Saved deckhand.json. Nothing ran; it applies once trusted (the next n/e/Dev asks you to review it).');
				}).catch(error => setEditorError(errorMessage(error))).finally(() => setBusy(false));
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
			if (key.return) { setMode('browse'); setEditorError(undefined); }
		}
	};

	const render = (width: number, height: number): React.ReactNode => {
		if (isWorktreeSetupMode(mode)) return worktreeSetup.render(width, height);
		if (mode === 'effective-settings' && effective) return <EffectiveSettingsPane info={effective} selected={effectiveRow} width={width} height={height} />;
		if (mode === 'pick-config' && targets) return <ConfigTargetPane targets={targets} selected={targetIndex} width={width} height={height} />;
		if (mode === 'edit-project' && document) return <ConfigEditorPane document={document} state={editor} error={editorError} width={width} height={height} />;
		if (mode === 'discard-project') return <DetailsPane title="Discard unsaved configuration?" text={`${document?.path ?? ''}\n\nYour draft has unsaved changes. Discarding closes this draft without writing it; earlier saves are not undone.\n\nEnter discards the draft. Escape returns to editing.`} footer="enter discard · esc keep editing" width={width} height={height} />;
		return null;
	};

	return {open, handleInput, render};
}
