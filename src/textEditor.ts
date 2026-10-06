import type {Key} from 'ink';
import {MAX_CONFIG_BYTES, MAX_CONFIG_LABEL} from './configDraft.js';
export interface EditorState {text: string; cursor: number; selectAll?: boolean; message?: string}
function previous(text: string, cursor: number): number {
	return Math.max(0, cursor - (cursor > 1 && /[\uDC00-\uDFFF]/.test(text[cursor - 1]!) && /[\uD800-\uDBFF]/.test(text[cursor - 2]!) ? 2 : 1));
}
function next(text: string, cursor: number): number {
	return Math.min(text.length, cursor + (/[\uD800-\uDBFF]/.test(text[cursor] ?? '') && /[\uDC00-\uDFFF]/.test(text[cursor + 1] ?? '') ? 2 : 1));
}
export function lineStart(text: string, cursor: number): number { return cursor === 0 ? 0 : text.lastIndexOf('\n', cursor - 1) + 1; }
export function lineEnd(text: string, cursor: number): number { const end = text.indexOf('\n', cursor); return end < 0 ? text.length : end; }
function vertical(text: string, cursor: number, direction: -1 | 1): number {
	const start = lineStart(text, cursor), column = Array.from(text.slice(start, cursor)).length;
	const targetStart = direction < 0 ? start > 0 ? lineStart(text, start - 1) : start : lineEnd(text, cursor) < text.length ? lineEnd(text, cursor) + 1 : start;
	return targetStart + Array.from(text.slice(targetStart, lineEnd(text, targetStart))).slice(0, column).join('').length;
}
const isWordCharacter = (character: string) => /[\p{L}\p{N}_]/u.test(character);
/** The start of the word before the cursor (Alt/Ctrl+←): skips non-word characters (a newline included), then the word. */
export function wordLeft(text: string, cursor: number): number {
	let position = cursor;
	while (position > 0 && !isWordCharacter(text.slice(previous(text, position), position))) position = previous(text, position);
	while (position > 0 && isWordCharacter(text.slice(previous(text, position), position))) position = previous(text, position);
	return position;
}
/** The end of the word after the cursor (Alt/Ctrl+→). */
export function wordRight(text: string, cursor: number): number {
	let position = cursor;
	while (position < text.length && !isWordCharacter(text.slice(position, next(text, position)))) position = next(text, position);
	while (position < text.length && isWordCharacter(text.slice(position, next(text, position)))) position = next(text, position);
	return position;
}

/** How editText behaves; the defaults are the JSON config editor's. */
export interface EditOptions {
	/** Ctrl+A selects everything (the config editor); otherwise Ctrl+A/Ctrl+E go to the line's start/end (emacs, macOS). */
	selectAll?: boolean;
	/** Text limit in characters (UTF-16 units); the default is the config editor's byte limit. */
	maxChars?: number;
	limitMessage?: string;
	/** Tab inserts two spaces (the default), or nothing (the caller uses Tab itself). */
	tab?: boolean;
	/** Width for Up/Down by visual (soft-wrapped) rows, and PageUp/PageDown by `pageRows` of them; without it, by line. */
	wrapWidth?: number;
	pageRows?: number;
}

// Pasted/typed text: terminal sequences (bracketed-paste markers too) and controls other than newline and tab dropped.
export function cleanInsertedText(input: string): string {
	return input.replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, '').replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '').replace(/\r\n?/g, '\n').replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/g, '');
}

/** Replaces `start..end` with `inserted` within the limit, or reports why it was not applied. */
export function replaceRange(state: EditorState, start: number, end: number, inserted: string, options: EditOptions = {}): EditorState {
	const changed = state.text.slice(0, start) + inserted + state.text.slice(end);
	if (options.maxChars !== undefined ? changed.length > options.maxChars && changed.length > state.text.length : Buffer.byteLength(changed) > MAX_CONFIG_BYTES) {
		return {...state, message: options.limitMessage ?? `Draft exceeds ${MAX_CONFIG_LABEL}; paste/edit was not applied.`};
	}
	return {text: changed, cursor: start + inserted.length};
}

export function editText(state: EditorState, input: string, key: Partial<Key>, options: EditOptions = {}): EditorState {
	const {text, cursor} = state;
	const selectAllKey = options.selectAll ?? true;
	const word = Boolean(key.meta || key.ctrl);
	if (selectAllKey && key.ctrl && input === 'a') return {text, cursor: text.length, selectAll: true};
	if (!selectAllKey && key.ctrl && (input === 'a' || input === 'e')) return {text, cursor: input === 'a' ? lineStart(text, cursor) : lineEnd(text, cursor)};
	if (key.leftArrow) return {text, cursor: state.selectAll ? 0 : word ? wordLeft(text, cursor) : previous(text, cursor)};
	if (key.rightArrow) return {text, cursor: state.selectAll ? text.length : word ? wordRight(text, cursor) : next(text, cursor)};
	// Alt+B / Alt+F: what macOS terminals send for Option+←/→ (Esc+ mode).
	if (key.meta && !key.ctrl && (input === 'b' || input === 'f')) return {text, cursor: input === 'b' ? wordLeft(text, cursor) : wordRight(text, cursor)};
	if (key.home) return {text, cursor: key.ctrl ? 0 : lineStart(text, cursor)};
	if (key.end) return {text, cursor: key.ctrl ? text.length : lineEnd(text, cursor)};
	if (key.upArrow || key.downArrow) {
		const direction = key.upArrow ? -1 : 1;
		return {text, cursor: options.wrapWidth ? moveVisual(text, cursor, options.wrapWidth, direction) : vertical(text, cursor, direction)};
	}
	if ((key.pageUp || key.pageDown) && options.wrapWidth) return {text, cursor: moveVisual(text, cursor, options.wrapWidth, key.pageUp ? -1 : 1, Math.max(1, options.pageRows ?? 1))};
	// Word deletion: Alt+Backspace, Ctrl+Backspace (where terminals send it), Ctrl+W.
	if ((key.backspace && word) || (key.ctrl && input === 'w')) {
		if (state.selectAll) return replaceRange(state, 0, text.length, '', options);
		return replaceRange(state, wordLeft(text, cursor), cursor, '', options);
	}
	if (key.delete && key.meta) return replaceRange(state, cursor, wordRight(text, cursor), '', options);
	if (key.ctrl || key.meta || key.escape || key.pageDown || key.pageUp) return state;
	let start = state.selectAll ? 0 : cursor, end = state.selectAll ? text.length : cursor;
	let inserted = '';
	if (key.backspace) start = state.selectAll ? 0 : previous(text, cursor);
	else if (key.delete) end = state.selectAll ? text.length : next(text, cursor);
	else if (key.return) inserted = '\n';
	else if (key.tab) { if (options.tab === false) return state; inserted = '  '; }
	else inserted = cleanInsertedText(input);
	if (!inserted && !key.backspace && !key.delete) return state;
	return replaceRange(state, start, end, inserted, options);
}
export interface EditorLine {before: string; cursor?: string; after: string}
export function editorLines(state: EditorState, width: number): {lines: EditorLine[]; cursorRow: number} {
	const lines: EditorLine[] = [];
	let position = 0, cursorRow = 0;
	for (const line of state.text.split('\n')) {
		const characters = Array.from(line);
		// A trailing space gives the cursor somewhere to sit at an exact wrap boundary.
		const segments = Math.max(1, Math.ceil((characters.length + 1) / Math.max(1, width)));
		let consumed = 0;
		for (let segment = 0; segment < segments; segment++) {
			const content = characters.slice(segment * Math.max(1, width), (segment + 1) * Math.max(1, width)).join('');
			const offset = state.cursor - position - consumed;
			const hasCursor = offset >= 0 && offset <= content.length && (offset < content.length || segment === segments - 1);
			if (hasCursor) {
				cursorRow = lines.length;
				const character = content.slice(offset, next(content, offset));
				lines.push({before: content.slice(0, offset), cursor: character || ' ', after: content.slice(offset + character.length)});
			} else lines.push({before: content, after: ''});
			consumed += content.length;
		}
		position += line.length + 1;
	}
	return {lines, cursorRow};
}

/**
 * One soft-wrapped row: `text.slice(start, end)` (no newline). `last` marks the last row of its line, where the cursor
 * may sit at `end`; elsewhere `end` is the next row's start, so the cursor there shows on the next row.
 */
export interface WrapRow {start: number; end: number; last: boolean}

/**
 * Word-wrapped rows of `text` at `width` columns (one column per code point): a line breaks after its last space that
 * fits, else mid-word. With `cursorRoom`, a line that exactly fills its last row gets one more empty row, so the cursor
 * after it stays on screen (the editor's view).
 */
export function wrapRows(text: string, width: number, cursorRoom = false): WrapRow[] {
	const columns = Math.max(1, Math.floor(width));
	const rows: WrapRow[] = [];
	let lineOffset = 0;
	for (const line of text.split('\n')) {
		const characters = Array.from(line);
		// Code-unit offset of each code point (and of the end).
		const offsets: number[] = [0];
		for (const character of characters) offsets.push(offsets.at(-1)! + character.length);
		let from = 0;
		while (characters.length - from > columns) {
			let cut = from + columns;
			for (let index = from + columns - 1; index > from; index--) if (characters[index] === ' ') { cut = index + 1; break; }
			rows.push({start: lineOffset + offsets[from]!, end: lineOffset + offsets[cut]!, last: false});
			from = cut;
		}
		const end = lineOffset + offsets[characters.length]!;
		if (cursorRoom && characters.length - from === columns && characters.length > 0) {
			rows.push({start: lineOffset + offsets[from]!, end, last: false});
			rows.push({start: end, end, last: true});
		} else rows.push({start: lineOffset + offsets[from]!, end, last: true});
		lineOffset += line.length + 1;
	}
	return rows;
}

/** The row showing `cursor`. */
export function cursorRowIndex(rows: WrapRow[], cursor: number): number {
	for (let index = 0; index < rows.length; index++) {
		const row = rows[index]!;
		if (cursor >= row.start && (cursor < row.end || (cursor === row.end && row.last))) return index;
	}
	return Math.max(0, rows.length - 1);
}

/** Up/Down (`count` rows) by visual rows, keeping the column where the target row has it. */
export function moveVisual(text: string, cursor: number, width: number, direction: -1 | 1, count = 1): number {
	const rows = wrapRows(text, width, true);
	const from = cursorRowIndex(rows, cursor);
	const target = Math.max(0, Math.min(rows.length - 1, from + direction * count));
	if (target === from) return direction < 0 ? 0 : text.length;
	const column = Array.from(text.slice(rows[from]!.start, cursor)).length;
	const row = rows[target]!;
	const characters = Array.from(text.slice(row.start, row.end));
	// A row's end is the next row's start unless it ends its line.
	const room = row.last ? characters.length : Math.max(0, characters.length - 1);
	return row.start + characters.slice(0, Math.min(column, room)).join('').length;
}

/** The soft-wrapped editor view: every row with the cursor's row split around it. */
export function wrappedEditorLines(state: EditorState, width: number): {lines: EditorLine[]; cursorRow: number} {
	const rows = wrapRows(state.text, width, true);
	const cursorRow = cursorRowIndex(rows, state.cursor);
	const lines = rows.map((row, index): EditorLine => {
		const content = state.text.slice(row.start, row.end);
		if (index !== cursorRow) return {before: content, after: ''};
		const offset = state.cursor - row.start;
		const character = content.slice(offset, next(content, offset));
		return {before: content.slice(0, offset), cursor: character || ' ', after: content.slice(offset + character.length)};
	});
	return {lines, cursorRow};
}

/** The first visible row: moved only as far as needed to keep the cursor's row within `rows`. */
export function scrollTopFor(previousTop: number, cursorRow: number, rows: number, lineCount: number): number {
	let top = previousTop;
	if (cursorRow < top) top = cursorRow;
	else if (cursorRow >= top + rows) top = cursorRow - rows + 1;
	return Math.max(0, Math.min(top, Math.max(0, lineCount - rows)));
}
