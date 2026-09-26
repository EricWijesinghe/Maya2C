//! Design tokens: two themes, one type scale, one spacing scale, one motion
//! scale. Components use the custom properties [`css`] emits and nothing else,
//! so a palette change is one edit here and one re-run of the contrast test.

use std::fmt::Write as _;

use crate::contrast::{Rgb, Use};

/// The two themes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    /// Light, low-saturation, the default.
    Calm,
    /// The dark "tactical command center" signature look.
    Command,
}

impl Theme {
    /// Both themes, in the order the specimen page shows them.
    pub const ALL: [Self; 2] = [Self::Calm, Self::Command];

    /// Name used in `data-theme` and file names.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Calm => "calm",
            Self::Command => "command",
        }
    }
}

/// The color roles a component may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)] // the role names are the documentation
pub enum Role {
    Bg,
    Panel,
    Fg,
    Muted,
    Accent,
    OnAccent,
    Danger,
    Warning,
    Success,
    Border,
    Focus,
}

impl Role {
    /// Every role, in CSS emission order.
    pub const ALL: [Self; 11] = [
        Self::Bg,
        Self::Panel,
        Self::Fg,
        Self::Muted,
        Self::Accent,
        Self::OnAccent,
        Self::Danger,
        Self::Warning,
        Self::Success,
        Self::Border,
        Self::Focus,
    ];

    /// The custom property name, without the leading `--`.
    #[must_use]
    pub const fn var(self) -> &'static str {
        match self {
            Self::Bg => "bg",
            Self::Panel => "panel",
            Self::Fg => "fg",
            Self::Muted => "muted",
            Self::Accent => "accent",
            Self::OnAccent => "on-accent",
            Self::Danger => "danger",
            Self::Warning => "warning",
            Self::Success => "success",
            Self::Border => "border",
            Self::Focus => "focus",
        }
    }
}

/// A theme's color for a role.
#[must_use]
pub const fn color(theme: Theme, role: Role) -> Rgb {
    match theme {
        Theme::Calm => match role {
            Role::Bg | Role::OnAccent => Rgb(0xff, 0xff, 0xff),
            Role::Panel => Rgb(0xf4, 0xf5, 0xf7),
            Role::Fg => Rgb(0x16, 0x18, 0x1d),
            Role::Muted => Rgb(0x55, 0x5c, 0x69),
            Role::Accent => Rgb(0x0b, 0x6e, 0x4f),
            Role::Border => Rgb(0xd5, 0xd9, 0xe0),
            Role::Danger => Rgb(0xb3, 0x26, 0x1e),
            Role::Warning => Rgb(0x87, 0x4d, 0x00),
            Role::Success => Rgb(0x1a, 0x73, 0x37),
            Role::Focus => Rgb(0x1d, 0x4e, 0xd8),
        },
        Theme::Command => match role {
            Role::Bg => Rgb(0x08, 0x0c, 0x0a),
            Role::Panel => Rgb(0x10, 0x17, 0x13),
            Role::Fg => Rgb(0xd7, 0xf5, 0xe4),
            Role::Muted => Rgb(0x8f, 0xa8, 0x9a),
            // Success shares the accent: the command theme's signal green.
            Role::Accent | Role::Success => Rgb(0x39, 0xe5, 0x8c),
            Role::OnAccent => Rgb(0x04, 0x11, 0x0a),
            Role::Danger => Rgb(0xff, 0x6b, 0x6b),
            Role::Warning => Rgb(0xff, 0xc1, 0x4d),
            Role::Border => Rgb(0x23, 0x33, 0x2a),
            Role::Focus => Rgb(0x7f, 0xf5, 0xff),
        },
    }
}

/// Every foreground/background pair a component renders, and what it is
/// used for. The contrast test walks this list for both themes; a component
/// that introduces a new pair adds it here or fails review.
pub const REQUIRED_PAIRS: &[(Role, Role, Use)] = &[
    (Role::Fg, Role::Bg, Use::Text),
    (Role::Fg, Role::Panel, Use::Text),
    (Role::Muted, Role::Bg, Use::Text),
    (Role::Muted, Role::Panel, Use::Text),
    (Role::Accent, Role::Bg, Use::Text),
    (Role::Accent, Role::Panel, Use::Text),
    (Role::OnAccent, Role::Accent, Use::Text),
    (Role::Danger, Role::Bg, Use::Text),
    (Role::Danger, Role::Panel, Use::Text),
    (Role::Warning, Role::Panel, Use::Text),
    (Role::Success, Role::Panel, Use::Text),
    (Role::Focus, Role::Bg, Use::NonText),
    (Role::Focus, Role::Panel, Use::NonText),
];

/// Type scale in px: caption, body, lead, h3, h2, h1. A 1.25 ratio, rounded.
pub const TYPE_SCALE_PX: [u16; 6] = [12, 14, 17, 21, 26, 33];

/// Spacing scale in px, a 4 px grid.
pub const SPACE_PX: [u16; 8] = [0, 4, 8, 12, 16, 24, 32, 48];

/// Motion durations in ms: instant feedback, a panel, a page transition.
/// All collapse to 0 under `prefers-reduced-motion` (see [`css`]).
pub const MOTION_MS: [u16; 3] = [80, 160, 240];

/// Minimum touch target in CSS px (WCAG 2.2 SC 2.5.8 asks for 24; 44 is the
/// platform guideline both Apple and Android publish, and the one used here).
pub const TOUCH_TARGET_PX: u16 = 44;

/// The stylesheet every interface includes: both themes as custom properties
/// under `[data-theme]`, calm as the default, the scales, and reduced motion.
#[must_use]
pub fn css() -> String {
    let mut out = String::from("/* Generated by maya-design-system; do not edit. */\n");
    for theme in Theme::ALL {
        let selector = match theme {
            Theme::Calm => ":root, [data-theme=\"calm\"]",
            Theme::Command => "[data-theme=\"command\"]",
        };
        let scheme = if theme == Theme::Calm {
            "light"
        } else {
            "dark"
        };
        let _ = write!(out, "{selector} {{\n  color-scheme: {scheme};\n");
        for role in Role::ALL {
            let _ = writeln!(out, "  --{}: {};", role.var(), color(theme, role).to_hex());
        }
        out.push_str("}\n");
    }
    out.push_str(":root {\n");
    for (i, px) in TYPE_SCALE_PX.iter().enumerate() {
        let _ = writeln!(out, "  --text-{i}: {px}px;");
    }
    for (i, px) in SPACE_PX.iter().enumerate() {
        let _ = writeln!(out, "  --space-{i}: {px}px;");
    }
    for (i, ms) in MOTION_MS.iter().enumerate() {
        let _ = writeln!(out, "  --motion-{i}: {ms}ms;");
    }
    let _ = write!(out, "  --touch-target: {TOUCH_TARGET_PX}px;\n}}\n");
    out.push_str("@media (prefers-reduced-motion: reduce) {\n  :root {");
    for i in 0..MOTION_MS.len() {
        let _ = write!(out, " --motion-{i}: 0ms;");
    }
    out.push_str(" }\n}\n");
    out
}
