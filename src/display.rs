use crate::debug_window::DebugWindow;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{LPARAM, RECT};
#[cfg(windows)]
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayTarget {
    pub id: usize,
    pub left: isize,
    pub top: isize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowConfig {
    pub x: isize,
    pub y: isize,
    pub width: usize,
    pub height: usize,
    pub monitor_ids: Option<Vec<usize>>,
}

impl WindowConfig {
    pub fn from_parts(
        x: isize,
        y: isize,
        width: usize,
        height: usize,
        monitor_ids: Option<Vec<usize>>,
    ) -> Self {
        Self::new(x, y, width, height, monitor_ids)
    }

    pub fn new(
        x: isize,
        y: isize,
        width: usize,
        height: usize,
        monitor_ids: Option<Vec<usize>>,
    ) -> Self {
        Self {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
            monitor_ids,
        }
    }

    fn selected_displays(&self) -> Vec<DisplayTarget> {
        let displays = enumerate_displays();
        let filtered = filter_displays(&displays, self.monitor_ids.as_deref());
        if filtered.is_empty() {
            displays
        } else {
            filtered
        }
    }
}

#[cfg(windows)]
pub fn enumerate_displays() -> Vec<DisplayTarget> {
    let mut displays = Vec::new();

    unsafe extern "system" fn enum_proc(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> windows_sys::core::BOOL {
        let displays = unsafe { &mut *(lparam as *mut Vec<DisplayTarget>) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            rcWork: RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            },
            dwFlags: 0,
        };

        if unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFO) } != 0 {
            let id = displays.len();
            displays.push(DisplayTarget {
                id,
                left: info.rcMonitor.left as isize,
                top: info.rcMonitor.top as isize,
            });
        }

        1
    }

    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(enum_proc),
            &mut displays as *mut Vec<DisplayTarget> as LPARAM,
        );
    }

    displays
}

#[cfg(not(windows))]
pub fn enumerate_displays() -> Vec<DisplayTarget> {
    vec![DisplayTarget {
        id: 0,
        left: 0,
        top: 0,
    }]
}

pub fn parse_display_selection(value: &str) -> Option<Vec<usize>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut displays = Vec::new();
    for part in trimmed.split(',') {
        let id = part.trim().parse::<usize>().ok()?;
        if !displays.contains(&id) {
            displays.push(id);
        }
    }

    Some(displays)
}

fn filter_displays(displays: &[DisplayTarget], selection: Option<&[usize]>) -> Vec<DisplayTarget> {
    match selection {
        Some(ids) => displays
            .iter()
            .copied()
            .filter(|display| ids.contains(&display.id))
            .collect(),
        None => displays.to_vec(),
    }
}

pub fn create_debug_windows(config: &WindowConfig, fps: usize) -> Vec<DebugWindow> {
    config
        .selected_displays()
        .into_iter()
        .filter_map(|display| {
            DebugWindow::new(
                config.width,
                config.height,
                fps,
                display.left + config.x,
                display.top + config.y,
            )
        })
        .collect()
}

pub fn sync_debug_windows(
    windows: &mut Vec<DebugWindow>,
    config: &WindowConfig,
    visible: bool,
    fps: usize,
) {
    if visible {
        let displays = config.selected_displays();
        let needs_resize = windows
            .iter()
            .any(|window| window.size() != (config.width, config.height));
        let needs_layout = windows.len() != displays.len();

        if needs_resize || needs_layout {
            *windows = create_debug_windows(config, fps);
        } else {
            for (window, display) in windows.iter_mut().zip(displays.iter()) {
                window.set_position(display.left + config.x, display.top + config.y);
            }
        }
    } else if !windows.is_empty() {
        windows.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_display_selection_accepts_unique_values() {
        assert_eq!(parse_display_selection("0,2,2,1"), Some(vec![0, 2, 1]));
    }

    #[test]
    fn parse_display_selection_rejects_invalid_values() {
        assert_eq!(parse_display_selection("0,abc,1"), None);
    }
}
