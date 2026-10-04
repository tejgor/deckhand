import {useEffect, useRef} from 'react';
import {useInput, useStdin} from 'ink';
import {normalizeTerminalKey} from './terminalKeys.js';

// Capture the same framed event Ink uses, before useInput erases the sequence.
// Do not listen to stdin directly: chunks may contain partial/multiple keys.
export function useTerminalInput(handler: Parameters<typeof useInput>[0]): void {
	const {internal_eventEmitter} = useStdin();
	const sequence = useRef('');
	useEffect(() => {
		const capture = (input: string) => { sequence.current = String(input); };
		internal_eventEmitter.on('input', capture);
		return () => { internal_eventEmitter.removeListener('input', capture); };
	}, [internal_eventEmitter]);
	useInput((input, key) => {
		if (key.eventType === 'release') return;
		handler(input, normalizeTerminalKey(key, sequence.current));
	});
}
