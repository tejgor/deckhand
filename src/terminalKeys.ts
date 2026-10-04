import type {Key} from 'ink';

// Ink labels ASCII DEL (the usual Backspace byte) and Kitty's Backspace
// codepoint 127 as `delete`. Forward Delete is a distinct CSI 3~ sequence.
export function normalizeTerminalKey<T extends Partial<Key>>(key: T, sequence: string): T & Partial<Key> {
	const backward = sequence === '\x7f' || sequence === '\b' || /^\x1b\[(?:8|127)(?:;\d+(?::[123])?(?:;[\d:]+)?)?u$/.test(sequence);
	return backward ? {...key, backspace: true, delete: false} : key;
}
