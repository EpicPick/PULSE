use windows::{
    Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
        UI::WindowsAndMessaging::MONITORINFOF_PRIMARY,
    },
    core::BOOL,
};

pub struct Placement {
    pub position: (i32, i32),
    pub size: (u32, u32),
}

pub fn secondary_window() -> Option<Placement> {
    let mut areas = Vec::<RECT>::new();
    unsafe extern "system" fn visit(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: EnumDisplayMonitors calls synchronously with our live vector pointer and
        // valid monitor handles. MonitorInfo has the correct ABI size.
        unsafe {
            if GetMonitorInfoW(monitor, &mut info).as_bool()
                && info.dwFlags & MONITORINFOF_PRIMARY == 0
            {
                (*(data.0 as *mut Vec<RECT>)).push(info.rcWork);
            }
        }
        BOOL(1)
    }
    // SAFETY: callback receives the stack pointer only during this synchronous call.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(visit),
            LPARAM((&mut areas as *mut Vec<RECT>) as isize),
        )
    };
    if !ok.as_bool() {
        return None;
    }
    areas.into_iter().filter_map(place).max_by_key(|p| {
        (
            u64::from(p.size.0) * u64::from(p.size.1),
            std::cmp::Reverse(p.position),
        )
    })
}

fn place(rect: RECT) -> Option<Placement> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    if width < 640 || height < 480 {
        return None;
    }
    let w = (width - 40).min(1440);
    let h = (height - 72).min(900);
    Some(Placement {
        position: (
            rect.left + (width - w) / 2,
            rect.top + (height - h - 40) / 2,
        ),
        size: (w as u32, h as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn centers_inside_secondary_work_area_even_at_negative_coordinates() {
        let p = place(RECT {
            left: -1920,
            top: -100,
            right: 0,
            bottom: 980,
        })
        .unwrap();
        assert_eq!(p.position, (-1680, -30));
        assert_eq!(p.size, (1440, 900));
    }
}
