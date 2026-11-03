// Coding-agent detection: which agents are running, and which are *working*.
//
// Each running agent is an *instance* (one process tree, i.e. one session).
// An instance counts as working when any of these fire, strongest first:
//
//   1. status  — the agent publishes its own state (Claude Code writes
//                `~/.claude/sessions/<pid>.json` with "status":"busy"/"idle").
//   2. store   — its session transcript / database keeps being written.
//                Agents append on every message and tool call, and leave it
//                alone while waiting for the user. A session counts only once
//                it changes *while watched*, so one-off writes at boot or
//                exit don't look like work.
//   3. tty     — its terminal got output recently (spinners, streaming).
//   4. cpu     — its process subtree is burning real CPU (builds, tests the
//                agent runs). The threshold sits well above idle TUIs.
//   5. tool    — (headless sessions: `-p`/`exec` runs, desktop/IDE-hosted;
//                interactive ones show tool runs on their tty) it has a child
//                it spawned after booting that has been running a while: a
//                tool call in progress (`sleep`, `npm
//                install`, a test waiting on I/O). Not counted: children from
//                boot (MCP servers), the agent's own helper processes (wrapper
//                → native binary, daemons), momentary polls (`gh auth token`,
//                `git status`), and anything older than TOOL_MAX_SECS (a dev
//                server left running in the background).
//
// Network traffic is deliberately *not* a signal: idle agents keep MCP,
// telemetry and update-check connections alive, which kept Macs awake forever.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

/// Store files modified within this window mean "working".
pub const STORE_FRESH_SECS: u64 = 90;
/// Terminal output within this window means "working".
pub const TTY_FRESH_SECS: u64 = 15;
/// Subtree CPU above this many cores means "working". Idle agent TUIs sit at
/// 0.00–0.03 cores (a logged-out copilot spins at ~0.27); builds and test
/// runs take a core or more.
pub const BUSY_CORES: f64 = 0.5;
/// Store writes this early in an instance's life are boot activity (DB setup).
pub const STARTUP_SECS: u64 = 5;
/// Children spawned this early are boot helpers (wrapper → native binary, MCP
/// servers: ≤ ~2s); copilot -p starts its first tool ~4s in.
pub const CHILD_BOOT_SECS: u64 = 3;
/// A tool child must have run this long (filters momentary polls)…
pub const TOOL_MIN_SECS: u64 = 2;
/// …and no longer than this (a background server isn't work).
pub const TOOL_MAX_SECS: u64 = 600;
/// An instance stays "working" this long after its last signal (smooths gaps
/// between transcript writes).
pub const HOLD_SECS: u64 = 30;
/// Keep the Mac awake this long after the last working instance went idle.
pub const GRACE_SECS: u64 = 300;

/// Where an agent writes per-session files as it works.
#[derive(Clone, Debug)]
pub enum Store {
    /// `($env or ~/<home>)/<sub>`, searched `depth` levels deep.
    Home {
        env: &'static str,
        home: &'static str,
        sub: &'static str,
        depth: usize,
        filter: &'static str, // file-name prefix to require ("" = any)
        per_dir: bool,        // a session is a directory, not a single file
    },
    /// `<process cwd>/<rel>` — project-local files (aider, crush).
    Cwd { rel: &'static str, depth: usize, filter: &'static str },
}

#[derive(Debug)]
pub struct Agent {
    pub name: &'static str,
    /// Executable, argv0 or script basenames.
    pub bins: &'static [&'static str],
    /// Substrings of an executable or script *path* (npm package dirs, …).
    pub paths: &'static [&'static str],
    pub stores: Vec<Store>,
    /// Editor host (VS Code extension host, …): CPU is noisy, so it's judged
    /// by its stores only, and CLI agents launched inside it stay separate.
    pub gui: bool,
}

const fn home(env: &'static str, home: &'static str, sub: &'static str, depth: usize) -> Store {
    Store::Home { env, home, sub, depth, filter: "", per_dir: false }
}
const fn home_dirs(env: &'static str, home: &'static str, sub: &'static str, depth: usize) -> Store {
    Store::Home { env, home, sub, depth, filter: "", per_dir: true }
}
const fn home_only(
    env: &'static str,
    home: &'static str,
    sub: &'static str,
    filter: &'static str,
) -> Store {
    Store::Home { env, home, sub, depth: 1, filter, per_dir: false }
}

/// Application Support dirs of VS Code–family editors, for extension storage.
const EDITORS: &[(&str, &str)] = &[
    ("vscode", "Library/Application Support/Code/User"),
    ("vscode", "Library/Application Support/Code - Insiders/User"),
    ("cursor", "Library/Application Support/Cursor/User"),
    ("windsurf", "Library/Application Support/Windsurf/User"),
];

/// In-editor agent extensions that keep per-task folders in globalStorage.
const EXT_TASKS: &[&str] = &[
    "globalStorage/saoudrizwan.claude-dev/tasks",     // Cline
    "globalStorage/rooveterinaryinc.roo-cline/tasks", // Roo Code
    "globalStorage/kilocode.kilo-code/tasks",         // Kilo Code (≤ v6)
];

fn editor_stores(editor: &str) -> Vec<Store> {
    let mut v = Vec::new();
    for (e, dir) in EDITORS {
        if *e != editor {
            continue;
        }
        for t in EXT_TASKS {
            let sub: &'static str = Box::leak(format!("{dir}/{t}").into_boxed_str());
            v.push(home_dirs("", "", sub, 2));
        }
        // VS Code chat (GitHub Copilot agent mode) sessions per workspace.
        let chat: &'static str =
            Box::leak(format!("{dir}/workspaceStorage/*/chatSessions").into_boxed_str());
        v.push(home("", "", chat, 1));
    }
    // Cline ≥ v3 extension shares the CLI's session store.
    v.push(home_dirs("CLINE_DIR", ".cline", "data/sessions", 2));
    v
}

/// Every agent Redbull recognizes.
pub fn registry() -> Vec<Agent> {
    vec![
        Agent {
            name: "claude",
            bins: &["claude"],
            paths: &["/claude/versions/", "@anthropic-ai/claude-code/"],
            stores: vec![home("CLAUDE_CONFIG_DIR", ".claude", "projects", 2)],
            gui: false,
        },
        Agent {
            name: "codex",
            bins: &["codex"],
            paths: &["@openai/codex"],
            stores: vec![home("CODEX_HOME", ".codex", "sessions", 4)],
            gui: false,
        },
        Agent {
            name: "copilot",
            bins: &["copilot"],
            paths: &["@github/copilot", "/copilot-cli/"],
            stores: vec![home_dirs("COPILOT_HOME", ".copilot", "session-state", 2)],
            gui: false,
        },
        Agent {
            name: "gemini",
            bins: &["gemini"],
            paths: &["@google/gemini-cli"],
            stores: vec![home("GEMINI_CLI_HOME", ".gemini", "tmp/*/chats", 2)],
            gui: false,
        },
        Agent {
            name: "opencode",
            bins: &["opencode", "opencode.exe"],
            paths: &["/opencode-ai/", "@opencode/cli"],
            stores: vec![home_only("XDG_DATA_HOME", ".local/share", "opencode", "opencode")],
            gui: false,
        },
        Agent {
            name: "cursor-agent",
            bins: &["cursor-agent"],
            paths: &["/cursor-agent/"],
            stores: vec![home_dirs("", ".cursor", "chats", 3)],
            gui: false,
        },
        Agent {
            name: "aider",
            bins: &["aider"],
            paths: &["/aider-chat/bin/aider"],
            stores: vec![Store::Cwd { rel: ".aider.chat.history.md", depth: 0, filter: "" }],
            gui: false,
        },
        Agent {
            name: "goose",
            bins: &["goose"],
            paths: &[],
            stores: vec![
                home("GOOSE_PATH_ROOT", ".local/share/goose", "sessions", 1),
                home("", "Library/Application Support/Block/goose", "sessions", 1),
            ],
            gui: false,
        },
        Agent {
            name: "amp",
            bins: &["amp"],
            paths: &["@sourcegraph/amp", "@ampcode/", "/.amp/bin/"],
            stores: vec![home("", ".local/share/amp", "threads", 1)],
            gui: false,
        },
        Agent {
            name: "qwen",
            bins: &["qwen"],
            paths: &["@qwen-code/qwen-code"],
            stores: vec![home("QWEN_HOME", ".qwen", "projects", 3)],
            gui: false,
        },
        Agent {
            name: "crush",
            bins: &["crush"],
            paths: &["@charmland/crush"],
            stores: vec![Store::Cwd { rel: ".crush", depth: 1, filter: "crush.db" }],
            gui: false,
        },
        Agent {
            name: "droid",
            bins: &["droid"],
            paths: &["/.factory/bin/"],
            stores: vec![home("", ".factory", "sessions", 2)],
            gui: false,
        },
        Agent {
            name: "auggie",
            bins: &["auggie", "augment.mjs"],
            paths: &["@augmentcode/auggie"],
            stores: vec![home("", ".augment", "sessions", 2)],
            gui: false,
        },
        Agent {
            name: "kiro",
            bins: &["kiro-cli", "kiro-cli-chat", "qchat"],
            paths: &[],
            stores: vec![
                home_only("", "Library/Application Support", "kiro-cli", "data.sqlite3"),
                home_only("", "Library/Application Support", "amazon-q", "data.sqlite3"),
                home("", ".kiro", "sessions", 3),
            ],
            gui: false,
        },
        Agent {
            name: "cline",
            bins: &["cline", ".cline"],
            paths: &["@cline/cli", "/node_modules/cline/"],
            stores: vec![home_dirs("CLINE_DIR", ".cline", "data/sessions", 2)],
            gui: false,
        },
        Agent {
            name: "kilo",
            bins: &["kilo", "kilocode", ".kilo"],
            paths: &["@kilocode/cli"],
            stores: vec![home_only("XDG_DATA_HOME", ".local/share", "kilo", "kilo.db")],
            gui: false,
        },
        Agent {
            name: "openhands",
            bins: &["openhands", "openhands-cli"],
            paths: &["/uv/tools/openhands/"],
            stores: vec![home_dirs("OPENHANDS_PERSISTENCE_DIR", ".openhands", "conversations", 3)],
            gui: false,
        },
        Agent {
            name: "continue",
            bins: &["cn"],
            paths: &["@continuedev/cli"],
            stores: vec![home("CONTINUE_GLOBAL_DIR", ".continue", "sessions", 1)],
            gui: false,
        },
        Agent {
            name: "gptme",
            bins: &["gptme"],
            paths: &[],
            stores: vec![home("", ".local/share/gptme", "logs", 2)],
            gui: false,
        },
        Agent { name: "plandex", bins: &["plandex"], paths: &[], stores: vec![], gui: false },
        Agent {
            name: "codebuff",
            bins: &["codebuff"],
            paths: &["/node_modules/codebuff/"],
            stores: vec![],
            gui: false,
        },
        // --- editors (agent runs inside the editor's extension host) ---
        Agent {
            name: "vscode",
            bins: &["Code Helper (Plugin)", "Code - Insiders Helper (Plugin)"],
            paths: &[],
            stores: editor_stores("vscode"),
            gui: true,
        },
        Agent {
            name: "cursor",
            bins: &["Cursor Helper (Plugin)"],
            paths: &[],
            stores: {
                let mut v = editor_stores("cursor");
                v.push(home_only(
                    "",
                    "Library/Application Support/Cursor/User",
                    "globalStorage",
                    "state.vscdb",
                ));
                v
            },
            gui: true,
        },
        Agent {
            name: "windsurf",
            bins: &["Windsurf Helper (Plugin)"],
            paths: &[],
            stores: {
                let mut v = editor_stores("windsurf");
                v.push(home("", ".codeium/windsurf", "cascade", 1));
                v
            },
            gui: true,
        },
        Agent {
            name: "zed",
            bins: &["zed"],
            paths: &[],
            stores: vec![home("", "Library/Application Support/Zed", "threads", 1)],
            gui: true,
        },
    ]
}

// --- Process identification --------------------------------------------------

fn basename(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

/// Interpreters that may run an agent as a script; we then look at the script.
fn is_interpreter(base: &str) -> bool {
    let b = base.to_ascii_lowercase(); // Homebrew's framework Python is "Python"
    matches!(b.as_str(), "node" | "bun" | "deno" | "ruby" | "npx" | "tsx" | "ts-node")
        || b.starts_with("python")
}

/// First non-flag argument after argv0: the script an interpreter runs.
fn script_of(cmd: &str) -> Option<&str> {
    cmd.split_whitespace().skip(1).find(|t| !t.starts_with('-'))
}

fn matches(agent: &Agent, path: &str) -> bool {
    let base = basename(path);
    agent.bins.contains(&base) || agent.paths.iter().any(|p| path.contains(p))
}

/// Identify which agent (index into `registry()`) a process is, from its
/// executable path (`comm`) and full command line. Only the executable, argv0
/// and an interpreter's script are considered — never other arguments — so a
/// directory or file merely *named* like an agent doesn't false-positive.
pub fn identify(agents: &[Agent], comm: &str, cmd: &str) -> Option<usize> {
    let find = |path: &str| agents.iter().position(|a| matches(a, path));
    if let Some(i) = find(comm) {
        return Some(i);
    }
    let base = basename(comm);
    if is_interpreter(base) {
        // argv0 may be rewritten by process.title (e.g. "claude").
        if let Some(argv0) = cmd.split_whitespace().next() {
            if !is_interpreter(basename(argv0)) {
                if let Some(i) = find(argv0) {
                    return Some(i);
                }
            }
        }
        return script_of(cmd).and_then(find);
    }
    None
}

// --- Process table -----------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Proc {
    pub ppid: i32,
    pub tty: Option<String>,
    pub cpu: f64,   // cumulative CPU seconds
    pub age: u64,   // seconds since start
    pub comm: String,
    pub cmd: String,
}

/// Snapshot of all processes via two `ps` calls (comm is unambiguous; the full
/// command line is needed only for interpreters).
pub fn ps_table() -> HashMap<i32, Proc> {
    let ps = |fmt: &str| -> String {
        Command::new("ps")
            .args(["-axww", "-o", fmt])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let mut t: HashMap<i32, Proc> = HashMap::new();
    for line in ps("pid=,ppid=,tty=,time=,etime=,comm=").lines() {
        let mut it = line.split_whitespace();
        let Some(pid) = it.next().and_then(|s| s.parse().ok()) else { continue };
        let ppid = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let tty = it.next().unwrap_or("??");
        let cpu = parse_time(it.next().unwrap_or("0"));
        let age = parse_time(it.next().unwrap_or("0")) as u64;
        let comm = it.collect::<Vec<_>>().join(" ");
        let tty = (tty != "??" && tty != "?").then(|| tty.to_string());
        t.insert(pid, Proc { ppid, tty, cpu, age, comm, cmd: String::new() });
    }
    for line in ps("pid=,command=").lines() {
        let line = line.trim_start();
        if let Some((p, cmd)) = line.split_once(' ') {
            if let Some(pr) = p.parse().ok().and_then(|pid: i32| t.get_mut(&pid)) {
                pr.cmd = cmd.trim_start().to_string();
            }
        }
    }
    t
}

/// Parse `ps` time values ("[D-][H:]M:SS[.ss]") into seconds.
pub fn parse_time(s: &str) -> f64 {
    let s = s.trim();
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().unwrap_or(0.0), r),
        None => (0.0, s),
    };
    let mut secs = 0.0f64;
    for p in rest.split(':') {
        secs = secs * 60.0 + p.parse::<f64>().unwrap_or(0.0);
    }
    secs + days * 86400.0
}

/// One running agent session: a root process and its subtree.
#[derive(Clone, Debug)]
pub struct Instance {
    pub agent: usize,
    pub pid: i32,
    pub pids: Vec<i32>,
    pub ttys: Vec<String>,
    pub age: u64,
}

/// Group agent processes into instances. A process matching an agent is a
/// root unless an ancestor is already a CLI agent (wrappers like `node
/// codex.js` → native `codex`, gemini's relaunch child, agents run as tools).
/// Editor hosts never absorb CLI agents started inside them.
pub fn instances(agents: &[Agent], procs: &HashMap<i32, Proc>, self_pid: i32) -> Vec<Instance> {
    let mut kind: HashMap<i32, usize> = HashMap::new();
    for (&pid, p) in procs {
        if pid == self_pid {
            continue;
        }
        if let Some(i) = identify(agents, &p.comm, &p.cmd) {
            kind.insert(pid, i);
        }
    }
    let absorbed = |pid: i32, gui: bool| -> bool {
        let mut cur = procs.get(&pid).map(|p| p.ppid).unwrap_or(0);
        let mut hops = 0;
        while cur > 1 && hops < 64 {
            if let Some(&k) = kind.get(&cur) {
                if !agents[k].gui || gui {
                    return true;
                }
            }
            cur = procs.get(&cur).map(|p| p.ppid).unwrap_or(0);
            hops += 1;
        }
        false
    };
    let mut roots: Vec<(i32, usize)> = kind
        .iter()
        .filter(|(&pid, &k)| !absorbed(pid, agents[k].gui))
        .map(|(&p, &k)| (p, k))
        .collect();
    roots.sort();
    let root_set: HashSet<i32> = roots.iter().map(|r| r.0).collect();

    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    for (&pid, p) in procs {
        children.entry(p.ppid).or_default().push(pid);
    }
    roots
        .into_iter()
        .map(|(root, k)| {
            let (mut pids, mut ttys) = (Vec::new(), Vec::new());
            let mut stack = vec![root];
            while let Some(p) = stack.pop() {
                if p != root && root_set.contains(&p) {
                    continue; // a nested agent is its own instance
                }
                pids.push(p);
                if let Some(t) = procs.get(&p).and_then(|x| x.tty.clone()) {
                    if !ttys.contains(&t) {
                        ttys.push(t);
                    }
                }
                if let Some(ch) = children.get(&p) {
                    stack.extend(ch);
                }
            }
            let age = procs.get(&root).map(|p| p.age).unwrap_or(0);
            Instance { agent: k, pid: root, pids, ttys, age }
        })
        .collect()
}

// --- Signals -----------------------------------------------------------------

/// Environment lookups (home dir + env vars), injectable for tests.
#[derive(Clone, Debug)]
pub struct Env {
    pub home: PathBuf,
    pub vars: HashMap<String, String>,
}

impl Env {
    pub fn from_process() -> Env {
        Env {
            home: std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
            vars: std::env::vars().collect(),
        }
    }
    fn var(&self, k: &str) -> Option<&str> {
        if k.is_empty() {
            return None;
        }
        self.vars.get(k).map(|s| s.as_str()).filter(|s| !s.is_empty())
    }
}

/// Resolve a home-based store to its directories (`*` path segments expand
/// to every subdirectory).
pub fn store_roots(env: &Env, s: &Store) -> Vec<PathBuf> {
    let Store::Home { env: var, home, sub, .. } = s else { return Vec::new() };
    let mut roots =
        vec![env.var(var).map(PathBuf::from).unwrap_or_else(|| env.home.join(home))];
    for seg in sub.split('/').filter(|s| !s.is_empty()) {
        if seg == "*" {
            roots = roots
                .iter()
                .filter_map(|r| std::fs::read_dir(r).ok())
                .flat_map(|rd| rd.flatten())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
        } else {
            roots.iter_mut().for_each(|r| r.push(seg));
        }
    }
    roots
}

/// Session keys (files, or their directories when `per_dir`) under `root`
/// modified within `fresh` seconds, with their latest mtime.
fn fresh_sessions(
    root: &Path,
    depth: usize,
    filter: &str,
    per_dir: bool,
    now: SystemTime,
    fresh: u64,
    out: &mut HashMap<PathBuf, SystemTime>,
) {
    let check = |path: &Path, md: &std::fs::Metadata, out: &mut HashMap<PathBuf, SystemTime>| {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !filter.is_empty() && !name.starts_with(filter) {
            return;
        }
        let Ok(m) = md.modified() else { return };
        if now.duration_since(m).unwrap_or_default().as_secs() <= fresh {
            let key = if per_dir { path.parent().unwrap_or(path) } else { path };
            let e = out.entry(key.to_path_buf()).or_insert(m);
            *e = (*e).max(m);
        }
    };
    let Ok(md) = std::fs::symlink_metadata(root) else { return };
    if md.is_file() {
        check(root, &md, out);
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let path = e.path();
        if ft.is_dir() && depth > 1 {
            fresh_sessions(&path, depth - 1, filter, per_dir, now, fresh, out);
        } else if ft.is_file() {
            if let Ok(md) = e.metadata() {
                check(&path, &md, out);
            }
        }
    }
}

/// The pid that owns a session directory, if the agent leaves a marker in it
/// (Copilot CLI: `inuse.<pid>.lock`).
fn session_owner(dir: &Path) -> Option<i32> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name();
        let n = n.to_str()?;
        n.strip_prefix("inuse.")?.strip_suffix(".lock")?.parse().ok()
    })
}

/// Whether a fresh session (latest write `m`) counts as activity: it must have
/// changed since the previous sample, after its owner booted. Once counted it
/// keeps counting until it goes stale (STORE_FRESH_SECS).
fn changed(
    seen: &mut HashMap<PathBuf, (SystemTime, bool)>,
    key: &Path,
    m: SystemTime,
    booted: SystemTime,
) -> bool {
    let counted = match seen.get(key) {
        Some(&(prev, c)) => (m > prev && m >= booted) || (c && m == prev),
        None => false, // first sight: baseline only
    };
    seen.insert(key.to_path_buf(), (m, counted));
    counted
}

/// Whether an instance has a tool call in progress: a descendant spawned after
/// boot, running for TOOL_MIN_SECS..TOOL_MAX_SECS, that isn't the agent itself.
fn has_tool_child(agents: &[Agent], inst: &Instance, procs: &HashMap<i32, Proc>) -> bool {
    inst.pids.iter().filter(|&&p| p != inst.pid).any(|p| {
        procs.get(p).is_some_and(|c| {
            c.age + CHILD_BOOT_SECS <= inst.age
                && (TOOL_MIN_SECS..TOOL_MAX_SECS).contains(&c.age)
                && identify(agents, &c.comm, &c.cmd).is_none()
        })
    })
}

/// Whether a session path has a component naming project dir `cwd` the way
/// Claude Code, droid and qwen do ("/Users/me/my.proj" -> "-Users-me-my-proj").
fn names_project(key: &Path, cwd: &Path) -> bool {
    let enc = |s: &str| -> String {
        s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
    };
    let want = enc(&cwd.to_string_lossy());
    want.len() > 1 && key.iter().any(|c| enc(&c.to_string_lossy()) == want)
}

/// When a process started, from its age in seconds.
fn started(wall: SystemTime, age: u64) -> SystemTime {
    wall - Duration::from_secs(age)
}

/// Claude Code's self-reported state for a pid: `Some(true)` busy,
/// `Some(false)` idle/waiting, `None` if it doesn't publish one.
pub fn claude_status(env: &Env, pid: i32) -> Option<bool> {
    let dir = env
        .var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".claude"));
    let s = std::fs::read_to_string(dir.join("sessions").join(format!("{pid}.json"))).ok()?;
    let status = json_str(&s, "status")?;
    Some(status == "busy")
}

/// Minimal JSON string-field lookup (`"key":"value"`); avoids a JSON crate.
fn json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\"");
    let at = s.find(&pat)? + pat.len();
    let rest = s[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    rest.split('"').next()
}

/// Seconds since a tty last had output written to it (its mtime).
pub fn tty_age(tty: &str) -> Option<u64> {
    let m = std::fs::metadata(format!("/dev/{tty}")).ok()?.modified().ok()?;
    Some(m.elapsed().map(|d| d.as_secs()).unwrap_or(0))
}

/// Working directories of the given pids (via lsof; used only for agents with
/// project-local stores).
fn cwds(pids: &[i32]) -> HashMap<i32, PathBuf> {
    let mut map = HashMap::new();
    if pids.is_empty() {
        return map;
    }
    let list = pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
    let Ok(out) = Command::new("lsof").args(["-a", "-d", "cwd", "-p", &list, "-Fpn"]).output()
    else {
        return map;
    };
    let mut cur = 0;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(p) = line.strip_prefix('p') {
            cur = p.parse().unwrap_or(0);
        } else if let Some(n) = line.strip_prefix('n') {
            map.insert(cur, PathBuf::from(n));
        }
    }
    map
}

// --- Detector ----------------------------------------------------------------

/// Why an instance counts as working (for `redbull scan` and tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Status,
    Store,
    Tty,
    Cpu,
    Tool,
}

#[derive(Clone, Debug)]
pub struct Verdict {
    pub agent: &'static str,
    pub pid: i32,
    pub procs: usize,
    pub reason: Option<Reason>, // signal seen in this sample
    pub working: bool,          // reason, or one within HOLD_SECS
    pub status: Option<bool>,
    pub tty_age: Option<u64>,
    pub cores: f64,
}

#[derive(Default)]
pub struct Report {
    pub verdicts: Vec<Verdict>,
}

impl Report {
    pub fn working(&self) -> usize {
        self.verdicts.iter().filter(|v| v.working).count()
    }
}

/// Stateful detector: keeps the previous CPU sample (for deltas) and when each
/// instance was last seen working.
pub struct Detector {
    pub agents: Vec<Agent>,
    pub env: Env,
    prev_cpu: HashMap<i32, f64>,
    prev_at: Option<Instant>,
    last_active: HashMap<i32, Instant>, // root pid -> last working signal
    last_any: Option<Instant>,
    seen: HashMap<PathBuf, (SystemTime, bool)>, // session -> (mtime, counted)
}

impl Detector {
    pub fn new(env: Env) -> Detector {
        Detector {
            agents: registry(),
            env,
            prev_cpu: HashMap::new(),
            prev_at: None,
            last_active: HashMap::new(),
            last_any: None,
            seen: HashMap::new(),
        }
    }

    /// Take one sample of the live system.
    pub fn step(&mut self) -> Report {
        let procs = ps_table();
        self.step_with(&procs, std::process::id() as i32)
    }

    pub fn step_with(&mut self, procs: &HashMap<i32, Proc>, self_pid: i32) -> Report {
        let now = Instant::now();
        let wall = SystemTime::now();
        let insts = instances(&self.agents, procs, self_pid);
        let elapsed = self.prev_at.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);

        // Per-instance signals that are attributable to a process.
        let need_cwd: Vec<i32> = insts
            .iter()
            .filter(|i| self.agents[i.agent].stores.iter().any(|s| matches!(s, Store::Cwd { .. })))
            .map(|i| i.pid)
            .collect();
        let cwd_of = cwds(&need_cwd);

        let mut verdicts: Vec<Verdict> = Vec::with_capacity(insts.len());
        let mut live_keys: HashSet<PathBuf> = HashSet::new();
        for inst in &insts {
            let agent = &self.agents[inst.agent];
            let status = if agent.name == "claude" { claude_status(&self.env, inst.pid) } else { None };
            let tty_age = inst.ttys.iter().filter_map(|t| tty_age(t)).min();
            let mut cores = 0.0;
            if elapsed > 0.0 {
                let mut d = 0.0;
                for p in &inst.pids {
                    let c = procs.get(p).map(|x| x.cpu).unwrap_or(0.0);
                    d += (c - self.prev_cpu.get(p).copied().unwrap_or(c)).max(0.0);
                }
                cores = d / elapsed;
            }
            let booted = started(wall, inst.age) + Duration::from_secs(STARTUP_SECS);
            let mut cwd_fresh = false;
            if let Some(cwd) = cwd_of.get(&inst.pid) {
                for s in &agent.stores {
                    if let Store::Cwd { rel, depth, filter } = s {
                        let mut out = HashMap::new();
                        fresh_sessions(&cwd.join(rel), *depth, filter, false, wall, STORE_FRESH_SECS, &mut out);
                        for (key, m) in out {
                            cwd_fresh |= changed(&mut self.seen, &key, m, booted);
                            live_keys.insert(key);
                        }
                    }
                }
            }
            let reason = if status == Some(true) {
                Some(Reason::Status)
            } else if cwd_fresh {
                Some(Reason::Store)
            } else if tty_age.is_some_and(|a| a <= TTY_FRESH_SECS) {
                Some(Reason::Tty)
            } else if !agent.gui && cores >= BUSY_CORES {
                Some(Reason::Cpu)
            } else if !agent.gui
                && inst.ttys.is_empty()
                && has_tool_child(&self.agents, inst, procs)
            {
                Some(Reason::Tool)
            } else {
                None
            };
            verdicts.push(Verdict {
                agent: agent.name,
                pid: inst.pid,
                procs: inst.pids.len(),
                reason,
                working: false,
                status,
                tty_age,
                cores,
            });
        }

        // Shared (home-dir) stores. A session that names its owner pid is
        // credited to that instance (or dropped if the owner has exited); one
        // that names its project goes to the instance working there. Never
        // assigned to an instance that self-reports its status.
        for k in 0..self.agents.len() {
            let idx: Vec<usize> = (0..insts.len()).filter(|&i| insts[i].agent == k).collect();
            if idx.is_empty() {
                continue;
            }
            let mut sessions = HashMap::new();
            for s in &self.agents[k].stores {
                if let Store::Home { depth, filter, per_dir, .. } = s {
                    for root in store_roots(&self.env, s) {
                        fresh_sessions(&root, *depth, filter, *per_dir, wall, STORE_FRESH_SECS, &mut sessions);
                    }
                }
            }
            let booted = |i: usize| started(wall, insts[i].age) + Duration::from_secs(STARTUP_SECS);
            let youngest = idx.iter().copied().min_by_key(|&i| insts[i].age).unwrap();
            let mut pooled = 0usize;
            let mut active_keys: Vec<PathBuf> = Vec::new();
            for (key, m) in sessions {
                let owner = session_owner(&key)
                    .map(|pid| idx.iter().copied().find(|&i| insts[i].pid == pid));
                let b = match owner {
                    Some(Some(i)) => booted(i),
                    Some(None) => continue, // its owner has exited
                    None => booted(youngest),
                };
                let active = changed(&mut self.seen, &key, m, b);
                match owner {
                    Some(Some(i)) if active && verdicts[i].reason.is_none() => {
                        verdicts[i].reason = Some(Reason::Store);
                    }
                    None if active => {
                        pooled += 1;
                        active_keys.push(key.clone());
                    }
                    _ => {}
                }
                live_keys.insert(key);
            }
            // Claude, droid and qwen name session dirs after the project
            // ("/Users/me/proj" -> "-Users-me-proj"): credit those sessions
            // to the instance running in that directory.
            if !active_keys.is_empty() {
                let dirs = cwds(&idx.iter().map(|&i| insts[i].pid).collect::<Vec<_>>());
                active_keys.retain(|key| {
                    let mine: Vec<usize> = idx
                        .iter()
                        .copied()
                        .filter(|&i| dirs.get(&insts[i].pid).is_some_and(|d| names_project(key, d)))
                        .collect();
                    let free = mine
                        .iter()
                        .find(|&&i| verdicts[i].reason.is_none() && verdicts[i].status.is_none());
                    if let Some(&i) = free {
                        verdicts[i].reason = Some(Reason::Store);
                    }
                    mine.is_empty() // keep only unattributed sessions
                });
                pooled = active_keys.len();
            }
            // The rest can't be tied to a pid: if N sessions are active, N
            // instances are working — counting those already explained by
            // another signal first, then the longest-running.
            let explained = idx.iter().filter(|&&i| verdicts[i].reason.is_some()).count();
            let extra = pooled.saturating_sub(explained);
            let mut cand: Vec<usize> = idx
                .into_iter()
                .filter(|&i| verdicts[i].reason.is_none() && verdicts[i].status.is_none())
                .collect();
            cand.sort_by_key(|&i| std::cmp::Reverse(insts[i].age));
            for i in cand.into_iter().take(extra) {
                verdicts[i].reason = Some(Reason::Store);
            }
        }

        for v in &mut verdicts {
            if v.reason.is_some() {
                self.last_active.insert(v.pid, now);
                self.last_any = Some(now);
            }
            v.working = self
                .last_active
                .get(&v.pid)
                .is_some_and(|t| now.duration_since(*t).as_secs() < HOLD_SECS);
        }
        let live: HashSet<i32> = insts.iter().map(|i| i.pid).collect();
        self.last_active.retain(|p, _| live.contains(p));
        self.seen.retain(|k, _| live_keys.contains(k));
        self.prev_cpu = procs.iter().map(|(&p, x)| (p, x.cpu)).collect();
        self.prev_at = Some(now);
        Report { verdicts }
    }

    /// Whether to hold the wake assertion: something is working, or was within
    /// the grace period.
    pub fn keep_awake(&self) -> bool {
        self.last_any.is_some_and(|t| t.elapsed().as_secs() < GRACE_SECS)
    }

    pub fn reset(&mut self) {
        self.prev_cpu.clear();
        self.prev_at = None;
        self.last_active.clear();
        self.last_any = None;
        self.seen.clear();
    }
}

#[cfg(test)]
mod tests;
