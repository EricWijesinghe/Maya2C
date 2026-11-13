//! The multisig screen (P-16): share a member key, set up or open an m-of-n
//! account, propose a spend, approve it, merge the other members' copies and
//! finish.
//!
//! The account and the proposal are hex text the members pass to each other
//! by any channel. Every rule (policy, approvals, fee cap, what a proposal
//! may contain) is the wallet core's; approval goes through the same native
//! confirm screen as a transfer.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::bridge::{self, multisig as ms};
use crate::receive::CopyButton;
use crate::screens::{input_amount, typed_amount};
use crate::{AppState, Screen};

/// Suite byte to the name a person compares.
fn suite_name(suite: u8) -> &'static str {
    match suite {
        0x10 => "ML-DSA-65",
        0x11 => "ML-DSA-87",
        0x20 => "SLH-DSA-SHA2-128s",
        0x21 => "SLH-DSA-SHAKE-256f",
        0x30 => "ML-DSA-65 + SLH-DSA (hybrid)",
        _ => "unknown suite",
    }
}

/// One member per line as `suite:hex` (`30:…`), the form "Copy my key" gives.
/// A line without a prefix is read as this wallet's hybrid suite.
fn parse_members(text: &str) -> Result<Vec<ms::MemberKey>, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let (suite, key) = line.split_once(':').unwrap_or(("30", line));
            let suite = u8::from_str_radix(suite.trim().trim_start_matches("0x"), 16)
                .map_err(|_| format!("\u{201c}{suite}\u{201d} is not a suite byte"))?;
            Ok(ms::MemberKey {
                suite,
                public_key: key.trim().to_string(),
            })
        })
        .collect()
}

/// The multisig screen.
#[component]
pub fn Multisig() -> impl IntoView {
    let state = expect_context::<AppState>();
    let my_key = RwSignal::new(String::new());
    let members = RwSignal::new(String::new());
    let threshold = RwSignal::new(String::from("2"));
    let account_hex = RwSignal::new(String::new());
    let account = RwSignal::new(None::<ms::Account>);
    let recipient = RwSignal::new(String::new());
    let amount = RwSignal::new(String::new());
    let fee = RwSignal::new(String::new());
    let proposal = RwSignal::new(String::new());
    let other = RwSignal::new(String::new());
    let progress = RwSignal::new(None::<ms::Proposal>);
    let review = RwSignal::new(None::<bridge::Confirmation>);
    let signed = RwSignal::new(None::<bridge::SignedTransfer>);
    let busy = RwSignal::new(false);

    // Any new proposal text invalidates what was reviewed or finished.
    let set_proposal = move |p: ms::Proposal| {
        proposal.set(p.proposal_hex.clone());
        progress.set(Some(p));
        review.set(None);
        signed.set(None);
    };

    // A v8 spend carries the policy and m approvals, several times a
    // transfer's bytes, so the fee is priced for that frame, not a transfer's.
    // Standard, the second tier, is the default, as on the send screen.
    Effect::new(move |_| {
        let (Some(a), node) = (account.get(), state.node_url.get()) else {
            return;
        };
        spawn_local(async move {
            match ms::fee_options(&node, &a.account_hex).await {
                Ok(terms) => {
                    if let Some(standard) = terms.options.get(1) {
                        fee.set(input_amount(&standard.fee));
                    }
                }
                Err(e) => state.fail(format!("Could not read fees from the node: {e}")),
            }
        });
    });

    let show_key = move |_| {
        let index = state.selected.get_untracked();
        spawn_local(async move {
            match ms::member_key(index).await {
                Ok(k) => my_key.set(format!("{:02x}:{}", k.suite, k.public_key)),
                Err(e) => state.fail(e),
            }
        });
    };

    let create = move |_| {
        state.clear();
        let parsed = parse_members(&members.get_untracked());
        let m = threshold.get_untracked().trim().parse::<usize>();
        spawn_local(async move {
            let result = match (parsed, m) {
                (Ok(keys), Ok(m)) => ms::create(m, &keys, &state.genesis()).await,
                (Err(e), _) => Err(e),
                (_, Err(_)) => Err("the threshold is not a whole number".to_string()),
            };
            match result {
                Ok(a) => {
                    account_hex.set(a.account_hex.clone());
                    account.set(Some(a));
                }
                Err(e) => state.fail(e),
            }
        });
    };

    let open = move |_| {
        state.clear();
        let text = account_hex.get_untracked();
        spawn_local(async move {
            match ms::open(&text, &state.genesis()).await {
                Ok(a) => account.set(Some(a)),
                Err(e) => state.fail(e),
            }
        });
    };

    let propose = move |_| {
        state.clear();
        let Some(a) = account.get_untracked() else {
            state.fail("set up or open an account first");
            return;
        };
        let (Ok(amount_value), Ok(fee_value)) = (
            typed_amount(&amount.get_untracked()),
            typed_amount(&fee.get_untracked()),
        ) else {
            state.fail("the amount or fee is not a number");
            return;
        };
        let (to, index, node) = (
            recipient.get_untracked(),
            state.selected.get_untracked(),
            state.node_url.get_untracked(),
        );
        busy.set(true);
        spawn_local(async move {
            // The nonce is the multisig account's; the fee terms the node's.
            let result = async {
                let nonce = bridge::fetch_account(&node, &a.address).await?.nonce;
                let terms = ms::fee_options(&node, &a.account_hex).await?;
                ms::propose(
                    index,
                    &a.account_hex,
                    &to,
                    &amount_value,
                    &fee_value,
                    nonce,
                    &terms,
                )
                .await
            }
            .await;
            match result {
                Ok(p) => set_proposal(p),
                Err(e) => state.fail(e),
            }
            busy.set(false);
        });
    };

    let preview = move |_| {
        state.clear();
        let text = proposal.get_untracked();
        spawn_local(async move {
            let genesis = state.genesis();
            // The progress line and Finish describe the proposal in the box,
            // read from its own text, never one handled earlier.
            let result = async {
                let shown = ms::preview(&text, &genesis).await?;
                Ok::<_, String>((shown, ms::status(&text, &genesis).await?))
            }
            .await;
            match result {
                Ok((c, p)) => {
                    review.set(Some(c));
                    progress.set(Some(p));
                }
                Err(e) => state.fail(e),
            }
        });
    };

    let approve = move |_| {
        let Some(shown) = review.get_untracked() else {
            return;
        };
        let (text, index, node) = (
            proposal.get_untracked(),
            state.selected.get_untracked(),
            state.node_url.get_untracked(),
        );
        busy.set(true);
        spawn_local(async move {
            // The core refuses to approve a fee the finished spend would be
            // refused for; the per-byte rate is the node's.
            let result = async {
                let base_fee = bridge::fee_options(&node, index, 0).await?.base_fee;
                ms::approve(
                    index,
                    &text,
                    &state.genesis(),
                    &shown.signing_hash,
                    &base_fee,
                )
                .await
            }
            .await;
            match result {
                Ok(p) => {
                    state.inform(format!(
                        "Approved: {} of {}.",
                        p.approved_by.len(),
                        p.threshold
                    ));
                    set_proposal(p);
                }
                Err(e) => state.fail(e),
            }
            busy.set(false);
        });
    };

    let merge = move |_| {
        state.clear();
        let (mine, theirs) = (proposal.get_untracked(), other.get_untracked());
        spawn_local(async move {
            match ms::merge(&mine, &theirs, &state.genesis()).await {
                Ok(p) => {
                    other.set(String::new());
                    set_proposal(p);
                }
                Err(e) => state.fail(e),
            }
        });
    };

    let finish = move |_| {
        state.clear();
        let text = proposal.get_untracked();
        spawn_local(async move {
            match ms::finish(&text, &state.genesis()).await {
                Ok(t) => signed.set(Some(t)),
                Err(e) => state.fail(e),
            }
        });
    };

    let broadcast = move |_| {
        let Some(t) = signed.get_untracked() else {
            return;
        };
        let node = state.node_url.get_untracked();
        busy.set(true);
        spawn_local(async move {
            match bridge::broadcast(&node, &t.raw_hex).await {
                Ok(txid) => state.inform(format!("Broadcast: {txid}")),
                Err(e) => state.fail(e),
            }
            busy.set(false);
        });
    };

    let account_card = move || {
        account.get().map(|a| {
            let rows = a
                .members
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let short = m.public_key.get(..16).unwrap_or(&m.public_key).to_string();
                    view! {
                        <div><span>{format!("Member {i}")}</span>
                        <span class="mono">{format!("{} \u{b7} {short}\u{2026}", suite_name(m.suite))}</span></div>
                    }
                })
                .collect_view();
            let copy_text = Signal::derive({
                let hex = a.account_hex.clone();
                move || hex.clone()
            });
            view! {
                <div class="summary">
                    <p class="description">
                        {format!("{} of {} members must approve every spend.", a.threshold, a.members.len())}
                    </p>
                    <div><span>"Address"</span><span class="mono address">{a.grouped.clone()}</span></div>
                    {rows}
                </div>
                <CopyButton text=copy_text label="Copy account for the other members"/>
            }
        })
    };

    let review_card = move || {
        review.get().map(|c| {
            view! {
                {c.warnings.into_iter().map(|w| view! { <p class="hint warning">{w}</p> }).collect_view()}
                <div class="summary">
                    <p class="description">{c.description}</p>
                    <div><span>"To"</span><span class="mono">{c.recipient.unwrap_or_default()}</span></div>
                    <div><span>"Amount"</span><span>{c.amount.unwrap_or_default()}</span></div>
                    <div><span>"Fee"</span><span>{c.fee.unwrap_or_default()}</span></div>
                    <div><span>"From (multisig)"</span><span class="mono">{c.sender}</span></div>
                    <div><span>"Network"</span><span>{c.network}</span></div>
                    <div><span>"Nonce"</span><span>{c.nonce}</span></div>
                    <div><span>"Signing hash"</span><span class="mono">{c.signing_hash}</span></div>
                </div>
                <button class="primary wide" disabled=move || busy.get() on:click=approve>
                    "Approve with this account"
                </button>
            }
        })
    };

    let progress_line = move || {
        progress.get().map(|p| {
            let who = p
                .approved_by
                .iter()
                .map(u8::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{} of {} approvals (members: {who}).",
                p.approved_by.len(),
                p.threshold
            )
        })
    };
    let complete = move || progress.get().is_some_and(|p| p.complete);
    let my_key_text = Signal::derive(move || my_key.get());
    let proposal_text = Signal::derive(move || proposal.get());

    view! {
        <section class="card multisig enter">
            <h1>"Multisig"</h1>
            <p class="lede">
                "An account that needs several keys to spend. Every approval it needs must \
                 come from a wallet like this one: a Ledger or another suite cannot approve \
                 yet, so the wallet refuses an account that would depend on one."
            </p>

            <h2>"1. Your member key"</h2>
            <p class="hint">"Give this to whoever sets the account up. It is a public key."</p>
            <div class="copy-row">
                <button class="ghost" on:click=show_key>"Show my key"</button>
                <Show when=move || !my_key.get().is_empty()>
                    <CopyButton text=my_key_text label="Copy my key"/>
                </Show>
            </div>

            <h2>"2. Set up or open the account"</h2>
            <label>"Member keys, one per line (suite:key)"</label>
            <textarea class="mono" rows="4" prop:value=move || members.get()
                on:input=move |ev| members.set(event_target_value(&ev))></textarea>
            <label>"Approvals needed"</label>
            <input inputmode="numeric" prop:value=move || threshold.get()
                on:input=move |ev| threshold.set(event_target_value(&ev))/>
            <button class="primary wide" on:click=create>"Create account"</button>
            <label>"Or paste an account another member shared"</label>
            <textarea class="mono" rows="2" prop:value=move || account_hex.get()
                on:input=move |ev| account_hex.set(event_target_value(&ev))></textarea>
            <button class="ghost wide" on:click=open>"Open account"</button>
            {account_card}

            <h2>"3. Propose a spend"</h2>
            <label>"Recipient"</label>
            <input class="mono" prop:value=move || recipient.get()
                on:input=move |ev| recipient.set(event_target_value(&ev))/>
            <label>"Amount (MAYA)"</label>
            <input inputmode="decimal" prop:value=move || amount.get()
                on:input=move |ev| amount.set(event_target_value(&ev))/>
            <label>"Fee (MAYA)"</label>
            <input inputmode="decimal" prop:value=move || fee.get()
                on:input=move |ev| fee.set(event_target_value(&ev))/>
            <button class="ghost wide" disabled=move || busy.get() on:click=propose>"Propose"</button>

            <h2>"4. Approve, collect, finish"</h2>
            <label>"Proposal (pass this to each member in turn)"</label>
            <textarea class="mono" rows="3" prop:value=move || proposal.get()
                on:input=move |ev| {
                    proposal.set(event_target_value(&ev));
                    progress.set(None);
                    review.set(None);
                    signed.set(None);
                }></textarea>
            <p class="hint">{progress_line}</p>
            <div class="copy-row">
                <button class="ghost" on:click=preview>"Review"</button>
                <CopyButton text=proposal_text label="Copy proposal"/>
            </div>
            {review_card}
            <label>"Another member's copy, to merge"</label>
            <textarea class="mono" rows="2" prop:value=move || other.get()
                on:input=move |ev| other.set(event_target_value(&ev))></textarea>
            <button class="ghost wide" on:click=merge>"Merge approvals"</button>
            <Show when=complete>
                <button class="primary wide" on:click=finish>"Finish"</button>
            </Show>
            {move || signed.get().map(|t| view! {
                <div class="raw">
                    <label>"Signed transaction"</label>
                    <textarea class="mono" rows="4" readonly>{t.raw_hex}</textarea>
                </div>
                <button class="primary wide" disabled=move || busy.get() on:click=broadcast>
                    "Broadcast now"
                </button>
            })}

            <button class="ghost wide" on:click=move |_| {
                state.clear();
                state.screen.set(Screen::Wallet);
            }>"Back"</button>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::parse_members;

    #[test]
    fn member_lines_read_their_suite_prefix() {
        let keys = parse_members("30:aa\n\n 10:bb \nCC").expect("parse");
        let got: Vec<(u8, &str)> = keys
            .iter()
            .map(|k| (k.suite, k.public_key.as_str()))
            .collect();
        assert_eq!(got, [(0x30, "aa"), (0x10, "bb"), (0x30, "CC")]);
        assert!(parse_members("zz:aa").is_err());
    }
}
