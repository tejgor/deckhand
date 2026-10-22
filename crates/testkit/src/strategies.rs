//! Shared proptest strategies for the property tests in the workspace.

use proptest::prelude::*;

/// Text mixing arbitrary scalar values with frequent real editing cases, including multi-code-point sequences
/// (combining marks, skin tones, ZWJ families), surrogate-pair emoji, CJK and every kind of line break.
pub fn unicode_text() -> impl Strategy<Value = String> {
    let chunk = prop_oneof![
        3 => any::<char>().prop_map(|character| character.to_string()),
        7 => prop::sample::select(vec![
            "a", "Z", "0", "_", "-", "é", "λ", "日", "中", "😀", "👍🏽", "e\u{301}",
            "👨‍👩‍👧", " ", "\t", "\n", "\r\n", "\u{a0}",
        ]).prop_map(str::to_owned),
    ];
    prop::collection::vec(chunk, 0..32).prop_map(|chunks| chunks.concat())
}
