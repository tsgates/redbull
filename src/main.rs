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
            tray = Some(
                TrayIconBuilder::new()
                    .with_menu(Box::new(menu.take().unwrap()))
                    .with_tooltip("Redbull — your Mac is allowed to sleep")
                    .with_icon(make_icon(false))
                    .with_icon_as_template(true) // adapt to light/dark menu bar
                    .build()
                    .expect("failed to create tray icon"),
            );
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

/// Build a 32×32 template icon of a coffee mug. When `active`, the mug is
/// filled and steaming; when idle it's just an outline. Drawn as a black
/// alpha mask so macOS tints it for the light/dark menu bar automatically.
fn make_icon(active: bool) -> Icon {
    const W: usize = 32;
    const H: usize = 32;
    let mut a = [0u8; W * H]; // alpha mask

    let mut set = |x: i32, y: i32| {
        if x >= 0 && x < W as i32 && y >= 0 && y < H as i32 {
            a[y as usize * W + x as usize] = 255;
        }
    };

    // Mug body: rectangle x[6..=21], y[11..=26].
    let (l, r, t, b) = (6i32, 21i32, 11i32, 26i32);
    if active {
        for y in t..=b {
            for x in l..=r {
                set(x, y);
            }
        }
    } else {
        for x in l..=r {
            set(x, t);
            set(x, b);
        }
        for y in t..=b {
            set(l, y);
            set(r, y);
        }
    }

    // Handle: a small "C" on the right side, both states.
    for y in 14..=22 {
        set(r + 4, y);
    }
    for x in (r + 1)..=(r + 4) {
        set(x, 14);
        set(x, 22);
    }

    // Steam: two short wavy columns rising from the mug when active.
    if active {
        for (sx, phase) in [(11i32, 0i32), (16i32, 2i32)] {
            for y in 2..=8 {
                let wiggle = ((y + phase) / 2) % 2; // 0/1 zig-zag
                set(sx + wiggle, y);
            }
        }
    }

    // Expand alpha mask into black RGBA.
    let mut rgba = Vec::with_capacity(W * H * 4);
    for &alpha in a.iter() {
        rgba.extend_from_slice(&[0, 0, 0, alpha]);
    }
    Icon::from_rgba(rgba, W as u32, H as u32).expect("valid icon")
}
