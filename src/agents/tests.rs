// Tests for agent detection. Process samples are the real `comm` / command
// lines observed (or documented) for each agent's usual install on macOS;
// store paths are where each agent writes its session files.

use super::*;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

fn name_of(comm: &str, cmd: &str) -> Option<&'static str> {
    let agents = registry();
    identify(&agents, comm, cmd).map(|i| agents[i].name)
}

#[track_caller]
fn assert_id(agent: &str, comm: &str, cmd: &str) {
    assert_eq!(name_of(comm, cmd), Some(agent), "comm={comm:?} cmd={cmd:?}");
}

const NM: &str = "/opt/homebrew/lib/node_modules";

// --- identification: one test per agent ---------------------------------------

#[test]
fn id_claude() {
    assert_id("claude", "claude", "claude"); // native install, run from PATH
    let v = "/Users/u/.local/share/claude/versions/2.1.220";
    assert_id("claude", v, &format!("{v} --resume")); // run by full path
    let d = "/Users/u/Library/Application Support/Claude/claude-code/2.1.288/48d5/claude.app/Contents/MacOS/claude";
    assert_id("claude", d, d); // Claude desktop app's Code sessions
    assert_id("claude", "node", &format!("node {NM}/@anthropic-ai/claude-code/cli.js"));
    assert_id("claude", "node", "claude"); // npm build with process.title
}

#[test]
fn id_codex() {
    let v = "/x/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex";
    assert_id("codex", v, v);
    assert_id("codex", "node", "node /opt/homebrew/bin/codex");
    assert_id("codex", "/opt/homebrew/bin/codex", "codex");
}

#[test]
fn id_copilot() {
    assert_id("copilot", "/opt/homebrew/bin/copilot", "/opt/homebrew/bin/copilot --acp");
    assert_id("copilot", "node", &format!("node {NM}/@github/copilot/index.js"));
}

#[test]
fn id_gemini() {
    assert_id("gemini", "node", "node ./node_modules/.bin/gemini");
    let n = "/opt/homebrew/Cellar/node/26.7.0/bin/node";
    assert_id("gemini", n, &format!("{n} --max-old-space-size=12288 /x/node_modules/.bin/gemini"));
    assert_id("gemini", "node", &format!("node {NM}/@google/gemini-cli/bundle/gemini.js"));
}

#[test]
fn id_opencode() {
    assert_id("opencode", "./node_modules/.bin/opencode", "./node_modules/.bin/opencode");
    assert_id("opencode", "/x/node_modules/opencode-ai/bin/opencode.exe", "opencode.exe");
    assert_id("opencode", "/opt/homebrew/bin/opencode", "opencode");
}

#[test]
fn id_cursor_agent() {
    let d = "/Users/u/.local/share/cursor-agent/versions/2026.01.28-abc";
    assert_id("cursor-agent", &format!("{d}/node"), &format!("{d}/node --use-system-ca {d}/index.js"));
    assert_id("cursor-agent", "cursor-agent", "cursor-agent");
}

#[test]
fn id_aider() {
    let py = "/Users/u/.local/pipx/venvs/aider-chat/bin/python";
    assert_id("aider", py, &format!("{py} /Users/u/.local/bin/aider --model sonnet"));
    // Homebrew's framework Python is "Python" (capitalized).
    let fw = "/opt/homebrew/Cellar/python@3.12/3.12.4/Frameworks/Python.framework/Versions/3.12/Resources/Python.app/Contents/MacOS/Python";
    assert_id("aider", fw, &format!("{fw} /opt/homebrew/bin/aider"));
}

#[test]
fn id_goose() {
    assert_id("goose", "/Users/u/.local/bin/goose", "goose session");
}

#[test]
fn id_amp() {
    assert_id("amp", "/Users/u/.amp/bin/amp", "amp");
    assert_id("amp", "./node_modules/.bin/amp", "./node_modules/.bin/amp");
    assert_id("amp", "node", &format!("node {NM}/@sourcegraph/amp/dist/main.js"));
}

#[test]
fn id_qwen() {
    assert_id("qwen", "node", "node ./node_modules/.bin/qwen");
    assert_id("qwen", "node", &format!("node {NM}/@qwen-code/qwen-code/scripts/cli-entry.js"));
}

#[test]
fn id_crush() {
    let v = "/x/node_modules/@charmland/crush/bin/crush";
    assert_id("crush", v, v);
    assert_id("crush", "/opt/homebrew/bin/crush", "crush");
}

#[test]
fn id_droid() {
    assert_id("droid", "/Users/u/.local/bin/droid", "droid");
}

#[test]
fn id_auggie() {
    assert_id("auggie", "node", "node ./node_modules/.bin/auggie");
    assert_id("auggie", "node", &format!("node {NM}/@augmentcode/auggie/augment.mjs"));
}

#[test]
fn id_kiro() {
    assert_id("kiro", "/Users/u/.local/bin/kiro-cli-chat", "kiro-cli-chat chat");
    assert_id("kiro", "/Users/u/.local/bin/kiro-cli", "kiro-cli chat");
    assert_id("kiro", "/Users/u/.local/bin/qchat", "qchat chat"); // Amazon Q
}

#[test]
fn id_cline() {
    assert_id("cline", "./node_modules/.bin/cline", "./node_modules/.bin/cline");
    let v = "/x/node_modules/@cline/cli-darwin-arm64/bin/cline";
    assert_id("cline", v, &format!("{v} --cline-hub-daemon --cwd /x"));
}

#[test]
fn id_kilo() {
    let v = "/x/node_modules/@kilocode/cli/bin/.kilo";
    assert_id("kilo", v, v);
    assert_id("kilo", "node", "node ./node_modules/.bin/kilo");
}

#[test]
fn id_openhands() {
    let py = "/Users/u/.local/share/uv/tools/openhands/bin/python";
    assert_id("openhands", py, &format!("{py} /Users/u/.local/bin/openhands"));
}

#[test]
fn id_continue() {
    assert_id("continue", "node", "node ./node_modules/.bin/cn");
    assert_id("continue", "node", &format!("node {NM}/@continuedev/cli/dist/cn.js"));
}

#[test]
fn id_editors() {
    let c = "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)";
    assert_id("vscode", c, c);
    let c = "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Plugin).app/Contents/MacOS/Cursor Helper (Plugin)";
    assert_id("cursor", c, c);
    let c = "/Applications/Windsurf.app/Contents/Frameworks/Windsurf Helper (Plugin).app/Contents/MacOS/Windsurf Helper (Plugin)";
    assert_id("windsurf", c, c);
    assert_id("zed", "/Applications/Zed.app/Contents/MacOS/zed", "zed");
}

#[test]
fn id_negatives() {
    for (comm, cmd) in [
        ("vim", "vim /Users/u/claude/notes.md"),
        ("node", "node /x/node_modules/.bin/vite --config claude.config.js"),
        ("/bin/zsh", "/bin/zsh -c claude --help"),
        ("python3", "python3 -m http.server"),
        ("/Applications/Claude.app/Contents/MacOS/Claude", "/Applications/Claude.app/Contents/MacOS/Claude"),
        ("Claude Helper", "Claude Helper --type=renderer"),
        ("/usr/bin/less", "less /x/copilot/README"),
        ("node", "node"),
    ] {
        assert_eq!(name_of(comm, cmd), None, "comm={comm:?} cmd={cmd:?}");
    }
}

#[test]
fn every_agent_has_an_identification_test() {
    // Guard: adding an agent to the registry without a test above should fail.
    let tested = [
        "claude", "codex", "copilot", "gemini", "opencode", "cursor-agent", "aider", "goose",
        "amp", "qwen", "crush", "droid", "auggie", "kiro", "cline", "kilo", "openhands",
        "continue", "gptme", "plandex", "codebuff", "vscode", "cursor", "windsurf", "zed",
    ];
    for a in registry() {
        assert!(tested.contains(&a.name), "no test for {}", a.name);
    }
    assert_id("gptme", "/Users/u/.local/bin/gptme", "gptme");
    assert_id("plandex", "/usr/local/bin/plandex", "plandex");
    assert_id("codebuff", "node", &format!("node {NM}/codebuff/dist/index.js"));
}

// --- process trees -------------------------------------------------------------

fn proc(ppid: i32, comm: &str, cmd: &str) -> Proc {
    Proc { ppid, tty: None, cpu: 0.0, age: 1000, comm: comm.into(), cmd: cmd.into() }
}

fn table(rows: &[(i32, Proc)]) -> HashMap<i32, Proc> {
    rows.iter().cloned().collect()
}

#[test]
fn wrapper_and_native_binary_are_one_session() {
    let codex = "/x/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex";
    let t = table(&[
        (10, proc(1, "zsh", "-zsh")),
        (11, proc(10, "node", "node /opt/homebrew/bin/codex")),
        (12, proc(11, codex, codex)),
        (13, proc(12, "/bin/zsh", "/bin/zsh -c cargo test")),
    ]);
    let a = registry();
    let i = instances(&a, &t, 0);
    assert_eq!(i.len(), 1);
    assert_eq!(i[0].pid, 11);
    assert_eq!(i[0].pids.len(), 3); // wrapper, binary, tool shell
}

#[test]
fn gemini_relaunch_child_is_same_session() {
    let t = table(&[
        (20, proc(1, "node", "node /opt/homebrew/bin/gemini")),
        (21, proc(20, "node", "node --max-old-space-size=8192 /opt/homebrew/bin/gemini")),
    ]);
    assert_eq!(instances(&registry(), &t, 0).len(), 1);
}

#[test]
fn separate_sessions_are_counted_separately() {
    let t = table(&[
        (30, proc(1, "claude", "claude")),
        (31, proc(1, "claude", "claude")),
        (32, proc(1, "/opt/homebrew/bin/copilot", "copilot")),
    ]);
    assert_eq!(instances(&registry(), &t, 0).len(), 3);
}

#[test]
fn cli_agent_inside_editor_is_its_own_session() {
    let host = "Code Helper (Plugin)";
    let t = table(&[
        (40, proc(1, host, host)),
        (41, proc(40, "/x/anthropic.claude-code/resources/native-binary/claude", "claude")),
        (42, proc(40, "/x/kilocode/bin/.kilo", ".kilo serve --port 0")),
    ]);
    let a = registry();
    let mut names: Vec<_> = instances(&a, &t, 0).iter().map(|i| a[i.agent].name).collect();
    names.sort();
    assert_eq!(names, ["claude", "kilo", "vscode"]);
}

#[test]
fn agent_run_as_a_tool_belongs_to_its_parent() {
    let t = table(&[
        (50, proc(1, "claude", "claude")),
        (51, proc(50, "/opt/homebrew/bin/codex", "codex exec 'review'")),
    ]);
    let i = instances(&registry(), &t, 0);
    assert_eq!(i.len(), 1);
    assert_eq!(i[0].pid, 50);
}

#[test]
fn ignores_self() {
    let t = table(&[(60, proc(1, "claude", "claude"))]);
    assert!(instances(&registry(), &t, 60).is_empty());
}

// --- signals -----------------------------------------------------------------

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// A throwaway home directory.
fn tmp_home() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("redbull-test-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn env_at(home: &Path) -> Env {
    Env { home: home.to_path_buf(), vars: HashMap::new() }
}

/// Create `path` (and parents), with its mtime `age_secs` in the past.
fn touch(path: &Path, age_secs: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let f = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    let t = SystemTime::now() - Duration::from_secs(age_secs);
    f.set_modified(t).unwrap();
}

/// Two detector samples over `t`, running `between` in the gap (a store
/// counts only when it changes while watched).
fn samples(env: Env, t: &HashMap<i32, Proc>, between: impl FnOnce()) -> Report {
    let mut d = Detector::new(env);
    d.step_with(t, 0);
    between();
    d.step_with(t, 0)
}

/// `samples` over a one-process table.
fn sample(env: Env, comm: &str, pid: i32, between: impl FnOnce()) -> Verdict {
    let r = samples(env, &table(&[(pid, proc(1, comm, comm))]), between);
    assert_eq!(r.verdicts.len(), 1, "not identified: {comm}");
    r.verdicts[0].clone()
}

/// For each agent: its session file at the real location makes it "working"
/// while being appended to, and "idle" when written only once or stale.
#[track_caller]
fn check_store(comm: &str, _cmd: &str, rel: &str) {
    let home = tmp_home();
    let f = home.join(rel);
    touch(&f, 30);
    let v = sample(env_at(&home), comm, 7001, || touch(&f, 0));
    assert_eq!(v.reason, Some(Reason::Store), "{comm}: appending to {rel} should mean working");
    assert!(v.working);

    touch(&f, 2); // fresh, but not written again: a one-off (boot/exit) write
    let v = sample(env_at(&home), comm, 7002, || {});
    assert_eq!(v.reason, None, "{comm}: a single write to {rel} should not count");

    touch(&f, STORE_FRESH_SECS + 60);
    let v = sample(env_at(&home), comm, 7003, || {});
    assert_eq!(v.reason, None, "{comm}: stale {rel} should mean idle");
    assert!(!v.working);
    let _ = fs::remove_dir_all(home);
}

#[test]
fn store_claude() {
    check_store("claude", "claude", ".claude/projects/-Users-u-proj/1b2c.jsonl");
}
#[test]
fn store_codex() {
    check_store("codex", "codex", ".codex/sessions/2026/11/01/rollout-2026-11-01T10-00-00-abc.jsonl");
}
#[test]
fn store_copilot() {
    check_store("copilot", "copilot", ".copilot/session-state/9303f4fc/events.jsonl");
}
#[test]
fn store_gemini() {
    check_store("gemini", "gemini", ".gemini/tmp/proj/chats/session-2026-11-01T10-00-abcd1234.jsonl");
}
#[test]
fn store_opencode() {
    check_store("opencode", "opencode", ".local/share/opencode/opencode.db-wal");
}
#[test]
fn store_cursor_agent() {
    check_store("cursor-agent", "cursor-agent", ".cursor/chats/0cc175b9/5e7a/store.db-wal");
}
#[test]
fn store_goose() {
    check_store("goose", "goose", ".local/share/goose/sessions/sessions.db-wal");
}
#[test]
fn store_amp() {
    check_store("amp", "amp", ".local/share/amp/threads/T-1234.json");
}
#[test]
fn store_qwen() {
    check_store("qwen", "qwen", ".qwen/projects/-Users-u-proj/chats/138a222c.jsonl");
}
#[test]
fn store_droid() {
    check_store("droid", "droid", ".factory/sessions/-Users-u-proj/77aa.jsonl");
}
#[test]
fn store_auggie() {
    check_store("auggie", "auggie", ".augment/sessions/abcd.json");
}
#[test]
fn store_kiro() {
    check_store("kiro-cli-chat", "kiro-cli-chat", "Library/Application Support/kiro-cli/data.sqlite3-wal");
    check_store("kiro-cli-chat", "kiro-cli-chat", ".kiro/sessions/ws1/sess_1/messages.jsonl");
    check_store("qchat", "qchat", "Library/Application Support/amazon-q/data.sqlite3");
}
#[test]
fn store_cline() {
    check_store("cline", "cline", ".cline/data/sessions/s1/s1.messages.json");
}
#[test]
fn store_kilo() {
    check_store(".kilo", ".kilo", ".local/share/kilo/kilo.db-wal");
}
#[test]
fn store_openhands() {
    check_store("openhands", "openhands", ".openhands/conversations/ab12/events/event-00001.json");
}
#[test]
fn store_continue() {
    check_store("cn", "cn", ".continue/sessions/s1.json");
}
#[test]
fn store_gptme() {
    check_store("gptme", "gptme", ".local/share/gptme/logs/2026-11-01-x/conversation.jsonl");
}
#[test]
fn store_vscode() {
    let h = "Code Helper (Plugin)";
    let u = "Library/Application Support/Code/User";
    check_store(h, h, &format!("{u}/workspaceStorage/a1b2/chatSessions/5f1e.json")); // Copilot
    check_store(h, h, &format!("{u}/globalStorage/saoudrizwan.claude-dev/tasks/17/ui_messages.json"));
    check_store(h, h, &format!("{u}/globalStorage/rooveterinaryinc.roo-cline/tasks/17/ui_messages.json"));
    check_store(h, h, ".cline/data/sessions/s1/s1.messages.json"); // Cline v3 extension
}
#[test]
fn store_cursor() {
    let h = "Cursor Helper (Plugin)";
    check_store(h, h, "Library/Application Support/Cursor/User/globalStorage/state.vscdb");
}
#[test]
fn store_windsurf() {
    let h = "Windsurf Helper (Plugin)";
    check_store(h, h, ".codeium/windsurf/cascade/c1.pb");
}
#[test]
fn store_zed() {
    check_store("zed", "zed", "Library/Application Support/Zed/threads/threads.db");
}

/// Project-local stores are found through the process's real cwd, so these
/// use a real (sleeping) process whose cwd is the project.
#[track_caller]
fn check_cwd_store(comm: &str, rel: &str, noise: &str) {
    let proj = tmp_home();
    let mut child = Command::new("sleep").arg("30").current_dir(&proj).spawn().unwrap();
    let pid = child.id() as i32;
    let (f, n) = (proj.join(rel), proj.join(noise));

    touch(&n, 30);
    let v = sample(env_at(&tmp_home()), comm, pid, || touch(&n, 0));
    assert_eq!(v.reason, None, "{comm}: {noise} should not count");

    touch(&f, 30);
    let v = sample(env_at(&tmp_home()), comm, pid, || touch(&f, 0));
    assert_eq!(v.reason, Some(Reason::Store), "{comm}: appending to {rel} should mean working");

    touch(&f, STORE_FRESH_SECS + 60);
    let v = sample(env_at(&tmp_home()), comm, pid, || {});
    assert_eq!(v.reason, None, "{comm}: stale {rel} should mean idle");
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_dir_all(proj);
}

#[test]
fn store_aider() {
    check_cwd_store("aider", ".aider.chat.history.md", ".aider.tags.cache.v4/cache.db");
}
#[test]
fn store_crush() {
    check_cwd_store("crush", ".crush/crush.db-wal", ".crush/logs/crush.log");
}

#[test]
fn idle_noise_is_not_activity() {
    // Files agents touch while idle (observed): logs, telemetry, other DBs.
    for (comm, rel) in [
        ("codex", ".codex/logs_2.sqlite-wal"),
        ("codex", ".codex/state_5.sqlite-wal"),
        ("opencode", ".local/share/opencode/log/2026-11-01.log"),
        ("kilo", ".local/share/kilo/telemetry-id"),
        ("qwen", ".qwen/tmp/0d4d/logs.json"),
        ("gemini", ".gemini/tmp/proj/.project_root"),
        ("gemini", ".gemini/tmp/proj/logs.json"),
        ("Code Helper (Plugin)", "Library/Application Support/Code/User/workspaceStorage/a1b2/state.vscdb"),
    ] {
        let home = tmp_home();
        let f = home.join(rel);
        touch(&f, 30);
        let v = sample(env_at(&home), comm, 7100, || touch(&f, 0));
        assert_eq!(v.reason, None, "{comm}: {rel} is idle noise");
        let _ = fs::remove_dir_all(home);
    }
}

#[test]
fn env_overrides_store_location() {
    let home = tmp_home();
    let alt = tmp_home();
    let f = alt.join("sessions/2026/11/01/rollout-x.jsonl");
    touch(&f, 30);
    let mut env = env_at(&home);
    env.vars.insert("CODEX_HOME".into(), alt.to_string_lossy().into());
    assert_eq!(sample(env, "codex", 7200, || touch(&f, 0)).reason, Some(Reason::Store));
}

#[test]
fn startup_writes_are_ignored() {
    // kilo/opencode write their DB while booting, sometimes twice.
    let home = tmp_home();
    let f = home.join(".local/share/kilo/kilo.db-wal");
    touch(&f, 1);
    let mut p = proc(1, ".kilo", ".kilo");
    p.age = 2; // just launched
    let r = samples(env_at(&home), &table(&[(7300, p)]), || touch(&f, 0));
    assert_eq!(r.verdicts[0].reason, None);
}

#[test]
fn new_session_counts_once_it_grows() {
    let home = tmp_home();
    let f = home.join(".codex/sessions/2026/11/01/rollout-new.jsonl");
    let mut d = Detector::new(env_at(&home));
    let t = table(&[(7400, proc(1, "codex", "codex"))]);
    d.step_with(&t, 0);
    touch(&f, 1); // first turn creates the file: not yet distinguishable from boot
    assert_eq!(d.step_with(&t, 0).verdicts[0].reason, None);
    touch(&f, 0); // appended again: real work
    assert_eq!(d.step_with(&t, 0).verdicts[0].reason, Some(Reason::Store));
}

fn write_claude_status(home: &Path, pid: i32, status: &str) {
    let dir = home.join(".claude/sessions");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{pid}.json")),
        format!(r#"{{"pid":{pid},"sessionId":"x","kind":"interactive","status":"{status}","updatedAt":1}}"#),
    )
    .unwrap();
}

#[test]
fn session_with_owner_marker_goes_to_its_owner() {
    // Copilot CLI drops inuse.<pid>.lock in the session it's using. A fresh
    // session is credited to its owner, not to another idle copilot.
    let home = tmp_home();
    let s = home.join(".copilot/session-state/s1");
    let ev = s.join("events.jsonl");
    touch(&ev, 30);
    touch(&s.join("inuse.8502.lock"), 30);
    let t = table(&[
        (8501, proc(1, "copilot", "copilot --acp")), // older, idle
        (8502, proc(1, "copilot", "copilot -p fix")),
    ]);
    let r = samples(env_at(&home), &t, || touch(&ev, 0));
    let by_pid: HashMap<i32, &Verdict> = r.verdicts.iter().map(|v| (v.pid, v)).collect();
    assert_eq!(by_pid[&8501].reason, None);
    assert_eq!(by_pid[&8502].reason, Some(Reason::Store));

    // Once the owner exits, its session must not be credited to the other.
    let t = table(&[(8501, proc(1, "copilot", "copilot --acp"))]);
    let r = samples(env_at(&home), &t, || touch(&ev, 0));
    assert_eq!(r.verdicts[0].reason, None);
}

#[test]
fn claude_self_reported_status_wins() {
    let home = tmp_home();
    write_claude_status(&home, 8001, "busy");
    write_claude_status(&home, 8002, "idle");
    write_claude_status(&home, 8003, "waiting");
    // A growing transcript must not make the idle sessions look busy.
    let f = home.join(".claude/projects/p/s.jsonl");
    touch(&f, 30);
    let t = table(&[
        (8001, proc(1, "claude", "claude")),
        (8002, proc(1, "claude", "claude")),
        (8003, proc(1, "claude", "claude")),
    ]);
    let r = samples(env_at(&home), &t, || touch(&f, 0));
    let by_pid: HashMap<i32, &Verdict> = r.verdicts.iter().map(|v| (v.pid, v)).collect();
    assert_eq!(by_pid[&8001].reason, Some(Reason::Status));
    assert_eq!(by_pid[&8002].reason, None);
    assert_eq!(by_pid[&8003].reason, None);
    assert_eq!(r.working(), 1);
}

#[test]
fn shared_store_marks_only_as_many_sessions_as_were_written() {
    let home = tmp_home();
    let dir = home.join(".codex/sessions/2026/11/01");
    let (a, b) = (dir.join("rollout-a.jsonl"), dir.join("rollout-b.jsonl"));
    touch(&a, 30);
    touch(&b, 30);
    let mut d = Detector::new(env_at(&home));
    let t = table(&[
        (8101, proc(1, "codex", "codex")),
        (8102, proc(1, "codex", "codex")),
        (8103, proc(1, "codex", "codex")),
    ]);
    d.step_with(&t, 0);
    touch(&a, 0);
    assert_eq!(d.step_with(&t, 0).working(), 1);
    touch(&b, 0);
    assert_eq!(d.step_with(&t, 0).working(), 2);
}

#[test]
fn ambiguous_session_goes_to_instance_in_that_project() {
    // Two claude CLIs without status files; the active transcript is under
    // the project dir of the *younger* one (a real process, for its cwd).
    let home = tmp_home();
    let proj = fs::canonicalize(tmp_home()).unwrap();
    let mut child = Command::new("sleep").arg("30").current_dir(&proj).spawn().unwrap();
    let pid = child.id() as i32;
    let enc = proj.to_string_lossy().replace(['/', '.'], "-");
    let f = home.join(".claude/projects").join(enc).join("s.jsonl");
    touch(&f, 30);
    let mut young = proc(1, "claude", "claude");
    young.age = 60;
    let t = table(&[(8601, proc(1, "claude", "claude")), (pid, young)]);
    let r = samples(env_at(&home), &t, || touch(&f, 0));
    let by_pid: HashMap<i32, &Verdict> = r.verdicts.iter().map(|v| (v.pid, v)).collect();
    assert_eq!(by_pid[&pid].reason, Some(Reason::Store));
    assert_eq!(by_pid[&8601].reason, None);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cpu_counts_only_above_threshold() {
    let mut d = Detector::new(env_at(&tmp_home()));
    let mut agent = proc(1, "goose", "goose");
    let mut tool = proc(8200, "cargo", "cargo build"); // child of the agent
    let snap = |a: &Proc, t: &Proc| table(&[(8200, a.clone()), (8201, t.clone())]);
    d.step_with(&snap(&agent, &tool), 0); // baseline
    std::thread::sleep(Duration::from_millis(300));
    agent.cpu += 0.003; // ~0.01 cores: an idle TUI
    let r = d.step_with(&snap(&agent, &tool), 0);
    assert_eq!(r.verdicts[0].reason, None, "idle CPU must not count");
    std::thread::sleep(Duration::from_millis(300));
    tool.cpu += 0.3; // a build in the agent's subtree: ~1 core
    let r = d.step_with(&snap(&agent, &tool), 0);
    assert_eq!(r.verdicts[0].reason, Some(Reason::Cpu));
}

#[test]
fn tool_child_counts_but_boot_children_and_old_ones_dont() {
    let mut agent = proc(1, "/opt/homebrew/bin/copilot", "copilot");
    agent.age = 1000;
    let child = |age: u64, cmd: &str| Proc { age, ppid: 8700, ..proc(8700, cmd, cmd) };
    let run = |kids: Vec<(i32, Proc)>| {
        let mut rows = vec![(8700, agent.clone())];
        rows.extend(kids);
        let mut d = Detector::new(env_at(&tmp_home()));
        d.step_with(&table(&rows), 0).verdicts[0].reason
    };
    // MCP servers started with the agent: idle.
    assert_eq!(run(vec![(8701, child(998, "node mcp-server.js")), (8702, child(1000, "uvx mcp"))]), None);
    // A tool call in progress: working, even with no CPU or output.
    assert_eq!(run(vec![(8703, child(12, "sleep 20"))]), Some(Reason::Tool));
    // A background dev server from long ago: idle.
    assert_eq!(run(vec![(8704, child(TOOL_MAX_SECS + 5, "node vite"))]), None);
    // Momentary polls (copilot runs `gh auth token` while logged out): idle.
    assert_eq!(run(vec![(8705, child(0, "gh"))]), None);
    // The agent's own daemon forked later (cline hub): idle.
    let hub = "/x/node_modules/@cline/cli-darwin-arm64/bin/cline";
    assert_eq!(run(vec![(8706, child(30, hub))]), None);
    // Interactive sessions report tool runs on their tty; child boot noise
    // (codex's startup `git` plugin sync) must not count for them.
    let mut tui = agent.clone();
    tui.tty = Some("ttys999".into());
    let rows = vec![(8700, tui), (8707, child(12, "git ls-remote"))];
    let mut d = Detector::new(env_at(&tmp_home()));
    assert_eq!(d.step_with(&table(&rows), 0).verdicts[0].reason, None);
    // Editors spawn helpers all the time: never a tool signal.
    let h = "Code Helper (Plugin)";
    let rows = vec![(8710, Proc { age: 1000, ..proc(1, h, h) }), (8711, Proc { age: 5, ..proc(8710, "rg", "rg") })];
    let mut d = Detector::new(env_at(&tmp_home()));
    assert_eq!(d.step_with(&table(&rows), 0).verdicts[0].reason, None);
}

#[test]
fn editor_cpu_is_ignored() {
    let h = "Code Helper (Plugin)";
    let mut d = Detector::new(env_at(&tmp_home()));
    let mut p = proc(1, h, h);
    d.step_with(&table(&[(8300, p.clone())]), 0);
    std::thread::sleep(Duration::from_millis(200));
    p.cpu += 1.0;
    let r = d.step_with(&table(&[(8300, p)]), 0);
    assert_eq!(r.verdicts[0].reason, None);
}

#[test]
fn working_holds_briefly_then_grace_keeps_awake() {
    let home = tmp_home();
    let f = home.join(".codex/sessions/2026/11/01/rollout-a.jsonl");
    touch(&f, 30);
    let mut d = Detector::new(env_at(&home));
    let t = table(&[(8401, proc(1, "codex", "codex"))]);
    d.step_with(&t, 0);
    touch(&f, 0);
    assert!(d.step_with(&t, 0).verdicts[0].working);
    touch(&f, STORE_FRESH_SECS + 60); // went quiet
    let r = d.step_with(&t, 0);
    assert_eq!(r.verdicts[0].reason, None);
    assert!(r.verdicts[0].working, "held for HOLD_SECS");
    assert!(d.keep_awake());
}

#[test]
fn nothing_running_means_nothing_working() {
    let mut d = Detector::new(env_at(&tmp_home()));
    let r = d.step_with(&table(&[(1, proc(0, "launchd", "launchd"))]), 0);
    assert_eq!(r.working(), 0);
    assert!(!d.keep_awake());
}

#[test]
fn parses_ps_times() {
    assert_eq!(parse_time("0:01.50"), 1.5);
    assert_eq!(parse_time("01:02:03"), 3723.0);
    assert_eq!(parse_time("2-00:00:01"), 172801.0);
}

#[test]
fn json_field_lookup() {
    let s = r#"{"pid":1,"status" : "busy","name":"x"}"#;
    assert_eq!(json_str(s, "status"), Some("busy"));
    assert_eq!(json_str(s, "missing"), None);
}
