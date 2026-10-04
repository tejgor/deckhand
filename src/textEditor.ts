import type {Key} from 'ink';
import {MAX_CONFIG_BYTES, MAX_CONFIG_LABEL} from './configDraft.js';
export interface EditorState {text: string; cursor: number; selectAll?: boolean; message?: string}
function previous(text: string, cursor: number): number {
	return Math.max(0, cursor - (cursor > 1 && /[\uDC00-\uDFFF]/.test(text[cursor - 1]!) && /[\uD800-\uDBFF]/.test(text[cursor - 2]!) ? 2 : 1));
}
function next(text: string, cursor: number): number {
	return Math.min(text.length, cursor + (/[\uD800-\uDBFF]/.test(text[cursor] ?? '') && /[\uDC00-\uDFFF]/.test(text[cursor + 1] ?? '') ? 2 : 1));
}
function lineStart(text: string, cursor: number): number { return cursor === 0 ? 0 : text.lastIndexOf('\n', cursor - 1) + 1; }
function lineEnd(text: string, cursor: number): number { const end = text.indexOf('\n', cursor); return end < 0 ? text.length : end; }
function vertical(text: string, cursor: number, direction: -1 | 1): number {
	const start = lineStart(text, cursor), column = Array.from(text.slice(start, cursor)).length;
	const targetStart = direction < 0 ? start > 0 ? lineStart(text, start - 1) : start : lineEnd(text, cursor) < text.length ? lineEnd(text, cursor) + 1 : start;
	return targetStart + Array.from(text.slice(targetStart, lineEnd(text, targetStart))).slice(0, column).join('').length;
}
export function editText(state: EditorState, input: string, key: Partial<Key>): EditorState {
	const {text, cursor} = state;
	if (key.ctrl && input === 'a') return {text, cursor: text.length, selectAll: true};
	if (key.leftArrow) return {text, cursor: state.selectAll ? 0 : previous(text, cursor)};
	if (key.rightArrow) return {text, cursor: state.selectAll ? text.length : next(text, cursor)};
	if (key.home) return {text, cursor: key.ctrl ? 0 : lineStart(text, cursor)};
	if (key.end) return {text, cursor: key.ctrl ? text.length : lineEnd(text, cursor)};
	if (key.upArrow || key.downArrow) return {text, cursor: vertical(text, cursor, key.upArrow ? -1 : 1)};
	if (key.ctrl || key.meta || key.escape || key.pageDown || key.pageUp) return state;
	let start = state.selectAll ? 0 : cursor, end = state.selectAll ? text.length : cursor;
	let inserted = '';
	if (key.backspace) start = state.selectAll ? 0 : previous(text, cursor);
	else if (key.delete) end = state.selectAll ? text.length : next(text, cursor);
	else if (key.return) inserted = '\n';
	else if (key.tab) inserted = '  ';
	else inserted = input.replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, '').replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '').replace(/\r\n?/g, '\n').replace(/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f]/g, '');
	if (!inserted && !key.backspace && !key.delete) return state;
	const changed = text.slice(0, start) + inserted + text.slice(end);
	if (Buffer.byteLength(changed) > MAX_CONFIG_BYTES) return {...state, message: `Draft exceeds ${MAX_CONFIG_LABEL}; paste/edit was not applied.`};
	return {text: changed, cursor: start + inserted.length};
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
