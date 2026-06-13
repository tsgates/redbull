// Redbull — a minimal macOS menu-bar app that keeps your Mac awake.
//
// Under the hood it runs the system `caffeinate` tool, e.g.:
//
//     caffeinate -d -i -t 3600
//
// (-d = keep display awake, -i = prevent idle sleep, -t = for N seconds;
//  omit -t to stay awake indefinitely). Pick a duration from the menu-bar
//  icon's menu; "Turn Off" (or Quit) releases the wake assertion.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Selectable wake durations. `None` means indefinite (no `-t` timeout).
const DURATIONS: [(&str, Option<u64>); 7] = [
    ("15 m", Some(15 * 60)),
    ("1 h", Some(60 * 60)),
    ("2 h", Some(2 * 60 * 60)),
    ("3 h", Some(3 * 60 * 60)),
    ("6 h", Some(6 * 60 * 60)),
    ("12 h", Some(12 * 60 * 60)),
    ("∞", None),
];

/// Things that wake the (otherwise fully idle) event loop. Using an explicit
/// user event lets us run with `ControlFlow::Wait` — the loop blocks at ~0% CPU
/// and is woken only by a menu click or the once-a-second countdown tick.
enum UserEvent {
    Menu(MenuEvent),
    Tick,
}

fn main() {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    // Accessory => no Dock icon, no menu, lives only in the menu bar.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    // Forward menu clicks into the event loop so it wakes only when needed,
    // instead of polling. (Recommended by the tray-icon docs for tao users.)
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    // A lightweight 1 Hz heartbeat to refresh the countdown and to notice when
    // caffeinate exits on its own. One wakeup per second — negligible CPU.
    let tick_proxy = event_loop.create_proxy();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        if tick_proxy.send_event(UserEvent::Tick).is_err() {
            break; // event loop has shut down
        }
    });

    // --- Menu ---------------------------------------------------------------
    let menu = Menu::new();

    // Disabled line at the top that mirrors the current state.
    let status = MenuItem::new("Off", false, None);
    menu.append(&status).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();

    // One checkable item per duration; they behave like radio buttons.
    let durations: Vec<(CheckMenuItem, Option<u64>)> = DURATIONS
        .iter()
        .map(|&(label, secs)| {
            let item = CheckMenuItem::new(label, true, false, None);
            menu.append(&item).unwrap();
            (item, secs)
        })
        .collect();

    menu.append(&PredefinedMenuItem::separator()).unwrap();
    let turn_off = MenuItem::new("Turn Off", false, None); // enabled only while awake
    menu.append(&turn_off).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&quit).unwrap();

    let turn_off_id = turn_off.id().clone();
    let quit_id = quit.id().clone();

    // The tray icon must be created on the main thread *after* the event loop
    // exists, so we build it in the Init event below.
    let mut menu = Some(menu);
    let mut tray: Option<TrayIcon> = None;
    let mut child: Option<Child> = None;
    let mut expiry: Option<Instant> = None;

    event_loop.run(move |event, _, control_flow| {
        // Block until something actually happens (menu click or 1 Hz tick).
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                match TrayIconBuilder::new()
                    .with_menu(Box::new(menu.take().unwrap()))
                    .with_tooltip("Redbull — keep your Mac awake")
                    .with_icon(make_icon(false))
                    .with_icon_as_template(true) // adapt to light/dark menu bar
                    .build()
                {
                    Ok(t) => tray = Some(t),
                    Err(e) => eprintln!("redbull: tray build FAILED: {e}"),
                }
            }

            // --- Menu clicks ----------------------------------------------
            Event::UserEvent(UserEvent::Menu(ev)) => {
                if ev.id == quit_id {
                    stop(&mut child, &mut expiry);
                    *control_flow = ControlFlow::Exit;
                    return;
                } else if ev.id == turn_off_id {
                    stop(&mut child, &mut expiry);
                    set_checked(&durations, None);
                } else if let Some(entry) = durations.iter().find(|(item, _)| ev.id == *item.id()) {
                    // The clicked CheckMenuItem just flipped its own checkmark.
                    if entry.0.is_checked() {
                        start(&mut child, &mut expiry, entry.1);
                        set_checked(&durations, Some(&ev.id));
                    } else {
                        stop(&mut child, &mut expiry); // unchecked the active duration
                        set_checked(&durations, None);
                    }
                }
                refresh(tray.as_ref(), &status, &turn_off, &child, expiry);
            }

            // --- 1 Hz heartbeat: refresh countdown / detect natural exit ---
            Event::UserEvent(UserEvent::Tick) => {
                let mut changed = false;
                // caffeinate exited on its own (a timed run elapsed)?
                if let Some(c) = child.as_mut() {
                    if matches!(c.try_wait(), Ok(Some(_))) {
                        child = None;
                        expiry = None;
                        set_checked(&durations, None);
                        changed = true;
                    }
                }
                if changed || expiry.is_some() {
                    refresh(tray.as_ref(), &status, &turn_off, &child, expiry);
                }
            }

            Event::LoopDestroyed => stop(&mut child, &mut expiry),

            _ => {}
        }
    });
}

/// Check exactly the one duration item whose id matches `id` (or none of them).
fn set_checked(durations: &[(CheckMenuItem, Option<u64>)], id: Option<&MenuId>) {
    for (item, _) in durations {
        item.set_checked(Some(item.id()) == id);
    }
}

/// Spawn `caffeinate` to keep the Mac awake. `secs` = `None` runs indefinitely.
fn start(child: &mut Option<Child>, expiry: &mut Option<Instant>, secs: Option<u64>) {
    stop(child, expiry); // never run two at once
    let mut cmd = Command::new("caffeinate");
    cmd.args(["-d", "-i"]); // keep display + system awake
    if let Some(s) = secs {
        cmd.args(["-t", &s.to_string()]);
    }
    match cmd.spawn() {
        Ok(c) => {
            *child = Some(c);
            *expiry = secs.map(|s| Instant::now() + Duration::from_secs(s));
        }
        Err(e) => eprintln!("redbull: failed to launch caffeinate: {e}"),
    }
}

/// Release the wake assertion by terminating caffeinate.
fn stop(child: &mut Option<Child>, expiry: &mut Option<Instant>) {
    if let Some(mut c) = child.take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    *expiry = None;
}

/// Remaining-time label whose resolution sharpens as the timer winds down:
///   ≥ 1h    → whole hours      (3h → 2h → 1h, staying "1h" until an hour is left)
///   10–59m  → nearest 5 min    (55m, 45m, 35m, 20m, 15m, 10m)
///   < 10m   → every minute     (9m, 8m, … 1m)
/// Always rounds up to ≥ 1, so it never reads "0m" while still awake.
fn remaining_label(until: Instant) -> String {
    let secs = until.saturating_duration_since(Instant::now()).as_secs();
    let m = ((secs + 59) / 60).max(1); // whole minutes remaining
    if m >= 60 {
        format!("{}h", m / 60)
    } else if m >= 10 {
        let rounded = (((m + 2) / 5) * 5).min(55); // nearest 5, never 60
        format!("{}m", rounded)
    } else {
        format!("{}m", m)
    }
}

/// Sync the icon, menu-bar title, status line, and Turn Off item to the state.
fn refresh(
    tray: Option<&TrayIcon>,
    status: &MenuItem,
    turn_off: &MenuItem,
    child: &Option<Child>,
    expiry: Option<Instant>,
) {
    let active = child.is_some();
    turn_off.set_enabled(active);

    status.set_text(match (active, expiry) {
        (false, _) => "Off".to_string(),
        (true, Some(until)) => format!("Awake · {}", remaining_label(until)),
        (true, None) => "Awake · ∞".to_string(),
    });

    if let Some(tray) = tray {
        let _ = tray.set_icon(Some(make_icon(active)));
        match (active, expiry) {
            (true, Some(until)) => {
                let _ = tray.set_title(Some(&remaining_label(until)));
            }
            (true, None) => {
                let _ = tray.set_title(Some("∞"));
            }
            (false, _) => {
                let _ = tray.set_title(None::<&str>);
            }
        }
    }
}

/// Build the menu-bar icon: a lightning bolt (energy = "stay awake"), rendered
/// as a high-resolution, anti-aliased template image so macOS downsamples it
/// crisply on Retina and tints it for the light/dark menu bar automatically.
///
/// When `active` the bolt is drawn at full opacity; when idle it's dimmed —
/// the standard macOS way to show a menu-bar item is "off".
fn make_icon(active: bool) -> Icon {
    // Lightning bolt outline (Feather "zap"), in a 24×24 design grid.
    const BOLT: [(f64, f64); 6] = [
        (13.0, 2.0),
        (3.0, 14.0),
        (12.0, 14.0),
        (11.0, 22.0),
        (21.0, 10.0),
        (12.0, 10.0),
    ];

    // Map the design grid into a high-res canvas with a small margin so the
    // bolt nearly fills the icon's height.
    const SCALE: f64 = 4.0;
    const MARGIN: f64 = 2.0; // grid units of padding around the bolt's bbox
    let (min_x, min_y) = (3.0 - MARGIN, 2.0 - MARGIN);
    let w = ((21.0 - 3.0 + 2.0 * MARGIN) * SCALE).ceil() as usize; // ~88
    let h = ((22.0 - 2.0 + 2.0 * MARGIN) * SCALE).ceil() as usize; // ~96
    let poly: Vec<(f64, f64)> = BOLT
        .iter()
        .map(|&(x, y)| ((x - min_x) * SCALE, (y - min_y) * SCALE))
        .collect();

    let opacity = if active { 1.0 } else { 0.40 };

    // Even-odd ray-cast point-in-polygon test.
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

    // 4×4 supersampling per pixel for smooth anti-aliased edges.
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
            let coverage = hits as f64 / 16.0;
            let alpha = (coverage * opacity * 255.0).round() as u8;
            let idx = (y * w + x) * 4;
            // RGB stays black; macOS uses alpha as the template mask.
            rgba[idx + 3] = alpha;
        }
    }

    Icon::from_rgba(rgba, w as u32, h as u32).expect("valid icon")
}
