//! Not in the TS: the mouse-wheel scaling shared by attach and Preview focus (`scroll_speed`, which replaced the TS's
//! `attach_scroll_sensitivity`).
//!
//! Claude is launched scrolling one line per wheel event with its own acceleration off (`claude_scroll_override`),
//! so the events Deckhand forwards decide the distance alone. The TS forwarded one event per whole unit of a running
//! total that was never reset: the first notch of a scroll usually did nothing, the remainder of one direction
//! counted towards the other, and Claude's acceleration read the thinned stream as random bursts. Here the total
//! starts over with every gesture, and the first event of one always moves.

use std::time::{Duration, Instant};

/// `scroll_speed` when unset: every wheel event as the terminal sends it.
pub const DEFAULT_SCROLL_SENSITIVITY: f64 = 1.0;

/// The highest sensitivity: three events per wheel event (vim's three lines per notch).
pub const MAX_SCROLL_SENSITIVITY: f64 = 3.0;

/// A pause this long ends a scroll gesture; the next wheel event starts a new one.
pub const WHEEL_GESTURE_GAP: Duration = Duration::from_millis(150);

/// Mouse wheel direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WheelDirection {
    Up,
    Down,
}

/// `scroll_speed` as used: clamped to 0..=[`MAX_SCROLL_SENSITIVITY`], the default when unset or not
/// finite.
pub fn normalize_scroll_sensitivity(value: Option<f64>) -> f64 {
    match value {
        Some(value) if value.is_finite() => value.clamp(0.0, MAX_SCROLL_SENSITIVITY),
        _ => DEFAULT_SCROLL_SENSITIVITY,
    }
}

/// Scales wheel events by a sensitivity, one gesture at a time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WheelScaler {
    /// The share of an event not forwarded yet, within the current gesture.
    credit: f64,
    /// The last event's direction and time.
    last: Option<(WheelDirection, Instant)>,
}

impl WheelScaler {
    /// How many events to forward for one wheel event at `now`: `sensitivity` of an event, accumulated within a
    /// gesture. A gesture ends after [`WHEEL_GESTURE_GAP`] without events or on a change of direction; the first
    /// event of the next one always forwards at least one (unless `sensitivity` is 0: none ever), and nothing is
    /// carried over.
    pub fn scale(&mut self, sensitivity: f64, direction: WheelDirection, now: Instant) -> usize {
        let continues = self
            .last
            .is_some_and(|(last, at)| last == direction && now.saturating_duration_since(at) < WHEEL_GESTURE_GAP);
        self.last = Some((direction, now));
        if sensitivity <= 0.0 {
            self.credit = 0.0;
            return 0;
        }
        if !continues {
            self.credit = (1.0 - sensitivity).max(0.0);
        }
        // The epsilon absorbs rounding (0.88 + 0.12 is just below 1 in binary).
        let total = self.credit + sensitivity + 1e-9;
        let whole = total.floor();
        self.credit = (total - whole - 1e-9).max(0.0);
        whole as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sensitivity: f64, events: &[(WheelDirection, u64)]) -> Vec<usize> {
        let start = Instant::now();
        let mut scaler = WheelScaler::default();
        events
            .iter()
            .map(|&(direction, ms)| scaler.scale(sensitivity, direction, start + Duration::from_millis(ms)))
            .collect()
    }

    fn steady(direction: WheelDirection, count: u64) -> Vec<(WheelDirection, u64)> {
        (0..count).map(|index| (direction, index * 10)).collect()
    }

    #[test]
    fn sensitivity_defaults_and_clamps() {
        assert_eq!(normalize_scroll_sensitivity(None), 1.0);
        assert_eq!(normalize_scroll_sensitivity(Some(f64::NAN)), 1.0);
        assert_eq!(normalize_scroll_sensitivity(Some(f64::INFINITY)), 1.0);
        assert_eq!(normalize_scroll_sensitivity(Some(5.0)), 3.0);
        assert_eq!(normalize_scroll_sensitivity(Some(-1.0)), 0.0);
        assert_eq!(normalize_scroll_sensitivity(Some(0.5)), 0.5);
    }

    #[test]
    fn one_forwards_every_event() {
        assert_eq!(run(1.0, &steady(WheelDirection::Down, 5)), [1, 1, 1, 1, 1]);
    }

    #[test]
    fn a_fraction_moves_on_the_first_event_then_evenly() {
        assert_eq!(run(0.5, &steady(WheelDirection::Up, 6)), [1, 0, 1, 0, 1, 0]);
        let forwarded = run(0.12, &steady(WheelDirection::Up, 18));
        assert_eq!(forwarded[0], 1);
        assert_eq!(forwarded.iter().sum::<usize>(), 3);
    }

    #[test]
    fn above_one_forwards_more_than_one() {
        assert_eq!(run(3.0, &steady(WheelDirection::Down, 3)), [3, 3, 3]);
        assert_eq!(run(1.5, &steady(WheelDirection::Down, 4)), [1, 2, 1, 2]);
    }

    #[test]
    fn a_pause_or_a_reversal_starts_a_new_gesture() {
        use WheelDirection::{Down, Up};
        // Without the reset the second Up would be swallowed, and the Down would use the Up's remainder.
        assert_eq!(run(0.5, &[(Up, 0), (Up, 200), (Down, 210), (Down, 220)]), [1, 1, 1, 0]);
        // Within the gap the gesture goes on.
        assert_eq!(run(0.5, &[(Up, 0), (Up, 140)]), [1, 0]);
    }

    #[test]
    fn zero_ignores_the_wheel() {
        assert_eq!(run(0.0, &steady(WheelDirection::Up, 3)), [0, 0, 0]);
    }
}
