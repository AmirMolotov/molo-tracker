use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation2, IUIAutomationCondition, IUIAutomationValuePattern,
    TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ClassNamePropertyId,
    UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_IsOffscreenPropertyId,
    UIA_IsValuePatternAvailablePropertyId, UIA_ValuePatternId,
};
use windows::core::BSTR;
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::platform::window_caption;

pub struct BrowserTarget {
    pub window: usize,
    pub caption: String,
    pub chrome: bool,
}

pub struct WebsiteProbe {
    requests: SyncSender<(i64, BrowserTarget)>,
    results: Receiver<(i64, String)>,
}

impl WebsiteProbe {
    pub fn start() -> std::io::Result<Self> {
        let (requests, work) = mpsc::sync_channel::<(i64, BrowserTarget)>(1);
        let (output, results) = mpsc::channel();
        thread::Builder::new()
            .name("website-probe".into())
            .spawn(move || {
                let Ok(reader) = Reader::open() else { return };
                while let Ok((stamp, target)) = work.recv() {
                    if target.still_active()
                        && let Some(domain) = reader.domain(&target)
                        && target.still_active()
                    {
                        let _ = output.send((stamp, domain));
                    }
                }
            })?;
        Ok(Self { requests, results })
    }

    pub fn sample(&self, stamp: i64, target: BrowserTarget) {
        let _ = self.requests.try_send((stamp, target));
    }

    pub fn result(&self, stamp: Option<i64>) -> Option<String> {
        self.results
            .try_iter()
            .filter_map(|(time, domain)| (Some(time) == stamp).then_some(domain))
            .last()
    }
}

impl BrowserTarget {
    fn still_active(&self) -> bool {
        let window = unsafe { GetForegroundWindow() };
        window as usize == self.window && window_caption(window) == self.caption
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Reader {
    automation: IUIAutomation2,
    document: IUIAutomationCondition,
    chrome_address: IUIAutomationCondition,
    firefox_address: IUIAutomationCondition,
    _apartment: Apartment,
}

impl Reader {
    fn open() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let apartment = Apartment;
            let automation: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            automation.SetConnectionTimeout(700)?;
            automation.SetTransactionTimeout(700)?;
            let document = automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &UIA_DocumentControlTypeId.0.into(),
            )?;
            let visible =
                automation.CreatePropertyCondition(UIA_IsOffscreenPropertyId, &false.into())?;
            let has_value = automation
                .CreatePropertyCondition(UIA_IsValuePatternAvailablePropertyId, &true.into())?;
            let document = automation.CreateAndCondition(&document, &has_value)?;
            let document = automation.CreateAndCondition(&document, &visible)?;
            let chrome_address = automation.CreatePropertyCondition(
                UIA_ClassNamePropertyId,
                &BSTR::from("OmniboxViewViews").into(),
            )?;
            let firefox_address = automation.CreatePropertyCondition(
                UIA_AutomationIdPropertyId,
                &BSTR::from("urlbar-input").into(),
            )?;
            Ok(Self {
                automation,
                document,
                chrome_address,
                firefox_address,
                _apartment: apartment,
            })
        }
    }

    fn domain(&self, target: &BrowserTarget) -> Option<String> {
        unsafe {
            let root = self
                .automation
                .ElementFromHandle(HWND(target.window as _))
                .ok()?;
            if let Ok(document) = root.FindFirst(TreeScope_Descendants, &self.document)
                && let Ok(value) =
                    document.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                && let Ok(address) = value.CurrentValue()
                && !address.is_empty()
            {
                return website_host(&address.to_string(), false);
            }
            let selector = if target.chrome {
                &self.chrome_address
            } else {
                &self.firefox_address
            };
            let address_bar = root.FindFirst(TreeScope_Descendants, selector).ok()?;
            if address_bar.CurrentHasKeyboardFocus().ok()?.as_bool()
                || address_bar.CurrentIsOffscreen().ok()?.as_bool()
            {
                return None;
            }
            let value: IUIAutomationValuePattern =
                address_bar.GetCurrentPatternAs(UIA_ValuePatternId).ok()?;
            website_host(&value.CurrentValue().ok()?.to_string(), true)
        }
    }
}

pub(crate) fn parent_domain(host: &str) -> Option<&str> {
    psl::domain_str(host).filter(|domain| *domain != host)
}

fn website_host(address: &str, allow_hidden_scheme: bool) -> Option<String> {
    let address = address.trim();
    let parsed = match url::Url::parse(address) {
        Ok(url) if !url.cannot_be_a_base() => url,
        Ok(_) | Err(_) if allow_hidden_scheme && !address.contains("://") => {
            url::Url::parse(&format!("https://{address}")).ok()?
        }
        _ => return None,
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.domain()?.trim_end_matches('.');
    let suffix = psl::suffix(host.as_bytes())?;
    if !suffix.is_known() {
        return None;
    }
    psl::domain_str(host).map(|_| host.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{parent_domain, website_host};

    #[test]
    fn keeps_hostname_without_path_and_skips_internal_pages() {
        for (url, domain) in [
            (
                "https://NEWS.bbc.co.uk.:443/article?q=foo",
                Some("news.bbc.co.uk"),
            ),
            ("https://www.youtube.com/watch", Some("www.youtube.com")),
            ("https://a.alice.github.io/blog", Some("a.alice.github.io")),
            ("https://www.مثال.com", Some("www.xn--mgbh0fb.com")),
            ("about:config", None),
            ("chrome://newtab", None),
            ("file:///C:/private.txt", None),
            ("http://127.0.0.1:9000/", None),
        ] {
            assert_eq!(website_host(url, false).as_deref(), domain);
        }
        assert_eq!(
            website_host("www.example.com/docs", true).as_deref(),
            Some("www.example.com")
        );
        assert_eq!(website_host("draft search terms", true), None);
        assert_eq!(parent_domain("news.bbc.co.uk"), Some("bbc.co.uk"));
        assert_eq!(parent_domain("bbc.co.uk"), None);
    }
}
