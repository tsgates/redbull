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
const POPOVER_H: f64 = 120.0;

#[derive(Default)]
struct AppState {
    child: Option<Child>,
    expiry: Option<Instant>,
    index: usize,
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
            {
                let mut st = self.ivars().state.borrow_mut();
                if let Some(c) = st.child.as_mut() {
                    if matches!(c.try_wait(), Ok(Some(_))) {
                        st.child = None;
                        st.expiry = None;
                        st.index = 0;
                    }
                }
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
    fn apply(&self, i: usize) {
        {
            let mut st = self.ivars().state.borrow_mut();
            let st = &mut *st;
            st.index = i;
            match i {
                0 => stop(&mut st.child, &mut st.expiry),
                7 => start(&mut st.child, &mut st.expiry, None),
                k => start(&mut st.child, &mut st.expiry, Some(SECS[k])),
            }
        }
        self.refresh();
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
            let title = match (active, st.expiry) {
                (true, Some(until)) => format!(" {}", remaining_label(until)),
                (true, None) => " ∞".to_string(),
                _ => String::new(),
            };
            button.setTitle(&ns(&title));
        }

        let state_str = match (active, st.expiry) {
            (false, _) => "Off".to_string(),
            (true, Some(until)) => format!("Awake · {}", remaining_label(until).trim_start()),
            (true, None) => "Awake · ∞".to_string(),
        };
        let js = format!("window.redbullSet&&redbullSet({},{:?})", st.index, state_str);
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

/// Remaining-time label whose resolution sharpens as the timer winds down:
///   ≥ 1h → whole hours; 10–59m → nearest 5 min; < 10m → every minute.
fn remaining_label(until: Instant) -> String {
    let secs = until.saturating_duration_since(Instant::now()).as_secs();
    let m = ((secs + 59) / 60).max(1);
    let (n, unit) = if m >= 60 {
        (m / 60, 'h')
    } else if m >= 10 {
        ((((m + 2) / 5) * 5).min(55), 'm')
    } else {
        (m, 'm')
    };
    // Pad to a fixed two-digit field with a figure space (U+2007, digit-width
    // and invisible) so the menu-bar item never changes width as time ticks.
    if n < 10 {
        format!("\u{2007}{n}{unit}")
    } else {
        format!("{n}{unit}")
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
.pill{margin-left:auto;font-size:12px;padding:3px 9px;border-radius:999px;font-weight:600;background:#E24B4A;color:#26262b}
.pill.off{background:rgba(255,255,255,.14);color:#cdcdd1}
.x{margin-left:6px;width:20px;height:20px;flex:none;display:flex;align-items:center;justify-content:center;border:none;background:transparent;color:#8e8e93;font-size:16px;line-height:1;cursor:pointer;border-radius:5px;font-family:inherit}
.x:hover{background:rgba(255,255,255,.12);color:#fff}
.sub{font-size:11px;color:#a9a9ad;margin:9px 2px 16px}
.slider{position:relative;height:28px;margin:0 8px}
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
<span class="pill off" id="pill">Off</span>
<button class="x" onclick="post('quit')" title="Quit" aria-label="Quit">&times;</button>
</div>
<div class="sub">Drag to set how long your Mac stays awake</div>
<div class="slider">
<div class="track"></div><div class="fill" id="fill"></div><div id="ticks"></div>
<div class="thumb" id="thumb"></div>
<input class="range" id="rng" type="range" min="0" max="7" step="1" value="0">
</div>
<div class="labels" id="labels"></div>
</div><script>
var LAB=["Off","15m","1h","2h","3h","6h","12h","∞"];
var rng=document.getElementById('rng'),fill=document.getElementById('fill'),thumb=document.getElementById('thumb'),pill=document.getElementById('pill'),labels=document.getElementById('labels'),ticks=document.getElementById('ticks');
LAB.forEach(function(s,i){var p=i/7*100;var t=document.createElement('div');t.className='tick';t.style.left=p+'%';ticks.appendChild(t);var l=document.createElement('span');l.textContent=s;labels.appendChild(l);});
function post(m){try{window.webkit.messageHandlers.rb.postMessage(m);}catch(e){}}
function paint(v,text){var p=v/7*100;fill.style.width=p+'%';thumb.style.left=p+'%';var off=(v==0);fill.style.opacity=off?0:1;pill.textContent=(text!=null)?text:(off?'Off':'Awake · '+LAB[v]);pill.className=off?'pill off':'pill';for(var i=0;i<labels.children.length;i++){labels.children[i].style.color=(i==v)?'#fff':'#8e8e93';}}
rng.addEventListener('input',function(){var v=+rng.value;paint(v);post('set:'+v);});
window.redbullSet=function(v,text){rng.value=v;paint(v,text);};
paint(0);
</script></body></html>"##;
