//! Port of terminalState.ts.
//!
//! Undoes the terminal modes a TUI may have left enabled (mouse reporting, bracketed paste, keyboard protocols,
//! scroll region, ...) when control of the outer terminal changes hands.

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
    todo!("K2")
}
