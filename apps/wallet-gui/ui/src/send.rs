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
use crate::receive::CopyButton;
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
    /// Amount plus fee, in base units. `None` if the sum would overflow 256
    /// bits, which no real transfer reaches.
    fn total(&self) -> Option<U256> {
        let amount = amount::parse_base_units(&self.amount).ok()?;
        let fee = amount::parse_base_units(&self.fee).ok()?;
        amount.checked_add(fee)
    }
}

/// A count of changes to what a review is computed from. A request takes a
/// ticket before it awaits the core; its answer is kept only if nothing has
/// changed since, so a review computed from the old form can never land on a
/// new one (the fee Effect can rewrite the fee while a preview is in flight).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Generation(u64);

impl Generation {
    /// Something the review depends on changed.
    fn bumped(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// `value` if `ticket` is still the current generation, else `None`.
    fn keep<T>(self, ticket: Self, value: T) -> Option<T> {
        (self == ticket).then_some(value)
    }
}

/// What the last step is doing, so the screen can show it happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Making the two signatures (the native dialog is open first).
    Signing,
    /// Handing the signed transaction to the node.
    Broadcasting,
}

/// A display amount as the summary shows it: trailing zeros dropped and the
/// ticker joined by a no-break space, so a wrapped figure never leaves
/// "MAYA" alone on a line.
fn figure(display: &str) -> String {
    format!("{}\u{a0}{}", trim_display(display), amount::TICKER)
}

/// Said when a signature is thrown away because what it was made from changed
/// on screen meanwhile (the account's nonce or the node's terms refreshed).
const CHANGED_WHILE_SIGNING: &str =
    "The transfer changed while it was being signed, so nothing was sent. Review it again.";

/// Decimals a Fee or Total row shows. The exact figure is under Details.
const SHOWN_DECIMALS: usize = 6;

/// A fee or total as its summary row shows it: exact when it has at most
/// [`SHOWN_DECIMALS`] decimals; otherwise rounded to them and marked "≈",
/// and a non-zero value too small to show reads "< 0.000001". The row is a
/// figure a person can check by eye; the marker says when it is not the exact
/// one, and Details always holds the exact one.
fn approx_figure(base: U256) -> String {
    let unit = U256::exp10(amount::DECIMALS - SHOWN_DECIMALS);
    if (base % unit).is_zero() {
        return figure(&amount::format_display(base));
    }
    let rounded = base.checked_add(unit / 2).unwrap_or(base) / unit * unit;
    if rounded.is_zero() {
        return format!("<\u{a0}{}", figure(&amount::format_display(unit)));
    }
    format!("\u{2248}\u{a0}{}", figure(&amount::format_display(rounded)))
}

/// A display amount from the core ("1,000.5000…") as base units.
fn display_value(display: &str) -> Option<U256> {
    amount::parse_display(&display.replace(',', "")).ok()
}

/// A row's value: the [`approx_figure`] of `value`, or a dash when there is
/// none, never a guessed figure. One line: the figure never wraps.
fn approx_view(value: Option<U256>) -> impl IntoView + use<> {
    let text = value.map_or_else(|| "\u{2014}".to_string(), approx_figure);
    view! { <span class="figure one-line">{text}</span> }
}

/// Long fractions get the smaller mono style, so 24 decimals fit a phone.
const LONG_FIGURE: usize = 14;

/// A [`figure`] in its span, smaller when long. A long fraction may wrap
/// anywhere, but its last digit, the no-break space and the ticker sit in one
/// unbreakable span, so "MAYA" is never split or left alone on a line.
fn figure_view(display: &str) -> impl IntoView + use<> {
    let text = figure(display);
    let class = if text.chars().count() > LONG_FIGURE {
        "figure long"
    } else {
        "figure"
    };
    let tail_len = amount::TICKER.chars().count() + 2;
    let split = text
        .char_indices()
        .rev()
        .nth(tail_len - 1)
        .map_or(0, |(i, _)| i);
    let (head, tail) = text.split_at(split);
    let (head, tail) = (head.to_string(), tail.to_string());
    view! { <span class=class>{head}<span class="keep">{tail}</span></span> }
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
    let phase = RwSignal::new(Phase::Idle);
    let fee_terms = RwSignal::new(bridge::FeeTerms::default());
    let fee_error = RwSignal::new(None::<String>);
    let generation = StoredValue::new(Generation::default());

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
        state.account_state.track();
        state.selected.track();
        generation.update_value(|g| *g = g.bumped());
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
        let ticket = generation.get_value();
        busy.set(true);
        spawn_local(async move {
            let outcome =
                bridge::preview_transfer(index, &to, &amount_value, &fee_value, nonce, &terms)
                    .await;
            match outcome {
                Ok(confirmation) => {
                    let review = Review {
                        confirmation,
                        index,
                        recipient: to,
                        amount: amount_value,
                        fee: fee_value,
                        nonce,
                        terms,
                    };
                    // Computed from a form that has changed since: dropped,
                    // and the user reviews again.
                    if let Some(review) = generation.get_value().keep(ticket, review) {
                        // A signature belongs to one review: never carry one
                        // over to a new summary.
                        signed.set(None);
                        preview.set(Some(review));
                    }
                }
                Err(error) => state.fail(error),
            }
            busy.set(false);
        });
    };

    let edit = move |_| {
        generation.update_value(|g| *g = g.bumped());
        preview.set(None);
        signed.set(None);
        state.clear();
    };

    // Offline: sign only, and show the hex to carry to an online device.
    let sign_only = move |_| {
        let Some(review) = preview.get_untracked() else {
            return;
        };
        let ticket = generation.get_value();
        busy.set(true);
        phase.set(Phase::Signing);
        spawn_local(async move {
            match sign_reviewed(&review).await {
                Ok(transfer) => {
                    // Shown, and announced, only beside the review it was
                    // made from.
                    if generation.get_value().keep(ticket, ()).is_some() {
                        signed.set(Some(transfer));
                        state.inform("Signed. This transaction can be broadcast from any device.");
                    } else {
                        state.fail(CHANGED_WHILE_SIGNING);
                    }
                    if let Ok(history) = bridge::history().await {
                        state.history.set(history);
                    }
                }
                Err(error) => state.fail(error),
            }
            phase.set(Phase::Idle);
            busy.set(false);
        });
    };

    // The one primary action: sign what was reviewed (unless already signed)
    // and broadcast it. Both steps check the ticket, so nothing leaves for a
    // review that is no longer on screen.
    let send_final = move |_| {
        let Some(review) = preview.get_untracked() else {
            return;
        };
        let ticket = generation.get_value();
        busy.set(true);
        spawn_local(async move {
            let transfer = match signed.get_untracked() {
                Some(transfer) => transfer,
                None => {
                    phase.set(Phase::Signing);
                    match sign_reviewed(&review).await {
                        Ok(transfer) if generation.get_value() == ticket => {
                            signed.set(Some(transfer.clone()));
                            transfer
                        }
                        Ok(_) => {
                            state.fail(CHANGED_WHILE_SIGNING);
                            phase.set(Phase::Idle);
                            busy.set(false);
                            return;
                        }
                        Err(error) => {
                            state.fail(error);
                            phase.set(Phase::Idle);
                            busy.set(false);
                            return;
                        }
                    }
                }
            };
            if generation.get_value() == ticket {
                phase.set(Phase::Broadcasting);
                broadcast(state, transfer).await;
            } else {
                state.fail(CHANGED_WHILE_SIGNING);
            }
            phase.set(Phase::Idle);
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
        let fee_value = c.fee.as_deref().and_then(display_value);
        let shown_amount = trim_display(&c.amount.clone().unwrap_or_default());
        let fee_exceeds = c.fee_exceeds_amount;
        // One form of the recipient only: the core's checksummed hex, in
        // groups of four, and that same string is what Copy copies.
        let to = c.recipient.clone().unwrap_or_default();
        let groups = chunks(&to)
            .into_iter()
            .map(|group| view! { <span>{group}</span> })
            .collect::<Vec<_>>();
        let is_signed = move || signed.get().is_some();
        let final_label = format!("Send {shown_amount} {}", amount::TICKER);
        view! {
            // The summary is the locked step: the form's values as text,
            // with Edit the one way back to the fields.
            <section class="summary" aria-labelledby="review-title">
                <div class="summary-top">
                    <span class="summary-kicker">
                        <span class="eyebrow" id="review-title">"You are sending"</span>
                        <NetBadge/>
                    </span>
                    // Disabled while signing or broadcasting: an Edit then
                    // would let a signature of this review land after the
                    // form had moved on.
                    <button
                        type="button"
                        class="link edit"
                        disabled=move || busy.get()
                        on:click=edit
                    >
                        "Edit"
                    </button>
                </div>
                <div class="amount-hero">
                    <span class="value">{shown_amount.clone()}</span>
                    <span class="unit">{amount::TICKER}</span>
                </div>
                <div class="kv">
                    <span>"To"</span>
                    <span class="to">
                        <span class="chunks">{groups}</span>
                        <CopyButton text=to.clone() label="Copy recipient" compact=true/>
                    </span>
                </div>
                <div class="kv inline">
                    <span>"Fee"</span>
                    {approx_view(fee_value)}
                </div>
                <div class="kv inline total">
                    <span>"Total"</span>
                    {approx_view(total)}
                </div>
                <details class="disclosure">
                    <summary>"Details"</summary>
                    <p class="description">{c.description}</p>
                    // The exact figures behind the rounded rows above.
                    <div class="kv"><span>"Exact fee"</span>{figure_view(&c.fee.unwrap_or_default())}</div>
                    <div class="kv">
                        <span>"Exact total"</span>
                        {match total {
                            Some(t) => figure_view(&amount::format_display(t)).into_any(),
                            None => view! { <span>"\u{2014}"</span> }.into_any(),
                        }}
                    </div>
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
                        "This hex is a complete signed transaction. Signing it made \
                         no network call (the review above used the node), and it \
                         can be copied to an online device and broadcast there."
                    </p>
                </div>
            </Show>

            // Pinned to the bottom of a short window, so the one action and
            // its warning are always on screen with the summary.
            <div class="send-bar">
                // The two signatures every transfer carries, drawn while they
                // are made: a lattice trace (ML-DSA-65) and a hash chain
                // (SLH-DSA). Shown only while the step really runs.
                <Show when=move || phase.get() != Phase::Idle>
                    <p class="sig-trace" role="status">
                        <span class="trace lattice-trace" aria-hidden="true">
                            <i></i><i></i><i></i><i></i><i></i><i></i>
                        </span>
                        {move || match phase.get() {
                            Phase::Broadcasting => "Handing it to the network\u{2026}",
                            _ => "Signing: ML-DSA-65 and SLH-DSA\u{2026}",
                        }}
                        <span class="trace hash-trace" aria-hidden="true">
                            <i></i><i></i><i></i><i></i><i></i><i></i>
                        </span>
                    </p>
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
                        class="link sign-only"
                        disabled=move || busy.get()
                        on:click=sign_only
                    >
                        "Sign only, to broadcast elsewhere"
                    </button>
                </Show>
            </div>
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

            // On the locked step Edit is the way back, so Back is the form's.
            <Show when=move || preview.get().is_none()>
                <button
                    class="ghost wide"
                    on:click=move |_| {
                        state.clear();
                        state.screen.set(Screen::Wallet);
                    }
                >
                    "Back"
                </button>
            </Show>
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
        let shown = |r: Review| r.total().map(|t| trim_display(&amount::format_display(t)));
        assert_eq!(shown(review(one, tenth)).as_deref(), Some("1.1"));
        assert_eq!(shown(review(one, "0")).as_deref(), Some("1"));
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
    fn a_review_from_a_changed_form_is_discarded() {
        let start = Generation::default();
        let ticket = start;
        assert_eq!(
            start.keep(ticket, "review"),
            Some("review"),
            "unchanged: kept"
        );
        let after_edit = start.bumped();
        assert_eq!(
            after_edit.keep(ticket, "review"),
            None,
            "the form changed while the preview was in flight: dropped"
        );
        assert_eq!(after_edit.keep(after_edit, ()), Some(()));
    }

    #[test]
    fn the_shown_fee_is_the_previewed_fee() {
        // What the summary prints for a fee must read back as the exact base
        // units that were previewed and will be signed.
        for base in [
            "10",
            "1",
            "100000000000000000000000",
            "1234567890123456789012345678",
        ] {
            let previewed = U256::from_dec_str(base).unwrap();
            let shown = figure(&amount::format_display(previewed));
            let number = shown
                .strip_suffix(&format!("\u{a0}{}", amount::TICKER))
                .expect("the ticker follows a no-break space")
                .replace(',', "");
            assert_eq!(amount::parse_display(&number), Ok(previewed), "{shown}");
        }
        assert_eq!(
            figure(&amount::format_display(U256::from(10))),
            "0.00000000000000000000001\u{a0}MAYA",
            "10 base units at 24 decimals"
        );
    }

    #[test]
    fn a_row_figure_is_exact_or_says_it_is_not() {
        let base = |s: &str| U256::from_dec_str(s).unwrap();
        let row = |s: &str| approx_figure(base(s)).replace('\u{a0}', " ");
        // Six decimals or fewer: exact, no marker.
        assert_eq!(row("1100000000000000000000000"), "1.1 MAYA");
        assert_eq!(row("1000000000000000000"), "0.000001 MAYA");
        assert_eq!(row("0"), "0 MAYA");
        // Too small to show at six decimals: a bound, never "0".
        assert_eq!(row("10"), "< 0.000001 MAYA");
        // More precision than the row shows: rounded and marked.
        assert_eq!(row("2000000000000000000000010"), "\u{2248} 2 MAYA");
        assert_eq!(row("1500000000000000000"), "\u{2248} 0.000002 MAYA");
        assert_eq!(row("1400000000000000000"), "\u{2248} 0.000001 MAYA");
        // Every row is short enough for one line at 360 px.
        assert!(row("19687500123456789012345678901234").chars().count() <= 28);
        // A fee the core shows as a display string reads back exactly.
        assert_eq!(
            display_value("1,000.000000000000000000000010"),
            Some(base("1000000000000000000000000010"))
        );
    }

    #[test]
    fn show_amount_is_the_untrimmed_core_format() {
        assert!(crate::screens::show_amount("1000000000000000000000000").starts_with("1."));
    }
}
