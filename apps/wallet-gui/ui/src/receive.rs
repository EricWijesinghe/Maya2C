//! Receive, and the moment after a send.
//!
//! Kept out of `screens.rs`: these two screens are the ones a first-time user
//! sees after "get coins" and "send coins", so they carry most of the
//! product's feel, and they change for that reason rather than for protocol
//! reasons.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::bridge;
use crate::screens::{group_digits, short};
use crate::{AppState, Screen};

/// How long "Copied" stays on a copy button before it reads "Copy" again.
const COPIED_FOR: std::time::Duration = std::time::Duration::from_millis(1600);

/// A transfer that has just left this wallet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentTransfer {
    /// Its id, hex.
    pub txid: String,
    /// Base units sent to the recipient.
    pub amount: u64,
    /// The recipient, hex.
    pub recipient: String,
}

/// A button that copies `text` and says so for a moment.
#[component]
fn CopyButton(#[prop(into)] text: Signal<String>, label: &'static str) -> impl IntoView {
    let state = expect_context::<AppState>();
    let copied = RwSignal::new(false);
    let copy = move |_| {
        spawn_local(async move {
            match bridge::copy_to_clipboard(&text.get_untracked()).await {
                Ok(()) => {
                    copied.set(true);
                    set_timeout(move || copied.set(false), COPIED_FOR);
                }
                Err(error) => state.fail(format!("Could not copy: {error}")),
            }
        });
    };
    view! {
        <button class=move || if copied.get() { "copy done" } else { "copy" } on:click=copy>
            {move || if copied.get() { "Copied" } else { label }}
        </button>
    }
}

/// The selected account's address and a QR code of its payment URI.
#[component]
pub fn Receive() -> impl IntoView {
    let state = expect_context::<AppState>();
    let request = RwSignal::new(None::<bridge::ReceiveRequest>);
    let amount = RwSignal::new(String::new());

    // Redrawn when the account or the requested amount changes. An amount
    // that does not parse simply asks for none, rather than erroring while
    // someone is still typing.
    Effect::new(move |_| {
        let index = state.selected.get();
        let wanted = amount.get().trim().parse::<u64>().ok().filter(|a| *a > 0);
        spawn_local(async move {
            match bridge::receive_request(index, wanted).await {
                Ok(drawn) => request.set(Some(drawn)),
                Err(error) => state.fail(error),
            }
        });
    });

    let address = Signal::derive(move || request.get().map(|r| r.address).unwrap_or_default());
    let uri = Signal::derive(move || request.get().map(|r| r.uri).unwrap_or_default());
    let testnet = move || state.network.get().is_none_or(|n| n.is_test_network());

    view! {
        <section class="card receive enter">
            <h1>"Receive"</h1>
            <p class="lede">
                {move || format!("Account {} \u{b7} anyone can pay this address.", state.selected.get())}
            </p>
            <div class="qr-frame">
                {move || match request.get() {
                    // The SVG comes from the wallet core's QR encoder, never
                    // from user input, so inserting it as markup is safe.
                    Some(drawn) => view! { <div class="qr" inner_html=drawn.qr_svg></div> }.into_any(),
                    None => view! { <div class="qr placeholder shimmer"></div> }.into_any(),
                }}
            </div>
            <Show when=testnet>
                <p class="hint testnet-note">"Testnet address: coins sent here have no value."</p>
            </Show>

            <label>"Address"</label>
            <div class="copy-row">
                <code class="mono address">{move || address.get()}</code>
                <CopyButton text=address label="Copy"/>
            </div>

            <label>"Ask for an amount (optional)"</label>
            <input
                type="number"
                min="0"
                placeholder="any amount"
                prop:value=move || amount.get()
                on:input=move |ev| amount.set(event_target_value(&ev))
            />
            <div class="copy-row">
                <code class="mono uri">{move || uri.get()}</code>
                <CopyButton text=uri label="Copy link"/>
            </div>

            <button
                class="ghost wide"
                on:click=move |_| {
                    state.clear();
                    state.screen.set(Screen::Wallet);
                }
            >
                "Back"
            </button>
        </section>
    }
}

/// The moment after a broadcast: what left, where to, and its id.
#[component]
pub fn Sent(sent: SentTransfer) -> impl IntoView {
    let state = expect_context::<AppState>();
    let txid = Signal::derive({
        let txid = sent.txid.clone();
        move || txid.clone()
    });
    view! {
        <section class="card sent enter">
            <div class="sent-check" aria-hidden="true">
                <svg viewBox="0 0 52 52">
                    <circle class="check-ring" cx="26" cy="26" r="24"/>
                    <path class="check-tick" d="M15 27 l7 7 l15 -16"/>
                </svg>
            </div>
            <h1>"Sent"</h1>
            <p class="sent-amount">
                {group_digits(sent.amount)}
                <span class="unit">" units"</span>
            </p>
            <p class="lede">{format!("to {}", short(&sent.recipient))}</p>
            <p class="hint">
                "Broadcast to the network. It is final once a block includes it, \
                 usually within seconds."
            </p>
            <div class="copy-row">
                <code class="mono">{short(&sent.txid)}</code>
                <CopyButton text=txid label="Copy id"/>
            </div>
            <button
                class="primary wide"
                on:click=move |_| {
                    state.clear();
                    state.screen.set(Screen::Wallet);
                }
            >
                "Done"
            </button>
        </section>
    }
}
