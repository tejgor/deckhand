import {useCallback, useEffect, useLayoutEffect, useRef} from 'react';
import {useInput, useStdin} from 'ink';
import {normalizeTerminalKey} from './terminalKeys.js';

// Capture the same framed event Ink uses, before useInput erases the sequence.
// Do not listen to stdin directly: chunks may contain partial/multiple keys.
export function useTerminalInput(handler: Parameters<typeof useInput>[0]): void {
	const {internal_eventEmitter} = useStdin();
	const sequence = useRef('');
	// Ink re-subscribes useInput's listener in a passive effect after each render, so a key arriving between a render
	// and that effect would reach the previous render's handler and act on stale state (a key typed right after the
	// screen changed was lost). One stable listener calls the latest handler, which is updated synchronously at commit.
	const latest = useRef(handler);
	useLayoutEffect(() => { latest.current = handler; });
	useEffect(() => {
		const capture = (input: string) => { sequence.current = String(input); };
		internal_eventEmitter.on('input', capture);
		return () => { internal_eventEmitter.removeListener('input', capture); };
	}, [internal_eventEmitter]);
	useInput(useCallback<Parameters<typeof useInput>[0]>((input, key) => {
		if (key.eventType === 'release') return;
		latest.current(input, normalizeTerminalKey(key, sequence.current));
	}, []));
}
