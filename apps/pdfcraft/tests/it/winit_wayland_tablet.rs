#![cfg(target_os = "linux")]
//! vendor/winit patch (14): a Wayland pen is a touch with its pressure while the tip is down,
//! and moves the cursor while it hovers, as winit 0.30 already reports pens on Windows.
//! Drawing itself needs a Wayland session and a tablet.

// Compile the production policy against winit's real event types, without a compositor.
#[path = "../../../../vendor/winit/src/platform_impl/linux/wayland/seat/tablet/frame.rs"]
mod frame;

use crate::event::{ElementState, MouseButton, TouchPhase};
use frame::{Changes, Step, normalized_pressure, steps, tool_button};

#[test]
fn a_pen_stroke_is_a_touch_and_hovering_moves_the_cursor() {
    let mut down = false;
    let mut run = |changes: Changes, entered: bool| steps(&changes, entered, &mut down);
    assert_eq!(run(Changes { moved: true, ..Default::default() }, true), [Step::Entered]);
    assert_eq!(run(Changes { moved: true, ..Default::default() }, false), [Step::Hover]);
    let press = Changes { moved: true, pressure_changed: true, down: true, ..Default::default() };
    assert_eq!(run(press, false), [Step::Touch(TouchPhase::Started)]);
    assert_eq!(run(Changes { moved: true, ..Default::default() }, false), [Step::Touch(TouchPhase::Moved)]);
    // Pressing harder without moving changes the touch's force.
    assert_eq!(run(Changes { pressure_changed: true, ..Default::default() }, false), [Step::Touch(TouchPhase::Moved)]);
    // The lift carries the last position: no extra zero-pressure point before it.
    let lift = Changes { moved: true, pressure_changed: true, up: true, ..Default::default() };
    assert_eq!(run(lift, false), [Step::Touch(TouchPhase::Ended)]);
    assert_eq!(run(Changes { moved: true, ..Default::default() }, false), [Step::Hover]);
    assert_eq!(run(Changes { left: true, ..Default::default() }, false), [Step::Left]);
}

#[test]
fn a_pen_leaving_while_down_cancels_its_touch() {
    let mut down = true;
    let away = Changes { left: true, ..Default::default() };
    assert_eq!(steps(&away, false, &mut down), [Step::Touch(TouchPhase::Cancelled), Step::Left]);
    assert!(!down);
}

#[test]
fn a_tap_within_one_frame_starts_and_ends_a_touch() {
    let mut down = false;
    let tap = Changes { down: true, up: true, ..Default::default() };
    assert_eq!(steps(&tap, false, &mut down), [Step::Touch(TouchPhase::Started), Step::Touch(TouchPhase::Ended)]);
    assert!(!down);
}

#[test]
fn barrel_buttons_click_like_mouse_buttons() {
    let mut down = false;
    let click = Changes { buttons: vec![(tool_button(0x14c), ElementState::Pressed)], ..Default::default() };
    assert_eq!(steps(&click, false, &mut down), [Step::Button(MouseButton::Right, ElementState::Pressed)]);
    assert_eq!(tool_button(0x14b), MouseButton::Middle);
    assert_eq!(tool_button(0x149), MouseButton::Back);
    assert_eq!(tool_button(u32::MAX), MouseButton::Other(u16::MAX));
}

#[test]
fn pen_pressure_is_a_force_from_zero_to_one() {
    assert_eq!(normalized_pressure(0), 0.0);
    assert_eq!(normalized_pressure(65535), 1.0);
    assert_eq!(normalized_pressure(u32::MAX), 1.0);
    assert!((normalized_pressure(32768) - 0.5).abs() < 0.001);
}

#[test]
fn winit_carries_the_wayland_tablet_patch() {
    // Keeps the patch in the build: winit from crates.io, or a re-vendored copy without it,
    // would bind no tablets and leave the pen a pressureless mouse (vendor/README.md).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).expect("Cargo.lock");
    let winit = lock.split("[[package]]").find(|p| p.contains("\nname = \"winit\"\n")).expect("winit is in Cargo.lock");
    assert!(!winit.contains("\nsource = "), "winit must resolve to vendor/winit, not:{winit}");
    let state = std::fs::read_to_string(root.join("vendor/winit/src/platform_impl/linux/wayland/state.rs")).expect("state.rs");
    assert!(state.contains("TabletManager::new(globals, queue_handle)"), "vendor/winit no longer binds zwp_tablet_manager_v2");
}
