//! What you typed into an agent's input and have not sent, kept from the keys alone (no screen reading), and the
//! guard that looks for it on the screen once the unsent-text hold's time limit has passed.
//!
//! **The count.** Printable characters add one, backspace takes one off (never below zero); at zero the input is
//! empty and holds nothing. Enter, `ctrl+c` and `ctrl+u` empty it, and so does a turn starting (what was in the input
//! was sent). Keys whose effect can't be known from the keys make it **uncertain** instead of guessed: `esc`, other
//! alt keys, word deletes (`ctrl+w`, `alt+backspace`), history keys (up, down), tab (completion adds text), an edit
//! after the cursor moved (left, right, Home, End, `ctrl+a/e/b/f`), and a bracketed paste (the agent may show it as a
//! placeholder one backspace removes). Uncertain holds until the time limit, where the guard decides.
//!
//! **The guard** compares what was typed and the screen's lower part as streams of letters and digits only (lower
//! case), so wrapping, box borders and indentation don't matter, and knows nothing of any agent's layout: the end of
//! the typed text, its start, or its end as it was a word or an edit ago, still on the screen means a real draft.

use std::collections::VecDeque;

/// How many characters of what was typed are kept at each end (its start and its end).
pub const KEPT_CHARS: usize = 60;

/// How many letters and digits the guard looks for (at most): the end (or start) of what was typed.
const NEEDLE: usize = 16;

/// Fewer letters and digits than this typed: nothing the guard can check, nor worth protecting.
pub const CHECKABLE: usize = 4;

/// How many earlier ends of the text (one per word typed, cursor move or uncertain key) the guard also looks for.
const MARKS: usize = 4;

/// The screen's lower part the guard reads: its lower half, and at least this many lines.
const LOWER_LINES: usize = 15;

/// The unsent text in an agent's input, as far as the keys tell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unsent {
    /// Characters typed and not deleted.
    pub count: usize,
    /// A key whose effect on the input can't be known was typed: the count is not to be trusted.
    pub uncertain: bool,
    /// A bracketed paste is in it (the agent may show it as a placeholder rather than its text).
    pub pasted: bool,
    /// A task draft Deckhand typed is in it.
    pub draft: bool,
    /// The cursor moved off the end: the next edit's effect can't be known.
    moved: bool,
    /// What was typed since the input was last empty: its first and last [`KEPT_CHARS`] characters.
    head: String,
    tail: String,
    /// The end of the text a word, a cursor move or an uncertain key ago (newest last).
    marks: VecDeque<String>,
}

/// What a chunk of keys did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    /// Enter, `ctrl+c` or `ctrl+u`: whatever was in the input was sent or cleared.
    pub cleared: bool,
}

/// What the guard found on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard {
    /// Fewer than [`CHECKABLE`] letters or digits typed: can't be checked, nothing worth protecting.
    Short,
    /// What was typed is still on the screen: a real draft.
    Visible,
    /// Not on the screen: the text is gone.
    Gone,
    /// Not on the screen, but a paste is in it, which the agent may show as a placeholder.
    Unverifiable,
}

impl Unsent {
    /// Whether it holds the session's queue of messages.
    pub fn holds(&self) -> bool {
        self.count > 0 || self.uncertain || self.pasted || self.draft
    }

    /// Empties it: Enter, `ctrl+c`, `ctrl+u`, a turn starting, or the guard finding the text gone.
    pub fn clear(&mut self) {
        *self = Unsent::default();
    }

    /// A task draft Deckhand typed into the input (`text`, as one paste).
    pub fn draft_typed(&mut self, text: &str) {
        self.draft = true;
        self.push_text(text);
    }

    /// Applies keys you typed. `in_dialog`: a question, permission or menu shows, so letters go to it, not to the
    /// input; only what empties the input still counts there.
    pub fn type_keys(&mut self, data: &str, in_dialog: bool) -> Typed {
        let mut typed = Typed::default();
        let chars: Vec<char> = data.chars().collect();
        let mut at = 0;
        while at < chars.len() {
            let key = chars[at];
            at += 1;
            match key {
                '\r' | '\x03' | '\x15' => {
                    typed.cleared = true;
                    if !in_dialog {
                        self.clear();
                    }
                }
                _ if in_dialog => {
                    // A paste into a dialog is skipped whole, so none of it reads as keys.
                    if key == '\x1b' && chars.get(at..at + 5) == Some(&['[', '2', '0', '0', '~'][..]) {
                        at = paste_end(&chars, at + 5).1;
                    }
                }
                '\x1b' => {
                    let (next, cleared) = self.escape(&chars, at);
                    typed.cleared |= cleared;
                    at = next;
                }
                '\x7f' | '\x08' => self.backspace(),
                '\n' => self.printable('\n'),
                // Tab completes (adds text); ctrl+w deletes a word; ctrl+y yanks; ctrl+r searches the history;
                // ctrl+g opens an editor; ctrl+_ undoes.
                '\t' | '\x17' | '\x19' | '\x12' | '\x07' | '\x1f' => self.unknown(),
                // ctrl+a, ctrl+e, ctrl+b, ctrl+f: the cursor moves.
                '\x01' | '\x05' | '\x02' | '\x06' => self.cursor(),
                // ctrl+k, ctrl+d: delete after the cursor, nothing at the end.
                '\x0b' | '\x04' => self.delete_forward(),
                _ if key.is_control() => {}
                _ => self.printable(key),
            }
        }
        typed
    }

    /// The keys after an `ESC` at `at`: a CSI or SS3 sequence, a paste, `esc` itself or an alt key. Where the next
    /// key starts, and whether it was Enter.
    fn escape(&mut self, chars: &[char], at: usize) -> (usize, bool) {
        match chars.get(at) {
            Some('[') => {
                let start = at + 1;
                let mut end = start;
                while chars.get(end).is_some_and(|c| !('@'..='~').contains(c)) {
                    end += 1;
                }
                let Some(&last) = chars.get(end) else {
                    self.unknown();
                    return (chars.len(), false);
                };
                let params: String = chars[start..end].iter().collect();
                if params == "200" && last == '~' {
                    let (text, next) = paste_end(chars, end + 1);
                    self.paste(&text);
                    return (next, false);
                }
                // An X10 mouse report: three raw bytes follow.
                if params.is_empty() && last == 'M' {
                    return ((end + 4).min(chars.len()), false);
                }
                (end + 1, self.csi(&params, last))
            }
            Some('O') if at + 1 < chars.len() => (at + 2, self.csi("", chars[at + 1])),
            // `esc` alone (the agent may clear its input on a second one); an alt key (alt+backspace deletes a word).
            Some('\x1b') | None => {
                self.unknown();
                (at, false)
            }
            Some(_) => {
                self.unknown();
                (at + 1, false)
            }
        }
    }

    /// A CSI (or SS3) key: `params`, then its final character. Whether it was Enter.
    fn csi(&mut self, params: &str, last: char) -> bool {
        let first = params.split(';').next().unwrap_or_default();
        match last {
            // Up and down recall the history (or move between lines).
            'A' | 'B' => self.unknown(),
            // Right, left, Home, End.
            'C' | 'D' | 'H' | 'F' => self.cursor(),
            '~' => match first {
                "1" | "7" | "4" | "8" => self.cursor(),
                "3" => self.delete_forward(),
                // Insert, page up and down, function keys; the paste's end.
                "2" | "5" | "6" | "15" | "17" | "18" | "19" | "20" | "21" | "23" | "24" | "201" => {}
                _ => self.unknown(),
            },
            // Enter, and shift+enter (a new line), in the kitty keyboard protocol.
            'u' if params == "13" => {
                self.clear();
                return true;
            }
            'u' if params == "13;2" => self.printable('\n'),
            // Shift+tab (the agent's mode), focus in and out, mouse reports, F1 to F4.
            'Z' | 'I' | 'O' | 'M' | 'm' | 'P' | 'Q' | 'R' | 'S' => {}
            _ => self.unknown(),
        }
        false
    }

    fn printable(&mut self, key: char) {
        if self.moved {
            self.unknown();
        }
        if key.is_whitespace() && self.tail.chars().last().is_some_and(|last| !last.is_whitespace()) {
            self.mark();
        }
        self.count += 1;
        self.push(key);
    }

    fn backspace(&mut self) {
        if self.moved {
            self.unknown();
        }
        self.count = self.count.saturating_sub(1);
        if self.uncertain || self.pasted || self.draft {
            // Which character went is unknown: the tail keeps what was typed.
            return;
        }
        self.tail.pop();
        if self.head.chars().count() > self.count {
            self.head = self.head.chars().take(self.count).collect();
        }
        if self.count == 0 {
            self.clear();
        }
    }

    fn delete_forward(&mut self) {
        if self.moved {
            self.unknown();
        }
    }

    fn cursor(&mut self) {
        if !self.moved {
            self.mark();
        }
        self.moved = true;
    }

    fn paste(&mut self, text: &str) {
        self.unknown();
        self.pasted = true;
        self.count += text.chars().count();
        self.push_text(text);
    }

    /// A key whose effect can't be known.
    fn unknown(&mut self) {
        if !self.uncertain {
            self.mark();
        }
        self.uncertain = true;
    }

    /// Keeps the text's end as it is now, for the guard.
    fn mark(&mut self) {
        if self.tail.is_empty() || self.marks.back() == Some(&self.tail) {
            return;
        }
        if self.marks.len() == MARKS {
            self.marks.pop_front();
        }
        self.marks.push_back(self.tail.clone());
    }

    fn push_text(&mut self, text: &str) {
        for key in text.chars() {
            self.push(key);
        }
    }

    fn push(&mut self, key: char) {
        if self.head.chars().count() < KEPT_CHARS {
            self.head.push(key);
        }
        self.tail.push(key);
        let extra = self.tail.chars().count().saturating_sub(KEPT_CHARS);
        if extra > 0 {
            self.tail = self.tail.chars().skip(extra).collect();
        }
    }

    /// Looks for what was typed on `screen` (its lower part).
    pub fn guard(&self, screen: &str) -> Guard {
        let typed = letters(&self.head).len().max(letters(&self.tail).len());
        if typed < CHECKABLE {
            return Guard::Short;
        }
        let lines: Vec<&str> = screen.trim_end().lines().collect();
        let lower = lines.len() - lines.len().div_ceil(2).max(LOWER_LINES).min(lines.len());
        let haystack: String = letters(&lines[lower..].join("\n")).into_iter().collect();
        let end = |text: &str| -> String {
            let found = letters(text);
            found[found.len().saturating_sub(NEEDLE)..].iter().collect()
        };
        let start: String = letters(&self.head).into_iter().take(NEEDLE).collect();
        let needles = std::iter::once(end(&self.tail))
            .chain(std::iter::once(start))
            .chain(self.marks.iter().map(|mark| end(mark)));
        let mut found = false;
        for needle in needles {
            if needle.chars().count() >= CHECKABLE && haystack.contains(&needle) {
                found = true;
                break;
            }
        }
        match (found, self.pasted) {
            (true, _) => Guard::Visible,
            (false, true) => Guard::Unverifiable,
            (false, false) => Guard::Gone,
        }
    }
}

/// A paste's text from `at` up to its end (`ESC [201~`), and where the next key starts.
fn paste_end(chars: &[char], at: usize) -> (String, usize) {
    let close = ['\x1b', '[', '2', '0', '1', '~'];
    let mut end = at;
    while end < chars.len() {
        if chars[end..].starts_with(&close) {
            return (chars[at..end].iter().collect(), end + close.len());
        }
        end += 1;
    }
    (chars[at..].iter().collect(), chars.len())
}

/// `text`'s letters and digits, lower case.
fn letters(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
