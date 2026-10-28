//! Not in the TS: Deckhand's one press-twice rule. A key that does something hard to undo only arms on its first
//! press, and the status line says what a second press will do. The same key on the same target within
//! [`PRESS_AGAIN_MS`] does it; any other key, or the wait running out, cancels.

use deckhand_core::jsstr::date::now_ms;

/// How long an armed key waits for its second press.
pub const PRESS_AGAIN_MS: i64 = 2000;

/// What a first press armed (`T` names the target, such as a task or a row), and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PressAgain<T> {
    armed: Option<(T, i64)>,
}

impl<T> Default for PressAgain<T> {
    fn default() -> Self {
        PressAgain { armed: None }
    }
}

impl<T: PartialEq> PressAgain<T> {
    /// Takes what is armed, so a key that does not confirm it cancels it: `Some` only within the wait.
    pub fn take(&mut self) -> Option<T> {
        self.take_at(now_ms())
    }

    pub fn take_at(&mut self, now: i64) -> Option<T> {
        self.armed
            .take()
            .filter(|(_, at)| now - at < PRESS_AGAIN_MS)
            .map(|(target, _)| target)
    }

    /// Arms `target` from now.
    pub fn arm(&mut self, target: T) {
        self.arm_at(target, now_ms());
    }

    pub fn arm_at(&mut self, target: T, now: i64) {
        self.armed = Some((target, now));
    }

    /// What is armed, however long ago.
    pub fn armed(&self) -> Option<&T> {
        self.armed.as_ref().map(|(target, _)| target)
    }

    pub fn clear(&mut self) {
        self.armed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_press_on_the_same_target_in_time_confirms() {
        let mut again = PressAgain::default();
        assert_eq!(again.take_at(0), None::<&str>);
        again.arm_at("a", 1000);
        assert_eq!(again.take_at(2999), Some("a"));
        // Taken: a third press arms again.
        assert_eq!(again.take_at(3000), None);
        again.arm_at("a", 1000);
        assert_eq!(again.take_at(3000), None, "the wait ran out");
        again.arm_at("a", 0);
        again.clear();
        assert_eq!(again.armed(), None);
    }
}
