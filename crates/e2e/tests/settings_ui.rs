//! Settings (C), Help (?) and the Dev/action flows in a real PTY, driving the isolated dev launcher.

use std::fs;
use std::path::Path;

use deckhand_e2e::daemon::path_with;
use deckhand_e2e::fakes::{READY, install_sh_agents};
use deckhand_e2e::harness::Sandbox;
use deckhand_e2e::ui::{Ui, spawn_launcher_env};
use serde_json::{Value, json};

/// `terminalUi({args: [launcher, '--sandbox'], cwd: home, home, env})`.
fn launch(sandbox: &Sandbox, env: &[(&str, &str)]) -> Ui {
    spawn_launcher_env(sandbox, &["--sandbox"], &sandbox.dev_home(), env)
}

fn read_json(file: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(file).expect("read json")).expect("parse json")
}

/// `{...original, ...extra}`.
fn with(original: &Value, extra: Value) -> Value {
    let mut merged = original.as_object().expect("object").clone();
    for (key, value) in extra.as_object().expect("object") {
        merged.insert(key.clone(), value.clone());
    }
    Value::Object(merged)
}

// ported from tests/settingsUi.test.ts: C → Settings in the sandbox: a grid with a Global and a This repo column; ←→ picks the cell's layer (sticky across rows), Enter edits that layer, Linked items writes its worktree.symlink, x clears, T trusts, e opens the raw JSON; Agents rows are global only
#[test]
fn settings_grid_global_and_repo_columns() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let codex_home = home.join("codex");
    fs::create_dir_all(&codex_home).expect("codex home");
    let codex_home_text = codex_home.display().to_string();
    let mut ui = launch(&sandbox, &[("CODEX_HOME", codex_home_text.as_str())]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    let repo = home.join("sandbox");
    let file = repo.join("deckhand.json");
    let config = home.join("config.json");
    let original = read_json(&file);
    fs::create_dir_all(repo.join("node_modules/pkg")).expect("node_modules");
    fs::write(repo.join("node_modules/pkg/index.js"), "module.exports = 1;\n").expect("index.js");
    fs::write(repo.join("notes.txt"), "local notes\n").expect("notes");
    fs::write(
        &config,
        json!({"defaults": {"actions": {"fmt": "npm run fmt"}}}).to_string(),
    )
    .expect("config");

    // ? : topics on the left, j/k (or a number) switch, / searches every topic, Esc steps back out.
    ui.press("?");
    ui.screen("Help · Start here");
    ui.screen("7 Tasks");
    ui.press("j");
    ui.screen("Help · Sessions");
    ui.press("0");
    ui.screen("Help · Dev & actions");
    ui.press("j");
    ui.screen("Help · Settings");
    ui.press("/");
    ui.screen("type to search");
    ui.press("lazygit");
    ui.screen("9  Git & PRs");
    ui.press("\x1b");
    ui.screen("Help · Settings");
    ui.press("\x1b");
    ui.screen("C settings");

    // One grid, grouped by section; the cursor starts on the This repo column. The sandbox's untrusted Dev command is
    // in its cell (needs trust) while the built-in applies.
    ui.press("C");
    ui.screen("Settings · sandbox");
    ui.screen("deckhand.json: needs trust");
    for text in [
        "Global (all repos)",
        "This repo",
        "General",
        "Commands",
        "Worktrees",
        "dev (built-in)",
        "npm run dev ⚠ needs trust",
        "not set: the built-in default applies",
    ] {
        ui.screen(text);
    }

    // Default agent in the repo column: a choice titled with its layer, saved to deckhand.json (which still needs review).
    ui.press("\r");
    ui.screen("Default agent · This repo");
    ui.screen("○ claude");
    ui.press("j");
    ui.screen("❯ ○ pi");
    ui.press("j");
    ui.screen("❯ ○ codex");
    ui.press("\r");
    ui.screen("Saved Default agent to this repo · review required (the file had changes you haven't reviewed)");
    assert_eq!(read_json(&file), with(&original, json!({"defaultAgent": "codex"})));

    // ← moves to the Global column; the column stays put while the row changes. Dev command saves a global value.
    ui.press("\x1b[D");
    ui.screen("not set; this repo sets it");
    ui.press("j");
    ui.screen("❯ Default workspace");
    ui.press("j");
    ui.screen("No Dev command is set, so d runs a shell command named `dev`.");
    // Enter lists what Dev runs; + Add a process asks for the (first, so single) command.
    ui.press("\r");
    ui.screen("Settings › Dev command · Global");
    ui.screen("+ Add a process");
    ui.press("\r");
    ui.screen("New Dev process · step 1 of 2: command");
    ui.press("printf global");
    ui.screen("printf global");
    // Step 2: the name, suggested from the command; Enter keeps it.
    ui.press("\r");
    ui.screen("New Dev process · step 2 of 2: name");
    ui.screen("› printf");
    ui.press("\r");
    ui.screen("Saved Dev process printf to global defaults");
    assert_eq!(
        read_json(&config)["defaults"],
        json!({"actions": {"fmt": "npm run fmt"}, "devCommand": {"printf": "printf global"}})
    );
    assert_eq!(read_json(&file), with(&original, json!({"defaultAgent": "codex"})));
    ui.press("\x1b");
    ui.screen("❯ Dev command");
    ui.press("j");
    ui.screen("No setup command");
    ui.press("\r");
    ui.screen("Setup command · Global");
    ui.press("\x1b");
    ui.screen("No setup command");

    // → back to This repo; Linked items picks what new worktrees link, saved as this repo's worktree.symlink.
    ui.press("\x1b[C");
    ui.screen("This repo · ");
    ui.press("\x1b[F");
    ui.screen("❯ Scroll speed");
    ui.press("k");
    ui.screen("❯ Notifications");
    ui.press("k");
    ui.screen("❯ Agent signals");
    ui.press("k");
    ui.screen("❯ Linked items");
    ui.press("\r");
    ui.screen("Settings › Linked items · This repo");
    ui.screen("[link] node_modules/");
    ui.press("j");
    ui.screen("❯ [skip] notes.txt");
    ui.press(" ");
    ui.screen("❯ [link] notes.txt");
    ui.press("\r");
    ui.screen("Saved Linked items to this repo · review required");
    assert_eq!(
        read_json(&file),
        with(
            &original,
            json!({"defaultAgent": "codex", "worktree": {"symlink": ["node_modules", "notes.txt"]}})
        )
    );

    // T reviews and trusts inline, then returns here; an edit made here now keeps the file trusted.
    ui.press("T");
    ui.screen("Review repository configuration");
    ui.press("\r");
    ui.screen("deckhand.json: trusted ✓");
    ui.screen("Settings · sandbox");
    ui.press("x");
    ui.screen("Clear Linked items in this repo?");
    ui.press("\r");
    ui.screen("Cleared Linked items in this repo · still trusted");
    ui.screen("deckhand.json: trusted ✓");
    assert_eq!(read_json(&file), with(&original, json!({"defaultAgent": "codex"})));
    ui.press("E");
    ui.screen("Repository config · deckhand.json");
    ui.press("\x1b");
    ui.screen("Settings · sandbox");

    // Agents: global-only config.json settings. The repo cell is empty and the cursor selects Global on these rows
    // (from the This repo column too); Scroll speed (not in the TS) saves scroll_speed as a number; Agent signals, not
    // set (on for Claude), saves agent_hooks and hints that Codex (its home exists here) has no Deckhand hooks yet.
    // Leaving the row returns to This repo.
    ui.press("\x1b[F");
    ui.screen("❯ Scroll speed");
    ui.screen("Not set: 1, each wheel event");
    ui.press("\r");
    ui.screen("Scroll speed · Global");
    ui.press("j");
    ui.screen("❯ ○ 2");
    ui.press("\r");
    ui.screen("Scroll speed 2 (global)");
    assert_eq!(read_json(&config)["scroll_speed"], json!(2));
    // custom… types any speed from 0 to 3; out of range keeps the field open with why.
    ui.press("\r");
    ui.screen("❯ ◉ 2");
    ui.press("j");
    ui.screen("❯ ○ 3");
    ui.press("j");
    ui.screen("❯ ○ custom…");
    ui.press("\r");
    ui.screen("a number from 0 to 3, e.g. 1.5");
    ui.press("9");
    ui.press("\r");
    ui.screen("From 0 to 3 (0 ignores the wheel)");
    ui.press("\x7f");
    ui.screen("1 passes each wheel event on as your terminal sends it");
    ui.press("1.5");
    ui.screen("→ 1.5× the wheel events your terminal sends");
    ui.press("\r");
    ui.screen("Scroll speed 1.5 (global)");
    assert_eq!(read_json(&config)["scroll_speed"], json!(1.5));
    ui.press("k");
    ui.screen("❯ Notifications");
    ui.press("k");
    ui.screen("❯ Agent signals");
    ui.screen("Not set: on for Claude");
    ui.press("\r");
    ui.screen("Agent signals · Global");
    ui.screen("❯ ○ on");
    ui.press("\r");
    ui.screen("Agent signals on (global)");
    ui.screen("on ⚠ Codex");
    ui.screen("Codex hooks not set up: scripts/deckhand-dev.sh hooks codex >");
    assert_eq!(read_json(&config)["agent_hooks"], json!(true));
    ui.press("k");
    ui.screen("❯ Linked items");
    ui.screen("This repo · ");
    ui.press("\x1b");
    ui.screen("C settings");
}

// ported from tests/settingsUi.test.ts: a Dev command saved in Settings keeps the repository file trusted (created and trusted, so d starts it without asking); an edit from outside Deckhand is reviewed on the next d; a guided new action saves
#[test]
fn dev_command_saved_in_settings_keeps_the_file_trusted() {
    let sandbox = Sandbox::new("ui");
    let home = sandbox.dev_home();
    let bin = home.join("bin");
    install_sh_agents(&bin, READY);
    let path = path_with(&bin);
    let mut ui = launch(&sandbox, &[("PATH", path.as_str())]);
    ui.screen("DEV (isolated)");
    ui.screen("● ready");
    let file = home.join("sandbox/deckhand.json");
    let config = home.join("config.json");
    // No repository file yet (nothing to review); the global Dev command applies.
    fs::remove_file(&file).expect("rm deckhand.json");
    fs::write(
        &config,
        json!({"defaults": {"devCommand": "echo fallback-$((40+2))"}}).to_string(),
    )
    .expect("config");
    ui.press("n");
    ui.screen("Choose an agent");
    ui.press("\r");
    ui.screen("Workspace: no worktree");
    ui.press("s1");
    ui.screen("Name: s1");
    ui.press("\r");
    ui.screen("fake agent ready");

    // The Dev command row: ← shows the global value selected and inherited, → the empty repo cell.
    ui.press("C");
    ui.screen("deckhand.json: not present");
    ui.press("j");
    ui.screen("❯ Default workspace");
    ui.press("j");
    ui.screen("not set: inherits global");
    ui.press("\x1b[D");
    ui.screen("inherited by this repo");
    ui.press("\x1b[C");
    ui.screen("not set: inherits global");
    ui.press("\r");
    ui.screen("Settings › Dev command · This repo");
    ui.screen("echo fallback-$((40+2))");
    ui.press("\r");
    ui.screen("New Dev process · step 1 of 2: command");
    ui.press("echo repo-$((40+2))");
    ui.screen("› echo repo-$((40+2))");
    ui.press("\r");
    ui.screen("› echo");
    ui.press("\r");
    ui.screen("Saved Dev process echo to this repo · created and trusted");
    assert_eq!(read_json(&file)["devCommand"], json!({"echo": "echo repo-$((40+2))"}));
    ui.press("\x1b");
    ui.screen("❯ Dev command");
    ui.screen("deckhand.json: trusted ✓");
    let trusted = read_json(&config)
        .get("trustedProjects")
        .and_then(Value::as_object)
        .map_or(0, |map| map.len());
    assert_eq!(trusted, 1);

    // A new action in this repo's Actions list: name (rules and a live check), then its command; still trusted.
    ui.press("j");
    ui.screen("❯ Setup command");
    ui.press("j");
    ui.screen("❯ Actions");
    ui.press("\r");
    ui.screen("Settings › Actions · This repo");
    ui.screen("Named shell commands you run with e on a session");
    ui.press("a");
    ui.screen("New action · step 1 of 2: name");
    ui.screen("starts with a letter or number");
    ui.press("/");
    ui.screen("must start with a letter or number");
    ui.press("\x7f");
    ui.screen("step 1 of 2: name");
    ui.press("lint frontend");
    ui.screen("adds a new action to this repo");
    ui.press("\r");
    ui.screen("New action · step 2 of 2: command for lint frontend");
    ui.screen("Runs with your shell in the session's worktree");
    ui.press("npm run lint");
    ui.screen("› npm run lint");
    ui.press("\r");
    ui.screen("Saved action lint frontend to this repo · still trusted · run it with e on a session");
    assert_eq!(read_json(&file)["actions"], json!({"lint frontend": "npm run lint"}));
    ui.press("\x1b");
    ui.screen("Settings · sandbox");
    ui.press("\x1b");
    ui.screen("C settings");

    // One d shows Dev; d twice quickly starts it: trusted, so no review.
    ui.press("d");
    ui.screen("d:Dev");
    ui.screen("d d start Dev");
    ui.press("d");
    ui.press("d");
    ui.screen("repo-42");
    // An edit from outside Deckhand: the next start reviews it.
    fs::write(&file, format!("{}\n", json!({"devCommand": "echo outside-$((40+2))"}))).expect("outside edit");
    ui.press("r");
    ui.screen("reviewed first");
    ui.press("\r");
    ui.screen("devCommand: echo outside-$((40+2))");
    ui.screen("s start echo fallback-$((40+2)) instead");
    // Esc steps back to the run list, a second Esc to the session list.
    ui.press("\x1b");
    ui.screen("enter start Dev");
    ui.press("\x1b");
    ui.screen("C settings");
}
