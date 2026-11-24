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

pub fn idle_ms() -> Option<u32> {
    let mut input = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    unsafe {
        (GetLastInputInfo(&mut input) != 0).then(|| GetTickCount().wrapping_sub(input.dwTime))
    }
}

pub fn protect(bytes: &[u8]) -> io::Result<Vec<u8>> {
    protect_data(bytes, true)
}

pub fn unprotect(bytes: &[u8]) -> io::Result<Vec<u8>> {
    protect_data(bytes, false)
}

fn protect_data(bytes: &[u8], encrypt: bool) -> io::Result<Vec<u8>> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(io::Error::other)?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // With no machine flag, Windows encrypts this token for the current Windows user.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let data = if output.cbData == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
        };
        LocalFree(output.pbData.cast());
        Ok(data)
    }
}

pub fn foreground(should_track: impl FnOnce(&AppId) -> bool) -> Option<FocusedApp> {
    let window = unsafe { GetForegroundWindow() };
    let app = window_app(window)?;
    if !should_track(&app) {
        return None;
    }
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
