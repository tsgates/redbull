use super::*;
use crate::agents::Detector;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp_home() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("redbull-activity-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn env_at(home: &Path) -> Env {
    Env { home: home.to_path_buf(), vars: HashMap::new() }
}

fn proc(ppid: i32, age: u64, comm: &str, cmd: &str) -> Proc {
    Proc { ppid, tty: None, cpu: 0.0, age, comm: comm.into(), cmd: cmd.into() }
}

fn table(rows: &[(i32, Proc)]) -> HashMap<i32, Proc> {
    rows.iter().cloned().collect()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn append(path: &Path, text: &str) {
    use std::io::Write;
    fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
}

fn scan(d: &mut Detector, t: &mut Tracker, env: &Env, procs: &HashMap<i32, Proc>, detail: bool) {
    let report = d.step_with(procs, 0);
    t.update(&d.agents, env, &report, procs, detail);
}

fn texts(t: &Tracker, kind: Kind) -> Vec<String> {
    t.events().iter().filter(|e| e.kind == kind).map(|e| e.text.clone()).collect()
}

fn events_of(agent: &str, line: &str) -> Vec<(Kind, String)> {
    let mut out = Vec::new();
    parse_line(agent, &json::parse(line).unwrap(), 7, &mut out, &mut Vec::new());
    out.into_iter().map(|(_, k, t)| (k, t)).collect()
}

fn files_of(agent: &str, line: &str) -> Vec<(Access, String)> {
    let mut files = Vec::new();
    parse_line(agent, &json::parse(line).unwrap(), 7, &mut Vec::new(), &mut files);
    files.into_iter().map(|(_, a, p)| (a, p)).collect()
}

#[test]
fn iso_timestamps() {
    assert_eq!(iso_secs("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(iso_secs("2000-03-01T00:00:00.000Z"), Some(951868800));
    assert_eq!(iso_secs("2026-11-19T01:02:23.589Z"), Some(1795050143));
    assert_eq!(iso_secs("yesterday"), None);
    assert_eq!(iso_secs("2026-13-01T00:00:00Z"), None);
}

#[test]
fn claude_transcript_lines() {
    let line = r#"{"type":"assistant","timestamp":"2026-11-19T01:02:23.589Z","message":{"content":[
        {"type":"thinking","thinking":"hmm"},
        {"type":"text","text":"\nRunning the tests now.\nMore."},
        {"type":"tool_use","name":"Bash","input":{"command":"cargo test\n# second line","description":"Run tests"}},
        {"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}},
        {"type":"tool_use","name":"TodoList","input":{}}]}}"#;
    let mut out = Vec::new();
    parse_line("claude", &json::parse(line).unwrap(), 7, &mut out, &mut Vec::new());
    assert_eq!(out[0], (1795050143, Kind::Reply, "Running the tests now.".into()));
    assert_eq!(out[1].2, "Bash: cargo test");
    assert_eq!(out[2].2, "Read: /src/main.rs");
    assert_eq!(out[3].2, "TodoList");

    // Prompts: typed text only — not injected context, tool results or meta.
    let user = r#"{"type":"user","message":{"content":[
        {"type":"text","text":"<system-reminder>ctx</system-reminder>"},
        {"type":"text","text":"fix the   flaky test"},
        {"type":"tool_result","content":"ok"}]}}"#;
    assert_eq!(events_of("claude", user), [(Kind::Prompt, "fix the flaky test".to_string())]);
    assert_eq!(
        events_of("claude", r#"{"type":"user","message":{"content":"hello"}}"#),
        [(Kind::Prompt, "hello".to_string())]
    );
    assert!(events_of("claude", r#"{"type":"user","isMeta":true,"message":{"content":"x"}}"#).is_empty());
    assert!(events_of("claude", r#"{"type":"api-request"}"#).is_empty());
}

#[test]
fn codex_transcript_lines() {
    let call = r#"{"timestamp":"2026-11-19T01:02:23Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"cargo build\"]}"}}"#;
    assert_eq!(events_of("codex", call), [(Kind::Tool, "shell: cargo build".to_string())]);
    let patch = r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** Update File: a.rs"}}"#;
    assert_eq!(events_of("codex", patch), [(Kind::Tool, "apply_patch: *** Begin Patch".to_string())]);
    let say = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}}"#;
    assert_eq!(events_of("codex", say), [(Kind::Reply, "Done.".to_string())]);
    let ask = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"add a flag"}]}}"#;
    assert_eq!(events_of("codex", ask), [(Kind::Prompt, "add a flag".to_string())]);
    let ctx = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>…"}]}}"#;
    assert!(events_of("codex", ctx).is_empty());
    assert!(events_of("codex", r#"{"type":"event_msg","payload":{"type":"user_message"}}"#).is_empty());
}

#[test]
fn copilot_transcript_lines() {
    let start = r#"{"type":"tool.execution_start","data":{"toolName":"grep","arguments":{"pattern":"TODO"}},"timestamp":"2026-11-14T18:38:47.458Z"}"#;
    assert_eq!(events_of("copilot", start), [(Kind::Tool, "grep: TODO".to_string())]);
    let msg = r#"{"type":"assistant.message","data":{"content":""}}"#;
    assert!(events_of("copilot", msg).is_empty());
    let user = r#"{"type":"user.message","data":{"content":"rename it"}}"#;
    assert_eq!(events_of("copilot", user), [(Kind::Prompt, "rename it".to_string())]);
}

#[test]
fn pi_transcript_lines() {
    let user = r#"{"type":"message","id":"a1","timestamp":"2026-11-19T01:02:23.000Z","message":{"role":"user","content":"Hello"}}"#;
    assert_eq!(events_of("pi", user), [(Kind::Prompt, "Hello".to_string())]);
    let asst = r#"{"type":"message","message":{"role":"assistant","content":[
        {"type":"thinking","thinking":"…"},{"type":"text","text":"On it."},
        {"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"ls -la"}}]}}"#;
    assert_eq!(
        events_of("pi", asst),
        [(Kind::Reply, "On it.".to_string()), (Kind::Tool, "bash: ls -la".to_string())]
    );
    let result = r#"{"type":"message","message":{"role":"toolResult","toolName":"bash","content":[{"type":"text","text":"out"}]}}"#;
    assert!(events_of("pi", result).is_empty());
    assert!(events_of("pi", r#"{"type":"session","version":3,"cwd":"/x"}"#).is_empty());
}

#[test]
fn file_uses_from_tool_calls() {
    use Access::{Read, Write};
    let f = |a: Access, p: &str| (a, p.to_string());
    // Claude Code: file tools carry a path; searches and shell commands don't count.
    let claude = r#"{"type":"assistant","message":{"content":[
        {"type":"tool_use","name":"Read","input":{"file_path":"/p/a.rs"}},
        {"type":"tool_use","name":"Edit","input":{"file_path":"/p/a.rs","old_string":"x","new_string":"y"}},
        {"type":"tool_use","name":"Write","input":{"file_path":"/p/new.rs","content":"…"}},
        {"type":"tool_use","name":"NotebookEdit","input":{"notebook_path":"/p/n.ipynb"}},
        {"type":"tool_use","name":"Grep","input":{"pattern":"x","path":"/p"}},
        {"type":"tool_use","name":"Bash","input":{"command":"cat /p/b.rs"}}]}}"#;
    assert_eq!(
        files_of("claude", claude),
        [f(Read, "/p/a.rs"), f(Write, "/p/a.rs"), f(Write, "/p/new.rs"), f(Write, "/p/n.ipynb")]
    );
    // Codex: patches, as a custom tool, a function call, or through the shell.
    let custom = r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"apply_patch","input":"*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** Add File: b.rs\n+z\n*** End Patch"}}"#;
    assert_eq!(files_of("codex", custom), [f(Write, "src/a.rs"), f(Write, "b.rs")]);
    let func = r#"{"type":"response_item","payload":{"type":"function_call","name":"apply_patch","arguments":"{\"input\":\"*** Begin Patch\\n*** Delete File: old.rs\\n*** End Patch\"}"}}"#;
    assert_eq!(files_of("codex", func), [f(Write, "old.rs")]);
    let shell = r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"apply_patch\",\"*** Begin Patch\\n*** Update File: c.rs\\n*** End Patch\"]}"}}"#;
    assert_eq!(files_of("codex", shell), [f(Write, "c.rs")]);
    let plain = r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"cat a.rs\"]}"}}"#;
    assert!(files_of("codex", plain).is_empty());
    // Copilot CLI and Pi.
    let view = r#"{"type":"tool.execution_start","data":{"toolName":"view","arguments":{"path":"/p/a.rs"}}}"#;
    assert_eq!(files_of("copilot", view), [f(Read, "/p/a.rs")]);
    let editor = r#"{"type":"tool.execution_start","data":{"toolName":"str_replace_editor","arguments":{"command":"str_replace","path":"/p/a.rs"}}}"#;
    assert_eq!(files_of("copilot", editor), [f(Write, "/p/a.rs")]);
    let pi = r#"{"type":"message","message":{"role":"assistant","content":[
        {"type":"toolCall","name":"read","arguments":{"path":"a.rs"}},
        {"type":"toolCall","name":"write","arguments":{"path":"b.rs","content":"…"}}]}}"#;
    assert_eq!(files_of("pi", pi), [f(Read, "a.rs"), f(Write, "b.rs")]);
}

#[test]
fn files_are_counted_per_session() {
    let mut t = Track { cwd: Some("/proj".into()), ..Track::default() };
    t.touch(10, Access::Read, "src/a.rs");
    t.touch(30, Access::Write, "/proj/src/a.rs");
    t.touch(20, Access::Read, "./b.rs");
    assert_eq!(
        t.files,
        [
            FileUse { path: "/proj/src/a.rs".into(), reads: 1, writes: 1, at: 30 },
            FileUse { path: "/proj/b.rs".into(), reads: 1, writes: 0, at: 20 },
        ]
    );
    // Over the cap, the oldest read-only file goes first; modified ones stay.
    for i in 0..MAX_FILES as u64 {
        t.touch(100 + i, Access::Read, &format!("/x/{i}"));
    }
    assert_eq!(t.files.len(), MAX_FILES);
    assert!(t.files.iter().any(|f| f.path == "/proj/src/a.rs"));
    assert!(!t.files.iter().any(|f| f.path == "/proj/b.rs" || f.path == "/x/0"));
}

#[test]
fn pi_transcript_is_found_by_its_cwd_header() {
    let home = tmp_home();
    let env = env_at(&home);
    let call = "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"name\":\"read\",\"arguments\":{\"path\":\"a.rs\"}}]}}\n";
    let dir = home.join(".pi/agent/sessions");
    write(&dir.join("--x--/1_a.jsonl"), &format!("{{\"type\":\"session\",\"cwd\":\"/x\"}}\n{call}"));
    // oh-my-pi shares the dialect, in its own directory.
    write(&home.join(".omp/agent/sessions/-y/1_b.jsonl"), &format!("{{\"type\":\"session\",\"cwd\":\"/y\"}}\n{call}"));
    let mut d = Detector::new(env.clone());
    let mut t = Tracker::new();
    let procs = table(&[(500, proc(1, 1000, "pi", "pi")), (501, proc(1, 1000, "omp", "omp"))]);
    scan(&mut d, &mut t, &env, &procs, true);
    let got: Vec<_> = t.events().iter().map(|e| (e.agent, e.text.clone())).collect();
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.contains(&("pi", "read: a.rs".to_string())) && got.contains(&("omp", "read: a.rs".to_string())));
}

/// aider (1000s old) with an MCP-style boot helper, and a shell running a
/// build that has its own children.
fn aider_tree(build: bool) -> HashMap<i32, Proc> {
    let mut rows = vec![
        (100, proc(1, 1000, "/usr/local/bin/aider", "aider")),
        (101, proc(100, 999, "node", "node /opt/mcp/server.js")),
    ];
    if build {
        rows.push((110, proc(100, 20, "/bin/zsh", "/bin/zsh -c cargo test --all")));
        rows.push((111, proc(110, 20, "/Users/u/.cargo/bin/cargo", "/Users/u/.cargo/bin/cargo test --all")));
        rows.push((112, proc(111, 9, "rustc", "rustc --crate-name x")));
    }
    table(&rows)
}

#[test]
fn running_lists_commands_not_plumbing() {
    let env = env_at(&tmp_home());
    let mut d = Detector::new(env);
    let procs = aider_tree(true);
    let report = d.step_with(&procs, 0);
    let r = running(&d.agents, &report.verdicts[0], &procs);
    assert_eq!(r, [Running { pid: 111, cmd: "cargo test --all".into(), age: 20 }]);
}

#[test]
fn process_events_for_agents_without_a_transcript() {
    let env = env_at(&tmp_home());
    let mut d = Detector::new(env.clone());
    let mut t = Tracker::new();

    // First sight is a baseline: nothing to report for a long-running session.
    let procs = aider_tree(false);
    scan(&mut d, &mut t, &env, &procs, false);
    assert!(t.events().is_empty());

    let procs = aider_tree(true);
    let report = d.step_with(&procs, 0);
    t.update(&d.agents, &env, &report, &procs, false);
    assert_eq!(texts(&t, Kind::Proc), ["cargo test --all"]);
    // …reported once, not on every scan.
    scan(&mut d, &mut t, &env, &procs, false);
    assert_eq!(texts(&t, Kind::Proc).len(), 1);

    let snap = json::parse(&t.snapshot(&report, true, false)).expect("snapshot is valid JSON");
    let s = &snap.get("sessions").arr()[0];
    assert_eq!(s.get("agent").str(), Some("aider"));
    assert_eq!(s.get("running").arr()[0].get("cmd").str(), Some("cargo test --all"));
    // The command, plus the session going to work (a tool call in progress).
    let kinds: Vec<_> = snap.get("events").arr().iter().filter_map(|e| e.get("k").str()).collect();
    assert_eq!(kinds, ["state", "proc"]);

    let none = table(&[]);
    scan(&mut d, &mut t, &env, &none, false);
    assert_eq!(texts(&t, Kind::State), ["working · running a tool", "exited"]);
    assert_eq!(t.events().last().unwrap().agent, "aider");
}

#[test]
fn state_events_follow_the_detector() {
    let home = tmp_home();
    let env = env_at(&home);
    let status = home.join(".claude/sessions/200.json");
    let mut d = Detector::new(env.clone());
    let mut t = Tracker::new();
    let procs = table(&[(200, proc(1, 5, "claude", "claude"))]);

    write(&status, r#"{"pid":200,"status":"busy"}"#);
    scan(&mut d, &mut t, &env, &procs, false);
    assert_eq!(texts(&t, Kind::State), ["started", "working · reports busy"]);
}

#[test]
fn claude_transcript_is_followed() {
    let home = tmp_home();
    let env = env_at(&home);
    write(
        &home.join(".claude/sessions/300.json"),
        r#"{"pid":300,"sessionId":"abc","cwd":"/Users/u/proj","name":"Fix the bug","status":"idle"}"#,
    );
    let log = home.join(".claude/projects/-Users-u-proj/abc.jsonl");
    let tool = |cmd: &str| {
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"2026-11-19T01:02:23Z\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{{\"command\":\"{cmd}\"}}}}]}}}}\n"
        )
    };
    let file = |name: &str, path: &str| {
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":\"2026-11-19T01:02:23Z\",\"message\":{{\"content\":[{{\"type\":\"tool_use\",\"name\":\"{name}\",\"input\":{{\"file_path\":\"{path}\"}}}}]}}}}\n"
        )
    };
    // A file read before the history shown in the feed still counts.
    let old: String = std::iter::once(file("Read", "/Users/u/proj/src/lib.rs"))
        .chain((0..SEED_EVENTS + 5).map(|i| tool(&format!("old {i}"))))
        .collect();
    write(&log, &old);

    let mut d = Detector::new(env.clone());
    let mut t = Tracker::new();
    let procs = table(&[
        (300, proc(1, 1000, "claude", "claude")),
        (301, proc(300, 10, "/bin/zsh", "zsh -c make")),
        (302, proc(301, 10, "make", "make")),
    ]);
    let step = |d: &mut Detector, t: &mut Tracker| {
        let report = d.step_with(&procs, 0);
        t.update(&d.agents, &env, &report, &procs, true);
        report
    };

    // Opening the window shows the most recent history…
    step(&mut d, &mut t);
    let tools = texts(&t, Kind::Tool);
    assert_eq!(tools.len(), SEED_EVENTS);
    assert_eq!(tools.last().unwrap(), &format!("Bash: old {}", SEED_EVENTS + 4));

    // …then only what's appended, including a line written in two pieces.
    append(&log, &tool("cargo test"));
    let half = tool("git status");
    append(&log, &half[..40]);
    step(&mut d, &mut t);
    assert_eq!(texts(&t, Kind::Tool).last().unwrap(), "Bash: cargo test");
    append(&log, &half[40..]);
    append(&log, &file("Edit", "src/lib.rs"));
    let report = step(&mut d, &mut t);
    assert_eq!(texts(&t, Kind::Tool).pop().unwrap(), "Edit: src/lib.rs");
    t.events.pop_back();
    assert_eq!(texts(&t, Kind::Tool).last().unwrap(), "Bash: git status");
    assert_eq!(texts(&t, Kind::Tool).len(), SEED_EVENTS + 2);
    // Tool calls come from the transcript, so the processes aren't repeated.
    assert!(texts(&t, Kind::Proc).is_empty());

    let snap = json::parse(&t.snapshot(&report, false, false)).unwrap();
    let s = &snap.get("sessions").arr()[0];
    assert_eq!(s.get("title").str(), Some("Fix the bug"));
    assert_eq!(s.get("cwd").str(), Some("/Users/u/proj"));
    assert_eq!(s.get("log"), &Value::Bool(true));
    assert_eq!(s.get("running").arr()[0].get("cmd").str(), Some("make"));
    let files = s.get("files").arr();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].get("p").str(), Some("/Users/u/proj/src/lib.rs"));
    assert_eq!((files[0].get("r"), files[0].get("w")), (&Value::Num(1.0), &Value::Num(1.0)));
}

#[test]
fn copilot_transcript_is_found_by_its_lock() {
    let home = tmp_home();
    let env = env_at(&home);
    let dir = home.join(".copilot/session-state/s1");
    write(&dir.join("inuse.400.lock"), "400");
    write(
        &dir.join("events.jsonl"),
        "{\"type\":\"tool.execution_start\",\"data\":{\"toolName\":\"view\",\"arguments\":{\"path\":\"a.rs\"}}}\n",
    );
    write(&home.join(".copilot/session-state/other/events.jsonl"), "{}\n");
    let mut d = Detector::new(env.clone());
    let mut t = Tracker::new();
    let procs = table(&[(400, proc(1, 1000, "copilot", "copilot"))]);
    scan(&mut d, &mut t, &env, &procs, true);
    assert_eq!(texts(&t, Kind::Tool), ["view: a.rs"]);
}

#[test]
fn tail_skips_ahead_when_far_behind() {
    let home = tmp_home();
    let path = home.join("big.jsonl");
    let line = format!("{{\"pad\":\"{}\"}}\n", "x".repeat(1000));
    write(&path, &line.repeat(2000)); // ~2 MB
    let mut tail = Tail { path: path.clone(), offset: 0, seeded: false };
    let seed = tail.read_new();
    assert!(!seed.is_empty() && seed.len() <= (SEED_BYTES as usize / line.len()));
    assert!(seed.iter().all(|l| json::parse(l).is_some()), "no partial lines");
    assert!(tail.read_new().is_empty());
    append(&path, "{\"new\":1}\n");
    assert_eq!(tail.read_new(), ["{\"new\":1}"]);
}

#[test]
fn feed_is_bounded() {
    let mut t = Tracker::new();
    for i in 0..MAX_EVENTS + 50 {
        t.push(1, "claude", i as u64, Kind::Tool, "x".into());
    }
    assert_eq!(t.events().len(), MAX_EVENTS);
    assert_eq!(t.events()[0].at, 50);
}
