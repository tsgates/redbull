// Agent activity: what each running agent session is doing, as a feed.
//
// Built on the detector's per-session verdicts (agents.rs). Every agent gets:
//
//   - state events — a session appears, starts working (and why), goes idle,
//     exits;
//   - process events — commands it launches (builds, tests, git…), seen in
//     the process table. Boot helpers (MCP servers), shells and the agent's
//     own wrapper processes aren't reported, only the commands under them.
//
// Agents with a readable transcript (Claude Code, Codex, Copilot CLI, Open
// Interpreter, Pi, oh-my-pi, Prime Agent) get a richer feed instead of
// process events: prompts, tool calls and replies,
// tailed from the session's JSONL file. That, and the working directory
// lookup, only runs in `detail` mode — while the activity window is open.
//
// The same tool calls say which files a session read and which it modified
// (its Read/Edit/Write-style tools and patches); that list is kept per
// session. Files touched only through shell commands aren't seen.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::agents::{self, Agent, Env, Proc, Reason, Report, Verdict};
use crate::json::{self, Value};

/// Events kept in the feed (all sessions together).
pub const MAX_EVENTS: usize = 500;
/// Working/idle samples kept per session (the sparkline).
pub const HIST: usize = 60;
/// A session first seen this young is reported as "started".
const NEW_SECS: u64 = 15;
/// Transcript events shown from before we started watching a session.
const SEED_EVENTS: usize = 12;
/// How far back to look for those.
const SEED_BYTES: u64 = 2 * 1024 * 1024;
/// Most transcript bytes read in one go; anything older is skipped.
const MAX_READ: u64 = 2 * 1024 * 1024;
/// Transcript lines longer than this (huge tool results) are skipped.
const MAX_LINE: usize = 512 * 1024;
/// Retry finding a session's transcript every this many scans.
const LOCATE_EVERY: u32 = 5;
const TEXT_MAX: usize = 160;
/// Files remembered per session; read-only ones are forgotten first.
pub const MAX_FILES: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    State,
    Proc,
    Tool,
    Prompt,
    Reply,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::State => "state",
            Kind::Proc => "proc",
            Kind::Tool => "tool",
            Kind::Prompt => "prompt",
            Kind::Reply => "reply",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub seq: u64,
    pub pid: i32,
    pub agent: &'static str,
    pub at: u64, // unix seconds
    pub kind: Kind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// A file a session's tools have touched.
#[derive(Clone, Debug, PartialEq)]
pub struct FileUse {
    pub path: String,
    pub reads: u32,
    pub writes: u32,
    pub at: u64, // last touched, unix seconds
}

/// A command a session is running right now.
#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub pid: i32,
    pub cmd: String,
    pub age: u64,
}

#[derive(Default)]
struct Track {
    working: bool,
    known: HashSet<i32>, // subtree pids already seen
    hist: VecDeque<bool>,
    running: Vec<Running>,
    cwd: Option<PathBuf>,
    cwd_tried: bool,
    title: Option<String>,
    session: Option<String>, // the agent's own session id, when known
    tail: Option<Tail>,
    lookups: u32,
    files: Vec<FileUse>,
}

impl Track {
    fn touch(&mut self, at: u64, access: Access, path: &str) {
        // Relative paths are relative to the session's working directory.
        let path = match &self.cwd {
            Some(cwd) if !path.starts_with('/') && !path.starts_with('~') => {
                cwd.join(path.trim_start_matches("./")).to_string_lossy().into_owned()
            }
            _ => path.to_string(),
        };
        let i = match self.files.iter().position(|f| f.path == path) {
            Some(i) => i,
            None => {
                if self.files.len() >= MAX_FILES {
                    let oldest = |fs: &[FileUse], ro: bool| {
                        fs.iter()
                            .enumerate()
                            .filter(|(_, f)| !ro || f.writes == 0)
                            .min_by_key(|(_, f)| f.at)
                            .map(|(i, _)| i)
                    };
                    let drop = oldest(&self.files, true).or_else(|| oldest(&self.files, false));
                    self.files.swap_remove(drop.unwrap_or(0));
                }
                self.files.push(FileUse { path, reads: 0, writes: 0, at: 0 });
                self.files.len() - 1
            }
        };
        let f = &mut self.files[i];
        match access {
            Access::Read => f.reads += 1,
            Access::Write => f.writes += 1,
        }
        f.at = f.at.max(at);
    }
}

#[derive(Default)]
pub struct Tracker {
    tracks: HashMap<i32, Track>,
    events: VecDeque<Event>,
    seq: u64,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker::default()
    }

    #[cfg(test)]
    pub fn events(&self) -> Vec<&Event> {
        self.events.iter().collect()
    }

    fn push(&mut self, pid: i32, agent: &'static str, at: u64, kind: Kind, text: String) {
        self.seq += 1;
        self.events.push_back(Event { seq: self.seq, pid, agent, at, kind, text });
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }

    /// Fold one detector sample into the feed.
    pub fn update(
        &mut self,
        agents: &[Agent],
        env: &Env,
        report: &Report,
        procs: &HashMap<i32, Proc>,
        detail: bool,
    ) {
        let now = unix_now();
        let mut new: Vec<(i32, &'static str, u64, Kind, String)> = Vec::new();

        for v in &report.verdicts {
            let fresh = !self.tracks.contains_key(&v.pid);
            let t = self.tracks.entry(v.pid).or_default();
            let running = running(agents, v, procs);
            if fresh {
                if v.age < NEW_SECS {
                    new.push((v.pid, v.agent, now.saturating_sub(v.age), Kind::State, "started".into()));
                }
                if v.working {
                    new.push((v.pid, v.agent, now, Kind::State, working_text(v.reason)));
                }
            } else {
                if v.working != t.working {
                    let text = if v.working { working_text(v.reason) } else { "idle".into() };
                    new.push((v.pid, v.agent, now, Kind::State, text));
                }
                // Commands it launched since the last sample. Sessions whose
                // transcript we follow report their tool calls instead.
                if !v.gui && t.tail.is_none() {
                    for r in running.iter().filter(|r| !t.known.contains(&r.pid)) {
                        new.push((v.pid, v.agent, now.saturating_sub(r.age), Kind::Proc, r.cmd.clone()));
                    }
                }
            }
            t.working = v.working;
            t.known = v.pids.iter().copied().collect();
            t.running = running;
            t.hist.push_back(v.working);
            while t.hist.len() > HIST {
                t.hist.pop_front();
            }
        }

        // Sessions that went away.
        let live: HashSet<i32> = report.verdicts.iter().map(|v| v.pid).collect();
        let gone: Vec<i32> = self.tracks.keys().copied().filter(|p| !live.contains(p)).collect();
        for pid in gone {
            self.tracks.remove(&pid);
            let agent = self.events.iter().rev().find(|e| e.pid == pid).map(|e| e.agent);
            new.push((pid, agent.unwrap_or("agent"), now, Kind::State, "exited".into()));
        }

        if detail {
            self.detail(env, report, now, &mut new);
        }
        for (pid, agent, at, kind, text) in new {
            self.push(pid, agent, at, kind, text);
        }
    }

    /// The expensive part: working directories, session titles, transcripts.
    fn detail(
        &mut self,
        env: &Env,
        report: &Report,
        now: u64,
        new: &mut Vec<(i32, &'static str, u64, Kind, String)>,
    ) {
        // Working directories, one `lsof` for every session that lacks one.
        let need: Vec<i32> = report
            .verdicts
            .iter()
            .filter(|v| v.agent != "claude" && !v.gui)
            .filter(|v| self.tracks.get(&v.pid).is_some_and(|t| !t.cwd_tried))
            .map(|v| v.pid)
            .collect();
        if !need.is_empty() {
            let found = agents::cwds(&need);
            for pid in need {
                if let Some(t) = self.tracks.get_mut(&pid) {
                    t.cwd_tried = true;
                    t.cwd = found.get(&pid).cloned();
                }
            }
        }

        let mut claimed: HashSet<PathBuf> =
            self.tracks.values().filter_map(|t| t.tail.as_ref().map(|x| x.path.clone())).collect();
        for v in &report.verdicts {
            let Some(t) = self.tracks.get_mut(&v.pid) else { continue };
            if v.agent == "claude" {
                if let Some(meta) = claude_meta(env, v.pid) {
                    t.title = meta.get("name").str().map(|s| clip(s, 80));
                    t.cwd = meta.get("cwd").str().map(PathBuf::from).or(t.cwd.take());
                    t.session = meta.get("sessionId").str().map(str::to_string);
                }
            }
            if t.tail.is_none() {
                if t.lookups % LOCATE_EVERY == 0 {
                    if let Some(path) = locate(env, v, t, &claimed) {
                        claimed.insert(path.clone());
                        t.tail = Some(Tail { path, offset: 0, seeded: false });
                    }
                }
                t.lookups += 1;
            }
            let Some(tail) = t.tail.as_mut() else { continue };
            let seeding = !tail.seeded;
            let mut evs: Vec<(u64, Kind, String)> = Vec::new();
            let mut files: Vec<(u64, Access, String)> = Vec::new();
            for line in tail.read_new() {
                if let Some(doc) = json::parse(&line) {
                    parse_line(format_of(v.agent), &doc, now, &mut evs, &mut files);
                }
            }
            for (at, access, path) in files {
                t.touch(at, access, &path);
            }
            if seeding && evs.len() > SEED_EVENTS {
                evs.drain(..evs.len() - SEED_EVENTS);
            }
            new.extend(evs.into_iter().map(|(at, k, text)| (v.pid, v.agent, at, k, text)));
        }
    }

    /// Everything the activity window shows, as JSON.
    pub fn snapshot(&self, report: &Report, auto: bool, awake: bool) -> String {
        let mut o = String::with_capacity(4096);
        o.push_str(&format!(
            "{{\"now\":{},\"auto\":{auto},\"awake\":{awake},\"sessions\":[",
            unix_now()
        ));
        for (i, v) in report.verdicts.iter().enumerate() {
            let none = Track::default();
            let t = self.tracks.get(&v.pid).unwrap_or(&none);
            if i > 0 {
                o.push(',');
            }
            o.push_str(&format!("{{\"pid\":{},\"agent\":", v.pid));
            json::quote(&mut o, v.agent);
            o.push_str(",\"title\":");
            json::quote(&mut o, t.title.as_deref().unwrap_or(""));
            o.push_str(",\"cwd\":");
            json::quote(&mut o, &t.cwd.as_ref().map(|p| p.to_string_lossy()).unwrap_or_default());
            o.push_str(",\"reason\":");
            json::quote(&mut o, &v.reason.map(|r| format!("{r:?}").to_lowercase()).unwrap_or_default());
            o.push_str(&format!(
                ",\"working\":{},\"gui\":{},\"log\":{},\"cores\":{:.2},\"age\":{},\"procs\":{},\"hist\":\"{}\",\"running\":[",
                v.working,
                v.gui,
                t.tail.is_some(),
                v.cores,
                v.age,
                v.pids.len(),
                t.hist.iter().map(|&w| if w { '1' } else { '0' }).collect::<String>(),
            ));
            for (j, r) in t.running.iter().enumerate() {
                if j > 0 {
                    o.push(',');
                }
                o.push_str(&format!("{{\"age\":{},\"cmd\":", r.age));
                json::quote(&mut o, &r.cmd);
                o.push('}');
            }
            o.push_str("],\"files\":[");
            let mut files: Vec<&FileUse> = t.files.iter().collect();
            files.sort_by(|a, b| b.at.cmp(&a.at).then(a.path.cmp(&b.path)));
            for (j, f) in files.iter().enumerate() {
                if j > 0 {
                    o.push(',');
                }
                o.push_str(&format!("{{\"r\":{},\"w\":{},\"t\":{},\"p\":", f.reads, f.writes, f.at));
                json::quote(&mut o, &f.path);
                o.push('}');
            }
            o.push_str("]}");
        }
        o.push_str("],\"events\":[");
        for (i, e) in self.events.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push_str(&format!("{{\"q\":{},\"p\":{},\"t\":{},\"k\":\"{}\",\"a\":", e.seq, e.pid, e.at, e.kind.tag()));
            json::quote(&mut o, e.agent);
            o.push_str(",\"x\":");
            json::quote(&mut o, &e.text);
            o.push('}');
        }
        o.push_str("]}");
        o
    }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn working_text(reason: Option<Reason>) -> String {
    let why = match reason {
        Some(Reason::Status) => "reports busy",
        Some(Reason::Store) => "writing its transcript",
        Some(Reason::Tty) => "terminal output",
        Some(Reason::Cpu) => "heavy CPU",
        Some(Reason::Tool) => "running a tool",
        None => return "working".into(),
    };
    format!("working · {why}")
}

// --- Text helpers ------------------------------------------------------------

/// Truncate to `max` characters, marking the cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// First non-empty line, whitespace collapsed, clipped.
fn clean(s: &str, max: usize) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    clip(&line.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

fn basename(p: &str) -> &str {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or(p)
}

// --- Process events ----------------------------------------------------------

fn is_shell(comm: &str) -> bool {
    matches!(
        basename(comm).trim_start_matches('-'),
        "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "env" | "login"
    )
}

/// Plumbing between an agent and the commands it runs: its own processes
/// (wrapper → native binary) and shells.
fn is_helper(agents: &[Agent], p: &Proc) -> bool {
    is_shell(&p.comm) || agents::identify(agents, &p.comm, &p.cmd).is_some()
}

/// The commands a session is running: the first non-plumbing process on each
/// branch of its subtree (`cargo test`, not the `rustc`s under it), started
/// after the agent booted. Longest-running first.
pub fn running(agents: &[Agent], v: &Verdict, procs: &HashMap<i32, Proc>) -> Vec<Running> {
    let mut out = Vec::new();
    for &pid in v.pids.iter().filter(|&&p| p != v.pid) {
        let Some(p) = procs.get(&pid) else { continue };
        if p.age + agents::CHILD_BOOT_SECS > v.age || is_helper(agents, p) {
            continue;
        }
        let (mut cur, mut hops, mut top) = (p.ppid, 0, true);
        while cur != v.pid {
            match procs.get(&cur) {
                Some(a) if hops < 32 && is_helper(agents, a) => cur = a.ppid,
                _ => {
                    top = false;
                    break;
                }
            }
            hops += 1;
        }
        if top {
            let full = if p.cmd.is_empty() { p.comm.as_str() } else { p.cmd.as_str() };
            let cmd = match full.split_once(' ') {
                Some((argv0, rest)) => format!("{} {rest}", basename(argv0)),
                None => basename(full).to_string(),
            };
            out.push(Running { pid, cmd: clean(&cmd, TEXT_MAX), age: p.age });
        }
    }
    out.sort_by(|a, b| b.age.cmp(&a.age).then(a.pid.cmp(&b.pid)));
    out
}

// --- Transcripts -------------------------------------------------------------

/// An append-only session file we follow.
struct Tail {
    path: PathBuf,
    offset: u64,
    seeded: bool,
}

impl Tail {
    /// Complete lines appended since the last call (the first call returns
    /// the file's last SEED_BYTES).
    fn read_new(&mut self) -> Vec<String> {
        let cap = if self.seeded { MAX_READ } else { SEED_BYTES };
        self.seeded = true;
        let Ok(mut f) = std::fs::File::open(&self.path) else { return Vec::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            self.offset = 0; // truncated or replaced
        }
        let mut start = self.offset;
        let mut partial = false;
        if len.saturating_sub(cap) > start {
            start = len - cap; // too far behind: skip ahead, mid-line
            partial = true;
        }
        if len == start || f.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::with_capacity((len - start) as usize);
        if f.take(len - start).read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else {
            // No complete line yet; give up on one that outgrows the cap.
            self.offset = if buf.len() as u64 >= cap { len } else { start };
            return Vec::new();
        };
        self.offset = start + end as u64 + 1;
        buf[..end]
            .split(|&b| b == b'\n')
            .skip(partial as usize)
            .filter(|l| !l.is_empty() && l.len() <= MAX_LINE)
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .collect()
    }
}

fn dir_of(env: &Env, var: &str, home: &str) -> PathBuf {
    env.var(var).map(PathBuf::from).unwrap_or_else(|| env.home.join(home))
}

/// Claude Code's own record of a session (`~/.claude/sessions/<pid>.json`).
fn claude_meta(env: &Env, pid: i32) -> Option<Value> {
    let dir = dir_of(env, "CLAUDE_CONFIG_DIR", ".claude");
    json::parse(&std::fs::read_to_string(dir.join("sessions").join(format!("{pid}.json"))).ok()?)
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()
}

/// Find the transcript a session is writing, for the agents we can read.
fn locate(env: &Env, v: &Verdict, t: &Track, claimed: &HashSet<PathBuf>) -> Option<PathBuf> {
    match v.agent {
        // projects/<project dir>/<session id>.jsonl
        "claude" => {
            let name = format!("{}.jsonl", t.session.as_ref()?);
            subdirs(&dir_of(env, "CLAUDE_CONFIG_DIR", ".claude").join("projects"))
                .into_iter()
                .map(|d| d.join(&name))
                .find(|p| p.is_file())
        }
        // session-state/<id>/events.jsonl, owner marked by inuse.<pid>.lock
        "copilot" => {
            let lock = format!("inuse.{}.lock", v.pid);
            subdirs(&dir_of(env, "COPILOT_HOME", ".copilot").join("session-state"))
                .into_iter()
                .find(|d| d.join(&lock).exists())
                .map(|d| d.join("events.jsonl"))
                .filter(|p| p.is_file())
        }
        // Dated or per-project JSONL files whose first line names the cwd.
        _ => {
            let (env_var, home, depth) = dated_store(v.agent)?;
            let since = agents::started(SystemTime::now(), v.age) - Duration::from_secs(5);
            let mut files = Vec::new();
            rollouts(&dir_of(env, env_var, home).join("sessions"), depth, since, &mut files);
            files.sort_by_key(|(_, m)| std::cmp::Reverse(*m));
            files
                .into_iter()
                .map(|(p, _)| p)
                .filter(|p| !claimed.contains(p))
                .find(|p| match (&t.cwd, rollout_cwd(p)) {
                    (Some(mine), Some(theirs)) => *mine == theirs,
                    _ => true,
                })
        }
    }
}

/// Agents that keep one JSONL file per session under `<root>/sessions`, found
/// by modification time and the cwd in its header: (env override, home dir,
/// directory depth).
fn dated_store(agent: &str) -> Option<(&'static str, &'static str, usize)> {
    Some(match agent {
        "codex" => ("CODEX_HOME", ".codex", 4), // YYYY/MM/DD/rollout-*.jsonl
        "interpreter" => ("INTERPRETER_HOME", ".openinterpreter", 4),
        "pi" => ("PI_CODING_AGENT_DIR", ".pi/agent", 2), // --<cwd>--/<time>_<id>.jsonl
        "omp" => ("PI_CODING_AGENT_DIR", ".omp/agent", 2),
        "prime-agent" => ("PRIME_AGENT_CODING_AGENT_DIR", ".prime/agent", 1),
        _ => return None,
    })
}

/// Which transcript dialect an agent writes (forks share their parent's).
fn format_of(agent: &str) -> &str {
    match agent {
        "interpreter" => "codex",
        "omp" | "prime-agent" => "pi",
        a => a,
    }
}

fn rollouts(dir: &Path, depth: usize, since: SystemTime, out: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let path = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() && depth > 1 {
            rollouts(&path, depth - 1, since, out);
        } else if md.is_file() && path.extension().is_some_and(|x| x == "jsonl") {
            if let Ok(m) = md.modified() {
                if m >= since {
                    out.push((path, m));
                }
            }
        }
    }
}

/// The working directory recorded in a session file's first line (Codex:
/// `payload.cwd`; Pi: `cwd`).
fn rollout_cwd(path: &Path) -> Option<PathBuf> {
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(MAX_LINE as u64).read_to_end(&mut buf).ok()?;
    let line = buf.split(|&b| b == b'\n').next()?;
    let doc = json::parse(std::str::from_utf8(line).ok()?)?;
    doc.get("payload").get("cwd").str().or(doc.get("cwd").str()).map(PathBuf::from)
}

/// "2026-11-19T01:02:23.589Z" -> unix seconds (UTC assumed).
pub fn iso_secs(s: &str) -> Option<u64> {
    let n = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (mut y, m, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, sec) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    if m <= 2 {
        y -= 1;
    }
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    u64::try_from(days * 86400 + h * 3600 + mi * 60 + sec).ok()
}

/// A command given as a string or an argv array; `sh -c <script>` wrappers
/// are reduced to the script.
fn command_text(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) if !s.is_empty() => Some(s.clone()),
        Value::Arr(a) if !a.is_empty() => {
            let args: Vec<&str> = a.iter().filter_map(|x| x.str()).collect();
            let script = args.len() >= 3 && is_shell(args[0]) && args[1].starts_with('-');
            Some(if script { args[2..].join(" ") } else { args.join(" ") })
        }
        _ => None,
    }
}

/// "Bash: cargo test" — a tool call's name plus its most telling argument.
fn tool_text(name: &str, input: &Value) -> String {
    const KEYS: [&str; 12] = [
        "command", "cmd", "file_path", "path", "pattern", "query", "url", "description", "prompt",
        "skill", "subject", "title",
    ];
    let arg = KEYS.iter().find_map(|k| command_text(input.get(k)));
    match arg {
        Some(a) => format!("{name}: {}", clean(&a, TEXT_MAX - 20)),
        None => name.to_string(),
    }
}

/// The files a tool call touches, judged by the tool's name and arguments:
/// patches name their files in the patch text, file tools in a path argument.
/// `raw` is the call's input when it's free text rather than JSON.
fn file_uses(name: &str, input: &Value, raw: Option<&str>, out: &mut Vec<(Access, String)>) {
    const PATCH: [&str; 4] = ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "*** Move to: "];
    let texts = [
        raw.map(str::to_string),
        input.get("input").str().map(str::to_string),
        input.get("patch").str().map(str::to_string),
        command_text(input.get("command")),
        command_text(input.get("cmd")),
    ];
    for text in texts.iter().flatten().filter(|t| t.contains("*** ")) {
        for line in text.lines() {
            if let Some(p) = PATCH.iter().find_map(|tag| line.strip_prefix(tag)) {
                let p = p.trim();
                if !p.is_empty() && !out.iter().any(|(_, q)| q == p) {
                    out.push((Access::Write, p.to_string()));
                }
            }
        }
    }
    let access = match name.to_ascii_lowercase().as_str() {
        "read" | "read_file" | "view" | "view_file" | "cat" | "notebookread" => Access::Read,
        "edit" | "multiedit" | "write" | "notebookedit" | "create" | "create_file" | "write_file"
        | "edit_file" | "replace" | "str_replace" | "insert" | "str_replace_based_edit_tool" => Access::Write,
        "str_replace_editor" if input.get("command").str() == Some("view") => Access::Read,
        "str_replace_editor" => Access::Write,
        _ => return,
    };
    const KEYS: [&str; 6] = ["file_path", "path", "notebook_path", "filePath", "target_file", "file"];
    if let Some(p) = KEYS.iter().find_map(|k| input.get(k).str()).map(str::trim).filter(|p| !p.is_empty()) {
        out.push((access, p.to_string()));
    }
}

/// Something a person typed, as opposed to context the harness injected
/// (`<system-reminder>`, `<environment_context>`, …).
fn prompt_text(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && !s.starts_with('<')).then(|| clean(s, TEXT_MAX))
}

/// Turn one transcript line (in dialect `agent`, see `format_of`) into feed
/// events, plus the files its tool calls touch.
pub fn parse_line(
    agent: &str,
    doc: &Value,
    now: u64,
    out: &mut Vec<(u64, Kind, String)>,
    files: &mut Vec<(u64, Access, String)>,
) {
    let at = doc.get("timestamp").str().and_then(iso_secs).unwrap_or(now);
    let mut touch = |name: &str, input: &Value, raw: Option<&str>| {
        let mut uses = Vec::new();
        file_uses(name, input, raw, &mut uses);
        files.extend(uses.into_iter().map(|(a, p)| (at, a, p)));
    };
    let mut add = |kind: Kind, text: String| {
        if !text.is_empty() {
            out.push((at, kind, text));
        }
    };
    let kind = doc.get("type").str().unwrap_or("");
    match agent {
        "claude" => {
            let content = doc.get("message").get("content");
            match kind {
                "assistant" => {
                    for c in content.arr() {
                        match c.get("type").str() {
                            Some("tool_use") => {
                                let name = c.get("name").str().unwrap_or("tool");
                                touch(name, c.get("input"), None);
                                add(Kind::Tool, tool_text(name, c.get("input")));
                            }
                            Some("text") => {
                                add(Kind::Reply, clean(c.get("text").str().unwrap_or(""), TEXT_MAX))
                            }
                            _ => {}
                        }
                    }
                }
                "user" if !doc.get("isMeta").is_true() && !doc.get("isSidechain").is_true() => {
                    if let Some(p) = content.str().and_then(prompt_text) {
                        add(Kind::Prompt, p);
                    }
                    for c in content.arr().iter().filter(|c| c.get("type").str() == Some("text")) {
                        if let Some(p) = c.get("text").str().and_then(prompt_text) {
                            add(Kind::Prompt, p);
                        }
                    }
                }
                _ => {}
            }
        }
        "codex" if kind == "response_item" => {
            let p = doc.get("payload");
            let text_of = |p: &Value| -> String {
                p.get("content")
                    .arr()
                    .iter()
                    .filter_map(|c| c.get("text").str())
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            match p.get("type").str() {
                Some("function_call") | Some("custom_tool_call") => {
                    let name = p.get("name").str().unwrap_or("tool");
                    // `arguments` is itself a JSON document, as a string.
                    let args = p.get("arguments").str().and_then(json::parse);
                    touch(name, args.as_ref().unwrap_or(&Value::Null), p.get("input").str());
                    let text = match (&args, p.get("input").str()) {
                        (Some(a), _) => tool_text(name, a),
                        (None, Some(raw)) => format!("{name}: {}", clean(raw, TEXT_MAX - 20)),
                        _ => name.to_string(),
                    };
                    add(Kind::Tool, text);
                }
                Some("local_shell_call") => {
                    touch("shell", p.get("action"), None);
                    add(Kind::Tool, tool_text("shell", p.get("action")));
                }
                Some("message") => match p.get("role").str() {
                    Some("assistant") => add(Kind::Reply, clean(&text_of(p), TEXT_MAX)),
                    Some("user") => {
                        if let Some(t) = prompt_text(&text_of(p)) {
                            add(Kind::Prompt, t);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        // {"type":"message","message":{"role":…,"content":[…]}} — Pi and forks.
        "pi" if kind == "message" => {
            let m = doc.get("message");
            let content = m.get("content");
            match m.get("role").str() {
                Some("user") => {
                    if let Some(p) = content.str().and_then(prompt_text) {
                        add(Kind::Prompt, p);
                    }
                    for c in content.arr().iter().filter(|c| c.get("type").str() == Some("text")) {
                        if let Some(p) = c.get("text").str().and_then(prompt_text) {
                            add(Kind::Prompt, p);
                        }
                    }
                }
                Some("assistant") => {
                    for c in content.arr() {
                        match c.get("type").str() {
                            Some("toolCall") => {
                                let name = c.get("name").str().unwrap_or("tool");
                                touch(name, c.get("arguments"), None);
                                add(Kind::Tool, tool_text(name, c.get("arguments")));
                            }
                            Some("text") => {
                                add(Kind::Reply, clean(c.get("text").str().unwrap_or(""), TEXT_MAX))
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        "copilot" => {
            let d = doc.get("data");
            match kind {
                "tool.execution_start" => {
                    let name = d.get("toolName").str().unwrap_or("tool");
                    touch(name, d.get("arguments"), None);
                    add(Kind::Tool, tool_text(name, d.get("arguments")));
                }
                "user.message" => add(Kind::Prompt, clean(d.get("content").str().unwrap_or(""), TEXT_MAX)),
                "assistant.message" => {
                    add(Kind::Reply, clean(d.get("content").str().unwrap_or(""), TEXT_MAX))
                }
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
