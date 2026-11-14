//! `maya2c-chat://` links from other apps and the OS.
//!
//! The only route is `open`, which brings the window forward. A link is
//! untrusted input, so anything else is refused: no query, no path, no
//! parameters, and no way for a link to name a contact, a message or a file.

use tauri::{AppHandle, Manager, Runtime};

/// Longest link accepted. `open` is 13 characters; the cap is generous.
const MAX_URL_LEN: usize = 512;

const PREFIX: &str = "maya2c-chat://";

/// Why a link was refused. Carries no input text, so it is safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteError {
    TooLong,
    NotAscii,
    Scheme,
    Route,
}

/// Accepts exactly `maya2c-chat://open`.
///
/// # Errors
///
/// [`RouteError`] for an over-long or non-ASCII link, a foreign or mis-cased
/// scheme, or any route other than `open`. A query, fragment or trailing
/// slash makes the path something other than `open`, so it is refused too.
pub fn parse(url: &str) -> Result<(), RouteError> {
    if url.len() > MAX_URL_LEN {
        return Err(RouteError::TooLong);
    }
    if !url.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(RouteError::NotAscii);
    }
    let path = url.strip_prefix(PREFIX).ok_or(RouteError::Scheme)?;
    match path {
        "open" => Ok(()),
        _ => Err(RouteError::Route),
    }
}

/// Applies links the OS or a second launch delivered. Refused links are
/// dropped with a static log line that names only the reason.
pub fn handle<R: Runtime, U: std::fmt::Display>(app: &AppHandle<R>, urls: &[U]) {
    for url in urls {
        if let Err(reason) = parse(&url.to_string()) {
            eprintln!("deep link refused: {reason:?}");
        }
    }
    focus(app);
}

/// Brings the main window forward. Best effort: a failed focus is harmless.
pub fn focus<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_is_accepted() {
        assert_eq!(parse("maya2c-chat://open"), Ok(()));
    }

    #[test]
    fn oversize_link_is_refused() {
        let url = format!("{PREFIX}open{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(parse(&url), Err(RouteError::TooLong));
    }

    #[test]
    fn foreign_or_mis_cased_scheme_is_refused() {
        assert_eq!(parse("maya2c-wallet://open"), Err(RouteError::Scheme));
        assert_eq!(parse("https://open"), Err(RouteError::Scheme));
        assert_eq!(parse("MAYA2C-CHAT://open"), Err(RouteError::Scheme));
        assert_eq!(parse("maya2c-chat:open"), Err(RouteError::Scheme));
    }

    #[test]
    fn every_other_route_is_refused() {
        let cases = [
            "maya2c-chat://",
            "maya2c-chat://OPEN",
            "maya2c-chat://open/",
            "maya2c-chat://open?contact=x",
            "maya2c-chat://open#x",
            "maya2c-chat://send?to=x",
            "maya2c-chat://open/../../x",
            "maya2c-chat://open%0a",
        ];
        for url in cases {
            assert!(parse(url).is_err(), "accepted: {url:?}");
        }
    }

    #[test]
    fn control_and_non_ascii_are_refused() {
        assert_eq!(parse("maya2c-chat://open\u{0}"), Err(RouteError::NotAscii));
        assert_eq!(parse("maya2c-chat://open "), Err(RouteError::NotAscii));
        assert_eq!(parse("maya2c-chat://\u{e9}pen"), Err(RouteError::NotAscii));
    }
}
