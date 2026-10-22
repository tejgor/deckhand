//! deckhand-e2e: black-box test harness (see README.md). Drives whatever binary `DECKHAND_TEST_BIN` names and never
//! links the deckhand crates.

pub mod client;
pub mod daemon;
pub mod fakes;
pub mod harness;
pub mod tasks;
pub mod ui;

/// The path of this crate's `fake_agent` binary. Only integration tests have `CARGO_BIN_EXE_fake_agent`, so it is a macro.
#[macro_export]
macro_rules! fake_agent {
    () => {
        ::std::path::Path::new(env!("CARGO_BIN_EXE_fake_agent"))
    };
}
