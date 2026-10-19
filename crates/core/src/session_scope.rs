//! Port of sessionScope.ts.

use deckhand_protocol::data::SessionRecord;

/// TS: sessionScope.ts isPathInside (lexical: `child` is `parent` or beneath it).
pub fn is_path_inside(parent: &str, child: &str) -> bool {
    let _ = (parent, child);
    todo!("P2")
}

/// TS: sessionScope.ts sessionMatchesScope.
pub fn session_matches_scope(session: &SessionRecord, repo_root: &str) -> bool {
    let _ = (session, repo_root);
    todo!("P2")
}
