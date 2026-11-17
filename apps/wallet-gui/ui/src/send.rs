//! Compose, review, sign and broadcast a transfer.
//!
//! Review is a locked step. Once the core has returned a confirmation, the
//! form becomes read-only text and the summary shows what will be signed;
//! Edit is the only way back to the fields, and it throws the review (and any
//! signature made from it) away. Signing replays the reviewed inputs, never
//! the live fields, and the core refuses a signature whose hash no confirm
//! screen showed (`Wallet::sign`), so this lock is what keeps the screen
//! honest: the summary on it is always the transaction that would be signed.

use leptos::prelude::*;
use leptos::task::spawn_local;
use primitive_types::U256;

use crate::amount;
use crate::bridge;
use crate::screens::{chunks, input_amount, pretty_amount, trim_display, typed_amount};
use crate::widgets::{NetBadge, PausedCard};
use crate::{AppState, Screen};

/// A transfer the user reviewed: what the core showed, and the exact inputs it
/// was computed from. Signing replays these, never the live form fields, so
/// the reviewed transaction is the one `sign_transfer` is asked to sign.
#[derive(Clone)]
pub struct Review {
    pub confirmation: bridge::Confirmation,
    pub index: u32,
    pub recipient: String,
    pub amount: String,
    pub fee: String,
    pub nonce: u64,
    pub terms: bridge::FeeTerms,
}

impl Review {
    /// Amount plus fee, in base units, as a display string. `None` if the
    /// sum would overflow 256 bits, which no real transfer reaches.
    fn total(&self) -> Option<String> {
        let amount = amount::parse_base_units(&self.amount).ok()?;
        let fee = amount::parse_base_units(&self.fee).ok()?;
        amount
            .checked_add(fee)
            .map(|sum: U256| trim_display(&amount::format_display(sum)))
    }
}

/// Signs exactly what was reviewed.
async fn sign_reviewed(review: &Review) -> Result<bridge::SignedTransfer, String> {
    bridge::sign_transfer(
        review.index,
        &review.recipient,
        &review.amount,
        &review.fee,
        review.nonce,
        &review.terms,
        &review.confirmation.signing_hash,
    )
    .await
}

/// Broadcasts a signed transfer and moves to Sent, or reports why not.
async fn broadcast(state: AppState, transfer: bridge::SignedTransfer) {
    match bridge::broadcast(&state.node_url.get_untracked(), &transfer.raw_hex).await {
        Ok(txid) => {
            let _ = bridge::record_outcome(&transfer.txid, true, None).await;
            if let Ok(history) = bridge::history().await {
                state.history.set(history);
            }
            state.clear();
            state.screen.set(Screen::Sent(crate::receive::SentTransfer {
                txid,
                amount: transfer.amount.clone(),
                recipient: transfer.recipient.clone(),
            }));
        }
        Err(error) => {
            let _ = bridge::record_outcome(&transfer.txid, false, Some(error.clone())).await;
            state.fail(error);
        }
    }
}

/// Send screen.
#[component]
pub fn Send(request: bridge::PaymentRequest) -> impl IntoView {
    let state = expect_context::<AppState>();

    let recipient = RwSignal::new(request.recipient.clone());
    // A payment request carries base units; the form edits whole coins.
    let amount = RwSignal::new(
        request
            .amount
            .as_deref()
            .map(input_amount)
            .unwrap_or_default(),
    );
    let fee = RwSignal::new(String::new());
    let label = request.label.clone();
    let preview = RwSignal::new(None::<Review>);
    let signed = RwSignal::new(None::<bridge::SignedTransfer>);
    let busy = RwSignal::new(false);
    let fee_terms = RwSignal::new(bridge::FeeTerms::default());
    let fee_error = RwSignal::new(None::<String>);

    // Priced by the node: on a fee-market chain a flat guess is refused as
    // underpaid, and the fee must go to the collector the node names.
    Effect::new(move |_| {
        let node_url = state.node_url.get();
        let index = state.selected.get();
        let nonce = state.account_state.get().nonce;
        spawn_local(async move {
            match bridge::fee_options(&node_url, index, nonce).await {
                Ok(terms) => {
                    // Standard, the second tier, is the default.
                    if let Some(standard) = terms.options.get(1) {
                        fee.set(input_amount(&standard.fee));
                    }
                    fee_terms.set(terms);
                    fee_error.set(None);
                }
                Err(error) => fee_error.set(Some(error)),
            }
        });
    });

    // The lock's second half: any change to what a review was computed from
    // (a field, or the node's terms arriving late) drops the review and any
    // signature, so a stale summary can never sit over a different form.
    Effect::new(move |_| {
        recipient.track();
        amount.track();
        fee.track();
        fee_terms.track();
        preview.set(None);
        signed.set(None);
    });

    let build_preview = move |_| {
        let Ok(amount_value) = typed_amount(&amount.get()) else {
            state.fail(amount::decimal_places_message("Amount"));
            return;
        };
        let Ok(fee_value) = typed_amount(&fee.get()) else {
            state.fail(amount::decimal_places_message("Fee"));
            return;
        };
        state.clear();
        let index = state.selected.get();
        let nonce = state.account_state.get().nonce;
        let to = recipient.get();
        // Captured now: signing replays these terms, so the chain and the
        // collector the user reviewed cannot drift before the signature.
        let terms = fee_terms.get_untracked();
        busy.set(true);
        spawn_local(async move {
            let outcome =
                bridge::preview_transfer(index, &to, &amount_value, &fee_value, nonce, &terms)
                    .await;
            match outcome {
                Ok(confirmation) => {
                    // A signature belongs to one review: never carry one
                    // over to a new summary.
                    signed.set(None);
                    preview.set(Some(Review {
                        confirmation,
                        index,
                        recipient: to,
                        amount: amount_value,
                        fee: fee_value,
                        nonce,
                        terms,
                    }));
                }
                Err(error) => state.fail(error),
            }
            busy.set(false);
        });
    };

    let edit = move |_| {
        preview.set(None);
        signed.set(None);
        state.clear();
    };

    // Offline: sign only, and show the hex to carry to an online device.
    let sign_only = move |_| {
        let Some(review) = preview.get_untracked() else {
            return;
        };
        busy.set(true);
        spawn_local(async move {
            match sign_reviewed(&review).await {
                Ok(transfer) => {
                    signed.set(Some(transfer));
                    state.inform("Signed. This transaction can be broadcast from any device.");
                    if let Ok(history) = bridge::history().await {
                        state.history.set(history);
                    }
                }
                Err(error) => state.fail(error),
            }
            busy.set(false);
        });
    };

    // The one primary action: sign what was reviewed (unless already signed)
    // and broadcast it.
    let send_final = move |_| {
        let Some(review) = preview.get_untracked() else {
            return;
        };
        busy.set(true);
        spawn_local(async move {
            let transfer = match signed.get_untracked() {
                Some(transfer) => transfer,
                None => match sign_reviewed(&review).await {
                    Ok(transfer) => {
                        signed.set(Some(transfer.clone()));
                        transfer
                    }
                    Err(error) => {
                        state.fail(error);
                        busy.set(false);
                        return;
                    }
                },
            };
            broadcast(state, transfer).await;
            busy.set(false);
        });
    };

    let fee_buttons = move || {
        fee_terms
            .get()
            .options
            .into_iter()
            .map(|option| {
                let shown = input_amount(&option.fee);
                let pressed = shown.clone();
                let on_click = shown.clone();
                let chip_value = format!("{} {}", pretty_amount(&option.fee), amount::TICKER);
                view! {
                    <button
                        type="button"
                        class=move || if fee.get() == shown { "chip selected" } else { "chip" }
                        aria-pressed=move || if fee.get() == pressed { "true" } else { "false" }
                        on:click=move |_| fee.set(on_click.clone())
                    >
                        {option.label.clone()}
                        <span class="chip-value">{chip_value}</span>
                    </button>
                }
            })
            .collect::<Vec<_>>()
    };

    let form = move || {
        view! {
            <PausedCard
                title="Fees could not be read"
                body="The node did not answer, so this transfer cannot be priced or \
                      reviewed yet. Nothing has been signed."
                error=fee_error
            />
            <div class="field">
                <label for="send-recipient">"Recipient"</label>
                <input
                    id="send-recipient"
                    class="mono"
                    autocomplete="off"
                    spellcheck="false"
                    autocapitalize="off"
                    prop:readonly=move || busy.get()
                    prop:value=move || recipient.get()
                    on:input=move |ev| recipient.set(event_target_value(&ev))
                />
            </div>
            <div class="field">
                <label for="send-amount">"Amount"</label>
                <input
                    id="send-amount"
                    inputmode="decimal"
                    autocomplete="off"
                    aria-describedby="send-amount-unit"
                    prop:readonly=move || busy.get()
                    prop:value=move || amount.get()
                    on:input=move |ev| amount.set(event_target_value(&ev))
                />
                <span class="hint" id="send-amount-unit">{format!("In {}", amount::TICKER)}</span>
            </div>

            <span class="field-label" id="fee-label">"Network fee"</span>
            <div class="chips" role="group" aria-labelledby="fee-label">{fee_buttons}</div>
            <div class="field">
                <label for="send-fee">"Custom fee"</label>
                <input
                    id="send-fee"
                    inputmode="decimal"
                    autocomplete="off"
                    aria-describedby="send-fee-hint"
                    prop:readonly=move || busy.get()
                    prop:value=move || fee.get()
                    on:input=move |ev| fee.set(event_target_value(&ev))
                />
            </div>
            <p class="hint" id="send-fee-hint">
                {move || if fee_terms.get().collector.is_some() {
                    "The network burns the base fee; anything above it tips the \
                     validators. Below Economy, the node refuses the transfer."
                } else {
                    "This network charges no fee. Any fee you set is burned."
                }}
            </p>

            <button
                class="primary wide"
                disabled=move || busy.get() || fee_terms.get().genesis.is_empty()
                on:click=build_preview
            >
                {move || if busy.get() { "Reviewing…" } else { "Review" }}
            </button>
        }
    };

    let locked = move |review: Review| {
        // Every line of the summary is the core's confirmation; this file
        // only lays it out (and adds amount + fee for the Total).
        let total = review.total();
        let c = review.confirmation;
        let shown_amount = trim_display(&c.amount.clone().unwrap_or_default());
        let fee_exceeds = c.fee_exceeds_amount;
        let to = c.recipient.clone().unwrap_or_default();
        let groups = chunks(&to)
            .into_iter()
            .map(|group| view! { <span>{group}</span> })
            .collect::<Vec<_>>();
        let is_signed = move || signed.get().is_some();
        let final_label = format!("Send {shown_amount} {}", amount::TICKER);
        view! {
            <div class="readouts">
                <div class="readouts-head">
                    <h2>"Your transfer"</h2>
                    // Disabled while signing or broadcasting: an Edit then
                    // would let a signature of this review land after the
                    // form had moved on.
                    <button
                        type="button"
                        class="link"
                        disabled=move || busy.get()
                        on:click=edit
                    >
                        "Edit"
                    </button>
                </div>
                <div class="readout">
                    <span class="field-label">"Recipient"</span>
                    <span class="value mono">{review.recipient.clone()}</span>
                </div>
                <div class="readout">
                    <span class="field-label">"Amount"</span>
                    <span class="value">{format!("{} {}", pretty_amount(&review.amount), amount::TICKER)}</span>
                </div>
                <div class="readout">
                    <span class="field-label">"Network fee"</span>
                    <span class="value">{format!("{} {}", pretty_amount(&review.fee), amount::TICKER)}</span>
                </div>
            </div>

            <section class="summary" aria-labelledby="review-title">
                <div class="summary-top">
                    <span class="eyebrow" id="review-title">"You are sending"</span>
                    <NetBadge/>
                </div>
                <div class="amount-hero">
                    <span class="value">{shown_amount.clone()}</span>
                    <span class="unit">{amount::TICKER}</span>
                </div>
                <p class="description">{c.description}</p>
                <div class="kv">
                    <span>"To"</span>
                    <span class="chunks">{groups}</span>
                </div>
                <div class="kv inline">
                    <span>"Fee"</span>
                    <span>{format!("{} {}", trim_display(&c.fee.unwrap_or_default()), amount::TICKER)}</span>
                </div>
                <div class="kv inline total">
                    <span>"Total"</span>
                    <span>
                        {total.map_or_else(
                            || "\u{2014}".to_string(),
                            |t| format!("{t} {}", amount::TICKER),
                        )}
                    </span>
                </div>
                <details>
                    <summary>"Details"</summary>
                    <div class="kv"><span>"Nonce"</span><span>{c.nonce}</span></div>
                    <div class="kv"><span>"Signing hash"</span><span class="mono">{c.signing_hash}</span></div>
                    <div class="kv"><span>"Chain"</span><span class="mono">{c.chain_id}</span></div>
                    <div class="kv"><span>"From"</span><span class="mono">{c.sender}</span></div>
                    {c.details
                        .into_iter()
                        .map(|(label, value)| view! {
                            <div class="kv"><span>{label}</span><span class="mono">{value}</span></div>
                        })
                        .collect_view()}
                </details>
            </section>

            // The collector itself is checked in the wallet core against the
            // chain's fixed address; the size of the fee is the user's to
            // judge, so an unusual one is called out before signing.
            <Show when=move || fee_exceeds>
                <p class="warn" role="status">
                    "The fee is larger than the amount you are sending. Check it before signing."
                </p>
            </Show>

            <Show when=is_signed>
                <div class="raw field">
                    <label for="signed-hex">"Signed transaction"</label>
                    <textarea id="signed-hex" class="mono" rows="4" readonly>
                        {move || signed.get().map(|t| t.raw_hex).unwrap_or_default()}
                    </textarea>
                    <p class="hint">
                        "This hex is a complete transaction. It can be copied to an \
                         online device and broadcast there, so this machine never \
                         needs a network connection."
                    </p>
                </div>
            </Show>

            <p class="irreversible">"This cannot be undone."</p>
            <button
                class="primary wide send-final"
                disabled=move || busy.get()
                on:click=send_final
            >
                {final_label}
            </button>
            <Show when=move || !is_signed()>
                <button
                    type="button"
                    class="ghost wide"
                    disabled=move || busy.get()
                    on:click=sign_only
                >
                    "Sign only, to broadcast elsewhere"
                </button>
            </Show>
        }
    };

    view! {
        <section class="card enter">
            <h1>{move || if preview.get().is_some() { "Review transfer" } else { "Send" }}</h1>

            <Show when={let label = label.clone(); move || label.is_some()}>
                <p class="hint">
                    {
                        let label = label.clone().unwrap_or_default();
                        format!("Requested by \u{201c}{label}\u{201d}: this name is \
                                 supplied by the payee and is not verified.")
                    }
                </p>
            </Show>

            {move || match preview.get() {
                Some(review) => locked(review).into_any(),
                None => form().into_any(),
            }}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn review(amount: &str, fee: &str) -> Review {
        Review {
            confirmation: bridge::Confirmation {
                signing_hash: String::new(),
                description: String::new(),
                details: Vec::new(),
                chain_id: String::new(),
                sender: String::new(),
                nonce: 0,
                recipient: None,
                amount: None,
                fee: None,
                fee_exceeds_amount: false,
            },
            index: 0,
            recipient: String::new(),
            amount: amount.to_string(),
            fee: fee.to_string(),
            nonce: 0,
            terms: bridge::FeeTerms::default(),
        }
    }

    #[test]
    fn the_total_is_amount_plus_fee_in_whole_coins() {
        let one = "1000000000000000000000000";
        let tenth = "100000000000000000000000";
        assert_eq!(review(one, tenth).total().as_deref(), Some("1.1"));
        assert_eq!(review(one, "0").total().as_deref(), Some("1"));
        assert_eq!(
            review("x", "0").total(),
            None,
            "unparsed input has no total"
        );
    }

    #[test]
    fn an_overflowing_total_is_none() {
        let max = U256::MAX.to_string();
        assert_eq!(review(&max, "1").total(), None);
    }

    #[test]
    fn show_amount_is_the_untrimmed_core_format() {
        assert!(crate::screens::show_amount("1000000000000000000000000").starts_with("1."));
    }
}
