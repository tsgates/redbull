// Redbull — a minimal macOS menu-bar app that keeps your Mac awake.
//
// Clicking the menu-bar bolt opens a popover whose UI is an embedded WKWebView
// rendering a styled slider: drag left to "Off", right to "∞", with stops at
// 15m / 1h / 2h / 3h / 6h / 12h. Under the hood it runs the system `caffeinate`
// tool (caffeinate -d -i [-t N]).
//
// JS -> Rust: the web view posts "set:<index>" / "quit" via a script message
// handler. Rust -> JS: refresh() pushes the live countdown via evaluateJavaScript.

use std::cell::RefCell;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSCellImagePosition, NSFont, NSFontWeightRegular,
    NSImage, NSPopover, NSPopoverBehavior, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
    NSView, NSViewController,
};
use objc2_foundation::{
    MainThreadMarker, NSData, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSTimer,
};
use objc2_web_kit::{
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView,
    WKWebViewConfiguration,
};

/// Slider stops. Index 0 = Off, 7 = indefinite; the rest are timed (seconds).
const SECS: [u64; 7] = [0, 15 * 60, 60 * 60, 2 * 3600, 3 * 3600, 6 * 3600, 12 * 3600];
const STOPS: usize = 8; // Off, 15m, 1h, 2h, 3h, 6h, 12h, ∞

const POPOVER_W: f64 = 264.0;
const POPOVER_H: f64 = 116.0;

// --- Agent auto-watch ---------------------------------------------------------
// Coding-agent CLIs to watch for. Matched against each process's executable
// basename, or the script name when launched via an interpreter.
const AGENTS: &[&str] = &[
    "claude", "codex", "copilot", "opencode", "aider", "cursor-agent", "gemini",
    "goose", "amp", "qwen", "crush", "droid", "cody", "gptme", "auggie",
];
// An agent's process subtree counts as "working" when, between samples, it
// either transfers network data above BUSY_BYTES_PER_SEC *or* burns more than
// BUSY_CORES CPU cores. Network is the primary signal — agents spend most of
// their busy time blocked on the LLM API (lots of I/O, ~no CPU) — while CPU
// catches local work like builds/tests the agent runs. Both are measured as
// deltas between samples, so they reflect *current* activity, not lifetime use.
const BUSY_BYTES_PER_SEC: f64 = 2000.0;
const BUSY_CORES: f64 = 0.03;
/// Keep the Mac awake this long after the last observed activity, so brief lulls
/// (e.g. waiting on a model response) don't drop the assertion.
const GRACE_SECS: u64 = 180;

#[derive(Default)]
struct AppState {
    child: Option<Child>,
    expiry: Option<Instant>,
    index: usize,
    // Auto-watch mode: keep awake while coding agents are working.
    auto: bool,
    last_active: Option<Instant>,
    agents: Vec<String>, // distinct agent names currently running
    prev_cpu: std::collections::HashMap<i32, f64>, // pid -> CPU seconds at last sample
    prev_net: std::collections::HashMap<i32, u64>, // pid -> bytes in+out at last sample
    prev_sample: Option<Instant>,
}

struct Ivars {
    state: RefCell<AppState>,
    status_item: Retained<NSStatusItem>,
    popover: Retained<NSPopover>,
    webview: Retained<WKWebView>,
    _vc: Retained<NSViewController>,
    mtm: MainThreadMarker,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RedbullController"]
    #[ivars = Ivars]
    struct Controller;

    impl Controller {
        #[unsafe(method(togglePopover:))]
        fn toggle_popover(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            if iv.popover.isShown() {
                unsafe { iv.popover.performClose(None) };
            } else if let Some(button) = iv.status_item.button(iv.mtm) {
                self.refresh();
                let view: &NSView = &button;
                iv.popover.showRelativeToRect_ofView_preferredEdge(
                    view.bounds(),
                    view,
                    NSRectEdge::MinY,
                );
            }
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            let auto = {
                let mut st = self.ivars().state.borrow_mut();
                if let Some(c) = st.child.as_mut() {
                    if matches!(c.try_wait(), Ok(Some(_))) {
                        st.child = None;
                        st.expiry = None;
                        st.index = 0;
                    }
                }
                st.auto
            };
            if auto {
                self.monitor();
            }
            self.refresh();
        }
    }

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl WKScriptMessageHandler for Controller {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _ucc: &WKUserContentController, message: &WKScriptMessage) {
            let body = unsafe { message.body() };
            if let Ok(s) = body.downcast::<NSString>() {
                let cmd = s.to_string();
                if cmd == "quit" {
                    self.quit_now();
                } else if cmd == "auto:1" {
                    self.set_auto(true);
                } else if cmd == "auto:0" {
                    self.set_auto(false);
                } else if let Some(n) = cmd.strip_prefix("set:") {
                    if let Ok(i) = n.parse::<usize>() {
                        self.apply(i.min(STOPS - 1));
                    }
                }
            }
        }
    }
);

impl Controller {
    /// Apply a slider index: 0 = off, 7 = indefinite, else a timed run.
    /// Using the slider takes over from auto-watch mode.
    fn apply(&self, i: usize) {
        {
            let mut st = self.ivars().state.borrow_mut();
            let st = &mut *st;
            st.auto = false;
            st.last_active = None;
            st.index = i;
            match i {
                0 => stop(&mut st.child, &mut st.expiry),
                7 => start(&mut st.child, &mut st.expiry, None),
                k => start(&mut st.child, &mut st.expiry, Some(SECS[k])),
            }
        }
        self.refresh();
    }

    /// Toggle auto-watch mode. When on, the slider is ignored and the awake
    /// state is driven by `monitor()` (agents working -> awake).
    fn set_auto(&self, on: bool) {
        {
            let mut st = self.ivars().state.borrow_mut();
            let st = &mut *st;
            stop(&mut st.child, &mut st.expiry); // hand control to the monitor
            st.index = 0;
            st.auto = on;
            st.last_active = None;
            st.prev_cpu.clear();
            st.prev_net.clear();
            st.prev_sample = None;
            if !on {
                st.agents.clear();
            }
        }
        if on {
            self.monitor(); // scan immediately so the popover shows status now
        }
        self.refresh();
    }

    /// Auto-watch step (each tick): measure how much network I/O and CPU each
    /// agent's process subtree used since the last sample. While any are working
    /// (or were within the grace window) keep the Mac awake; else let it sleep.
    fn monitor(&self) {
        use std::collections::HashMap;
        let (agents, cpu_now) = scan_agents();
        let net_now = net_bytes();
        let now = Instant::now();
        let mut st = self.ivars().state.borrow_mut();
        let st = &mut *st;

        let elapsed = st
            .prev_sample
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(1.0)
            .max(0.2);

        // CPU-seconds and network bytes consumed across the agent subtrees.
        let mut cpu_delta = 0.0f64;
        let mut net_delta = 0u64;
        let mut net_keep: HashMap<i32, u64> = HashMap::new();
        for (pid, c) in &cpu_now {
            let pc = st.prev_cpu.get(pid).copied().unwrap_or(*c); // new pid -> no delta
            cpu_delta += (c - pc).max(0.0);
            let b = net_now.get(pid).copied().unwrap_or(0);
            let pb = st.prev_net.get(pid).copied().unwrap_or(b);
            net_delta += b.saturating_sub(pb);
            net_keep.insert(*pid, b);
        }
        let busy = !agents.is_empty()
            && (net_delta as f64 / elapsed >= BUSY_BYTES_PER_SEC
                || cpu_delta / elapsed >= BUSY_CORES);

        st.prev_cpu = cpu_now;
        st.prev_net = net_keep;
        st.prev_sample = Some(now);
        st.agents = agents;
        if busy {
            st.last_active = Some(now);
        }
        let keep = st
            .last_active
            .map(|t| now.duration_since(t).as_secs() < GRACE_SECS)
            .unwrap_or(false);
        if keep && st.child.is_none() {
            start(&mut st.child, &mut st.expiry, None); // indefinite while working
        } else if !keep && st.child.is_some() {
            stop(&mut st.child, &mut st.expiry);
        }
    }

    fn quit_now(&self) {
        {
            let mut st = self.ivars().state.borrow_mut();
            let st = &mut *st;
            stop(&mut st.child, &mut st.expiry);
        }
        NSApplication::sharedApplication(self.ivars().mtm).terminate(None);
    }

    /// Sync the menu-bar icon + countdown and push state into the web view.
    fn refresh(&self) {
        let iv = self.ivars();
        let st = iv.state.borrow();
        let active = st.child.is_some();

        if let Some(button) = iv.status_item.button(iv.mtm) {
            let (rgba, w, h) = bolt_rgba(active);
            let png = encode_png(&rgba, w as usize, h as usize);
            let data = NSData::with_bytes(&png);
            if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
                img.setTemplate(true);
                img.setSize(NSSize::new(w as f64 / h as f64 * 18.0, 18.0));
                button.setImage(Some(&img));
            }
            let title = if st.auto {
                String::new() // bolt brightness conveys awake state in auto mode
            } else {
                match (active, st.expiry) {
                    (true, Some(until)) => format!(" {}", remaining_label(until)),
                    (true, None) => " ∞".to_string(),
                    _ => String::new(),
                }
            };
            button.setTitle(&ns(&title));
        }

        let time_str = match (active, st.expiry) {
            (false, _) => String::new(),
            (true, Some(until)) => remaining_label(until),
            (true, None) => "∞".to_string(),
        };
        let n = st.agents.len();
        // Fallback text for non-active auto states (active shows count + icon).
        let auto_text = if n == 0 { "watching" } else { "idle" };
        let js = format!(
            "window.redbullSet&&redbullSet({},{:?});window.redbullAuto&&redbullAuto({},{},{},{:?})",
            st.index, time_str, st.auto as u8, active as u8, n, auto_text
        );
        unsafe {
            iv.webview
                .evaluateJavaScript_completionHandler(&ns(&js), None);
        }
    }
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn main() {
    // Debug: `redbull scan` samples agents twice (~1.5s apart) and reports the
    // recent network + CPU usage and whether that counts as "working".
    if std::env::args().any(|a| a == "scan") {
        let (names, ca) = scan_agents();
        let na = net_bytes();
        std::thread::sleep(Duration::from_millis(1500));
        let (_, cb) = scan_agents();
        let nb = net_bytes();
        let (mut cpu, mut net) = (0.0f64, 0u64);
        for (pid, c) in &cb {
            cpu += (c - ca.get(pid).copied().unwrap_or(*c)).max(0.0);
            let b = nb.get(pid).copied().unwrap_or(0);
            net += b.saturating_sub(na.get(pid).copied().unwrap_or(b));
        }
        let (cores, bps) = (cpu / 1.5, net as f64 / 1.5);
        let busy = !names.is_empty() && (bps >= BUSY_BYTES_PER_SEC || cores >= BUSY_CORES);
        println!("agents: {names:?}  net: {bps:.0} B/s  cpu: {cores:.3} cores  busy: {busy}");
        return;
    }

    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let controller = build_controller(mtm);
    controller.refresh();

    let target: &AnyObject = &controller;
    unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            1.0,
            target,
            sel!(tick:),
            None,
            true,
        );
    }

    app.run();
}

fn build_controller(mtm: MainThreadMarker) -> Retained<Controller> {
    let status_item =
        NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status_item.button(mtm) {
        button.setImagePosition(NSCellImagePosition::ImageLeft);
        // Tabular (monospaced) digits so the countdown never changes width.
        let font = unsafe {
            NSFont::monospacedDigitSystemFontOfSize_weight(NSFont::systemFontSize(), NSFontWeightRegular)
        };
        button.setFont(Some(&font));
    }

    // --- WKWebView hosting the slider UI ----------------------------------
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    let ucc = unsafe { config.userContentController() };
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(POPOVER_W, POPOVER_H));
    let webview =
        unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config) };
    unsafe { webview.loadHTMLString_baseURL(&ns(HTML), None) };

    let vc = NSViewController::new(mtm);
    vc.setView(&webview);

    let popover = NSPopover::new(mtm);
    popover.setBehavior(NSPopoverBehavior::Transient);
    popover.setContentSize(NSSize::new(POPOVER_W, POPOVER_H));
    popover.setContentViewController(Some(&vc));

    let ivars = Ivars {
        state: RefCell::new(AppState::default()),
        status_item: status_item.clone(),
        popover,
        webview,
        _vc: vc,
        mtm,
    };
    let this = Controller::alloc(mtm).set_ivars(ivars);
    let controller: Retained<Controller> = unsafe { msg_send![super(this), init] };

    // Wire JS -> Rust message handler and the status-button click.
    let handler = ProtocolObject::from_ref(&*controller);
    unsafe { ucc.addScriptMessageHandler_name(handler, &ns("rb")) };

    let target: &AnyObject = &controller;
    if let Some(button) = status_item.button(mtm) {
        unsafe {
            button.setTarget(Some(target));
            button.setAction(Some(sel!(togglePopover:)));
        }
    }

    controller
}

// --- caffeinate process management -------------------------------------------

fn start(child: &mut Option<Child>, expiry: &mut Option<Instant>, secs: Option<u64>) {
    stop(child, expiry);
    let mut cmd = Command::new("caffeinate");
    cmd.arg("-d").arg("-i");
    if let Some(s) = secs {
        cmd.arg("-t").arg(s.to_string());
    }
    match cmd.spawn() {
        Ok(c) => {
            *child = Some(c);
            *expiry = secs.map(|s| Instant::now() + Duration::from_secs(s));
        }
        Err(e) => eprintln!("redbull: failed to launch caffeinate: {e}"),
    }
}

fn stop(child: &mut Option<Child>, expiry: &mut Option<Instant>) {
    if let Some(mut c) = child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    *expiry = None;
}

// --- Coding-agent detection --------------------------------------------------

/// Scan running processes for coding agents. Returns the distinct agent names
/// and a map of every agent-subtree pid -> accumulated CPU seconds, so the
/// caller can derive *recent* activity from the delta between two samples.
///
/// Matches against `comm` (the executable path, no arguments) so directory or
/// argument paths that merely contain an agent's name don't false-positive.
fn scan_agents() -> (Vec<String>, std::collections::HashMap<i32, f64>) {
    use std::collections::{HashMap, HashSet};
    let out = match Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,time=,comm="])
        .output()
    {
        Ok(o) => o.stdout,
        Err(_) => return (Vec::new(), HashMap::new()),
    };
    let text = String::from_utf8_lossy(&out);

    let mut cpu_of: HashMap<i32, f64> = HashMap::new();
    let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
    let mut roots: Vec<i32> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let self_pid = std::process::id() as i32;

    for line in text.lines() {
        let mut it = line.split_whitespace();
        let pid: i32 = match it.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let ppid: i32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let cpu = parse_cputime(it.next().unwrap_or("0"));
        let comm = it.collect::<Vec<_>>().join(" "); // executable path (may contain spaces)
        let base = comm.rsplit('/').next().unwrap_or(&comm);
        cpu_of.insert(pid, cpu);
        children.entry(ppid).or_default().push(pid);
        if pid != self_pid && AGENTS.contains(&base) {
            roots.push(pid);
            let name = base.to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }

    // Walk each agent's subtree, collecting CPU seconds per pid (deduped).
    let mut sub: HashMap<i32, f64> = HashMap::new();
    let mut seen = HashSet::new();
    let mut stack = roots;
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        if let Some(c) = cpu_of.get(&p) {
            sub.insert(p, *c);
        }
        if let Some(ch) = children.get(&p) {
            stack.extend(ch);
        }
    }
    names.sort();
    (names, sub)
}

/// Per-process cumulative network bytes (in + out) via `nettop`, keyed by pid.
/// Diffing two snapshots yields the bytes transferred in between — the signal
/// that an agent is actively talking to the LLM API.
fn net_bytes() -> std::collections::HashMap<i32, u64> {
    use std::collections::HashMap;
    let mut map = HashMap::new();
    let out = match Command::new("nettop")
        .args(["-P", "-L", "1", "-J", "bytes_in,bytes_out", "-n", "-x"])
        .output()
    {
        Ok(o) => o.stdout,
        Err(_) => return map,
    };
    for line in String::from_utf8_lossy(&out).lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 3 {
            continue;
        }
        // f[0] is "name.pid" (the name may contain dots/spaces; pid is last).
        let pid = match f[0].rsplit('.').next().and_then(|s| s.trim().parse::<i32>().ok()) {
            Some(p) => p,
            None => continue,
        };
        let bytes = f[1].trim().parse::<u64>().unwrap_or(0) + f[2].trim().parse::<u64>().unwrap_or(0);
        *map.entry(pid).or_insert(0) += bytes;
    }
    map
}

/// Parse a `ps -o time` value ("[D-][H:]M:SS.ss") into seconds.
fn parse_cputime(s: &str) -> f64 {
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

/// Remaining-time label whose resolution sharpens as the timer winds down:
///   ≥ 1h → whole hours; 10–59m → nearest 5 min; < 10m → every minute.
fn remaining_label(until: Instant) -> String {
    let secs = until.saturating_duration_since(Instant::now()).as_secs();
    let m = ((secs + 59) / 60).max(1);
    if m >= 60 {
        format!("{}h", m / 60)
    } else if m >= 10 {
        format!("{}m", (((m + 2) / 5) * 5).min(55))
    } else {
        format!("{}m", m)
    }
}

// --- Lightning-bolt template icon (anti-aliased; dimmed when idle) ------------

fn bolt_rgba(active: bool) -> (Vec<u8>, u32, u32) {
    const BOLT: [(f64, f64); 6] = [
        (13.0, 2.0), (3.0, 14.0), (12.0, 14.0), (11.0, 22.0), (21.0, 10.0), (12.0, 10.0),
    ];
    const SCALE: f64 = 4.0;
    const MARGIN: f64 = 2.0;
    let (min_x, min_y) = (3.0 - MARGIN, 2.0 - MARGIN);
    let w = ((21.0 - 3.0 + 2.0 * MARGIN) * SCALE).ceil() as usize;
    let h = ((22.0 - 2.0 + 2.0 * MARGIN) * SCALE).ceil() as usize;
    let poly: Vec<(f64, f64)> = BOLT
        .iter()
        .map(|&(x, y)| ((x - min_x) * SCALE, (y - min_y) * SCALE))
        .collect();
    let opacity = if active { 1.0 } else { 0.40 };

    let inside = |px: f64, py: f64| -> bool {
        let n = poly.len();
        let mut c = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = poly[i];
            let (xj, yj) = poly[j];
            if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
                c = !c;
            }
            j = i;
        }
        c
    };

    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut hits = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f64 + (sx as f64 + 0.5) / 4.0;
                    let py = y as f64 + (sy as f64 + 0.5) / 4.0;
                    if inside(px, py) {
                        hits += 1;
                    }
                }
            }
            rgba[(y * w + x) * 4 + 3] = (hits as f64 / 16.0 * opacity * 255.0).round() as u8;
        }
    }
    (rgba, w as u32, h as u32)
}

// --- Minimal PNG encoder: 8-bit RGBA, stored (uncompressed) deflate ----------

fn encode_png(rgba: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut raw = Vec::with_capacity(h * (1 + w * 4));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut zlib = vec![0x78, 0x01];
    let mut pos = 0;
    while pos < raw.len() {
        let n = (raw.len() - pos).min(65535);
        zlib.push(if pos + n >= raw.len() { 1 } else { 0 });
        zlib.extend_from_slice(&(n as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(n as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + n]);
        pos += n;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(kind);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

// --- Embedded slider UI ------------------------------------------------------

const HTML: &str = r##"<!DOCTYPE html><html><head><meta charset="utf-8"><style>
*{margin:0;padding:0;box-sizing:border-box}
html,body{background:#26262b;color:#f2f2f4;font-family:-apple-system,BlinkMacSystemFont,'SF Pro Text',sans-serif;-webkit-user-select:none;user-select:none;overflow:hidden}
.wrap{padding:16px 18px 12px}
.head{display:flex;align-items:center;gap:9px;margin-bottom:4px}
.logo{width:22px;height:22px;background:#E24B4A;border-radius:6px;display:flex;align-items:center;justify-content:center}
.name{font-size:14px;font-weight:600}
.pill{margin-left:6px;flex:none;font-size:12px;padding:3px 9px;border-radius:999px;font-weight:600;white-space:nowrap;background:#E24B4A;color:#26262b}
.name{flex:none}
.pill.off{background:rgba(255,255,255,.14);color:#cdcdd1}
.auto{margin-left:auto;width:24px;height:24px;flex:none;display:flex;align-items:center;justify-content:center;padding:0;border:1px solid rgba(255,255,255,.16);background:transparent;color:#a9a9ad;border-radius:7px;cursor:pointer}
.auto:hover{color:#fff;border-color:rgba(255,255,255,.32)}
.auto.on{background:#E24B4A;border-color:transparent;color:#fff}
.auto svg{display:block}
.auto.on svg{transform-origin:center;animation:rbpulse 1.1s ease-in-out infinite}
@keyframes rbpulse{0%,100%{opacity:1;transform:scale(1)}50%{opacity:.4;transform:scale(.8)}}
.x{margin-left:6px;width:20px;height:20px;flex:none;display:flex;align-items:center;justify-content:center;border:none;background:transparent;color:#8e8e93;font-size:16px;line-height:1;cursor:pointer;border-radius:5px;font-family:inherit}
.x:hover{background:rgba(255,255,255,.12);color:#fff}
.slider{position:relative;height:28px;margin:18px 8px 0}
.track{position:absolute;top:12px;left:0;right:0;height:4px;background:rgba(255,255,255,.15);border-radius:2px}
.fill{position:absolute;top:12px;left:0;height:4px;background:#E24B4A;border-radius:2px}
.tick{position:absolute;top:9px;width:2px;height:10px;margin-left:-1px;border-radius:1px;background:rgba(255,255,255,.28)}
.thumb{position:absolute;top:5px;width:18px;height:18px;margin-left:-9px;background:#fff;border-radius:50%;box-shadow:0 1px 3px rgba(0,0,0,.5)}
.range{position:absolute;top:3px;left:-9px;width:calc(100% + 18px);height:22px;margin:0;opacity:0;cursor:pointer}
.labels{display:flex;justify-content:space-between;margin:7px 2px 0;font-size:10px;color:#8e8e93}
</style></head><body><div class="wrap">
<div class="head">
<span class="logo"><svg width="13" height="13" viewBox="0 0 24 24"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" fill="#fff"/></svg></span>
<span class="name">Redbull</span>
<button class="auto" id="auto" title="Keep awake while coding agents (claude, codex, copilot, opencode, …) are working">
<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 12 7 12 10 5 14 19 17 12 21 12"/></svg></button>
<span class="pill off" id="pill">Off</span>
<button class="x" onclick="post('quit')" title="Quit" aria-label="Quit">&times;</button>
</div>
<div class="slider">
<div class="track"></div><div class="fill" id="fill"></div><div id="ticks"></div>
<div class="thumb" id="thumb"></div>
<input class="range" id="rng" type="range" min="0" max="7" step="1" value="0">
</div>
<div class="labels" id="labels"></div>
</div><script>
var LAB=["Off","15m","1h","2h","3h","6h","12h","∞"];
var BOLT_ICON='<svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" style="vertical-align:-1px"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/></svg>';
var AGENT_ICON='<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="vertical-align:-3px"><rect x="4" y="8" width="16" height="11" rx="3.5"/><path d="M12 8V4.6"/><circle cx="12" cy="3.3" r="1.3" fill="currentColor" stroke="none"/><circle cx="9" cy="13.4" r="1.35" fill="currentColor" stroke="none"/><circle cx="15" cy="13.4" r="1.35" fill="currentColor" stroke="none"/><path d="M2.5 13v3M21.5 13v3"/></svg>';
var rng=document.getElementById('rng'),fill=document.getElementById('fill'),thumb=document.getElementById('thumb'),pill=document.getElementById('pill'),labels=document.getElementById('labels'),ticks=document.getElementById('ticks');
LAB.forEach(function(s,i){var p=i/7*100;var t=document.createElement('div');t.className='tick';t.style.left=p+'%';ticks.appendChild(t);var l=document.createElement('span');l.textContent=s;labels.appendChild(l);});
function post(m){try{window.webkit.messageHandlers.rb.postMessage(m);}catch(e){}}
function paint(v,time){var p=v/7*100;fill.style.width=p+'%';thumb.style.left=p+'%';var off=(v==0);fill.style.opacity=off?0:1;if(off){pill.textContent='Off';pill.className='pill off';}else{var t=(time!=null&&time!=='')?time:LAB[v];pill.innerHTML=BOLT_ICON+' '+t;pill.className='pill';}for(var i=0;i<labels.children.length;i++){labels.children[i].style.color=(i==v)?'#fff':'#8e8e93';}}
rng.addEventListener('input',function(){var v=+rng.value;paint(v);post('set:'+v);});
window.redbullSet=function(v,text){rng.value=v;paint(v,text);};
var autoBtn=document.getElementById('auto'),slider=document.querySelector('.slider');
autoBtn.addEventListener('click',function(){post('auto:'+(autoBtn.classList.contains('on')?0:1));});
window.redbullAuto=function(on,awake,count,text){
  on=!!on;autoBtn.classList.toggle('on',on);
  slider.style.opacity=on?0.35:1;slider.style.pointerEvents=on?'none':'auto';rng.disabled=on;
  if(on){
    if(awake&&count>0){pill.innerHTML=count+' '+AGENT_ICON;pill.className='pill';}
    else{pill.textContent=text;pill.className='pill off';}
  }
};
paint(0);
</script></body></html>"##;
