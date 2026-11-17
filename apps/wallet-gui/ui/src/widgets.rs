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

/// A passphrase input with its label, a Show/Hide toggle and, optionally, a
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
    let name = label.to_lowercase();
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
                <button
                    type="button"
                    class="reveal"
                    aria-pressed=move || if shown.get() { "true" } else { "false" }
                    aria-label=move || {
                        format!("{} {name}", if shown.get() { "Hide" } else { "Show" })
                    }
                    on:click=move |_| shown.update(|s| *s = !*s)
                >
                    {move || if shown.get() { "Hide" } else { "Show" }}
                </button>
            </div>
            <Show when=move || show_strength>
                <p
                    id=hint_id.clone()
                    class=move || format!("strength s{}", strength(&value.get()).0)
                    aria-live="polite"
                >
                    <span class="bars" aria-hidden="true">
                        <span></span><span></span><span></span><span></span>
                    </span>
                    <span>
                        {move || {
                            let (_, word) = strength(&value.get());
                            if word.is_empty() { String::new() } else { format!("Strength: {word}") }
                        }}
                    </span>
                </p>
            </Show>
        </div>
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
                <details>
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
    use super::strength;

    #[test]
    fn strength_grows_with_length_and_mix() {
        assert_eq!(strength("").0, 0, "empty shows no hint");
        assert_eq!(strength("abc").0, 1, "under 8 characters is too short");
        assert_eq!(
            strength("abcdefgh").0,
            1,
            "8 lowercase letters is about 32 bits: weak"
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
