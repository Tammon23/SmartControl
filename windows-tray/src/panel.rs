use crate::backend::{Action, Snapshot};
use std::{
    mem::size_of,
    ptr::{null, null_mut},
    sync::{Mutex, OnceLock},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{
        Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETLIMITTEXT, EM_SETSEL},
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        Shell::{DefSubclassProc, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const WIDTH: i32 = 360;
const HEIGHT: i32 = 460;
static WINDOW: OnceLock<usize> = OnceLock::new();
static OWNER: OnceLock<usize> = OnceLock::new();
static VIEW: Mutex<View> = Mutex::new(View {
    snapshot: None,
    message: String::new(),
    label: String::new(),
    busy: false,
    failed: false,
    drag: None,
    updated: None,
    renaming: false,
});
struct View {
    snapshot: Option<Snapshot>,
    message: String,
    label: String,
    busy: bool,
    failed: bool,
    drag: Option<u8>,
    updated: Option<Instant>,
    renaming: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub(crate) fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}
fn owner() -> HWND {
    *OWNER.get().unwrap() as HWND
}
pub fn snapshot() -> Option<Snapshot> {
    VIEW.lock().unwrap().snapshot.clone()
}
pub fn recent_power() -> Option<bool> {
    let view = VIEW.lock().unwrap();
    if !view.failed && view.updated.is_some_and(|t| t.elapsed().as_secs() < 15) {
        view.snapshot.as_ref().and_then(|s| s.power)
    } else {
        None
    }
}
pub unsafe fn begin_rename() {
    let window = *WINDOW.get().unwrap() as HWND;
    if IsWindowVisible(window) == 0 {
        toggle_visibility();
    }
    if !GetDlgItem(window, 900).is_null() {
        return;
    }
    let label = {
        let mut view = VIEW.lock().unwrap();
        view.renaming = true;
        view.label.clone()
    };
    InvalidateRect(window, null(), 0);
    let dpi = GetDpiForWindow(window).max(96) as i32;
    let edit = CreateWindowExW(
        0,
        wide("EDIT").as_ptr(),
        wide(&label).as_ptr(),
        WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
        36 * dpi / 96,
        166 * dpi / 96,
        288 * dpi / 96,
        28 * dpi / 96,
        window,
        900usize as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    let font = CreateFontW(
        -18 * dpi / 96,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide("Segoe UI").as_ptr(),
    );
    SendMessageW(edit, WM_SETFONT, font as usize, 1);
    SetWindowSubclass(edit, Some(edit_procedure), 1, font as usize);
    SendMessageW(edit, EM_SETLIMITTEXT, 120, 0);
    SetFocus(edit);
    SendMessageW(edit, EM_SETSEL, 0, -1);
}
unsafe fn close_rename(hwnd: HWND) {
    DestroyWindow(GetDlgItem(hwnd, 900));
    VIEW.lock().unwrap().renaming = false;
    SetFocus(hwnd);
    InvalidateRect(hwnd, null(), 0);
}
fn previous_word_start(text: &[u16], caret: usize) -> usize {
    let caret = caret.min(text.len());
    let prefix = String::from_utf16_lossy(&text[..caret]);
    let mut chars: Vec<char> = prefix.chars().collect();
    while chars.last().is_some_and(|c| c.is_whitespace()) {
        chars.pop();
    }
    if let Some(last) = chars.last() {
        let word = last.is_alphanumeric() || *last == '_';
        while chars
            .last()
            .is_some_and(|c| !c.is_whitespace() && (c.is_alphanumeric() || *c == '_') == word)
        {
            chars.pop();
        }
    }
    chars.iter().map(|c| c.len_utf16()).sum()
}
unsafe extern "system" fn edit_procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    font: usize,
) -> LRESULT {
    if message == WM_KEYDOWN {
        let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
        if wparam == VK_BACK as usize && ctrl {
            let (mut start, mut end) = (0u32, 0u32);
            SendMessageW(
                hwnd,
                EM_GETSEL,
                &mut start as *mut _ as usize,
                &mut end as *mut _ as isize,
            );
            if start == end {
                let mut buffer = [0u16; 256];
                let count = GetWindowTextW(hwnd, buffer.as_mut_ptr(), 256);
                start =
                    previous_word_start(&buffer[..count.max(0) as usize], start as usize) as u32;
            }
            SendMessageW(hwnd, EM_SETSEL, start as usize, end as isize);
            SendMessageW(hwnd, EM_REPLACESEL, 1, wide("").as_ptr() as isize);
            return 0;
        }
        if ctrl && wparam == b'A' as usize {
            SendMessageW(hwnd, EM_SETSEL, 0, -1);
            return 0;
        }
        if wparam == VK_RETURN as usize || wparam == VK_ESCAPE as usize {
            PostMessageW(
                GetParent(hwnd),
                WM_COMMAND,
                if wparam == VK_RETURN as usize {
                    901
                } else {
                    902
                },
                0,
            );
            return 0;
        }
    }
    if message == WM_CHAR
        && ([0x7f, 0x0d, 0x1b, 0x01].contains(&wparam)
            || (wparam == 8 && GetKeyState(VK_CONTROL as i32) < 0))
    {
        return 0;
    }
    let result = DefSubclassProc(hwnd, message, wparam, lparam);
    if message == WM_NCDESTROY && font != 0 {
        DeleteObject(font as HGDIOBJ);
    }
    result
}

pub fn busy() -> bool {
    VIEW.lock().unwrap().busy
}
pub fn update(state: Option<Snapshot>, message: String, working: bool, failed: bool) {
    let mut view = VIEW.lock().unwrap();
    if let Some(state) = state {
        view.snapshot = Some(state);
        view.updated = Some(Instant::now());
    }
    view.message = message;
    view.busy = working;
    view.failed = failed;
    drop(view);
    unsafe {
        if let Some(window) = WINDOW.get() {
            InvalidateRect(*window as HWND, null(), 0);
        }
    }
}
pub unsafe fn create(root: HWND, label: &str) -> Result<(), String> {
    OWNER.set(root as usize).ok();
    VIEW.lock().unwrap().label = label.to_string();
    let instance = GetModuleHandleW(null());
    let name = wide("BulbBridgeGraphicPanel");
    let mut class: WNDCLASSW = std::mem::zeroed();
    class.lpfnWndProc = Some(procedure);
    class.hInstance = instance;
    class.lpszClassName = name.as_ptr();
    class.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    class.style = CS_DROPSHADOW;
    if RegisterClassW(&class) == 0 {
        return Err("Cannot register control panel".into());
    }
    let window = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        name.as_ptr(),
        wide("Bulb Bridge controls").as_ptr(),
        WS_POPUP | WS_CLIPCHILDREN,
        0,
        0,
        WIDTH,
        HEIGHT,
        root,
        null_mut(),
        instance,
        null(),
    );
    if window.is_null() {
        return Err("Cannot create control panel".into());
    }
    WINDOW.set(window as usize).ok();
    Ok(())
}
pub unsafe fn toggle_visibility() {
    let window = *WINDOW.get().unwrap() as HWND;
    if IsWindowVisible(window) != 0 {
        ShowWindow(window, SW_HIDE);
        return;
    }
    let mut cursor = POINT { x: 0, y: 0 };
    GetCursorPos(&mut cursor);
    let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
    let mut info: MONITORINFO = std::mem::zeroed();
    info.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(monitor, &mut info);
    let scale = GetDpiForWindow(window).max(96) as f64 / 96.0;
    let (w, h) = (
        (WIDTH as f64 * scale) as i32,
        (HEIGHT as f64 * scale) as i32,
    );
    let x = (cursor.x - w / 2).clamp(
        info.rcWork.left + 8,
        (info.rcWork.right - w - 8).max(info.rcWork.left + 8),
    );
    let y = (cursor.y - h - 10).clamp(
        info.rcWork.top + 8,
        (info.rcWork.bottom - h - 8).max(info.rcWork.top + 8),
    );
    SetWindowPos(window, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    SetForegroundWindow(window);
    let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, 20, 20);
    if !region.is_null() {
        SetWindowRgn(window, region, 1);
    }
    let refresh = VIEW
        .lock()
        .unwrap()
        .updated
        .is_none_or(|time| time.elapsed().as_secs() > 15);
    if refresh {
        crate::tray::enqueue(owner(), Action::Refresh);
    }
}

pub(crate) unsafe fn fill(dc: HDC, rect: RECT, colour: u32, radius: i32) {
    let brush = CreateSolidBrush(colour);
    let old = SelectObject(dc, brush);
    let pen = SelectObject(dc, GetStockObject(NULL_PEN));
    if radius > 0 {
        RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius,
            radius,
        );
    } else {
        Rectangle(dc, rect.left, rect.top, rect.right, rect.bottom);
    }
    SelectObject(dc, old);
    SelectObject(dc, pen);
    DeleteObject(brush);
}
unsafe fn circle(dc: HDC, x: i32, y: i32, r: i32, colour: u32) {
    let brush = CreateSolidBrush(colour);
    let old = SelectObject(dc, brush);
    let pen = SelectObject(dc, GetStockObject(NULL_PEN));
    Ellipse(dc, x - r, y - r, x + r, y + r);
    SelectObject(dc, old);
    SelectObject(dc, pen);
    DeleteObject(brush);
}
unsafe fn line(dc: HDC, x: i32, y: i32, x2: i32, y2: i32, colour: u32, width: i32) {
    let pen = CreatePen(PS_SOLID, width, colour);
    let old = SelectObject(dc, pen);
    MoveToEx(dc, x, y, null_mut());
    LineTo(dc, x2, y2);
    SelectObject(dc, old);
    DeleteObject(pen);
}
pub(crate) unsafe fn text(
    dc: HDC,
    value: &str,
    rect: RECT,
    size: i32,
    weight: i32,
    colour: u32,
    flags: u32,
) {
    let font = CreateFontW(
        -size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide("Segoe UI").as_ptr(),
    );
    let old = SelectObject(dc, font);
    SetTextColor(dc, colour);
    SetBkMode(dc, TRANSPARENT as i32);
    let mut bounds = rect;
    let words = wide(value);
    DrawTextW(
        dc,
        words.as_ptr(),
        words.len() as i32 - 1,
        &mut bounds,
        flags | DT_NOPREFIX,
    );
    SelectObject(dc, old);
    DeleteObject(font);
}
pub(crate) fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}
unsafe fn bulb(dc: HDC, cx: i32, cy: i32, on: bool, colour: u32) {
    if on {
        for (x, y, x2, y2) in [
            (-26, -16, -33, -21),
            (26, -16, 33, -21),
            (0, -28, 0, -36),
            (-31, 2, -39, 2),
            (31, 2, 39, 2),
        ] {
            line(dc, cx + x, cy + y, cx + x2, cy + y2, colour, 2);
        }
    }
    circle(
        dc,
        cx,
        cy - 4,
        18,
        if on { colour } else { rgb(106, 117, 135) },
    );
    fill(
        dc,
        rect(cx - 10, cy + 7, cx + 10, cy + 24),
        if on { colour } else { rgb(106, 117, 135) },
        8,
    );
    fill(
        dc,
        rect(cx - 10, cy + 22, cx + 10, cy + 27),
        rgb(165, 177, 194),
        3,
    );
    fill(
        dc,
        rect(cx - 7, cy + 29, cx + 7, cy + 33),
        rgb(121, 135, 154),
        3,
    );
    line(dc, cx - 7, cy - 2, cx - 2, cy + 8, rgb(40, 46, 60), 2);
    line(dc, cx - 2, cy + 8, cx + 2, cy + 8, rgb(40, 46, 60), 2);
    line(dc, cx + 2, cy + 8, cx + 7, cy - 2, rgb(40, 46, 60), 2);
}
unsafe fn content(dc: HDC, w: i32, h: i32) {
    SetMapMode(dc, MM_ANISOTROPIC);
    SetWindowExtEx(dc, WIDTH, HEIGHT, null_mut());
    SetViewportExtEx(dc, w, h, null_mut());
    let view = VIEW.lock().unwrap();
    let state = view.snapshot.clone().unwrap_or_default();
    let on = state.power == Some(true);
    let accent = rgb(255, 204, 107);
    let secondary = rgb(145, 155, 173);
    let foreground = rgb(237, 241, 247);
    let (r, g, b) = state.rgb();
    let tint = rgb(r, g, b);
    fill(dc, rect(0, 0, WIDTH, HEIGHT), rgb(20, 24, 33), 0);
    text(
        dc,
        "BULB BRIDGE",
        rect(24, 17, 270, 37),
        11,
        600,
        secondary,
        DT_SINGLELINE | DT_VCENTER,
    );
    if view.renaming {
        text(
            dc,
            "Rename bulb",
            rect(24, 48, 310, 80),
            23,
            600,
            foreground,
            DT_SINGLELINE,
        );
        text(
            dc,
            "Choose a name for this app.",
            rect(24, 94, 336, 120),
            13,
            400,
            secondary,
            DT_SINGLELINE,
        );
        text(
            dc,
            "BULB NAME",
            rect(24, 130, 336, 152),
            11,
            600,
            secondary,
            DT_SINGLELINE,
        );
        fill(dc, rect(24, 154, 336, 206), accent, 12);
        fill(dc, rect(25, 155, 335, 205), rgb(31, 37, 49), 10);
        text(
            dc,
            &view.label,
            rect(36, 166, 324, 194),
            18,
            400,
            foreground,
            DT_SINGLELINE,
        );
        fill(dc, rect(24, 226, 172, 268), rgb(42, 50, 65), 12);
        text(
            dc,
            "Cancel",
            rect(24, 226, 172, 268),
            14,
            600,
            foreground,
            DT_SINGLELINE | DT_CENTER | DT_VCENTER,
        );
        fill(dc, rect(188, 226, 336, 268), accent, 12);
        text(
            dc,
            "Save name",
            rect(188, 226, 336, 268),
            14,
            600,
            rgb(28, 30, 37),
            DT_SINGLELINE | DT_CENTER | DT_VCENTER,
        );
        text(
            dc,
            "Enter to save · Esc to cancel",
            rect(24, 286, 336, 310),
            12,
            400,
            secondary,
            DT_SINGLELINE,
        );
        return;
    }
    text(
        dc,
        &view.label,
        rect(24, 38, 300, 66),
        23,
        600,
        foreground,
        DT_SINGLELINE | DT_END_ELLIPSIS,
    );
    text(
        dc,
        "×",
        rect(312, 17, 342, 49),
        25,
        400,
        secondary,
        DT_SINGLELINE | DT_CENTER | DT_VCENTER,
    );
    fill(dc, rect(23, 79, 337, 188), rgb(64, 70, 84), 20);
    fill(
        dc,
        rect(24, 80, 336, 187),
        if on { rgb(43, 40, 35) } else { rgb(32, 38, 49) },
        20,
    );
    circle(
        dc,
        72,
        130,
        41,
        if on { rgb(65, 53, 36) } else { rgb(41, 49, 63) },
    );
    bulb(dc, 72, 124, on, tint);
    let label = if state.power.is_none() {
        "POWER UNKNOWN"
    } else if on {
        "LIGHT IS ON"
    } else {
        "LIGHT IS OFF"
    };
    text(
        dc,
        label,
        rect(128, 99, 282, 117),
        10,
        600,
        if on { accent } else { secondary },
        DT_SINGLELINE | DT_VCENTER,
    );
    text(
        dc,
        if on { "Turn off" } else { "Turn on" },
        rect(128, 120, 279, 152),
        24,
        600,
        foreground,
        DT_SINGLELINE | DT_VCENTER,
    );
    text(
        dc,
        if view.busy {
            "Updating..."
        } else {
            "Click to toggle"
        },
        rect(128, 156, 282, 174),
        11,
        400,
        secondary,
        DT_SINGLELINE,
    );
    fill(
        dc,
        rect(282, 123, 320, 145),
        if on { accent } else { rgb(79, 90, 110) },
        22,
    );
    circle(
        dc,
        if on { 309 } else { 293 },
        134,
        8,
        if on {
            rgb(44, 37, 26)
        } else {
            rgb(226, 232, 240)
        },
    );

    let brightness = view.drag.or(state.brightness).unwrap_or(50);
    text(
        dc,
        "Brightness",
        rect(24, 207, 250, 231),
        13,
        600,
        foreground,
        DT_SINGLELINE | DT_VCENTER,
    );
    text(
        dc,
        &format!("{brightness}%"),
        rect(270, 207, 337, 231),
        15,
        600,
        accent,
        DT_SINGLELINE | DT_RIGHT | DT_VCENTER,
    );
    fill(dc, rect(24, 249, 336, 255), rgb(57, 66, 82), 6);
    let knob = 24 + (brightness as i32 * 312 / 100);
    if knob > 24 {
        fill(dc, rect(24, 249, knob, 255), accent, 6);
    }
    circle(dc, knob, 252, 9, accent);
    circle(dc, knob, 252, 3, rgb(54, 44, 30));
    text(
        dc,
        "0%",
        rect(24, 269, 90, 286),
        10,
        400,
        secondary,
        DT_SINGLELINE,
    );
    text(
        dc,
        "100%",
        rect(270, 269, 337, 286),
        10,
        400,
        secondary,
        DT_SINGLELINE | DT_RIGHT,
    );
    text(
        dc,
        "Colour",
        rect(24, 302, 300, 323),
        13,
        600,
        foreground,
        DT_SINGLELINE,
    );
    for (i, (name, colour)) in [
        ("Warm", rgb(255, 184, 82)),
        ("White", rgb(233, 239, 249)),
        ("Red", rgb(244, 91, 109)),
        ("Green", rgb(93, 211, 164)),
        ("Blue", rgb(98, 155, 250)),
        ("More", rgb(153, 123, 224)),
    ]
    .iter()
    .enumerate()
    {
        let x = 43 + i as i32 * 52;
        circle(dc, x, 350, 20, rgb(52, 61, 77));
        circle(dc, x, 350, 16, *colour);
        if i == 5 {
            line(dc, x - 5, 350, x + 5, 350, rgb(255, 255, 255), 2);
            line(dc, x, 345, x, 355, rgb(255, 255, 255), 2);
        }
        text(
            dc,
            name,
            rect(x - 25, 377, x + 25, 396),
            10,
            400,
            secondary,
            DT_SINGLELINE | DT_CENTER,
        );
    }
    fill(dc, rect(24, 409, 336, 410), rgb(48, 56, 71), 0);
    let note = if view.message.is_empty() {
        "Connecting to your light..."
    } else {
        &view.message
    };
    text(
        dc,
        note,
        rect(24, 421, 282, 455),
        11,
        400,
        if view.failed {
            rgb(255, 133, 133)
        } else {
            secondary
        },
        DT_WORDBREAK,
    );
    text(
        dc,
        "↻",
        rect(294, 417, 338, 453),
        26,
        400,
        if view.busy { secondary } else { accent },
        DT_SINGLELINE | DT_CENTER | DT_VCENTER,
    );
    if view.busy {
        text(
            dc,
            "•",
            rect(308, 206, 340, 230),
            10,
            400,
            secondary,
            DT_RIGHT,
        );
    }
}

unsafe fn coordinates(hwnd: HWND, lparam: LPARAM) -> (i32, i32) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut r);
    (
        ((lparam as i16) as i32) * WIDTH / (r.right - r.left).max(1),
        (((lparam >> 16) as i16) as i32) * HEIGHT / (r.bottom - r.top).max(1),
    )
}
unsafe fn slider(hwnd: HWND, x: i32) {
    VIEW.lock().unwrap().drag = Some((((x - 24) * 100) / 312).clamp(0, 100) as u8);
    InvalidateRect(hwnd, null(), 0);
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            let mut bounds: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut bounds);
            let memory = CreateCompatibleDC(dc);
            let bitmap = CreateCompatibleBitmap(dc, bounds.right.max(1), bounds.bottom.max(1));
            let old = SelectObject(memory, bitmap);
            content(memory, bounds.right, bounds.bottom);
            SetMapMode(memory, MM_TEXT);
            BitBlt(dc, 0, 0, bounds.right, bounds.bottom, memory, 0, 0, SRCCOPY);
            SelectObject(memory, old);
            DeleteObject(bitmap);
            DeleteDC(memory);
            EndPaint(hwnd, &paint);
            0
        }
        WM_CTLCOLOREDIT => {
            let dc = wparam as HDC;
            SetTextColor(dc, rgb(237, 241, 247));
            SetBkColor(dc, rgb(31, 37, 49));
            static BRUSH: OnceLock<usize> = OnceLock::new();
            *BRUSH.get_or_init(|| CreateSolidBrush(rgb(31, 37, 49)) as usize) as LRESULT
        }
        WM_COMMAND if wparam & 0xffff == 902 => {
            close_rename(hwnd);
            0
        }
        WM_COMMAND if wparam & 0xffff == 901 => {
            let edit = GetDlgItem(hwnd, 900);
            let mut buffer = [0u16; 128];
            let count = GetWindowTextW(edit, buffer.as_mut_ptr(), 128);
            let label = String::from_utf16_lossy(&buffer[..count.max(0) as usize]);
            match crate::tray::rename(&label) {
                Ok(()) => {
                    VIEW.lock().unwrap().label = label.trim().into();
                    close_rename(hwnd);
                    update(None, "Name saved".into(), busy(), false);
                }
                Err(e) => {
                    MessageBoxW(
                        hwnd,
                        wide(&e).as_ptr(),
                        wide("Rename bulb").as_ptr(),
                        MB_OK | MB_ICONERROR,
                    );
                }
            }
            0
        }
        WM_ERASEBKGND => 1,
        WM_CLOSE => {
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_ACTIVATE if (wparam as u32 & 0xffff) == WA_INACTIVE => {
            if !crate::tray::choosing_colour() {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_KEYDOWN if wparam as u32 == VK_ESCAPE as u32 => {
            if VIEW.lock().unwrap().renaming {
                close_rename(hwnd);
            } else {
                ShowWindow(hwnd, SW_HIDE);
            }
            0
        }
        WM_KEYDOWN if wparam as u32 == VK_SPACE as u32 => {
            crate::tray::enqueue(owner(), Action::Toggle);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = coordinates(hwnd, lparam);
            if x >= 310 && y < 60 {
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            if VIEW.lock().unwrap().renaming {
                if (226..=268).contains(&y) {
                    if (188..=336).contains(&x) {
                        SendMessageW(hwnd, WM_COMMAND, 901, 0);
                    } else if (24..=172).contains(&x) {
                        close_rename(hwnd);
                    }
                }
                return 0;
            }
            if (24..=300).contains(&x) && (35..=73).contains(&y) {
                begin_rename();
                return 0;
            }
            if (24..=336).contains(&x) && (80..=188).contains(&y) {
                crate::tray::enqueue(owner(), Action::Toggle);
            } else if (16..=344).contains(&x) && (236..=280).contains(&y) {
                slider(hwnd, x);
                SetCapture(hwnd);
            } else if (324..=398).contains(&y) {
                let i = ((x - 17) / 52).clamp(0, 5);
                if (17..=330).contains(&x) {
                    if i == 5 {
                        if let Some(action) = crate::tray::choose_colour(hwnd) {
                            crate::tray::enqueue(owner(), action);
                            SetForegroundWindow(hwnd);
                        }
                    } else {
                        crate::tray::enqueue(
                            owner(),
                            Action::Preset(["warm", "white", "red", "green", "blue"][i as usize]),
                        );
                    }
                }
            } else if x >= 290 && y >= 414 {
                crate::tray::enqueue(owner(), Action::Refresh);
            }
            0
        }
        WM_MOUSEMOVE => {
            if VIEW.lock().unwrap().drag.is_some() {
                let (x, _) = coordinates(hwnd, lparam);
                slider(hwnd, x);
            }
            0
        }
        WM_LBUTTONUP => {
            let percent = VIEW.lock().unwrap().drag.take();
            if let Some(percent) = percent {
                ReleaseCapture();
                crate::tray::enqueue(owner(), Action::Brightness(percent));
            }
            0
        }
        WM_CAPTURECHANGED => {
            VIEW.lock().unwrap().drag = None;
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_DPICHANGED => {
            let bounds = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOACTIVATE,
            );
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

pub fn preview(path: &std::path::Path) -> Result<(), String> {
    {
        let mut v = VIEW.lock().unwrap();
        v.label = "A19 - Color".into();
        v.message = "Connected".into();
        v.renaming = std::env::args().any(|a| a == "--rename-preview");
        v.snapshot = Some(Snapshot {
            power: Some(true),
            brightness: Some(50),
            temperature: Some(5000),
            online: Some(true),
            ..Snapshot::default()
        });
    }
    unsafe {
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = WIDTH;
        info.bmiHeader.biHeight = -HEIGHT;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() {
            return Err("Cannot render preview".into());
        }
        let old = SelectObject(dc, bitmap);
        content(dc, WIDTH, HEIGHT);
        GdiFlush();
        let bytes = std::slice::from_raw_parts(bits as *const u8, (WIDTH * HEIGHT * 4) as usize);
        let mut output = Vec::new();
        output.extend_from_slice(b"BM");
        output.extend_from_slice(&(54u32 + bytes.len() as u32).to_le_bytes());
        output.extend_from_slice(&[0u8; 4]);
        output.extend_from_slice(&54u32.to_le_bytes());
        output.extend_from_slice(&40u32.to_le_bytes());
        output.extend_from_slice(&WIDTH.to_le_bytes());
        output.extend_from_slice(&(-HEIGHT).to_le_bytes());
        output.extend_from_slice(&1u16.to_le_bytes());
        output.extend_from_slice(&32u16.to_le_bytes());
        output.extend_from_slice(&[0u8; 24]);
        output.extend_from_slice(bytes);
        std::fs::write(path, output).map_err(|_| "Cannot save preview")?;
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
    }
    Ok(())
}

#[cfg(test)]
mod edit_tests {
    use super::*;
    #[test]
    fn native_edit_ctrl_backspace_deletes_selection_or_word_and_supports_undo() {
        unsafe {
            let parent = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("").as_ptr(),
                0,
                0,
                0,
                360,
                460,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!parent.is_null());
            let edit = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                wide("Desk light").as_ptr(),
                WS_CHILD | ES_AUTOHSCROLL as u32,
                0,
                0,
                250,
                30,
                parent,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            assert!(!edit.is_null());
            SetWindowSubclass(edit, Some(edit_procedure), 1, 0);
            let mut old = [0u8; 256];
            assert_ne!(GetKeyboardState(old.as_mut_ptr()), 0);
            let mut keys = old;
            keys[VK_CONTROL as usize] = 0x80;
            assert_ne!(SetKeyboardState(keys.as_ptr()), 0);
            SendMessageW(edit, EM_SETSEL, 10, 10);
            SendMessageW(edit, WM_KEYDOWN, VK_BACK as usize, 0);
            SendMessageW(edit, WM_CHAR, 0x7f, 0);
            let mut buffer = [0u16; 128];
            let n = GetWindowTextW(edit, buffer.as_mut_ptr(), 128);
            let result = String::from_utf16_lossy(&buffer[..n as usize]);
            SendMessageW(edit, 0x00c7, 0, 0); // EM_UNDO
            let n = GetWindowTextW(edit, buffer.as_mut_ptr(), 128);
            let undone = String::from_utf16_lossy(&buffer[..n as usize]);
            SendMessageW(edit, EM_SETSEL, 0, 4);
            SendMessageW(edit, WM_KEYDOWN, VK_BACK as usize, 0);
            SendMessageW(edit, WM_CHAR, 0x7f, 0);
            let n = GetWindowTextW(edit, buffer.as_mut_ptr(), 128);
            let selected = String::from_utf16_lossy(&buffer[..n as usize]);
            SetKeyboardState(old.as_ptr());
            DestroyWindow(parent);
            assert_eq!(result, "Desk ");
            assert_eq!(undone, "Desk light");
            assert_eq!(selected, " light");
        }
    }
    #[test]
    fn word_deletion_handles_spaces_punctuation_and_unicode() {
        for (input, remaining) in [
            ("Desk light", "Desk "),
            ("Desk light   ", "Desk "),
            ("Desk-light", "Desk-"),
            ("café bulb", "café "),
            ("Lamp 💡", "Lamp "),
            ("", ""),
            ("   ", ""),
        ] {
            let units: Vec<u16> = input.encode_utf16().collect();
            let start = previous_word_start(&units, units.len());
            assert_eq!(String::from_utf16(&units[..start]).unwrap(), remaining);
        }
    }
}
