//! PdfCraft patch (14): graphics tablets through `zwp_tablet_v2`.
//!
//! winit 0.30 does not bind the tablet protocol, so a compositor turns a pen into an ordinary
//! pointer and its pressure is lost. This reports a pen the way winit 0.30 already does on
//! Windows (`WM_POINTER` with `PT_PEN`): while the tip touches the tablet it is a
//! [`WindowEvent::Touch`] whose `force` is the pen pressure; while it hovers it moves the cursor
//! (`CursorEntered`, `CursorMoved`, `CursorLeft`). Barrel buttons are mouse buttons, mapped as
//! libinput desktops do (`BTN_STYLUS` middle, `BTN_STYLUS2` right).
//!
//! Binding the protocol tells the compositor that this client handles tablets itself, so the
//! pen's cursor is set here, through `wp_cursor_shape_v1` when the compositor offers it.
//! Without it the compositor's default cursor shows. Set `WINIT_WAYLAND_TABLET=0` to leave
//! tablets to the compositor's pointer emulation instead.
//!
//! The tablet handling of winit master (`winit-wayland/src/types/wp_tablet_input_v2.rs`) emits
//! the pointer events of the 0.31 API, which 0.30 doesn't have; this module emits 0.30 events.
//! Like it, only events on a window's main surface are reported: a pen over client-side
//! decorations does nothing (server-side decorations are the compositor's own).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use tracing::warn;

use sctk::compositor::SurfaceData;
use sctk::globals::GlobalData;
use sctk::reexports::client::delegate_dispatch;
use sctk::reexports::client::event_created_child;
use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::protocol::wl_seat::WlSeat;
use sctk::reexports::client::protocol::wl_surface::WlSurface;
use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use sctk::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use sctk::reexports::protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_manager_v2::ZwpTabletManagerV2;
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_pad_group_v2::{
    self, ZwpTabletPadGroupV2,
};
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2;
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2;
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_pad_v2::{
    self, ZwpTabletPadV2,
};
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_seat_v2::{
    self, ZwpTabletSeatV2,
};
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::{
    self, ButtonState, ZwpTabletToolV2,
};
use sctk::reexports::protocols::wp::tablet::zv2::client::zwp_tablet_v2::{self, ZwpTabletV2};

use crate::dpi::LogicalPosition;
use crate::event::{ElementState, Force, Touch, WindowEvent};
use crate::window::CursorIcon;

use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::{self, DeviceId};

mod frame;

use frame::{normalized_pressure, tool_button, Changes, Step};

/// The most button changes kept for one frame; a real tool has a handful of buttons.
const MAX_FRAME_BUTTONS: usize = 16;

/// Touch ids of tablet tools start here, above any `wl_touch` point id (an `i32`, so at most
/// `i32::MAX`, or `u64::MAX` and below once sign-extended), so the two never collide.
const TOOL_TOUCH_ID_BASE: u64 = 1 << 32;

/// The `zwp_tablet_manager_v2` global.
#[derive(Debug)]
pub struct TabletManager {
    manager: ZwpTabletManagerV2,
}

impl TabletManager {
    /// Binds the tablet manager, unless `WINIT_WAYLAND_TABLET=0` leaves tablets to the
    /// compositor's pointer emulation.
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Option<Self> {
        if !tablet_enabled(std::env::var("WINIT_WAYLAND_TABLET").ok().as_deref()) {
            return None;
        }
        // Version 1: version 2 only adds pad dials and the tablet bus type, neither used here.
        let bound: Result<ZwpTabletManagerV2, BindError> =
            globals.bind(queue_handle, 1..=1, GlobalData);
        bound.ok().map(|manager| Self { manager })
    }

    /// The tablets, tools and pads of `seat`.
    pub fn tablet_seat(
        &self,
        seat: &WlSeat,
        queue_handle: &QueueHandle<WinitState>,
    ) -> ZwpTabletSeatV2 {
        self.manager.get_tablet_seat(seat, queue_handle, GlobalData)
    }
}

/// Whether tablets are handled here, from the `WINIT_WAYLAND_TABLET` value.
fn tablet_enabled(value: Option<&str>) -> bool {
    !matches!(value.map(str::trim), Some("0" | "false" | "off" | "no"))
}

/// The cursor of a tablet tool in proximity of a window; the window keeps a weak reference
/// so cursor changes reach the pen as well as the mouse.
#[derive(Debug)]
pub struct TabletToolCursor {
    tool: ZwpTabletToolV2,
    shape_device: Option<WpCursorShapeDeviceV1>,
    /// The `proximity_in` serial every cursor request must name.
    serial: u32,
}

impl TabletToolCursor {
    pub fn set_icon(&self, icon: CursorIcon) {
        if let Some(device) = self.shape_device.as_ref() {
            device.set_shape(self.serial, cursor_icon_to_shape(icon));
        }
    }

    pub fn hide(&self) {
        self.tool.set_cursor(self.serial, None, 0, 0);
    }
}

/// Per-tool state, the user data of a `zwp_tablet_tool_v2`.
#[derive(Debug, Default)]
pub struct TabletToolData {
    inner: Mutex<ToolState>,
}

impl TabletToolData {
    fn lock(&self) -> MutexGuard<'_, ToolState> {
        // The state is plain data, valid whatever a panicking holder was doing.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug, Default)]
struct ToolState {
    /// Whether the tool reports pressure (`capability` event).
    has_pressure: bool,
    /// The window surface the tool is in proximity of.
    surface: Option<WlSurface>,
    /// Position on that surface, in logical pixels.
    position: LogicalPosition<f64>,
    /// Pressure from 0 to 1.
    pressure: f64,
    /// Whether the tip touches the tablet (a touch is in progress).
    in_contact: bool,
    /// The cursor while in proximity of a window.
    cursor: Option<Arc<TabletToolCursor>>,
    /// The tool's cursor-shape device, created on first proximity.
    shape_device: Option<WpCursorShapeDeviceV1>,
    /// Events since the last `frame`.
    frame: PendingFrame,
}

/// What happened since the last `frame` event; the protocol groups a tool's events in frames.
#[derive(Debug, Default)]
struct PendingFrame {
    entered: Option<(u32, WlSurface)>,
    changes: Changes,
}

/// The window surface `surface` belongs to as a main surface, or `None` for a subsurface
/// (client-side decorations) or a surface winit didn't make.
fn main_surface(surface: &WlSurface) -> Option<WlSurface> {
    let data = surface.data::<SurfaceData>()?;
    data.parent_surface().is_none().then(|| surface.clone())
}

impl Dispatch<ZwpTabletToolV2, TabletToolData, WinitState> for TabletManager {
    fn event(
        state: &mut WinitState,
        tool: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        data: &TabletToolData,
        _: &Connection,
        queue_handle: &QueueHandle<WinitState>,
    ) {
        use zwp_tablet_tool_v2::{Capability, Event};

        let mut tool_state = data.lock();
        match event {
            Event::Capability { capability: WEnum::Value(Capability::Pressure) } => {
                tool_state.has_pressure = true;
            },
            Event::ProximityIn { serial, surface, .. } => {
                // Leaving one surface and entering another in one frame: leave the old first.
                if tool_state.frame.changes.left {
                    let frame = std::mem::take(&mut tool_state.frame);
                    state.tablet_tool_frame(tool, &mut tool_state, frame, queue_handle);
                }
                tool_state.frame.entered = Some((serial, surface));
            },
            Event::ProximityOut => tool_state.frame.changes.left = true,
            Event::Down { .. } => tool_state.frame.changes.down = true,
            Event::Up => tool_state.frame.changes.up = true,
            Event::Motion { x, y } => {
                tool_state.position = LogicalPosition::new(x, y);
                tool_state.frame.changes.moved = true;
            },
            Event::Pressure { pressure } => {
                tool_state.pressure = normalized_pressure(pressure);
                tool_state.frame.changes.pressure_changed = true;
            },
            Event::Button { button, state: WEnum::Value(button_state), .. } => {
                let button_state = match button_state {
                    ButtonState::Pressed => ElementState::Pressed,
                    _ => ElementState::Released,
                };
                if tool_state.frame.changes.buttons.len() < MAX_FRAME_BUTTONS {
                    tool_state.frame.changes.buttons.push((tool_button(button), button_state));
                }
            },
            Event::Frame { .. } => {
                let frame = std::mem::take(&mut tool_state.frame);
                state.tablet_tool_frame(tool, &mut tool_state, frame, queue_handle);
            },
            Event::Removed => {
                if let Some(device) = tool_state.shape_device.take() {
                    device.destroy();
                }
                tool_state.cursor = None;
                tool.destroy();
            },
            // Type, hardware ids, distance, tilt, rotation, slider and wheel aren't reported.
            _ => (),
        }
    }
}

impl WinitState {
    fn tablet_tool_frame(
        &mut self,
        tool: &ZwpTabletToolV2,
        tool_state: &mut ToolState,
        frame: PendingFrame,
        queue_handle: &QueueHandle<WinitState>,
    ) {
        let mut entered = false;
        if let Some((serial, surface)) = frame.entered.as_ref() {
            tool_state.surface = main_surface(surface);
            tool_state.cursor = None;
            if tool_state.surface.is_some() {
                entered = true;
                if tool_state.shape_device.is_none() {
                    tool_state.shape_device = self.cursor_shape_manager.as_ref().map(|manager| {
                        manager.get_tablet_tool_v2(tool, queue_handle, GlobalData)
                    });
                }
                tool_state.cursor = Some(Arc::new(TabletToolCursor {
                    tool: tool.clone(),
                    shape_device: tool_state.shape_device.clone(),
                    serial: *serial,
                }));
            }
        }

        let surface = match tool_state.surface.clone() {
            Some(surface) => surface,
            None => return,
        };
        let window_id = wayland::make_wid(&surface);
        let window = self.windows.get_mut().get(&window_id).cloned();
        let window = match window {
            Some(window) => window,
            None => {
                // The window is gone; forget it so later frames don't look it up again.
                if frame.changes.left {
                    tool_state.surface = None;
                    tool_state.cursor = None;
                    tool_state.in_contact = false;
                }
                return;
            },
        };
        let mut window = window.lock().unwrap_or_else(PoisonError::into_inner);

        if entered {
            if let Some(cursor) = tool_state.cursor.as_ref() {
                window.tablet_tool_entered(Arc::downgrade(cursor));
            }
        }

        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        let position = tool_state.position.to_physical(window.scale_factor());
        let force = tool_state.has_pressure.then_some(Force::Normalized(tool_state.pressure));
        let id = TOOL_TOUCH_ID_BASE + u64::from(tool.id().protocol_id());
        let mut events = Vec::new();
        for step in frame::steps(&frame.changes, entered, &mut tool_state.in_contact) {
            match step {
                Step::Entered => {
                    events.push(WindowEvent::CursorEntered { device_id });
                    events.push(WindowEvent::CursorMoved { device_id, position });
                },
                Step::Hover => events.push(WindowEvent::CursorMoved { device_id, position }),
                Step::Touch(phase) => events.push(WindowEvent::Touch(Touch {
                    device_id,
                    phase,
                    location: position,
                    force,
                    id,
                })),
                Step::Button(button, state) => {
                    events.push(WindowEvent::MouseInput { device_id, state, button })
                },
                Step::Left => events.push(WindowEvent::CursorLeft { device_id }),
            }
        }

        if frame.changes.left {
            if let Some(cursor) = tool_state.cursor.take() {
                window.tablet_tool_left(&cursor);
            }
            tool_state.surface = None;
        }
        drop(window);

        for event in events {
            self.events_sink.push_window_event(event, window_id);
        }
    }
}

/// The cursor-shape name of a cursor icon; icons the protocol lacks fall back to the default.
fn cursor_icon_to_shape(icon: CursorIcon) -> Shape {
    match icon {
        CursorIcon::Default => Shape::Default,
        CursorIcon::ContextMenu => Shape::ContextMenu,
        CursorIcon::Help => Shape::Help,
        CursorIcon::Pointer => Shape::Pointer,
        CursorIcon::Progress => Shape::Progress,
        CursorIcon::Wait => Shape::Wait,
        CursorIcon::Cell => Shape::Cell,
        CursorIcon::Crosshair => Shape::Crosshair,
        CursorIcon::Text => Shape::Text,
        CursorIcon::VerticalText => Shape::VerticalText,
        CursorIcon::Alias => Shape::Alias,
        CursorIcon::Copy => Shape::Copy,
        CursorIcon::Move => Shape::Move,
        CursorIcon::NoDrop => Shape::NoDrop,
        CursorIcon::NotAllowed => Shape::NotAllowed,
        CursorIcon::Grab => Shape::Grab,
        CursorIcon::Grabbing => Shape::Grabbing,
        CursorIcon::EResize => Shape::EResize,
        CursorIcon::NResize => Shape::NResize,
        CursorIcon::NeResize => Shape::NeResize,
        CursorIcon::NwResize => Shape::NwResize,
        CursorIcon::SResize => Shape::SResize,
        CursorIcon::SeResize => Shape::SeResize,
        CursorIcon::SwResize => Shape::SwResize,
        CursorIcon::WResize => Shape::WResize,
        CursorIcon::EwResize => Shape::EwResize,
        CursorIcon::NsResize => Shape::NsResize,
        CursorIcon::NeswResize => Shape::NeswResize,
        CursorIcon::NwseResize => Shape::NwseResize,
        CursorIcon::ColResize => Shape::ColResize,
        CursorIcon::RowResize => Shape::RowResize,
        CursorIcon::AllScroll => Shape::AllScroll,
        CursorIcon::ZoomIn => Shape::ZoomIn,
        CursorIcon::ZoomOut => Shape::ZoomOut,
        _ => Shape::Default,
    }
}

/// Keeps only the cursors still alive, other than `removed`.
pub fn retain_tablet_cursors(
    cursors: &mut Vec<Weak<TabletToolCursor>>,
    removed: Option<&Arc<TabletToolCursor>>,
) {
    cursors.retain(|cursor| match cursor.upgrade() {
        Some(cursor) => !removed.is_some_and(|removed| Arc::ptr_eq(&cursor, removed)),
        None => false,
    });
}

impl Dispatch<ZwpTabletManagerV2, GlobalData, WinitState> for TabletManager {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletManagerV2,
        _: <ZwpTabletManagerV2 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // No events.
    }
}

impl Dispatch<ZwpTabletSeatV2, GlobalData, WinitState> for TabletManager {
    event_created_child!(WinitState, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, GlobalData),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, TabletToolData::default()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, GlobalData),
    ]);

    fn event(
        _: &mut WinitState,
        _: &ZwpTabletSeatV2,
        _: <ZwpTabletSeatV2 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // The added objects arrive through `event_created_child`.
    }
}

impl Dispatch<ZwpTabletV2, GlobalData, WinitState> for TabletManager {
    fn event(
        _: &mut WinitState,
        tablet: &ZwpTabletV2,
        event: zwp_tablet_v2::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        if let zwp_tablet_v2::Event::Removed = event {
            tablet.destroy();
        }
    }
}

impl Dispatch<ZwpTabletPadV2, GlobalData, WinitState> for TabletManager {
    event_created_child!(WinitState, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, GlobalData),
    ]);

    fn event(
        _: &mut WinitState,
        pad: &ZwpTabletPadV2,
        event: zwp_tablet_pad_v2::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
        // Pad buttons, rings and strips aren't reported.
        if let zwp_tablet_pad_v2::Event::Removed = event {
            pad.destroy();
        }
    }
}

impl Dispatch<ZwpTabletPadGroupV2, GlobalData, WinitState> for TabletManager {
    event_created_child!(WinitState, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, GlobalData),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, GlobalData),
    ]);

    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadGroupV2,
        _: zwp_tablet_pad_group_v2::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<ZwpTabletPadRingV2, GlobalData, WinitState> for TabletManager {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadRingV2,
        _: <ZwpTabletPadRingV2 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<ZwpTabletPadStripV2, GlobalData, WinitState> for TabletManager {
    fn event(
        _: &mut WinitState,
        _: &ZwpTabletPadStripV2,
        _: <ZwpTabletPadStripV2 as Proxy>::Event,
        _: &GlobalData,
        _: &Connection,
        _: &QueueHandle<WinitState>,
    ) {
    }
}

/// Binds the cursor-shape manager for tablet tool cursors; mouse pointers get theirs from sctk.
pub fn bind_cursor_shape_manager(
    globals: &GlobalList,
    queue_handle: &QueueHandle<WinitState>,
) -> Option<WpCursorShapeManagerV1> {
    let bound: Result<WpCursorShapeManagerV1, BindError> =
        globals.bind(queue_handle, 1..=1, GlobalData);
    match bound {
        Ok(manager) => Some(manager),
        Err(error) => {
            warn!("wp_cursor_shape_v1 unavailable, tablet tools keep the default cursor: {error}");
            None
        },
    }
}

delegate_dispatch!(WinitState: [ZwpTabletManagerV2: GlobalData] => TabletManager);
delegate_dispatch!(WinitState: [ZwpTabletSeatV2: GlobalData] => TabletManager);
delegate_dispatch!(WinitState: [ZwpTabletV2: GlobalData] => TabletManager);
delegate_dispatch!(WinitState: [ZwpTabletToolV2: TabletToolData] => TabletManager);
delegate_dispatch!(WinitState: [ZwpTabletPadV2: GlobalData] => TabletManager);
delegate_dispatch!(WinitState: [ZwpTabletPadGroupV2: GlobalData] => TabletManager);
delegate_dispatch!(WinitState: [ZwpTabletPadRingV2: GlobalData] => TabletManager);
delegate_dispatch!(WinitState: [ZwpTabletPadStripV2: GlobalData] => TabletManager);

#[cfg(test)]
mod tests {
    use super::*;

    // The frame policy is tested from apps/pdfcraft/tests/winit_wayland_tablet.rs.

    #[test]
    fn the_environment_can_turn_tablets_off() {
        assert!(tablet_enabled(None));
        assert!(tablet_enabled(Some("1")));
        assert!(!tablet_enabled(Some("0")));
        assert!(!tablet_enabled(Some(" off ")));
    }

    #[test]
    fn tool_touch_ids_never_meet_wl_touch_ids() {
        assert!((i32::MAX as u64) < TOOL_TOUCH_ID_BASE);
        assert!(TOOL_TOUCH_ID_BASE + u64::from(u32::MAX) < (-1i32) as u64);
    }
}
