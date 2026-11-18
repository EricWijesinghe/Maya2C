//! Pieces more than one screen uses: a secret field, the testnet tag and the
//! calm paused card. Each is the wallet's copy of a Forged Light 2 kit
//! component (docs/site/src/styles/components.css), in Leptos.

use leptos::prelude::*;

use crate::AppState;

/// How strong a passphrase looks, 0 (empty) to 4, and the word for it.
///
/// An estimate from length and character mix only: it cannot see that a
/// phrase is a song lyric, so it never refuses one, it only nudges.
#[must_use]
pub fn strength(passphrase: &str) -> (u8, &'static str) {
    let length = passphrase.chars().count();
    if length == 0 {
        return (0, "");
    }
    if length < 8 {
        return (1, "Too short");
    }
    let mut pool = 0u32;
    if passphrase.chars().any(|c| c.is_ascii_lowercase()) {
        pool += 26;
    }
    if passphrase.chars().any(|c| c.is_ascii_uppercase()) {
        pool += 26;
    }
    if passphrase.chars().any(|c| c.is_ascii_digit()) {
        pool += 10;
    }
    if passphrase.chars().any(|c| !c.is_ascii_alphanumeric()) {
        pool += 33;
    }
    // Bits ≈ length × log2(pool), in integers: log2 via leading zeros is
    // close enough for four buckets.
    let bits_per_char = 32 - pool.max(2).leading_zeros() - 1;
    let bits = u32::try_from(length)
        .unwrap_or(u32::MAX)
        .saturating_mul(bits_per_char);
    match bits {
        0..36 => (1, "Weak: add more words"),
        36..60 => (2, "Fair: longer is stronger"),
        60..90 => (3, "Strong"),
        _ => (4, "Very strong"),
    }
}

/// Line icons in the kit's style (24-unit box, `currentColor` stroke, round
/// joins), drawn inline so they never depend on a fallback font's glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// An arrow leaving, up and right.
    Send,
    /// An arrow arriving, down and left, onto a line.
    Receive,
    /// A viewfinder's four corners and its scan line.
    Scan,
    /// An eye.
    Eye,
    /// Two overlapping sheets.
    Copy,
    /// A padlock.
    Lock,
}

impl Glyph {
    fn path(self) -> &'static str {
        match self {
            Self::Send => "M7 17 17 7M9 7h8v8",
            Self::Receive => "M17 5 7 15M15 15H7V7M4 20h16",
            Self::Scan => {
                "M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3\
                 M8 20H5a1 1 0 0 1-1-1v-3M4 12h16"
            }
            Self::Eye => {
                "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12Z\
                 M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6Z"
            }
            Self::Copy => {
                "M9 9h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V10a1 1 0 0 1 1-1Z\
                 M5 15H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v1"
            }
            Self::Lock => {
                "M6 11h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-8a1 1 0 0 1 1-1Z\
                 M8 11V8a4 4 0 0 1 8 0v3"
            }
        }
    }
}

/// A decorative icon; the control around it carries the name.
#[component]
pub fn Icon(glyph: Glyph) -> impl IntoView {
    view! {
        <svg class="glyph" viewBox="0 0 24 24" width="20" height="20" aria-hidden="true" focusable="false">
            <path
                d=glyph.path()
                fill="none"
                stroke="currentColor"
                stroke-width="2.2"
                stroke-linecap="round"
                stroke-linejoin="round"
            />
        </svg>
    }
}

/// The Show toggle of a secret field. Its name is "Show " and the field's
/// label ("Show passphrase", "Show confirm passphrase", "Show recovery
/// phrase"), so two toggles on one screen are told apart, and it never
/// changes while pressed: `aria-pressed` carries the state (the toggle-button
/// pattern), so a screen reader hears "Show passphrase, toggle button,
/// pressed" rather than a name that flips between two verbs. The eye is
/// struck through while shown.
#[component]
pub fn RevealToggle(
    /// What the button reveals, lower case ("passphrase").
    what: String,
    /// Whether the secret is shown.
    shown: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <button
            type="button"
            class="reveal"
            class:on=move || shown.get()
            aria-label=format!("Show {what}")
            aria-pressed=move || if shown.get() { "true" } else { "false" }
            on:click=move |_| shown.update(|s| *s = !*s)
        >
            <Icon glyph=Glyph::Eye/>
            <span class="slash" aria-hidden="true"></span>
        </button>
    }
}

/// A passphrase input with its label, a Show toggle and, optionally, a
/// strength hint. `autocomplete` is `new-password` where a passphrase is
/// being chosen and `current-password` where one is typed back.
#[component]
pub fn SecretField(
    /// The input's id, which the label points at.
    id: &'static str,
    /// The visible label, which is also the input's accessible name.
    label: &'static str,
    /// The value.
    value: RwSignal<String>,
    /// The `autocomplete` token.
    autocomplete: &'static str,
    /// Show the strength hint under the field.
    #[prop(optional)]
    show_strength: bool,
    /// Marks the field invalid (a confirmation that does not match).
    #[prop(optional, into)]
    invalid: Option<Signal<bool>>,
    /// Runs on Enter.
    #[prop(optional, into)]
    on_enter: Option<Callback<()>>,
) -> impl IntoView {
    let shown = RwSignal::new(false);
    let hint_id = format!("{id}-hint");
    let described = show_strength.then(|| hint_id.clone());
    // A memo, so the live region changes (and is announced) only when the
    // level does, not on every keystroke.
    let level = Memo::new(move |_| strength(&value.get()));
    view! {
        <div class="field">
            <label for=id>{label}</label>
            <div class="secret">
                <input
                    id=id
                    type=move || if shown.get() { "text" } else { "password" }
                    autocomplete=autocomplete
                    spellcheck="false"
                    autocapitalize="off"
                    aria-describedby=described
                    aria-invalid=move || {
                        invalid.is_some_and(|i| i.get()).then_some("true")
                    }
                    prop:value=move || value.get()
                    on:input=move |ev| value.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter"
                            && let Some(enter) = on_enter
                        {
                            enter.run(());
                        }
                    }
                />
                <RevealToggle what=label.to_lowercase() shown=shown/>
            </div>
            <Show when=move || show_strength>
                <p
                    id=hint_id.clone()
                    class=move || format!("strength s{}", level.get().0)
                    aria-live="polite"
                >
                    <span class="bars" aria-hidden="true">
                        <span></span><span></span><span></span><span></span>
                    </span>
                    <span>
                        {move || {
                            let (_, word) = level.get();
                            if word.is_empty() { String::new() } else { format!("Strength: {word}") }
                        }}
                    </span>
                </p>
            </Show>
        </div>
    }
}

/// Half the lattice drawing's square box, in SVG units.
const LATTICE_HALF: i32 = 120;
/// The two basis vectors: skewed, so the grid reads as a lattice and not as
/// graph paper.
const LATTICE_B1: (i32, i32) = (27, 7);
const LATTICE_B2: (i32, i32) = (10, 25);
/// Points further than this from the centre are left out, giving a disc.
const LATTICE_RADIUS: i32 = 112;

/// The lattice's points within the disc, as offsets from the centre.
fn lattice_points() -> Vec<(i32, i32)> {
    let mut points = Vec::new();
    for i in -6..=6 {
        for j in -6..=6 {
            let x = i * LATTICE_B1.0 + j * LATTICE_B2.0;
            let y = i * LATTICE_B1.1 + j * LATTICE_B2.1;
            if x * x + y * y <= LATTICE_RADIUS * LATTICE_RADIUS {
                points.push((x, y));
            }
        }
    }
    points
}

/// The welcome screen's backdrop: lattice points, joined along both basis
/// vectors. Decorative (`aria-hidden` on its stage); CSS turns it slowly
/// and lets a few points glow, and holds it still under reduced motion.
#[component]
pub fn Lattice() -> impl IntoView {
    let points = lattice_points();
    let inside = |p: (i32, i32)| points.contains(&p);
    let mut edges = String::new();
    for &(x, y) in &points {
        for (dx, dy) in [LATTICE_B1, LATTICE_B2] {
            if inside((x + dx, y + dy)) {
                edges.push_str(&format!("M{x} {y}l{dx} {dy}"));
            }
        }
    }
    let dots = points
        .iter()
        .map(|&(x, y)| view! { <circle cx=x cy=y r="2"/> })
        .collect::<Vec<_>>();
    let view_box = format!(
        "{} {} {} {}",
        -LATTICE_HALF,
        -LATTICE_HALF,
        2 * LATTICE_HALF,
        2 * LATTICE_HALF
    );
    view! {
        <svg class="lattice" viewBox=view_box focusable="false">
            <path class="edges" d=edges/>
            <g class="dots">{dots}</g>
        </svg>
    }
}

/// "Testnet, coins have no value", in amber, wherever the network is a test
/// network or has not answered yet (the safe direction to be wrong in).
#[component]
pub fn TestnetTag() -> impl IntoView {
    let state = expect_context::<AppState>();
    view! {
        <Show when=move || state.network.get().is_none_or(|n| n.is_test_network())>
            <span class="testnet-tag">"Testnet, coins have no value"</span>
        </Show>
    }
}

/// The calm card for a node that does not answer: what happened in one
/// line, what still holds in another, and the raw error under Details.
/// Never a red banner, which reads as "you did something wrong".
#[component]
pub fn PausedCard(
    /// What happened.
    title: &'static str,
    /// What still holds, and what happens next.
    body: &'static str,
    /// The raw error; the card shows only while this is `Some`.
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    view! {
        <Show when=move || error.get().is_some()>
            <section class="state paused" role="status">
                <p class="state-title">{title}</p>
                <p>{body}</p>
                <details class="disclosure">
                    <summary>"Details"</summary>
                    <code class="mono">{move || error.get().unwrap_or_default()}</code>
                </details>
            </section>
        </Show>
    }
}

/// A network badge for a summary: the kind, then the name.
#[component]
pub fn NetBadge() -> impl IntoView {
    let state = expect_context::<AppState>();
    move || {
        state.network.get().map(|status| {
            let (class, kind) = if status.is_impostor() {
                ("net-badge impostor", "Not mainnet")
            } else if status.is_test_network() {
                ("net-badge test", "Testnet")
            } else {
                ("net-badge main", "Mainnet")
            };
            let name = status.network.unwrap_or_default();
            view! {
                <span class=class title=name.clone()>
                    {if name.is_empty() { kind.to_string() } else { format!("{kind} \u{b7} {name}") }}
                </span>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LATTICE_HALF, lattice_points, strength};

    #[test]
    fn the_lattice_fits_its_box_and_is_symmetric() {
        let points = lattice_points();
        assert!(points.len() > 20, "enough points to read as a lattice");
        assert!(points.contains(&(0, 0)));
        for &(x, y) in &points {
            assert!(
                x.abs() < LATTICE_HALF && y.abs() < LATTICE_HALF,
                "({x}, {y})"
            );
            assert!(points.contains(&(-x, -y)), "a lattice is symmetric");
        }
    }

    #[test]
    fn strength_grows_with_length_and_mix() {
        assert_eq!(strength("").0, 0, "empty shows no hint");
        assert_eq!(strength("abc").0, 1, "under 8 characters is too short");
        assert_eq!(
            strength("abcdefgh").0,
            1,
            "8 lowercase letters: the estimate floors log2(26) to 4 bits a \
             character, 8 x 4 = 32, under the Fair line: weak"
        );
        assert_eq!(strength("abcdefghij").0, 2);
        assert_eq!(
            strength("Abcdefgh1!").0,
            3,
            "the same length with a full mix"
        );
        assert_eq!(strength("correct horse battery staple").0, 4);
    }
}
