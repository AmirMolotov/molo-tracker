use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr::null;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongW,
    GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, WS_EX_TOOLWINDOW,
};

#[derive(Clone, Debug)]
pub struct AppId {
    pub key: String,
    pub name: String,
}

impl AppId {
    pub fn browser_brand(&self) -> Option<&'static str> {
        if self.name.eq_ignore_ascii_case("chrome") {
            Some("Google Chrome")
        } else if self.name.eq_ignore_ascii_case("firefox") {
            Some("Mozilla Firefox")
        } else {
            None
        }
    }
}

pub struct FocusedApp {
    pub app: AppId,
    pub page_title: Option<String>,
    pub browser: Option<crate::websites::BrowserTarget>,
}

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub struct Instance {
    _handle: Handle,
}

impl Instance {
    pub fn acquire() -> io::Result<Self> {
        let name: Vec<_> = "Local\\MoloTracker.FocusRecorder"
            .encode_utf16()
            .chain([0])
            .collect();
        unsafe {
            let handle = CreateMutexW(null(), 0, name.as_ptr());
            let status = GetLastError();
            if handle.is_null() {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            let handle = Handle(handle);
            if status == ERROR_ALREADY_EXISTS {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Molo is already running. Open it from the Windows notification area.",
                ));
            }
            Ok(Self { _handle: handle })
        }
    }
}

fn window_app(window: HWND) -> Option<AppId> {
    if window.is_null() {
        return None;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let process = Handle(process);
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]));
    let name = path.file_stem()?.to_string_lossy().into_owned();
    Some(AppId {
        key: path.to_string_lossy().to_lowercase(),
        name,
    })
}

pub fn foreground() -> Option<FocusedApp> {
    let mut input = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    let idle = unsafe {
        GetLastInputInfo(&mut input) != 0 && GetTickCount().wrapping_sub(input.dwTime) >= 300_000
    };
    if idle {
        return None;
    }
    let window = unsafe { GetForegroundWindow() };
    let app = window_app(window)?;
    let (page_title, browser) = app.browser_brand().map_or((None, None), |brand| {
        let caption = window_caption(window);
        (
            browser_title(&caption, brand),
            Some(crate::websites::BrowserTarget {
                window: window as usize,
                caption,
                chrome: app.name.eq_ignore_ascii_case("chrome"),
            }),
        )
    });
    Some(FocusedApp {
        app,
        page_title,
        browser,
    })
}

pub fn window_caption(window: HWND) -> String {
    let mut buffer = [0u16; 2048];
    let length = unsafe { GetWindowTextW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..length as usize])
}

fn browser_title(caption: &str, brand: &str) -> Option<String> {
    let title = caption
        .trim()
        .strip_suffix(brand)
        .and_then(|prefix| {
            [" - ", " — ", " – "]
                .into_iter()
                .find_map(|separator| prefix.strip_suffix(separator))
        })
        .unwrap_or(caption)
        .trim();
    (!title.is_empty() && title != brand).then(|| title.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_titles_remove_the_caption_suffix() {
        assert_eq!(
            browser_title("MDN Web Docs — Mozilla Firefox", "Mozilla Firefox").as_deref(),
            Some("MDN Web Docs")
        );
        assert_eq!(
            browser_title("Rust documentation - Google Chrome", "Google Chrome").as_deref(),
            Some("Rust documentation")
        );
        assert_eq!(browser_title("Mozilla Firefox", "Mozilla Firefox"), None);
    }
}

pub fn open_apps() -> Vec<AppId> {
    unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> i32 {
        let mut cloaked: u32 = 0;
        let visible = unsafe {
            IsWindowVisible(window) != 0
                && GetWindow(window, GW_OWNER).is_null()
                && GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW == 0
        };
        if visible {
            unsafe {
                DwmGetWindowAttribute(
                    window,
                    DWMWA_CLOAKED as u32,
                    (&mut cloaked as *mut u32).cast::<c_void>(),
                    size_of::<u32>() as u32,
                );
            }
            if cloaked == 0
                && let Some(app) = window_app(window)
            {
                unsafe { &mut *(data as *mut Vec<AppId>) }.push(app);
            }
        }
        1
    }
    let mut apps: Vec<AppId> = Vec::new();
    unsafe { EnumWindows(Some(collect), (&mut apps as *mut Vec<AppId>) as LPARAM) };
    apps.sort_unstable_by(|a, b| a.key.cmp(&b.key));
    apps.dedup_by(|a, b| a.key == b.key);
    apps
}
