//! PdfCraft patch (14): what one tablet-tool frame reports, shared with the desktop regression
//! tests (`apps/pdfcraft/tests/winit_wayland_tablet.rs`).

use crate::event::{ElementState, MouseButton, TouchPhase};

/// What happened to a tool since the last `frame` event, other than entering a surface.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    pub moved: bool,
    pub pressure_changed: bool,
    pub down: bool,
    pub up: bool,
    pub left: bool,
    pub buttons: Vec<(MouseButton, ElementState)>,
}

/// One thing a frame reports, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Step {
    /// The tool came into proximity of the window: `CursorEntered`, then `CursorMoved`.
    Entered,
    /// The tool hovers: `CursorMoved`.
    Hover,
    /// The tip touches the tablet: a `Touch` with the pen pressure as its force.
    Touch(TouchPhase),
    /// A barrel button: `MouseInput`.
    Button(MouseButton, ElementState),
    /// The tool left: `CursorLeft`.
    Left,
}

/// The steps of one frame, given whether it entered the window and whether the tip was down
/// before it; `in_contact` is updated.
pub(crate) fn steps(changes: &Changes, entered: bool, in_contact: &mut bool) -> Vec<Step> {
    let mut steps = Vec::new();
    if entered {
        steps.push(Step::Entered);
    }
    if changes.down && !*in_contact {
        *in_contact = true;
        steps.push(Step::Touch(TouchPhase::Started));
    } else if *in_contact && !changes.up && (changes.moved || changes.pressure_changed) {
        // Pressure alone moves the touch, as on Windows. A lifting frame's position and
        // pressure go with `Ended` instead of a separate move.
        steps.push(Step::Touch(TouchPhase::Moved));
    } else if !*in_contact && changes.moved && !entered {
        steps.push(Step::Hover);
    }
    for &(button, state) in &changes.buttons {
        steps.push(Step::Button(button, state));
    }
    if changes.up && *in_contact {
        *in_contact = false;
        steps.push(Step::Touch(TouchPhase::Ended));
    }
    if changes.left {
        // The protocol sends `up` first; a tool that leaves while down ends its touch anyway.
        if *in_contact {
            *in_contact = false;
            steps.push(Step::Touch(TouchPhase::Cancelled));
        }
        steps.push(Step::Left);
    }
    steps
}

/// A barrel button, mapped as libinput desktops do (the lower button is the middle button).
pub(crate) fn tool_button(button: u32) -> MouseButton {
    const BTN_STYLUS3: u32 = 0x149;
    const BTN_STYLUS: u32 = 0x14b;
    const BTN_STYLUS2: u32 = 0x14c;
    match button {
        BTN_STYLUS => MouseButton::Middle,
        BTN_STYLUS2 => MouseButton::Right,
        BTN_STYLUS3 => MouseButton::Back,
        // Linux button codes fit in 16 bits (KEY_MAX is 0x2ff).
        other => MouseButton::Other(u16::try_from(other).unwrap_or(u16::MAX)),
    }
}

/// `zwp_tablet_tool_v2.pressure` (0 to 65535) as a force from 0 to 1.
pub(crate) fn normalized_pressure(pressure: u32) -> f64 {
    f64::from(pressure.min(65535)) / 65535.0
}
