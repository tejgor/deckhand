import React, {useState} from 'react';
import type {Key} from 'ink';
import type {LiveClient} from './client.js';
import {ConfigEditorPane, ConfigTargetPane} from './configEditorPane.js';
import {DetailsPane} from './detailsPane.js';
import {editText, type EditorState} from './textEditor.js';
import {formatConfigJson} from './configDraft.js';
import type {ConfigTargetKind, ConfigTargets, ProjectConfigDocument} from './types.js';
import {errorMessage} from './ui.js';
import {isWorktreeSetupMode, useWorktreeSetupFlow, type WorktreeSetupMode} from './worktreeSetupFlow.js';

export type ConfigFlowMode = 'pick-config' | 'edit-project' | 'discard-project' | WorktreeSetupMode;

export function isConfigFlowMode(mode: string): mode is ConfigFlowMode {
	return mode === 'pick-config' || mode === 'edit-project' || mode === 'discard-project' || isWorktreeSetupMode(mode);
}
const TARGET_COUNT = 3;

interface ProjectConfigFlowOptions {
	client?: LiveClient;
	mode: string;
	setMode: (mode: ConfigFlowMode | 'browse') => void;
	setBusy: (busy: boolean) => void;
	setError: (error: string | undefined) => void;
	setStatusMessage: (message: string | undefined) => void;
}

export interface ProjectConfigFlow {
	// C: load the global/repository targets and show the picker. Nothing is written.
	open(cwd: string): void;
	// Input for pick-config/edit-project/discard-project. The caller busy-gates it.
	handleInput(input: string, key: Partial<Key>): void;
	render(width: number, height: number): React.ReactNode;
}

// The config editor flow: target picker (global defaults / repository / worktree setup) → editor (or the
// worktree setup screen) → discard confirmation.
// Opening/cancelling never creates a file; only Ctrl+S writes.
export function useProjectConfigFlow({client, mode, setMode, setBusy, setError, setStatusMessage}: ProjectConfigFlowOptions): ProjectConfigFlow {
	const [targets, setTargets] = useState<ConfigTargets>();
	const [cwd, setCwd] = useState('');
	const [targetIndex, setTargetIndex] = useState(0);
	const [document, setDocument] = useState<ProjectConfigDocument>();
	const [editor, setEditor] = useState<EditorState>({text: '', cursor: 0});
	const [editorError, setEditorError] = useState<string>();
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
				const next = targetIndex === 0 ? targets.global : targets.repository;
				if (next) editDocument(next);
			}
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
		if (mode === 'pick-config' && targets) return <ConfigTargetPane targets={targets} selected={targetIndex} width={width} height={height} />;
		if (mode === 'edit-project' && document) return <ConfigEditorPane document={document} state={editor} error={editorError} width={width} height={height} />;
		if (mode === 'discard-project') return <DetailsPane title="Discard unsaved configuration?" text={`${document?.path ?? ''}\n\nYour draft has unsaved changes. Discarding closes this draft without writing it; earlier saves are not undone.\n\nEnter discards the draft. Escape returns to editing.`} footer="enter discard · esc keep editing" width={width} height={height} />;
		return null;
	};

	return {open, handleInput, render};
}
