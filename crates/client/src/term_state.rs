//! Undoes the terminal modes a TUI may have left enabled (mouse reporting, bracketed paste, keyboard protocols,
//! scroll region, ...) when control of the outer terminal changes hands.

use std::io::{IsTerminal, Write};

/// TS: terminalState.ts RESET_TERMINAL_STATE_SEQUENCE.
pub const RESET_TERMINAL_STATE_SEQUENCE: &str = concat!(
    "\x1b[0m",                                                                       // reset attributes
    "\x1b[?25h",                                                                     // show cursor
    "\x1b[?7h",                                                                      // enable line wrapping
    "\x1b[?6l",                                                                      // origin mode off
    "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l", // mouse/focus modes off
    "\x1b[?2004l",                                                                   // bracketed paste off
    "\x1b[>4;0m",                                                                    // xterm modifyOtherKeys off
    "\x1b[<u",                                                                       // kitty keyboard protocol off
    "\x1b[r",                                                                        // reset scroll region
);

/// TS: terminalState.ts resetTerminalState: writes [`RESET_TERMINAL_STATE_SEQUENCE`] to stdout when it is a TTY.
pub fn reset_terminal_state() {
    let mut stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return;
    }
    // Best effort, as a failed `process.stdout.write` is not reported to the caller either.
    let _ = stdout.write_all(RESET_TERMINAL_STATE_SEQUENCE.as_bytes());
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_sequence_matches_the_ts_literal() {
        assert_eq!(
            RESET_TERMINAL_STATE_SEQUENCE,
            "\x1b[0m\x1b[?25h\x1b[?7h\x1b[?6l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1004l\x1b[?1005l\x1b[?1006l\x1b[?1015l\
             \x1b[?2004l\x1b[>4;0m\x1b[<u\x1b[r"
        );
    }

    #[test]
    fn reset_never_fails() {
        // Under the test runner stdout is usually captured (not a TTY), so this writes nothing; either way it is
        // best effort and must not panic.
        reset_terminal_state();
    }
}
