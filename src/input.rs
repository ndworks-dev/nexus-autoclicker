//! Thin wrappers around the Win32 APIs for mouse input, keyboard state and DPI.

use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

/// Makes screen capture and cursor coordinates use physical pixels, so they
/// line up even with display scaling (125%, 150%, ...).
pub fn enable_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

pub fn esc_pressed() -> bool {
    unsafe { (GetAsyncKeyState(VK_ESCAPE.0 as i32) as u16 & 0x8000) != 0 }
}

fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// Moves the cursor to `(x, y)` with an ease-out-quad curve, like
/// `pyautogui.moveTo(..., pyautogui.easeOutQuad)`. Returns `false` if
/// `should_stop` asked to abort midway.
pub fn move_smooth(x: i32, y: i32, duration: Duration, should_stop: &dyn Fn() -> bool) -> bool {
    let (sx, sy) = cursor_pos();
    let start = Instant::now();
    loop {
        if should_stop() {
            return false;
        }
        let t = (start.elapsed().as_secs_f64() / duration.as_secs_f64().max(1e-3)).min(1.0);
        let e = -t * (t - 2.0);
        let cx = sx as f64 + (x - sx) as f64 * e;
        let cy = sy as f64 + (y - sy) as f64 * e;
        unsafe {
            let _ = SetCursorPos(cx.round() as i32, cy.round() as i32);
        }
        if t >= 1.0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: flags,
                ..Default::default()
            },
        },
    }
}

pub fn left_click() {
    let inputs = [mouse_input(MOUSEEVENTF_LEFTDOWN), mouse_input(MOUSEEVENTF_LEFTUP)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}
