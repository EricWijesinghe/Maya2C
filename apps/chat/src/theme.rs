//! Chat themes and wallpapers (ADR-140 section 2, build PR 4).
//!
//! Local presentation only: a theme is a named set of the desktop app's CSS
//! colour variables, a wallpaper is a gradient or an image from this
//! computer, and a chat may override either. Nothing here is sent; sharing a
//! theme as a frame is left for later (ADR-140 section 2 "Share this theme").
//!
//! The tokens are the chat app's existing variables (`apps/chat-gui/ui/style.css`).
//! Forged Light 2's tokens (ADR-136) exist only as a design document so far;
//! when they land in the app, [`ALLOWED_TOKENS`] is the one list to change.
//!
//! Every value is checked when it is set and again when a saved profile is
//! loaded ([`Looks::cleaned`]): only names in [`ALLOWED_TOKENS`] with a hex
//! colour survive, so no stored value can smuggle `url(...)` or a second CSS
//! declaration into the window, and no image id can name a path.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::frame::{Format, FrameError};

/// The CSS variables a theme may set.
pub const ALLOWED_TOKENS: [&str; 8] = [
    "--bg", "--panel", "--line", "--text", "--dim", "--cyan", "--violet", "--rose",
];
/// The theme the app starts with: the current Forged Light colours.
pub const DEFAULT_THEME: &str = "forged";
/// The largest gradient angle, degrees.
pub const MAX_ANGLE: u16 = 359;

/// A built-in theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Theme {
    pub id: &'static str,
    pub label: &'static str,
    pub tokens: &'static [(&'static str, &'static str)],
}

/// The themes the app offers. All are dark, as the app's `color-scheme` is;
/// each keeps text and accent contrast near the default's.
pub const BUILT_IN: &[Theme] = &[
    Theme {
        id: DEFAULT_THEME,
        label: "Forged Light",
        tokens: &[
            ("--bg", "#050b0f"),
            ("--panel", "#0b161b"),
            ("--text", "#edf3f2"),
            ("--dim", "#93a4aa"),
            ("--cyan", "#1fb5c2"),
            ("--violet", "#d9b97a"),
            ("--rose", "#c9a86a"),
        ],
    },
    Theme {
        id: "aurora",
        label: "Aurora",
        tokens: &[
            ("--bg", "#060a14"),
            ("--panel", "#0d1424"),
            ("--text", "#eef2fb"),
            ("--dim", "#98a3bd"),
            ("--cyan", "#3ddc97"),
            ("--violet", "#9b8cff"),
            ("--rose", "#7fd1ff"),
        ],
    },
    Theme {
        id: "ember",
        label: "Ember",
        tokens: &[
            ("--bg", "#0f0807"),
            ("--panel", "#1a0f0c"),
            ("--text", "#f7efe9"),
            ("--dim", "#b5a49b"),
            ("--cyan", "#ff8a4c"),
            ("--violet", "#ffc46b"),
            ("--rose", "#e8707a"),
        ],
    },
    Theme {
        id: "tide",
        label: "Tide",
        tokens: &[
            ("--bg", "#03101a"),
            ("--panel", "#081a28"),
            ("--text", "#e8f4fb"),
            ("--dim", "#8fa7b8"),
            ("--cyan", "#4cb8ff"),
            ("--violet", "#7ee0d2"),
            ("--rose", "#a7c7ff"),
        ],
    },
];

/// Whether `id` is a built-in theme, or empty (the default).
#[must_use]
pub fn is_theme(id: &str) -> bool {
    id.is_empty() || BUILT_IN.iter().any(|t| t.id == id)
}

/// `#rrggbb` or `#rrggbbaa`, lower-cased; `None` for anything else.
#[must_use]
pub fn colour(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#')?;
    let ok = matches!(hex.len(), 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| value.to_ascii_lowercase())
}

/// The tokens of `given` that are in [`ALLOWED_TOKENS`] with a hex colour.
#[must_use]
pub fn sanitize(given: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    given
        .iter()
        .filter(|(k, _)| ALLOWED_TOKENS.contains(&k.as_str()))
        .filter_map(|(k, v)| colour(v).map(|v| (k.clone(), v)))
        .collect()
}

/// A chat's background.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Wallpaper {
    /// The theme's own background.
    #[default]
    None,
    /// A linear gradient between two colours.
    Gradient {
        from: String,
        to: String,
        angle: u16,
    },
    /// An image from this computer, cached encrypted under its 64-hex id like
    /// a sticker ([`check_wallpaper`]).
    Image { id: String },
}

impl Wallpaper {
    /// This wallpaper with bad values dropped: a gradient with a colour that
    /// is not hex, or an image id that is not 64 lowercase hex, becomes
    /// [`Wallpaper::None`]; an angle past [`MAX_ANGLE`] is clamped.
    #[must_use]
    pub fn cleaned(&self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Gradient { from, to, angle } => match (colour(from), colour(to)) {
                (Some(from), Some(to)) => Self::Gradient {
                    from,
                    to,
                    angle: (*angle).min(MAX_ANGLE),
                },
                _ => Self::None,
            },
            Self::Image { id } if crate::frame::check_id(id).is_ok() => {
                Self::Image { id: id.clone() }
            }
            Self::Image { .. } => Self::None,
        }
    }
}

/// How a chat looks: a theme, token overrides on top of it, and a wallpaper.
/// An empty field means "as the global look".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Look {
    pub theme: String,
    pub tokens: BTreeMap<String, String>,
    pub wallpaper: Wallpaper,
}

impl Look {
    /// This look with an unknown theme, bad tokens and a bad wallpaper dropped.
    #[must_use]
    pub fn cleaned(&self) -> Self {
        Self {
            theme: if is_theme(&self.theme) {
                self.theme.clone()
            } else {
                String::new()
            },
            tokens: sanitize(&self.tokens),
            wallpaper: self.wallpaper.cleaned(),
        }
    }
}

/// The global look and each chat's override, by peer address.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Looks {
    pub global: Look,
    pub per_chat: BTreeMap<String, Look>,
}

impl Looks {
    /// The look of the chat with `peer`: its override, field by field, over
    /// the global look.
    #[must_use]
    pub fn look_for(&self, peer: &str) -> Look {
        let Some(own) = self.per_chat.get(peer) else {
            return self.global.clone();
        };
        Look {
            theme: (if own.theme.is_empty() {
                &self.global.theme
            } else {
                &own.theme
            })
            .clone(),
            tokens: (if own.tokens.is_empty() {
                &self.global.tokens
            } else {
                &own.tokens
            })
            .clone(),
            wallpaper: (if own.wallpaper == Wallpaper::None {
                &self.global.wallpaper
            } else {
                &own.wallpaper
            })
            .clone(),
        }
    }

    /// Every look [`Look::cleaned`], for a profile read from disk.
    #[must_use]
    pub fn cleaned(&self) -> Self {
        Self {
            global: self.global.cleaned(),
            per_chat: self
                .per_chat
                .iter()
                .map(|(peer, look)| (peer.clone(), look.cleaned()))
                .collect(),
        }
    }
}

/// Checks a wallpaper image: a still PNG or WebP within the sticker caps of
/// [`crate::media`] (the 120 KB ceiling, 512 px a side), so the window
/// decodes nothing larger than a sticker. A moving or vector file is
/// refused: a background never animates.
///
/// # Errors
///
/// What [`crate::frame::check_file`] refuses, or [`FrameError::BadCard`] for
/// another format.
pub fn check_wallpaper(bytes: &[u8]) -> Result<(), FrameError> {
    // An APNG or animated WebP passes here; the window draws the image once
    // onto a canvas and uses that still copy, so it never moves.
    let (_, format) = crate::frame::check_file(bytes)?;
    matches!(format, Format::Png | Format::Webp)
        .then_some(())
        .ok_or(FrameError::BadCard)
}
