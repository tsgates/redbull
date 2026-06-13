// Redbull — a minimal macOS menu-bar app that keeps your Mac awake.
//
// Under the hood it just runs the system `caffeinate` tool:
//
//     caffeinate -d -i -t 3600
//
// (-d = keep display awake, -i = prevent idle sleep, -t = for N seconds).
// Click the menu-bar icon, toggle "Keep Awake", and the machine stays up
// for one hour. Toggle it off (or quit) and the assertion is released.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// How long each activation keeps the Mac awake, in seconds. (1 hour)
const DURATION_SECS: u64 = 3600;


fn main() {
    let mut event_loop = EventLoopBuilder::new().build();
    // Accessory => no Dock icon, no menu, lives only in the menu bar.
    event_loop.set_activation_policy(ActivationPolicy::Accessory);

    // --- Menu ---------------------------------------------------------------
    let toggle = CheckMenuItem::new("Keep Awake (1 hour)", true, false, None);
    let quit = MenuItem::new("Quit Redbull", true, None);

    let menu = Menu::new();
    menu.append(&toggle).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&quit).unwrap();

    let toggle_id = toggle.id().clone();
    let quit_id = quit.id().clone();
    let menu_channel = MenuEvent::receiver();

    // The tray icon must be created on the main thread *after* the event loop
    // exists, so we build it in the Init event below.
    let mut menu = Some(menu);
    let mut tray: Option<TrayIcon> = None;
    let mut child: Option<Child> = None;
    let mut expiry: Option<Instant> = None;

    event_loop.run(move |event, _, control_flow| {
        // Wake up about once a second to refresh the countdown and to notice
        // when caffeinate exits on its own (timer elapsed).
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(1));

        if let Event::NewEvents(StartCause::Init) = event {
            match TrayIconBuilder::new()
                .with_menu(Box::new(menu.take().unwrap()))
                .with_tooltip("Redbull — your Mac is allowed to sleep")
                .with_icon(make_icon(false))
                .with_icon_as_template(true) // adapt to light/dark menu bar
                .build()
            {
                Ok(t) => tray = Some(t),
                Err(e) => eprintln!("redbull: tray build FAILED: {e}"),
            }
        }

        // --- Handle menu clicks -------------------------------------------
        if let Ok(ev) = menu_channel.try_recv() {
            if ev.id == toggle_id {
                // CheckMenuItem flips its own checkmark on click; read the new state.
                if toggle.is_checked() {
                    start(&mut child, &mut expiry);
                } else {
                    stop(&mut child, &mut expiry);
                }
                refresh(tray.as_ref(), &toggle, expiry);
            } else if ev.id == quit_id {
                stop(&mut child, &mut expiry);
                *control_flow = ControlFlow::Exit;
                return;
            }
        }

        // --- Detect caffeinate exiting on its own (timer ran out) ---------
        if let Some(c) = child.as_mut() {
            if matches!(c.try_wait(), Ok(Some(_))) {
                child = None;
                expiry = None;
                toggle.set_checked(false);
            }
        }

        // Keep the countdown in the menu bar fresh.
        if expiry.is_some() {
            refresh(tray.as_ref(), &toggle, expiry);
        }

        if let Event::LoopDestroyed = event {
            stop(&mut child, &mut expiry);
        }
    });
}

/// Spawn `caffeinate` to keep the Mac awake for `DURATION_SECS`.
fn start(child: &mut Option<Child>, expiry: &mut Option<Instant>) {
    stop(child, expiry); // never run two at once
    match Command::new("caffeinate")
        .args(["-d", "-i", "-t", &DURATION_SECS.to_string()])
        .spawn()
    {
        Ok(c) => {
            *child = Some(c);
            *expiry = Some(Instant::now() + Duration::from_secs(DURATION_SECS));
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

/// Update the icon, tray title (countdown), and menu label to match state.
fn refresh(tray: Option<&TrayIcon>, toggle: &CheckMenuItem, expiry: Option<Instant>) {
    let Some(tray) = tray else { return };
    let active = expiry.is_some();
    let _ = tray.set_icon(Some(make_icon(active)));

    match expiry {
        Some(until) => {
            let secs = until.saturating_duration_since(Instant::now()).as_secs();
            let mins = (secs + 59) / 60; // round up so it never shows "0m" while active
            let _ = tray.set_title(Some(&format!("{mins}m")));
            toggle.set_text(format!("Awake — {mins} min left"));
        }
        None => {
            let _ = tray.set_title(None::<&str>);
            toggle.set_text("Keep Awake (1 hour)");
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
