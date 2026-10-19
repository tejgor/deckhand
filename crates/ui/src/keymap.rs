//! Port of the keys as data (what keysDocumented.test.ts read from the handlers' source) and the keymap ↔ help
//! coverage rules.
//!
//! TS checked that every `input === 'x'` a handler tests appears in the ? guide (src/help.ts) by scanning
//! app.tsx / tasksFlow.tsx / worktreesFlow.tsx. Rust handlers cannot be scanned that way, so each mode and flow
//! declares its keys as a `pub static KEYMAP: &[Binding]` next to its handler, and every binding names where the
//! guide documents it: a [`HelpRef::Topic`] by the topic's title (the topic id: [`HELP_TOPICS`]' titles are unique),
//! or [`HelpRef::Hidden`] with the reason it is left out. U6's coverage test applies [`undocumented_in_guide`] /
//! [`undocumented_in_topic`] to every keymap.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use deckhand_core::help::{HELP_TOPICS, HelpLine, HelpTopic};
use deckhand_core::key::Input;

/// A key that is not text (a `key.*` flag in TS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Return,
    Escape,
    Tab,
    /// `key.tab && key.shift`.
    ShiftTab,
    Backspace,
    Delete,
}

/// One way to press a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeySpec {
    /// TS `input === 'x'` (a letter, digit or symbol; `" "` is Space).
    Char(&'static str),
    /// TS `key.ctrl && input === 'x'`.
    Ctrl(&'static str),
    /// TS `key.upArrow`, `key.return`, ...
    Named(NamedKey),
}

impl KeySpec {
    /// Whether `input` (as useInput reports it) presses this key.
    pub fn matches(&self, input: &Input) -> bool {
        let key = &input.key;
        match *self {
            KeySpec::Char(text) => !key.ctrl && input.input == text,
            KeySpec::Ctrl(text) => key.ctrl && input.input == text,
            KeySpec::Named(named) => match named {
                NamedKey::Up => key.up_arrow,
                NamedKey::Down => key.down_arrow,
                NamedKey::Left => key.left_arrow,
                NamedKey::Right => key.right_arrow,
                NamedKey::PageUp => key.page_up,
                NamedKey::PageDown => key.page_down,
                NamedKey::Home => key.home,
                NamedKey::End => key.end,
                NamedKey::Return => key.return_,
                NamedKey::Escape => key.escape,
                NamedKey::Tab => key.tab && !key.shift,
                NamedKey::ShiftTab => key.tab && key.shift,
                NamedKey::Backspace => key.backspace,
                NamedKey::Delete => key.delete,
            },
        }
    }
}

/// Where the ? guide documents a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HelpRef {
    /// In the topic with this title (a [`HELP_TOPICS`] title).
    Topic(&'static str),
    /// Deliberately not in the guide, and why (e.g. "digits select by number: documented as 'type its number'").
    Hidden(&'static str),
}

/// One key binding of a mode or flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub keys: &'static [KeySpec],
    /// What it does, in a few words.
    pub action: &'static str,
    pub help: HelpRef,
}

impl Binding {
    /// Whether `input` presses any of the binding's keys.
    pub fn matches(&self, input: &Input) -> bool {
        self.keys.iter().any(|key| key.matches(input))
    }
}

/// A mode's or flow's bindings (each exposes one as `pub static KEYMAP: Keymap`).
pub type Keymap = &'static [Binding];

/// The ? guide topic with this title.
pub fn find_topic(title: &str) -> Option<&'static HelpTopic> {
    HELP_TOPICS.iter().find(|topic| topic.title == title)
}

/// TS keysDocumented.test.ts keyTokens: a key column's space-separated keys (`x  X`, `[ ]`).
pub fn key_tokens(text: &str) -> BTreeSet<&str> {
    text.split_whitespace().collect()
}

static WORD_SEPARATORS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\s/,;:()]+").expect("word separators are a valid regex"));

/// TS keysDocumented.test.ts words: descriptions also separate keys with `/`, commas and parentheses.
pub fn words(text: &str) -> BTreeSet<&str> {
    WORD_SEPARATORS.split(text).filter(|word| !word.is_empty()).collect()
}

/// Every key token in the guide's key columns, over all topics (what browse keys are checked against).
pub fn documented_keys() -> BTreeSet<&'static str> {
    HELP_TOPICS
        .iter()
        .flat_map(|topic| topic.lines.iter())
        .filter_map(|line| match line {
            HelpLine::Key { keys, .. } => Some(key_tokens(keys)),
            _ => None,
        })
        .flatten()
        .collect()
}

/// Every word of one topic: its key columns, descriptions, notes and headings (what flow keys are checked against).
pub fn topic_words(topic: &HelpTopic) -> BTreeSet<&'static str> {
    topic
        .lines
        .iter()
        .flat_map(|line| match *line {
            HelpLine::Key { keys, text } => words(keys).into_iter().chain(words(text)).collect::<Vec<_>>(),
            HelpLine::Note { note } => words(note).into_iter().collect(),
            HelpLine::Heading { heading } => words(heading).into_iter().collect(),
        })
        .collect()
}

/// The single-character keys a keymap handles (TS `input === '(.)'`, Space excluded as in the test).
pub fn handled_chars(keymap: Keymap) -> BTreeSet<&'static str> {
    keymap
        .iter()
        .flat_map(|binding| binding.keys.iter())
        .filter_map(|key| match key {
            KeySpec::Char(text) if text.chars().count() == 1 && *text != " " => Some(*text),
            _ => None,
        })
        .collect()
}

/// TS 'every key the browse view handles has a line in the ? guide': the keymap's character keys missing from
/// every topic's key column.
pub fn undocumented_in_guide(keymap: Keymap) -> Vec<&'static str> {
    let documented = documented_keys();
    handled_chars(keymap)
        .into_iter()
        .filter(|key| !documented.contains(key))
        .collect()
}

/// TS 'every key the Tasks board / worktree manager handles is in the guide's <topic> topic': the keymap's character
/// keys missing from the words of the topic titled `title` (all of them when there is no such topic).
pub fn undocumented_in_topic(keymap: Keymap, title: &str) -> Vec<&'static str> {
    let documented = find_topic(title).map(topic_words).unwrap_or_default();
    handled_chars(keymap)
        .into_iter()
        .filter(|key| !documented.contains(key))
        .collect()
}

/// Bindings whose [`HelpRef::Topic`] names no guide topic.
pub fn unknown_topics(keymap: Keymap) -> Vec<&'static str> {
    keymap
        .iter()
        .filter_map(|binding| match binding.help {
            HelpRef::Topic(title) if find_topic(title).is_none() => Some(title),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deckhand_core::key::Key;

    static SAMPLE: Keymap = &[
        Binding {
            keys: &[KeySpec::Char("q")],
            action: "quit",
            help: HelpRef::Topic("Start here"),
        },
        Binding {
            keys: &[KeySpec::Char(" "), KeySpec::Named(NamedKey::Return)],
            action: "toggle",
            help: HelpRef::Hidden("space is not checked"),
        },
        Binding {
            keys: &[KeySpec::Char("§")],
            action: "nothing",
            help: HelpRef::Topic("No such topic"),
        },
    ];

    #[test]
    fn keys_match_inputs() {
        assert!(KeySpec::Char("q").matches(&Input::text("q")));
        assert!(!KeySpec::Char("q").matches(&Input::key(
            "q",
            Key {
                ctrl: true,
                ..Key::NONE
            }
        )));
        assert!(KeySpec::Ctrl("s").matches(&Input::key(
            "s",
            Key {
                ctrl: true,
                ..Key::NONE
            }
        )));
        let shift_tab = Input::key(
            "",
            Key {
                tab: true,
                shift: true,
                ..Key::NONE
            },
        );
        assert!(KeySpec::Named(NamedKey::ShiftTab).matches(&shift_tab));
        assert!(!KeySpec::Named(NamedKey::Tab).matches(&shift_tab));
        assert!(SAMPLE[1].matches(&Input::text(" ")));
    }

    #[test]
    fn splits_like_the_ts_test() {
        assert_eq!(key_tokens("x  X"), BTreeSet::from(["x", "X"]));
        assert_eq!(
            words("Edit it (Tab: details, Ctrl+S saves); a/A"),
            BTreeSet::from(["Edit", "it", "Tab", "details", "Ctrl+S", "saves", "a", "A"])
        );
    }

    #[test]
    fn coverage_reports_missing_keys_and_topics() {
        assert!(documented_keys().contains("q"));
        assert_eq!(handled_chars(SAMPLE), BTreeSet::from(["q", "§"]));
        assert_eq!(undocumented_in_guide(SAMPLE), ["§"]);
        assert_eq!(undocumented_in_topic(SAMPLE, "Start here"), ["§"]);
        assert_eq!(undocumented_in_topic(SAMPLE, "No such topic"), ["q", "§"]);
        assert_eq!(unknown_topics(SAMPLE), ["No such topic"]);
        assert!(find_topic("Tasks").is_some());
    }

    use crate::app::modes::{
        action_picker, browse, confirm, create, filter, preview_focus, review, search, workspace_info,
    };
    use crate::flows::{agents, changes, notes, settings, tasks, worktrees};

    fn all_keymaps() -> Vec<(&'static str, Keymap)> {
        vec![
            ("browse", browse::KEYMAP),
            ("review", review::KEYMAP),
            ("search", search::KEYMAP),
            ("filter", filter::KEYMAP),
            ("workspace-info", workspace_info::KEYMAP),
            ("action-picker", action_picker::KEYMAP),
            ("preview-focus", preview_focus::KEYMAP),
            ("pick-program", create::PICK_PROGRAM_KEYMAP),
            ("enter-name", create::ENTER_NAME_KEYMAP),
            ("pick-worktree", create::PICK_WORKTREE_KEYMAP),
            ("confirm-kill", confirm::KILL_KEYMAP),
            ("confirm-merge", confirm::MERGE_KEYMAP),
            ("confirm-remove", confirm::REMOVE_KEYMAP),
            ("agents", agents::KEYMAP),
            ("notes", notes::KEYMAP),
            ("settings", settings::KEYMAP),
            ("changes", changes::KEYMAP),
            ("tasks", tasks::KEYMAP),
            ("worktrees", worktrees::KEYMAP),
        ]
    }

    // covers: B-U6-01 every key the browse view handles has a line in the ? guide
    #[test]
    fn every_key_the_browse_view_handles_has_a_line_in_the_guide() {
        assert!(!handled_chars(browse::KEYMAP).is_empty());
        let missing = undocumented_in_guide(browse::KEYMAP);
        assert!(
            missing.is_empty(),
            "browse keys missing from the help guide: {}",
            missing.join(" ")
        );
    }

    // covers: B-U6-02 every key the Tasks board handles is in the guide's Tasks topic
    #[test]
    fn every_key_the_tasks_board_handles_is_in_the_guides_tasks_topic() {
        assert!(find_topic("Tasks").is_some());
        assert!(!handled_chars(tasks::KEYMAP).is_empty());
        let missing = undocumented_in_topic(tasks::KEYMAP, "Tasks");
        assert!(
            missing.is_empty(),
            "Tasks keys missing from the Tasks help topic: {}",
            missing.join(" ")
        );
    }

    // covers: B-U6-03 every key the worktree manager handles is in the guide's Worktrees topic
    #[test]
    fn every_key_the_worktree_manager_handles_is_in_the_guides_worktrees_topic() {
        assert!(find_topic("Worktrees").is_some());
        assert!(!handled_chars(worktrees::KEYMAP).is_empty());
        let missing = undocumented_in_topic(worktrees::KEYMAP, "Worktrees");
        assert!(
            missing.is_empty(),
            "Worktrees keys missing from the Worktrees help topic: {}",
            missing.join(" ")
        );
    }

    #[test]
    fn every_keymap_names_real_guide_topics() {
        for (name, keymap) in all_keymaps() {
            assert!(
                unknown_topics(keymap).is_empty(),
                "{name} names unknown topics: {:?}",
                unknown_topics(keymap)
            );
        }
    }
}
