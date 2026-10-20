//! Stand-in agents written as `/bin/sh` scripts (never a real agent), for the PTY suites that need a particular "ready"
//! line, e.g. one that echoes `NODE_ENV`. The configurable Rust fake is `bin/fake_agent` (see `harness::install_agent`).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::harness::Sandbox;

/// The ready line the sh fakes print unless a test wants another.
pub const READY: &str = "echo 'fake agent ready'";

/// sh that sets `$json` to the JSON-string elements of `"$@"` joined by commas (a JSON array without the brackets).
pub const SH_JSON_ARGS: &str = r#"json=''
for a in "$@"; do e=$(printf '%s' "$a" | sed 's/\\/\\\\/g; s/"/\\"/g'); json="$json${json:+,}\"$e\""; done
"#;

/// The tail of an agent script that behaves like a resume of a conversation that no longer exists: it prints
/// "No conversation found with session ID: <id after --resume>" and exits 1 after 300ms. `--version` answers at once, so the
/// daemon's version read of the changed binary never races the agent's exit (which would cancel the startup).
pub const MISSING_CONVERSATION_BODY: &str = r#"case " $* " in *" --version "*) echo '1.0.0'; exit 0;; esac
id=''
prev=''
for a in "$@"; do if [ "$prev" = --resume ] && [ -z "$id" ]; then id=$a; fi; prev=$a; done
echo "No conversation found with session ID: $id"
sleep 0.3
exit 1
"#;

/// A complete agent script made of [`MISSING_CONVERSATION_BODY`].
pub const MISSING_CONVERSATION_AGENT: &str = r#"#!/bin/sh
case " $* " in *" --version "*) echo '1.0.0'; exit 0;; esac
id=''
prev=''
for a in "$@"; do if [ "$prev" = --resume ] && [ -z "$id" ]; then id=$a; fi; prev=$a; done
echo "No conversation found with session ID: $id"
sleep 0.3
exit 1
"#;

/// An agent that answers `--help` / `--version`, runs `ready` (shell) and then stays alive.
pub fn sh_agent_script(ready: &str) -> String {
    format!(
        "#!/bin/sh\ncase \" $* \" in\n*\" --help \"*) echo 'PROMPT --settings --no-daemon resume'; exit 0;;\n*\" --version \"*) echo '1.0.0'; exit 0;;\nesac\n{ready}\nexec sleep 100000\n"
    )
}

/// claude, pi and codex sh stand-ins in `dir` (created) that run `ready` once started.
pub fn install_sh_agents(dir: &Path, ready: &str) -> PathBuf {
    fs::create_dir_all(dir).expect("agent bin dir");
    for agent in ["claude", "pi", "codex"] {
        let file = dir.join(agent);
        fs::write(&file, sh_agent_script(ready)).expect("write fake agent");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod fake agent");
    }
    dir.to_path_buf()
}

/// [`install_sh_agents`] into `<sandbox>/agents`.
pub fn install_sandbox_agents(sandbox: &Sandbox, ready: &str) -> PathBuf {
    install_sh_agents(&sandbox.root().join("agents"), ready)
}
