//! System / Light / Dark.
//!
//! The colours are all in tokens.css: `data-theme` on the root element picks
//! a theme explicitly, and without it the OS preference decides (dark when
//! there is none, ADR-136). The choice is a per-device convenience kept in
//! `localStorage`; losing it only means the wallet follows the OS again.

/// The three choices in Settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Follow the operating system.
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl Theme {
    /// Every choice, in the order Settings shows them.
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// The control's label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// The `data-theme` value, `None` for System.
    fn attribute(self) -> Option<&'static str> {
        match self {
            Self::System => None,
            Self::Light => Some("light"),
            Self::Dark => Some("dark"),
        }
    }

    fn from_stored(text: &str) -> Self {
        match text {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}

const KEY: &str = "maya2c-wallet-theme";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// The remembered choice; System when there is none or storage is blocked.
#[must_use]
pub fn load() -> Theme {
    storage()
        .and_then(|s| s.get_item(KEY).ok().flatten())
        .map_or(Theme::System, |t| Theme::from_stored(&t))
}

/// Remembers `theme`. A failure (storage blocked) is not worth telling the
/// user about: the theme still applies for this session.
pub fn save(theme: Theme) {
    if let Some(s) = storage() {
        let _ = match theme.attribute() {
            Some(value) => s.set_item(KEY, value),
            None => s.remove_item(KEY),
        };
    }
}

/// Applies `theme` to the document, and points the browser chrome colour
/// (`theme-color`) at the theme's `--bg`, so the title bar and the page
/// never disagree.
pub fn apply(theme: Theme) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Some(root) = document.document_element() else {
        return;
    };
    let _ = match theme.attribute() {
        Some(value) => root.set_attribute("data-theme", value),
        None => root.remove_attribute("data-theme"),
    };
    let background = window
        .get_computed_style(&root)
        .ok()
        .flatten()
        .and_then(|style| style.get_property_value("--bg").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    for selector in [
        "meta[name=theme-color]:not([media^='not'])",
        "meta[name=theme-color][media^='not']",
    ] {
        let Ok(Some(meta)) = document.query_selector(selector) else {
            continue;
        };
        // System puts back the colour index.html gave each media query;
        // an explicit theme wins over both queries.
        let content = match (theme, &background) {
            (Theme::System, _) | (_, None) => meta.get_attribute("data-system"),
            (_, Some(bg)) => Some(bg.clone()),
        };
        if let Some(content) = content {
            let _ = meta.set_attribute("content", &content);
        }
    }
}
