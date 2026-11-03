#!/usr/bin/env python3
"""Live end-to-end test of Redbull's agent detection against real agent CLIs.

Launches each agent's real binary in its own pseudo-terminal (with a throwaway
HOME, so nothing touches your config and no login is needed), then checks with
`redbull scan` that each one is:

  1. recognized   — found as its own session, with the right name
  2. idle         — not counted as working while it sits at its prompt
  3. store        — working while its session transcript/DB is appended to
  4. tty          — working once its terminal gets output

npm-distributed agents are installed into --agents-dir on first run. Others
(claude, copilot, aider, goose) are used from PATH or `--bin name=path`.

usage: scripts/live-agents-test.py [--agents-dir DIR] [--only a,b,...] [--bin name=path ...]
"""

import argparse, fcntl, os, re, select, shutil, signal, subprocess, sys, tempfile, termios, threading, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REDBULL = os.path.join(ROOT, "target", "debug", "redbull")

# name -> (npm package or None for PATH, bin, store file relative to HOME
#          or "CWD/…" for project-local stores)
AGENTS = {
    # {cwd} = the project dir as Claude encodes it; copilot marks the session
    # it uses with inuse.<pid>.lock
    "claude":   (None, "claude", ".claude/projects/{cwd}/s.jsonl"),
    "copilot":  (None, "copilot", ".copilot/session-state/s1/events.jsonl"),
    "codex":    ("@openai/codex", "codex", ".codex/sessions/2026/11/01/rollout-x.jsonl"),
    "gemini":   ("@google/gemini-cli", "gemini", ".gemini/tmp/proj/chats/session-x.jsonl"),
    "opencode": ("opencode-ai", "opencode", ".local/share/opencode/opencode.db-wal"),
    "qwen":     ("@qwen-code/qwen-code", "qwen", ".qwen/projects/p/chats/x.jsonl"),
    "amp":      ("@sourcegraph/amp", "amp", ".local/share/amp/threads/T-x.json"),
    "crush":    ("@charmland/crush", "crush", "CWD/.crush/crush.db-wal"),
    "auggie":   ("@augmentcode/auggie", "auggie", ".augment/sessions/x.json"),
    "kilo":     ("@kilocode/cli", "kilo", ".local/share/kilo/kilo.db-wal"),
    "cline":    ("cline", "cline", ".cline/data/sessions/s1/s1.messages.json"),
    "continue": ("@continuedev/cli", "cn", ".continue/sessions/x.json"),
    # not on npm: from PATH, or --bin aider=… / --bin goose=…
    "aider":    (None, "aider", "CWD/.aider.chat.history.md"),
    "goose":    (None, "goose", ".local/share/goose/sessions/sessions.db-wal"),
}

LINE = re.compile(r"^(\S+)\s+pid:(\d+)\s+procs:(\d+)\s+status:(\S+)\s+tty:\s*(\S+)\s+cpu:\s*(\S+)\s+(WORKING|idle) \((\S+)\)")


def scan(home):
    env = dict(os.environ, HOME=home)
    out = subprocess.run([REDBULL, "scan", "1"], env=env, capture_output=True, text=True).stdout
    rows = {}
    for line in out.splitlines():
        m = LINE.match(line)
        if m:
            rows[int(m.group(2))] = dict(agent=m.group(1), status=m.group(4), tty=m.group(5),
                                         working=m.group(7) == "WORKING", reason=m.group(8))
    return rows


def descendants(pid):
    out = subprocess.run(["ps", "-axo", "pid=,ppid="], capture_output=True, text=True).stdout
    kids = {}
    for l in out.split("\n"):
        p = l.split()
        if len(p) == 2:
            kids.setdefault(int(p[1]), []).append(int(p[0]))
    seen, stack = [], [pid]
    while stack:
        p = stack.pop()
        seen.append(p)
        stack.extend(kids.get(p, []))
    return seen


def spawn(argv, home, cwd):
    """Run argv on a new pty as its controlling terminal, double-forked so it
    is reparented to launchd like an agent started from a terminal (an agent
    launched from inside another agent counts as that agent's tool).
    Returns (pid, master fd, slave tty path)."""
    master, slave = os.openpty()
    tty = os.ttyname(slave)
    r, w = os.pipe()
    if os.fork() == 0:
        pid = os.fork()
        if pid == 0:
            os.setsid()
            os.close(master)
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            for fd in (0, 1, 2):
                os.dup2(slave, fd)
            os.chdir(cwd)
            os.execvpe(argv[0], argv, dict(os.environ, HOME=home, TERM="xterm-256color"))
        os.write(w, str(pid).encode())
        os._exit(0)
    os.close(w)
    os.close(slave)
    pid = int(os.read(r, 32))
    os.close(r)
    os.wait()
    return pid, master, tty


def drain(fds):
    r, _, _ = select.select(fds, [], [], 0)
    for fd in r:
        try:
            os.read(fd, 1 << 16)
        except OSError:
            pass


def touch(path, age=0):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "a"):
        pass
    t = time.time() - age
    os.utime(path, (t, t))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--agents-dir", default=os.path.join(tempfile.gettempdir(), "redbull-live-agents"))
    ap.add_argument("--only", default="")
    ap.add_argument("--bin", action="append", default=[], help="name=path of an agent binary")
    args = ap.parse_args()
    bins = dict(b.split("=", 1) for b in args.bin)
    only = set(filter(None, args.only.split(",")))
    agents = {k: v for k, v in AGENTS.items() if not only or k in only}

    subprocess.run(["cargo", "build", "-q"], cwd=ROOT, check=True)
    pkgs = [v[0] for v in agents.values() if v[0]]
    nm = os.path.join(args.agents_dir, "node_modules", ".bin")
    missing = [p for p, _, _ in [agents[k] for k in agents] if p and not os.path.exists(os.path.join(nm, AGENTS_BIN[p]))]
    if missing:
        os.makedirs(args.agents_dir, exist_ok=True)
        print(f"installing {len(missing)} agents into {args.agents_dir} …")
        subprocess.run(["npm", "install", "--prefix", args.agents_dir, "--no-fund", "--no-audit", *missing],
                       check=True, capture_output=True)

    home = tempfile.mkdtemp(prefix="redbull-home-")
    procs = {}
    for name, (pkg, binary, store) in agents.items():
        exe = bins.get(name) or (os.path.join(nm, binary) if pkg else shutil.which(binary))
        if not exe:
            print(f"SKIP {name}: not installed")
            continue
        cwd = tempfile.mkdtemp(prefix=f"redbull-proj-{name}-")
        pid, fd, tty = spawn([exe], home, cwd)
        procs[name] = dict(pid=pid, fd=fd, tty=tty, cwd=cwd, store=store)

    results = {n: {} for n in procs}

    def wait(secs):
        end = time.time() + secs
        while time.time() < end:
            drain([p["fd"] for p in procs.values()])
            time.sleep(0.2)

    def find(name):
        rows = scan(home)
        tree = descendants(procs[name]["pid"])
        hits = [dict(r, pid=p) for p, r in rows.items() if p in tree]
        return hits

    try:
        # Let every TUI boot and go quiet (tty window is 15s).
        wait(22)

        # 1+2: recognized and idle.
        for name in procs:
            hits = find(name)
            ok_rec = len(hits) == 1 and hits[0]["agent"] == name
            results[name]["recognized"] = ok_rec or f"got {hits}"
            results[name]["idle"] = (ok_rec and not hits[0]["working"]) or f"got {hits}"

        # 3: a store write makes it working (status-reporting claude excepted).
        for name, p in procs.items():
            rel = p["store"].replace("{cwd}", os.path.realpath(p["cwd"]).replace("/", "-").replace(".", "-"))
            path = os.path.join(p["cwd"], rel[4:]) if rel.startswith("CWD/") else os.path.join(home, rel)
            if name == "copilot":
                hits = find(name)
                touch(os.path.join(os.path.dirname(path), f"inuse.{hits[0]['pid'] if hits else 0}.lock"))
            # Append continuously while redbull samples, like a working agent.
            stop = threading.Event()
            def writer():
                while not stop.is_set():
                    touch(path)
                    time.sleep(0.25)
            th = threading.Thread(target=writer)
            th.start()
            try:
                hits = find(name)
            finally:
                stop.set()
                th.join()
            if hits and hits[0]["status"] != "-":
                results[name]["store"] = f"n/a (self-reports status: {hits[0]['status']})"
            else:
                results[name]["store"] = (bool(hits) and hits[0]["reason"] == "store") or f"got {hits}"
            touch(path, age=600)  # quiet again

        # 4: terminal output makes it working.
        for name, p in procs.items():
            with open(p["tty"], "w") as t:
                t.write("\x1b[s.\x1b[u")  # a byte of output, cursor restored
            hits = find(name)
            results[name]["tty"] = (bool(hits) and hits[0]["working"] and hits[0]["reason"] in ("tty", "status")) or f"got {hits}"
    finally:
        for p in procs.values():
            for q in reversed(descendants(p["pid"])):
                try:
                    os.kill(q, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        shutil.rmtree(home, ignore_errors=True)

    failed = 0
    print(f"\n{'agent':<10} {'recognized':<11} {'idle':<6} {'store':<6} {'tty':<6}")
    for name, r in results.items():
        cells = []
        for k in ("recognized", "idle", "store", "tty"):
            v = r.get(k)
            if v is True:
                cells.append("ok")
            elif isinstance(v, str) and v.startswith("n/a"):
                cells.append("n/a")
            else:
                cells.append("FAIL")
                failed += 1
        print(f"{name:<10} {cells[0]:<11} {cells[1]:<6} {cells[2]:<6} {cells[3]:<6}")
        for k, v in r.items():
            if v is not True:
                print(f"    {k}: {v}")
    print(f"\n{len(results)} agents, {failed} failures")
    sys.exit(1 if failed else 0)


AGENTS_BIN = {v[0]: v[1] for v in AGENTS.values() if v[0]}

if __name__ == "__main__":
    main()
