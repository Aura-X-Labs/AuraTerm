//! Window placement: restoring the main window's saved bounds at startup,
//! recording them as it moves, and keeping every window inside the monitor it
//! sits on.
//!
//! AuraTerm's windows are undecorated (the titlebar is drawn by the webview),
//! so the OS does nothing to fit them when they land on a smaller monitor.
//! AppKit never constrains a borderless `NSWindow`. Windows keeps a window's
//! size when it is moved or its monitor goes away, and tao keeps the *logical*
//! size across a DPI change, so the window grows on a higher-scale monitor.
//! tao also reports no event at all when the monitor layout changes under a
//! window that does not move. A window bigger than its monitor hides its own
//! titlebar buttons and resize edges, so the user can neither shrink nor close
//! it. Once a window settles after moving, resizing or a display change, it is
//! therefore shrunk and moved back inside its monitor's work area.

use std::{collections::HashMap, sync::Mutex, time::Duration};

use tauri::{
    AppHandle, Manager, Monitor, PhysicalPosition, PhysicalSize, Position, Size, Window,
    WindowEvent,
};

use crate::settings::{self, WindowBounds};

/// The window whose bounds are saved and restored.
const MAIN_WINDOW: &str = "main";

/// Quiet time after a window's last move or resize before it is checked. tao
/// has no drag-end event, and fitting mid-drag would fight the drag.
const SETTLE_DELAY: Duration = Duration::from_millis(400);

/// How often the monitor layout is sampled. tao reports no event when a
/// monitor is added, removed, rearranged or rescaled, or when a work area
/// changes (tauri#8405). A window that stays put while its monitor shrinks
/// under it gets no event of its own either.
const LAYOUT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Height of the custom titlebar (`.titlebar` in base-and-titlebar.css), in
/// logical points: the only place the window can be dragged from.
const TITLEBAR_HEIGHT: f64 = 35.0;

/// How much of the titlebar, in logical points, must stay on screen for the
/// user to grab it and drag the window back.
const MIN_TITLEBAR_GRAB: f64 = 64.0;

/// Rounding slack, in desktop units, between physical pixels and logical
/// points.
const SLACK: f64 = 2.0;

/// Windows parks minimized windows at (-32000, -32000).
const MINIMIZED_POSITION: i32 = -32000;

// ============================================================================
// Geometry
// ============================================================================

/// A rectangle in desktop units: the one coordinate space every monitor
/// shares. Windows lays all monitors out in physical pixels. macOS and GTK lay
/// them out in logical points, each monitor scaling its own pixels, so
/// physical rects from two monitors with different scales do not line up
/// there.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn bottom(&self) -> f64 {
        self.y + self.height
    }

    fn area(&self) -> f64 {
        self.width * self.height
    }

    fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > x && bottom > y).then_some(Rect { x, y, width: right - x, height: bottom - y })
    }

    fn approx_eq(&self, other: &Rect) -> bool {
        (self.x - other.x).abs() <= SLACK
            && (self.y - other.y).abs() <= SLACK
            && (self.width - other.width).abs() <= SLACK
            && (self.height - other.height).abs() <= SLACK
    }
}

/// The titlebar strip a window is dragged by, in desktop units.
#[derive(Debug, Clone, Copy)]
struct Grip {
    height: f64,
    min_width: f64,
}

impl Grip {
    fn at_scale(scale_factor: f64) -> Self {
        Grip {
            height: desktop_from_logical(TITLEBAR_HEIGHT, scale_factor),
            min_width: desktop_from_logical(MIN_TITLEBAR_GRAB, scale_factor),
        }
    }
}

/// Where `window` should go so the user can still reach it, or `None` when it
/// already can.
///
/// A window is left alone while its titlebar can be grabbed and it is either
/// no bigger than `monitor` or fully visible across `work_areas` (stretched
/// over several monitors on purpose). So windows the user parks partly off
/// screen, or spans across monitors, stay put. Otherwise the window is shrunk
/// to fit `monitor`'s work area and moved fully inside it.
fn fit_window(window: Rect, monitor: Rect, work_areas: &[Rect], grip: Grip) -> Option<Rect> {
    let fits_monitor =
        window.width <= monitor.width + SLACK && window.height <= monitor.height + SLACK;
    if titlebar_reachable(&window, work_areas, grip)
        && (fits_monitor || fully_visible(&window, work_areas))
    {
        return None;
    }

    let width = window.width.min(monitor.width);
    let height = window.height.min(monitor.height);
    let fitted = Rect {
        x: window.x.max(monitor.x).min(monitor.right() - width),
        y: window.y.max(monitor.y).min(monitor.bottom() - height),
        width,
        height,
    };
    (!fitted.approx_eq(&window)).then_some(fitted)
}

/// Whether the full height of the titlebar is on one monitor, with enough of
/// its width showing to grab.
fn titlebar_reachable(window: &Rect, work_areas: &[Rect], grip: Grip) -> bool {
    let titlebar = Rect { height: grip.height.min(window.height), ..*window };
    let min_width = grip.min_width.min(window.width);
    work_areas.iter().any(|area| {
        titlebar.intersection(area).is_some_and(|visible| {
            visible.height >= titlebar.height - SLACK && visible.width >= min_width - SLACK
        })
    })
}

/// Whether every part of `window` is on some monitor.
fn fully_visible(window: &Rect, work_areas: &[Rect]) -> bool {
    let covered: f64 = work_areas
        .iter()
        .filter_map(|area| window.intersection(area))
        .map(|visible| visible.area())
        .sum();
    covered >= window.area() - SLACK * (window.width + window.height)
}

/// The work area holding most of `window`, or the nearest one when the window
/// is on none of them.
fn home_work_area(window: &Rect, work_areas: &[Rect]) -> Option<Rect> {
    let overlap = |area: &Rect| window.intersection(area).map_or(0.0, |visible| visible.area());
    let (center_x, center_y) = window.center();
    let distance = |area: &Rect| {
        let dx = (area.x - center_x).max(center_x - area.right()).max(0.0);
        let dy = (area.y - center_y).max(center_y - area.bottom()).max(0.0);
        dx * dx + dy * dy
    };
    let busiest = work_areas
        .iter()
        .copied()
        .filter(|area| overlap(area) > 0.0)
        .max_by(|a, b| overlap(a).total_cmp(&overlap(b)));
    busiest.or_else(|| work_areas.iter().copied().min_by(|a, b| distance(a).total_cmp(&distance(b))))
}

/// `target`, made a little shorter if it would exactly fill `monitor`. tao
/// reports a borderless macOS window that fills its screen's work area as
/// maximized, so the titlebar's maximize toggle would "restore" it to tao's
/// fallback 800x600 frame instead of maximizing it.
fn clear_of_maximized(target: Rect, monitor: Rect) -> Rect {
    let fills = (target.width - monitor.width).abs() < 1.0 && (target.height - monitor.height).abs() < 1.0;
    if fills {
        Rect { height: target.height - SLACK, ..target }
    } else {
        target
    }
}

// ============================================================================
// Desktop units
// ============================================================================

/// Converts a length the platform reports in physical pixels into desktop
/// units (see [`Rect`]).
#[cfg(windows)]
fn desktop(physical: f64, _scale_factor: f64) -> f64 {
    physical
}

#[cfg(not(windows))]
fn desktop(physical: f64, scale_factor: f64) -> f64 {
    physical / scale_factor
}

/// Converts a length in logical points into desktop units.
#[cfg(windows)]
fn desktop_from_logical(logical: f64, scale_factor: f64) -> f64 {
    logical * scale_factor
}

#[cfg(not(windows))]
fn desktop_from_logical(logical: f64, _scale_factor: f64) -> f64 {
    logical
}

#[cfg(windows)]
fn desktop_size(width: f64, height: f64) -> Size {
    Size::Physical(PhysicalSize::new(width.round() as u32, height.round() as u32))
}

#[cfg(not(windows))]
fn desktop_size(width: f64, height: f64) -> Size {
    Size::Logical(tauri::LogicalSize::new(width, height))
}

#[cfg(windows)]
fn desktop_position(x: f64, y: f64) -> Position {
    Position::Physical(PhysicalPosition::new(x.round() as i32, y.round() as i32))
}

#[cfg(not(windows))]
fn desktop_position(x: f64, y: f64) -> Position {
    Position::Logical(tauri::LogicalPosition::new(x, y))
}

/// A monitor's work area (the part the taskbar, Dock and menu bar leave
/// free), or `None` for a stale handle of an unplugged monitor.
fn work_area(monitor: &Monitor) -> Option<Rect> {
    let area = monitor.work_area();
    let scale_factor = monitor.scale_factor();
    let rect = Rect {
        x: desktop(area.position.x.into(), scale_factor),
        y: desktop(area.position.y.into(), scale_factor),
        width: desktop(area.size.width.into(), scale_factor),
        height: desktop(area.size.height.into(), scale_factor),
    };
    (scale_factor > 0.0 && rect.width >= 1.0 && rect.height >= 1.0).then_some(rect)
}

fn work_areas(monitors: &[Monitor]) -> Vec<Rect> {
    let mut areas: Vec<Rect> = Vec::new();
    // Mirrored displays report the same work area twice.
    for area in monitors.iter().filter_map(work_area) {
        if !areas.iter().any(|known| known.approx_eq(&area)) {
            areas.push(area);
        }
    }
    areas
}

/// Moves and resizes `window`: `x`/`y` place its outer frame, `width`/`height`
/// size its content. On Windows the move goes first, because moving onto a
/// monitor with another DPI makes Windows rescale the window and would undo a
/// size set before it. Elsewhere the resize goes first, because AppKit resizes
/// around the bottom-left corner and would shift a position set before it.
fn place(window: &Window, x: f64, y: f64, width: f64, height: f64) -> tauri::Result<()> {
    if cfg!(windows) {
        window.set_position(desktop_position(x, y))?;
        window.set_size(desktop_size(width, height))
    } else {
        window.set_size(desktop_size(width, height))?;
        window.set_position(desktop_position(x, y))
    }
}

/// Wayland compositors place windows themselves: tao reports every window at
/// (0, 0) there and ignores moves, so only a window's size can be fitted.
#[cfg(target_os = "linux")]
fn can_position_windows() -> bool {
    // GTK prefers Wayland whenever a Wayland display is available, unless
    // GDK_BACKEND lists X11 first.
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let forced_x11 = std::env::var("GDK_BACKEND").is_ok_and(|backend| backend.starts_with("x11"));
    !wayland || forced_x11
}

#[cfg(not(target_os = "linux"))]
fn can_position_windows() -> bool {
    true
}

/// Whether a mouse button is held, which most likely means the user is
/// dragging or resizing a window right now.
#[cfg(target_os = "macos")]
fn mouse_button_down() -> bool {
    // SAFETY: `+[NSEvent pressedMouseButtons]` takes no arguments and returns
    // an NSUInteger bit mask.
    let buttons: usize =
        unsafe { objc2::msg_send![objc2::class!(NSEvent), pressedMouseButtons] };
    buttons != 0
}

#[cfg(windows)]
fn mouse_button_down() -> bool {
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(key: i32) -> i16;
    }
    const VK_LBUTTON: i32 = 0x01;
    const VK_RBUTTON: i32 = 0x02;
    // SAFETY: GetAsyncKeyState only reads input state. The high bit, and so a
    // negative result, means the button is down.
    unsafe { GetAsyncKeyState(VK_LBUTTON) < 0 || GetAsyncKeyState(VK_RBUTTON) < 0 }
}

/// GTK has no cheap global button query, so on Linux a pause of
/// [`SETTLE_DELAY`] mid-drag can start a fit; X11 window managers usually keep
/// windows on screen themselves.
#[cfg(not(any(target_os = "macos", windows)))]
fn mouse_button_down() -> bool {
    false
}

/// The work areas a window may count as showing it. With "Displays have
/// separate Spaces" (the macOS default) a window straddling two displays is
/// drawn on only one of them, so only that one counts.
#[cfg(target_os = "macos")]
fn visible_work_areas(monitor: Rect, work_areas: &[Rect]) -> Vec<Rect> {
    // SAFETY: `+[NSScreen screensHaveSeparateSpaces]` takes no arguments and
    // returns a BOOL.
    let separate: bool =
        unsafe { objc2::msg_send![objc2::class!(NSScreen), screensHaveSeparateSpaces] };
    if separate {
        vec![monitor]
    } else {
        work_areas.to_vec()
    }
}

#[cfg(not(target_os = "macos"))]
fn visible_work_areas(_monitor: Rect, work_areas: &[Rect]) -> Vec<Rect> {
    work_areas.to_vec()
}

/// Whether the OS sizes `window` as maximized. A maximized window's frame is
/// not its normal one, and moving or resizing it un-maximizes it. tao reports
/// a borderless macOS window as maximized whenever it fills its screen, which
/// a fit or the user can also produce, so macOS windows are never treated so.
fn os_maximized(window: &Window) -> tauri::Result<bool> {
    Ok(!cfg!(target_os = "macos") && window.is_maximized()?)
}

/// How far `window`'s outer frame extends left of and above its content, in
/// desktop units: the invisible resize borders of an undecorated window on
/// Windows, and nothing elsewhere.
fn frame_offset(window: &Window, scale_factor: f64) -> tauri::Result<(f64, f64)> {
    let outer = window.outer_position()?;
    let inner = window.inner_position()?;
    Ok((
        desktop(f64::from(outer.x - inner.x), scale_factor),
        desktop(f64::from(outer.y - inner.y), scale_factor),
    ))
}

// ============================================================================
// Runtime fitting
// ============================================================================

/// Placement bookkeeping, shared by every window.
#[derive(Default)]
pub(crate) struct WindowPlacementState {
    /// Per-window settle bookkeeping, by window label.
    windows: Mutex<HashMap<String, SettleState>>,
    /// The main window's bounds as last written to settings.
    saved_bounds: Mutex<Option<WindowBounds>>,
    /// The monitor layout as last sampled.
    layout: Mutex<Option<Vec<MonitorLayout>>>,
}

#[derive(Default)]
struct SettleState {
    /// Bumped by every move or resize; only the latest settle timer runs.
    generation: u64,
    /// The last fit applied: where the window was and where it was sent.
    last_fit: Option<(Rect, Rect)>,
}

#[derive(Debug, PartialEq)]
struct MonitorLayout {
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
    work_area: (PhysicalPosition<i32>, PhysicalSize<u32>),
    scale_factor: f64,
}

enum Fit {
    /// The user can reach the window where it is.
    Fits,
    /// The window was moved or resized; the resulting events settle it again.
    Refitted,
    /// The window needs fitting, but a mouse button is down.
    Deferred,
    /// Minimized, maximized, fullscreen, hidden, no monitor to fit it to, or
    /// the last fit did not take.
    Skipped,
}

/// Global window-event hook, registered with `Builder::on_window_event` so it
/// covers every window, including ones opened later.
pub(crate) fn handle_window_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::Moved(_)
        | WindowEvent::Resized(_)
        | WindowEvent::ScaleFactorChanged { .. }
        | WindowEvent::Focused(true) => schedule_settle(window),
        WindowEvent::CloseRequested { .. } if window.label() == MAIN_WINDOW => {
            save_bounds(window);
        }
        WindowEvent::Destroyed => {
            if let Ok(mut windows) = window.state::<WindowPlacementState>().windows.lock() {
                windows.remove(window.label());
            }
        }
        _ => {}
    }
}

/// Checks `window` once it has been quiet for [`SETTLE_DELAY`].
pub(crate) fn schedule_settle(window: &Window) {
    let Ok(generation) = bump_generation(window) else {
        return;
    };
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SETTLE_DELAY).await;
        if !is_latest(&window, generation) {
            return;
        }
        let target = window.clone();
        if let Err(error) = window.run_on_main_thread(move || settle(&target, generation)) {
            crate::warn_log!("failed to check window {} placement: {error}", window.label());
        }
    });
}

fn bump_generation(window: &Window) -> Result<u64, String> {
    let state = window.state::<WindowPlacementState>();
    let mut windows = state.windows.lock().map_err(|error| error.to_string())?;
    let entry = windows.entry(window.label().to_string()).or_default();
    entry.generation = entry.generation.wrapping_add(1);
    Ok(entry.generation)
}

fn is_latest(window: &Window, generation: u64) -> bool {
    let state = window.state::<WindowPlacementState>();
    let windows = state.windows.lock();
    windows.is_ok_and(|windows| {
        windows.get(window.label()).is_some_and(|entry| entry.generation == generation)
    })
}

/// Runs on the main thread once `window` has been quiet: fits it to its
/// monitor, and records the main window's bounds once it fits.
fn settle(window: &Window, generation: u64) {
    if !is_latest(window, generation) {
        return;
    }
    match fit_to_monitor(window) {
        Ok(Fit::Fits) => {
            if window.label() == MAIN_WINDOW {
                save_bounds(window);
            }
        }
        Ok(Fit::Deferred) => schedule_settle(window),
        Ok(Fit::Refitted | Fit::Skipped) => {}
        Err(error) => {
            crate::warn_log!("failed to fit window {} to its monitor: {error}", window.label());
        }
    }
}

fn fit_to_monitor(window: &Window) -> tauri::Result<Fit> {
    if !window.is_visible()? || window.is_minimized()? || window.is_fullscreen()? || os_maximized(window)? {
        return Ok(Fit::Skipped);
    }

    let scale_factor = window.scale_factor()?;
    let work_areas = work_areas(&window.available_monitors()?);
    // Fit the content rect: on Windows the outer frame includes invisible
    // resize borders, which may hang off screen like any window's do.
    let inner_position = window.inner_position()?;
    let inner_size = window.inner_size()?;
    let mut rect = Rect {
        x: desktop(inner_position.x.into(), scale_factor),
        y: desktop(inner_position.y.into(), scale_factor),
        width: desktop(inner_size.width.into(), scale_factor),
        height: desktop(inner_size.height.into(), scale_factor),
    };
    // The OS's choice of monitor is also the one whose DPI the window uses. A
    // macOS window entirely off screen has none, so take the nearest.
    let current = window.current_monitor()?.as_ref().and_then(work_area);
    let Some(monitor) = current.or_else(|| home_work_area(&rect, &work_areas)) else {
        return Ok(Fit::Skipped);
    };
    let positionable = can_position_windows();
    if !positionable {
        rect.x = monitor.x;
        rect.y = monitor.y;
    }

    let visible = visible_work_areas(monitor, &work_areas);
    let Some(target) = fit_window(rect, monitor, &visible, Grip::at_scale(scale_factor)) else {
        record_fit(window, None);
        return Ok(Fit::Fits);
    };
    let target = if cfg!(target_os = "macos") { clear_of_maximized(target, monitor) } else { target };
    if mouse_button_down() {
        return Ok(Fit::Deferred);
    }
    // If the last fit sent the window from this same spot to this same target
    // and it is still here, the OS refused it; retrying would only loop.
    let refused = last_fit(window).is_some_and(|(from, to)| from.approx_eq(&rect) && to.approx_eq(&target));
    if refused {
        return Ok(Fit::Skipped);
    }
    record_fit(window, Some((rect, target)));

    crate::debug_log!(
        "[Window] fitting {} from {:?} to {:?} on monitor {:?}",
        window.label(),
        rect,
        target,
        monitor
    );
    if positionable {
        let (frame_x, frame_y) = frame_offset(window, scale_factor)?;
        place(window, target.x + frame_x, target.y + frame_y, target.width, target.height)?;
    } else {
        window.set_size(desktop_size(target.width, target.height))?;
    }
    Ok(Fit::Refitted)
}

fn last_fit(window: &Window) -> Option<(Rect, Rect)> {
    let state = window.state::<WindowPlacementState>();
    let windows = state.windows.lock().ok()?;
    windows.get(window.label())?.last_fit
}

fn record_fit(window: &Window, fit: Option<(Rect, Rect)>) {
    let state = window.state::<WindowPlacementState>();
    if let Ok(mut windows) = state.windows.lock() {
        windows.entry(window.label().to_string()).or_default().last_fit = fit;
    };
}

/// Samples the monitor layout every [`LAYOUT_POLL_INTERVAL`] and re-checks
/// every window when it changes.
pub(crate) fn watch_monitor_layout(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticks = tokio::time::interval(LAYOUT_POLL_INTERVAL);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            let handle = app.clone();
            // Fails only once the event loop has exited.
            if app.run_on_main_thread(move || check_monitor_layout(&handle)).is_err() {
                break;
            }
        }
    });
}

fn check_monitor_layout(app: &AppHandle) {
    let Ok(monitors) = app.available_monitors() else {
        return;
    };
    let layout: Vec<MonitorLayout> = monitors
        .iter()
        .map(|monitor| MonitorLayout {
            position: *monitor.position(),
            size: *monitor.size(),
            work_area: (monitor.work_area().position, monitor.work_area().size),
            scale_factor: monitor.scale_factor(),
        })
        .collect();

    let changed = {
        let state = app.state::<WindowPlacementState>();
        let Ok(mut last) = state.layout.lock() else {
            return;
        };
        let changed = last.as_ref().is_some_and(|last| *last != layout);
        *last = Some(layout);
        changed
    };
    if changed {
        crate::debug_log!("[Window] monitor layout changed; re-checking windows");
        for window in app.webview_windows().values() {
            schedule_settle(&window.as_ref().window());
        }
    }
}

// ============================================================================
// Saved bounds
// ============================================================================

/// The main window's bounds in the form they are saved (outer position and
/// content size, as `set_position`/`set_size` take them), or `None` while it
/// is minimized, fullscreen or (see [`os_maximized`]) maximized, and so not
/// where it should come back.
fn normal_bounds(window: &Window) -> tauri::Result<Option<WindowBounds>> {
    if window.is_minimized()? || window.is_fullscreen()? || os_maximized(window)? {
        return Ok(None);
    }
    let position = window.outer_position()?;
    let size = window.inner_size()?;
    let bounds = WindowBounds {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        scale_factor: Some(window.scale_factor()?),
    };
    Ok(Some(bounds).filter(is_plausible))
}

/// Saves the main window's bounds if they changed since the last save.
fn save_bounds(window: &Window) {
    let bounds = match normal_bounds(window) {
        Ok(Some(bounds)) => bounds,
        Ok(None) => return,
        Err(error) => {
            crate::warn_log!("failed to read window bounds: {error}");
            return;
        }
    };

    let state = window.state::<WindowPlacementState>();
    if state.saved_bounds.lock().map_or(true, |saved| saved.as_ref() == Some(&bounds)) {
        return;
    }
    // Remembered only once written, so a failed write is retried next time.
    // Every caller runs on the main thread, so no other save interleaves.
    match settings::save_window_bounds(window.app_handle(), bounds.clone()) {
        Ok(()) => {
            if let Ok(mut saved) = state.saved_bounds.lock() {
                *saved = Some(bounds);
            }
        }
        Err(error) => crate::warn_log!("failed to save window bounds: {error}"),
    }
}

/// Rejects bounds that cannot be a window's normal bounds, such as those
/// older builds saved while the window was minimized.
fn is_plausible(bounds: &WindowBounds) -> bool {
    bounds.width > 0
        && bounds.height > 0
        && bounds.x > MINIMIZED_POSITION
        && bounds.y > MINIMIZED_POSITION
        && bounds.scale_factor.is_none_or(|scale_factor| scale_factor > 0.0)
}

/// Moves the main window to its saved bounds, fitted to the monitors present
/// now, and checks it again once it settles.
pub(crate) fn restore_main_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(MAIN_WINDOW)
        .ok_or_else(|| "Main window not found".to_string())?
        .as_ref()
        .window();
    // A window from the default size is fitted by the settle, too.
    schedule_settle(&window);

    let Some(saved) = settings::get_settings(app.clone())?.window_bounds else {
        return Ok(());
    };
    if !is_plausible(&saved) {
        return Ok(());
    }
    place_at_saved_bounds(&window, &saved).map_err(|error| error.to_string())
}

fn place_at_saved_bounds(window: &Window, saved: &WindowBounds) -> tauri::Result<()> {
    let window_scale = window.scale_factor()?;
    // Bounds saved before the scale factor was recorded were applied with the
    // window's own scale factor, so keep doing that for them. They also hold
    // the outer size, which is the content size plus the frame.
    let (scale_factor, frame_width, frame_height) = match saved.scale_factor {
        Some(scale_factor) => (scale_factor, 0.0, 0.0),
        None => {
            let outer = window.outer_size()?;
            let inner = window.inner_size()?;
            let frame_width = f64::from(outer.width.saturating_sub(inner.width));
            let frame_height = f64::from(outer.height.saturating_sub(inner.height));
            (window_scale, frame_width, frame_height)
        }
    };
    let width = desktop((f64::from(saved.width) - frame_width).max(1.0), scale_factor);
    let height = desktop((f64::from(saved.height) - frame_height).max(1.0), scale_factor);
    if !can_position_windows() {
        // The saved position is meaningless here; the settle fits the size to
        // whichever monitor the compositor picks.
        return window.set_size(desktop_size(width, height));
    }

    // Fit the content rect, as the settle does: the saved position is the
    // outer frame's, which on Windows starts at the invisible resize border.
    let (frame_x, frame_y) = frame_offset(window, window_scale)?;
    let rect = Rect {
        x: desktop(saved.x.into(), scale_factor) - frame_x,
        y: desktop(saved.y.into(), scale_factor) - frame_y,
        width,
        height,
    };
    let work_areas = work_areas(&window.available_monitors()?);
    let Some(monitor) = home_work_area(&rect, &work_areas) else {
        return Ok(());
    };
    let visible = visible_work_areas(monitor, &work_areas);
    let target = match fit_window(rect, monitor, &visible, Grip::at_scale(scale_factor)) {
        Some(target) if cfg!(target_os = "macos") => clear_of_maximized(target, monitor),
        Some(target) => target,
        None => rect,
    };
    place(window, target.x + frame_x, target.y + frame_y, target.width, target.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect { x, y, width, height }
    }

    const GRIP: Grip = Grip { height: 35.0, min_width: 64.0 };

    /// A 2560x1440 monitor with a 1470x956 one to its right, each with a menu
    /// bar-sized strip taken off the top of the work area.
    fn desk() -> Vec<Rect> {
        vec![rect(0.0, 25.0, 2560.0, 1415.0), rect(2560.0, 33.0, 1470.0, 923.0)]
    }

    #[test]
    fn a_window_inside_its_monitor_is_left_alone() {
        let areas = desk();
        assert_eq!(fit_window(rect(100.0, 100.0, 1000.0, 800.0), areas[0], &areas, GRIP), None);
    }

    #[test]
    fn a_window_too_big_for_a_smaller_monitor_is_shrunk_into_it() {
        // Dragged or relocated from the big monitor onto the small one.
        let areas = desk();
        let fitted = fit_window(rect(2700.0, 100.0, 2000.0, 1200.0), areas[1], &areas, GRIP);
        assert_eq!(fitted, Some(rect(2560.0, 33.0, 1470.0, 923.0)));
    }

    #[test]
    fn a_window_too_tall_keeps_its_left_edge_and_moves_up() {
        let areas = desk();
        let fitted = fit_window(rect(2600.0, 300.0, 800.0, 1200.0), areas[1], &areas, GRIP);
        assert_eq!(fitted, Some(rect(2600.0, 33.0, 800.0, 923.0)));
    }

    #[test]
    fn a_window_that_grew_with_the_scale_factor_is_fitted() {
        // Windows: 1600x1000 logical moved from 100% to a 150% 1920x1080
        // monitor becomes 2400x1500 physical against a 1920x1032 work area.
        let areas = vec![rect(0.0, 0.0, 2560.0, 1400.0), rect(2560.0, 0.0, 1920.0, 1032.0)];
        let fitted = fit_window(rect(2600.0, 20.0, 2400.0, 1500.0), areas[1], &areas, GRIP);
        assert_eq!(fitted, Some(rect(2560.0, 0.0, 1920.0, 1032.0)));
    }

    #[test]
    fn a_window_parked_partly_off_screen_stays_put() {
        let areas = desk();
        // Right half past the edge of the right monitor, bottom past the
        // bottom of the left one: both still fit and can be grabbed.
        assert_eq!(fit_window(rect(3600.0, 200.0, 800.0, 600.0), areas[1], &areas, GRIP), None);
        assert_eq!(fit_window(rect(200.0, 1100.0, 800.0, 600.0), areas[0], &areas, GRIP), None);
    }

    #[test]
    fn a_window_stretched_across_monitors_stays_put() {
        let areas = vec![rect(0.0, 0.0, 1920.0, 1040.0), rect(1920.0, 0.0, 1920.0, 1040.0)];
        assert_eq!(fit_window(rect(400.0, 100.0, 3000.0, 900.0), areas[0], &areas, GRIP), None);
    }

    #[test]
    fn a_window_stretched_past_every_monitor_is_fitted() {
        let areas = vec![rect(0.0, 0.0, 1920.0, 1040.0), rect(1920.0, 0.0, 1920.0, 1040.0)];
        let fitted = fit_window(rect(400.0, 100.0, 3000.0, 1200.0), areas[1], &areas, GRIP);
        assert_eq!(fitted, Some(rect(1920.0, 0.0, 1920.0, 1040.0)));
    }

    #[test]
    fn a_titlebar_under_the_menu_bar_is_brought_down() {
        let areas = desk();
        let fitted = fit_window(rect(200.0, 10.0, 800.0, 600.0), areas[0], &areas, GRIP);
        assert_eq!(fitted, Some(rect(200.0, 25.0, 800.0, 600.0)));
    }

    #[test]
    fn a_titlebar_almost_off_screen_is_brought_back() {
        let areas = desk();
        // Only 40 points of the titlebar show past the left monitor's left edge.
        let fitted = fit_window(rect(-760.0, 200.0, 800.0, 600.0), areas[0], &areas, GRIP);
        assert_eq!(fitted, Some(rect(0.0, 200.0, 800.0, 600.0)));
    }

    #[test]
    fn a_titlebar_straddling_two_monitors_counts_when_one_side_is_wide_enough() {
        let areas = desk();
        assert_eq!(fit_window(rect(2400.0, 100.0, 800.0, 600.0), areas[1], &areas, GRIP), None);
    }

    #[test]
    fn a_window_on_negative_coordinates_is_fitted_in_place() {
        // A monitor left of and above the primary one.
        let areas = vec![rect(-1440.0, -900.0, 1440.0, 875.0), rect(0.0, 0.0, 2560.0, 1400.0)];
        let fitted = fit_window(rect(-1400.0, -880.0, 1800.0, 1000.0), areas[0], &areas, GRIP);
        assert_eq!(fitted, Some(rect(-1440.0, -900.0, 1440.0, 875.0)));
    }

    #[test]
    fn rounding_differences_do_not_cause_a_fit() {
        let areas = desk();
        // One point over in each direction, as physical-to-logical rounding produces.
        assert_eq!(fit_window(rect(2560.0, 33.0, 1471.0, 924.0), areas[1], &areas, GRIP), None);
    }

    #[test]
    fn a_fitted_window_is_left_alone_on_the_next_check() {
        let areas = desk();
        let fitted = fit_window(rect(2700.0, 100.0, 2000.0, 1200.0), areas[1], &areas, GRIP);
        let fitted = fitted.expect("the oversized window should be fitted");
        assert_eq!(fit_window(fitted, areas[1], &areas, GRIP), None);
    }

    #[test]
    fn the_home_monitor_holds_most_of_the_window() {
        let areas = desk();
        assert_eq!(home_work_area(&rect(2000.0, 100.0, 800.0, 600.0), &areas), Some(areas[0]));
        assert_eq!(home_work_area(&rect(2400.0, 100.0, 800.0, 600.0), &areas), Some(areas[1]));
    }

    #[test]
    fn a_window_on_no_monitor_goes_home_to_the_nearest_one() {
        let areas = desk();
        assert_eq!(home_work_area(&rect(5000.0, 100.0, 800.0, 600.0), &areas), Some(areas[1]));
        assert_eq!(home_work_area(&rect(-3000.0, 100.0, 800.0, 600.0), &areas), Some(areas[0]));
        assert_eq!(home_work_area(&rect(0.0, 0.0, 10.0, 10.0), &[]), None);
    }

    #[test]
    fn a_lost_window_is_brought_onto_its_nearest_monitor() {
        let areas = desk();
        let lost = rect(5000.0, 100.0, 800.0, 600.0);
        let home = home_work_area(&lost, &areas).expect("a monitor to go home to");
        assert_eq!(fit_window(lost, home, &areas, GRIP), Some(rect(3230.0, 100.0, 800.0, 600.0)));
    }

    #[test]
    fn a_fit_that_fills_the_monitor_stops_short_of_looking_maximized() {
        let monitor = rect(0.0, 33.0, 1470.0, 838.0);
        let filled = clear_of_maximized(monitor, monitor);
        assert_eq!(filled, rect(0.0, 33.0, 1470.0, 836.0));
        // Still counts as fitting, so the next check leaves it alone.
        assert_eq!(fit_window(filled, monitor, &[monitor], GRIP), None);
        // Anything not filling the monitor is untouched.
        let smaller = rect(0.0, 33.0, 1000.0, 838.0);
        assert_eq!(clear_of_maximized(smaller, monitor), smaller);
    }

    #[test]
    fn a_straddling_window_counts_only_the_display_drawing_it() {
        // With separate Spaces, only the window's own monitor is passed in, so
        // a window too big for it is fitted even though the other display
        // would show the rest.
        let areas = vec![rect(0.0, 0.0, 1920.0, 1040.0), rect(1920.0, 0.0, 1920.0, 1040.0)];
        let window = rect(400.0, 100.0, 3000.0, 900.0);
        assert_eq!(fit_window(window, areas[0], &areas, GRIP), None);
        assert_eq!(fit_window(window, areas[0], &areas[..1], GRIP), Some(rect(0.0, 100.0, 1920.0, 900.0)));
    }

    #[test]
    fn minimized_and_empty_bounds_are_not_restored() {
        let bounds = |x, y, width, height| WindowBounds { x, y, width, height, scale_factor: None };
        assert!(is_plausible(&bounds(100, 100, 1000, 800)));
        assert!(is_plausible(&bounds(-2560, -300, 1000, 800)));
        assert!(!is_plausible(&bounds(-32000, -32000, 160, 28)));
        assert!(!is_plausible(&bounds(100, 100, 0, 800)));
        assert!(!is_plausible(&WindowBounds { scale_factor: Some(0.0), ..bounds(100, 100, 1000, 800) }));
    }
}
