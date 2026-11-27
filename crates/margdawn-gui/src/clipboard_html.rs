//! Putting rich text on the system clipboard.
//!
//! egui's own copy path only carries plain text (`clipboard.set_text`), so a
//! paste into LibreOffice Writer arrives unformatted. Word processors keep the
//! formatting only when the clipboard also offers `text/html`.
//!
//! `arboard` is already in the dependency tree (egui-winit uses it), and its
//! `set_html` publishes HTML *and* a plain-text fallback in one ownership, so
//! a word processor gets the markup while a terminal or a plain editor still
//! gets readable text. Driving `wl-copy` instead was tried and does not work:
//! it carries exactly one MIME type, and a second invocation replaces the
//! first rather than adding a flavour.

use std::cell::RefCell;

/// A clipboard that can carry HTML with a plain-text fallback.
///
/// `arboard::Clipboard` is not `Send`/`Sync` on every platform and the app
/// talks to it from the UI thread only, so it lives in a `RefCell` inside one
/// struct rather than being a global.
#[derive(Default)]
pub struct RichClipboard {
    inner: RefCell<Option<arboard::Clipboard>>,
}

impl RichClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, f: impl FnOnce(&mut arboard::Clipboard) -> T) -> Option<T> {
        let mut slot = self.inner.borrow_mut();
        if slot.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => *slot = Some(c),
                Err(e) => {
                    log_once(e);
                    return None;
                }
            }
        }
        slot.as_mut().map(f)
    }

    /// Store `html` with `plain` as the fallback flavour.
    ///
    /// Returns whether the clipboard took it. A `false` here means the caller
    /// should fall back to egui's plain-text copy so the user is never left
    /// with nothing on the clipboard.
    pub fn set_html(&self, html: &str, plain: &str) -> bool {
        // Wrap the fragment so the clipboard carries a complete document; some
        // consumers (LibreOffice among them) want a body, not a bare fragment.
        let wrapped = format!("<html><body>{html}</body></html>");
        self.with(|cb| cb.set_html(wrapped, Some(plain.to_string())).is_ok())
            .unwrap_or(false)
    }
}

/// Report the first clipboard failure once, so a broken compositor does not
/// spam a line per copy.
fn log_once(err: arboard::Error) {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| eprintln!("rich clipboard unavailable, copying plain text only: {err}"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_clipboard_can_be_constructed_without_panicking() {
        // Construction must never panic, even headless or without a seat.
        let cb = RichClipboard::new();
        assert!(cb.inner.borrow().is_none(), "opens lazily");
    }

    #[test]
    fn set_html_reports_a_bool_and_never_panics() {
        // Without a compositor this returns false; with one, true. Either way
        // it must not panic: the caller relies on the boolean to fall back.
        let cb = RichClipboard::new();
        let _ = cb.set_html("<b>x</b>", "x");
    }
}
