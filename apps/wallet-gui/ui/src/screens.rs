//! Wallet screens.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::bridge;
use crate::{AppState, Screen};

/// Shortens a hex string for display.
pub fn short(hash: &str) -> String {
    if hash.len() <= 18 {
        return hash.to_string();
    }
    format!("{}…{}", &hash[..10], &hash[hash.len() - 6..])
}

// ---------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------

/// First run: create a wallet or recover an existing one.
pub mod setup {
    use super::*;

    /// Create-or-recover screen.
    #[component]
    pub fn Setup() -> impl IntoView {
        let state = expect_context::<AppState>();
        let passphrase = RwSignal::new(String::new());
        let confirm = RwSignal::new(String::new());
        let phrase = RwSignal::new(String::new());
        let recovering = RwSignal::new(false);
        let busy = RwSignal::new(false);

        // Validated live, so a mistyped word is caught while it is still easy
        // to fix rather than after the wallet is created.
        let phrase_ok = RwSignal::new(false);
        Effect::new(move |_| {
            let candidate = phrase.get();
            if candidate.split_whitespace().count() < 12 {
                phrase_ok.set(false);
                return;
            }
            spawn_local(async move {
                phrase_ok.set(bridge::check_phrase(&candidate).await.unwrap_or(false));
            });
        });

        let create = move |_| {
            if passphrase.get() != confirm.get() {
                state.fail("The passphrases do not match.");
                return;
            }
            if passphrase.get().is_empty() {
                state.fail("A passphrase is required.");
                return;
            }
            busy.set(true);
            state.clear();
            spawn_local(async move {
                match bridge::create_wallet("primary", &passphrase.get()).await {
                    Ok(created) => {
                        state.accounts.set(vec![created.account]);
                        state.screen.set(Screen::Backup(created.mnemonic));
                    }
                    Err(error) => state.fail(error),
                }
                busy.set(false);
            });
        };

        let recover = move |_| {
            if passphrase.get() != confirm.get() {
                state.fail("The passphrases do not match.");
                return;
            }
            busy.set(true);
            state.clear();
            spawn_local(async move {
                match bridge::recover_wallet("primary", &phrase.get(), &passphrase.get()).await {
                    Ok(account) => {
                        state.accounts.set(vec![account]);
                        state.inform("Wallet recovered.");
                        state.screen.set(Screen::Wallet);
                    }
                    Err(error) => state.fail(error),
                }
                busy.set(false);
            });
        };

        view! {
            <section class="card">
                <h1>"Set up your wallet"</h1>
                <p class="muted">
                    "Your keys are encrypted with this passphrase and stored in your \
                     operating system's keychain. They never leave this device."
                </p>

                <label>"Passphrase"</label>
                <input
                    type="password"
                    prop:value=move || passphrase.get()
                    on:input=move |ev| passphrase.set(event_target_value(&ev))
                />

                <label>"Confirm passphrase"</label>
                <input
                    type="password"
                    prop:value=move || confirm.get()
                    on:input=move |ev| confirm.set(event_target_value(&ev))
                />

                <Show when=move || recovering.get()>
                    <label>"Recovery phrase"</label>
                    <textarea
                        rows="4"
                        placeholder="twenty four words, separated by spaces"
                        prop:value=move || phrase.get()
                        on:input=move |ev| phrase.set(event_target_value(&ev))
                    ></textarea>
                    <p class=move || if phrase_ok.get() { "hint ok" } else { "hint" }>
                        {move || {
                            let count = phrase.get().split_whitespace().count();
                            if count == 0 {
                                "Enter your recovery phrase.".to_string()
                            } else if phrase_ok.get() {
                                format!("{count} words · checksum valid")
                            } else {
                                format!("{count} words · not yet a valid phrase")
                            }
                        }}
                    </p>
                </Show>

                <div class="row">
                    <Show
                        when=move || !recovering.get()
                        fallback=move || view! {
                            <button
                                class="primary"
                                disabled=move || busy.get() || !phrase_ok.get()
                                on:click=recover
                            >
                                "Recover wallet"
                            </button>
                        }
                    >
                        <button class="primary" disabled=move || busy.get() on:click=create>
                            "Create new wallet"
                        </button>
                    </Show>
                </div>

                <button
                    class="ghost wide"
                    on:click=move |_| {
                        state.clear();
                        recovering.update(|value| *value = !*value);
                    }
                >
                    {move || if recovering.get() {
                        "Create a new wallet instead"
                    } else {
                        "I already have a recovery phrase"
                    }}
                </button>
            </section>
        }
    }
}

// ---------------------------------------------------------------------------
// backup
// ---------------------------------------------------------------------------

/// One-time display of a new recovery phrase.
pub mod backup {
    use super::*;

    /// Shows the phrase once and requires an explicit acknowledgement.
    #[component]
    pub fn Backup(phrase: String) -> impl IntoView {
        let state = expect_context::<AppState>();
        let acknowledged = RwSignal::new(false);
        let words: Vec<String> = phrase.split_whitespace().map(str::to_string).collect();

        let numbered: Vec<_> = words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                view! {
                    <li><span class="index">{index + 1}</span>{word.clone()}</li>
                }
            })
            .collect();

        view! {
            <section class="card">
                <h1>"Write this down"</h1>
                <p class="warn">
                    "These twenty four words are the only way to recover this wallet. \
                     They are shown once and cannot be displayed again."
                </p>

                <ol class="phrase">{numbered}</ol>

                <label class="check">
                    <input
                        type="checkbox"
                        prop:checked=move || acknowledged.get()
                        on:change=move |ev| acknowledged.set(event_target_checked(&ev))
                    />
                    "I have written down my recovery phrase and stored it safely."
                </label>

                <button
                    class="primary wide"
                    disabled=move || !acknowledged.get()
                    on:click=move |_| {
                        state.inform("Wallet created.");
                        state.screen.set(Screen::Wallet);
                    }
                >
                    "Continue"
                </button>
            </section>
        }
    }
}

// ---------------------------------------------------------------------------
// unlock
// ---------------------------------------------------------------------------

/// Passphrase prompt for a stored wallet.
pub mod unlock {
    use super::*;

    /// Unlock screen.
    #[component]
    pub fn Unlock() -> impl IntoView {
        let state = expect_context::<AppState>();
        let passphrase = RwSignal::new(String::new());
        let busy = RwSignal::new(false);

        let submit = move || {
            busy.set(true);
            state.clear();
            spawn_local(async move {
                match bridge::unlock("primary", &passphrase.get(), 3).await {
                    Ok(accounts) => {
                        state.accounts.set(accounts);
                        passphrase.set(String::new());
                        state.screen.set(Screen::Wallet);
                    }
                    Err(error) => state.fail(error),
                }
                busy.set(false);
            });
        };

        view! {
            <section class="card">
                <h1>"Unlock"</h1>
                <label>"Passphrase"</label>
                <input
                    type="password"
                    prop:value=move || passphrase.get()
                    on:input=move |ev| passphrase.set(event_target_value(&ev))
                    on:keydown=move |ev| { if ev.key() == "Enter" { submit(); } }
                />
                <button
                    class="primary wide"
                    disabled=move || busy.get()
                    on:click=move |_| submit()
                >
                    {move || if busy.get() { "Unlocking…" } else { "Unlock" }}
                </button>
                <p class="muted small">
                    "Unlocking runs a memory-hard key derivation, so it takes a moment."
                </p>
            </section>
        }
    }
}

// ---------------------------------------------------------------------------
// wallet
// ---------------------------------------------------------------------------

/// The main wallet view: accounts, balance, history.
pub mod wallet {
    use super::*;

    /// Wallet home.
    #[component]
    pub fn WalletView() -> impl IntoView {
        let state = expect_context::<AppState>();

        let refresh = move || {
            spawn_local(async move {
                let accounts = state.accounts.get_untracked();
                let index = state.selected.get_untracked() as usize;
                let Some(account) = accounts.get(index) else {
                    return;
                };
                match bridge::fetch_account(&state.node_url.get_untracked(), &account.address).await
                {
                    Ok(info) => state.account_state.set(info),
                    // A node being unreachable is an ordinary offline state,
                    // not an error worth a red banner every few seconds.
                    Err(error) => state.fail(format!("Could not reach the node: {error}")),
                }
                if let Ok(history) = bridge::history().await {
                    state.history.set(history);
                }
            });
        };

        Effect::new(move |_| {
            let _ = state.selected.get();
            refresh();
        });

        // The account list is a selector, so without this the wallet could only
        // ever show the one account derived at setup.
        let adding = RwSignal::new(false);
        let add_account = move |_| {
            adding.set(true);
            state.clear();
            spawn_local(async move {
                // Derive the next unused index rather than `len()`, so a gap in
                // the derived set cannot silently re-derive an existing account.
                let next = state
                    .accounts
                    .get_untracked()
                    .iter()
                    .map(|account| account.index + 1)
                    .max()
                    .unwrap_or(0);
                match bridge::derive_account(next).await {
                    // Re-read from the core instead of pushing locally: the core
                    // owns the account set, and this keeps the two in step.
                    Ok(_) => match bridge::list_accounts().await {
                        Ok(accounts) => {
                            state.accounts.set(accounts);
                            state.selected.set(next);
                        }
                        Err(error) => state.fail(error),
                    },
                    Err(error) => state.fail(error),
                }
                adding.set(false);
            });
        };

        let account_rows = move || {
            state
                .accounts
                .get()
                .into_iter()
                .map(|account| {
                    let index = account.index;
                    let address = account.address.clone();
                    view! {
                        <button
                            class=move || if state.selected.get() == index {
                                "account selected"
                            } else {
                                "account"
                            }
                            on:click=move |_| state.selected.set(index)
                        >
                            <span class="label">{format!("Account {index}")}</span>
                            <span class="mono">{short(&address)}</span>
                        </button>
                    }
                })
                .collect::<Vec<_>>()
        };

        let history_rows = move || {
            let entries = state.history.get();
            if entries.is_empty() {
                return view! {
                    <p class="muted small">"Nothing signed yet this session."</p>
                }
                .into_any();
            }

            entries
                .into_iter()
                .map(|entry| {
                    let status = entry.status.clone();
                    let pill_class = format!("pill {}", status.to_lowercase());
                    let detail = entry.detail.clone().unwrap_or_default();
                    let has_detail = !detail.is_empty();
                    view! {
                        <div class="tx">
                            <div class="tx-head">
                                <span class="mono">{short(&entry.transfer.txid)}</span>
                                <span class=pill_class>{status}</span>
                            </div>
                            <div class="tx-body">
                                <span>{format!("→ {}", short(&entry.transfer.recipient))}</span>
                                <span class="amount">
                                    {format!(
                                        "{} (+{} fee)",
                                        entry.transfer.amount, entry.transfer.fee,
                                    )}
                                </span>
                            </div>
                            <Show when=move || has_detail>
                                <p class="hint">{detail.clone()}</p>
                            </Show>
                        </div>
                    }
                })
                .collect::<Vec<_>>()
                .into_any()
        };

        view! {
            <section class="card">
                <div class="balance">
                    <span class="label">"Balance"</span>
                    <span class="value">{move || state.account_state.get().balance}</span>
                    <span class="sub">
                        {move || format!("next nonce {}", state.account_state.get().nonce)}
                    </span>
                </div>

                <div class="accounts">{account_rows}</div>
                <button
                    class="ghost wide"
                    disabled=move || adding.get()
                    on:click=add_account
                >
                    {move || if adding.get() { "Deriving…" } else { "Add account" }}
                </button>

                <div class="row">
                    <button
                        class="primary"
                        on:click=move |_| {
                            state.clear();
                            state.screen.set(Screen::Scan);
                        }
                    >
                        "Scan to pay"
                    </button>
                    <button
                        class="secondary"
                        on:click=move |_| {
                            state.clear();
                            state.screen.set(Screen::Send(bridge::PaymentRequest::default()));
                        }
                    >
                        "Send"
                    </button>
                </div>

                <label>"Node endpoint"</label>
                <input
                    class="mono"
                    prop:value=move || state.node_url.get()
                    on:input=move |ev| state.node_url.set(event_target_value(&ev))
                />
                <p class="hint">
                    "Maya2C public testnet by default. Testnet coins have no value.                      For your own node, use http://127.0.0.1:8545."
                </p>
                <button class="ghost wide" on:click=move |_| refresh()>"Refresh"</button>
            </section>

            <section class="card">
                <h2>"Pending transactions"</h2>
                {history_rows}
            </section>
        }
    }
}

// ---------------------------------------------------------------------------
// scan
// ---------------------------------------------------------------------------

/// Camera QR scanner.
pub mod scan {
    use super::*;
    use crate::camera;

    const VIDEO_ID: &str = "scanner-video";
    const CANVAS_ID: &str = "scanner-canvas";

    /// Scanner screen.
    #[component]
    pub fn Scan() -> impl IntoView {
        let state = expect_context::<AppState>();
        let starting = RwSignal::new(true);
        let manual = RwSignal::new(String::new());

        Effect::new(move |_| {
            spawn_local(async move {
                if let Err(error) = camera::start(VIDEO_ID).await {
                    state.fail(format!("{error}. You can paste an address instead."));
                }
                starting.set(false);
            });
        });

        let capture = move |_| {
            state.clear();
            match camera::capture_frame(VIDEO_ID, CANVAS_ID) {
                Ok(png) => spawn_local(async move {
                    // Decoded and parsed in Rust: a QR payload is
                    // attacker-controlled input.
                    match bridge::scan_qr(&png).await {
                        Ok(request) => {
                            camera::stop(VIDEO_ID);
                            state.screen.set(Screen::Send(request));
                        }
                        Err(error) => state.fail(error),
                    }
                }),
                Err(error) => state.fail(error),
            }
        };

        let use_manual = move |_| {
            let input = manual.get();
            state.clear();
            spawn_local(async move {
                match bridge::parse_payment_uri(&input).await {
                    Ok(request) => {
                        camera::stop(VIDEO_ID);
                        state.screen.set(Screen::Send(request));
                    }
                    Err(error) => state.fail(error),
                }
            });
        };

        view! {
            <section class="card">
                <h1>"Scan a payment code"</h1>
                <div class="viewfinder">
                    <video id=VIDEO_ID autoplay playsinline muted></video>
                    <canvas id=CANVAS_ID class="hidden"></canvas>
                    <div class="reticle"></div>
                </div>

                <button class="primary wide" on:click=capture>"Capture"</button>

                <label>"Or paste an address"</label>
                <input
                    class="mono"
                    placeholder="maya:… or a 64-character address"
                    prop:value=move || manual.get()
                    on:input=move |ev| manual.set(event_target_value(&ev))
                />
                <button class="secondary wide" on:click=use_manual>"Use this address"</button>

                <button
                    class="ghost wide"
                    on:click=move |_| {
                        camera::stop(VIDEO_ID);
                        state.clear();
                        state.screen.set(Screen::Wallet);
                    }
                >
                    "Cancel"
                </button>
            </section>
        }
    }
}

// ---------------------------------------------------------------------------
// send
// ---------------------------------------------------------------------------

/// Compose, confirm, sign, and broadcast a transfer.
pub mod send {
    use super::*;

    /// Send screen.
    #[component]
    pub fn Send(request: bridge::PaymentRequest) -> impl IntoView {
        let state = expect_context::<AppState>();

        let recipient = RwSignal::new(request.recipient.clone());
        let amount = RwSignal::new(request.amount.map(|a| a.to_string()).unwrap_or_default());
        let fee = RwSignal::new(String::new());
        let label = request.label.clone();
        let preview = RwSignal::new(None::<bridge::TransferPreview>);
        let signed = RwSignal::new(None::<bridge::SignedTransfer>);
        let busy = RwSignal::new(false);
        let fee_terms = RwSignal::new(bridge::FeeTerms::default());

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
                            fee.set(standard.fee.to_string());
                        }
                        fee_terms.set(terms);
                    }
                    Err(error) => state.fail(format!("Could not read fees from the node: {error}")),
                }
            });
        });

        let build_preview = move |_| {
            let Ok(amount_value) = amount.get().parse::<u64>() else {
                state.fail("Amount must be a whole number.");
                return;
            };
            let Ok(fee_value) = fee.get().parse::<u64>() else {
                state.fail("Fee must be a whole number.");
                return;
            };
            state.clear();
            let index = state.selected.get();
            let nonce = state.account_state.get().nonce;
            let to = recipient.get();

            spawn_local(async move {
                match bridge::preview_transfer(index, &to, amount_value, fee_value, nonce).await {
                    Ok(summary) => preview.set(Some(summary)),
                    Err(error) => state.fail(error),
                }
            });
        };

        let sign = move |_| {
            let Some(summary) = preview.get() else {
                return;
            };
            busy.set(true);
            spawn_local(async move {
                let collector = fee_terms.get_untracked().collector;
                match bridge::sign_transfer(
                    state.selected.get_untracked(),
                    &summary.recipient,
                    summary.amount,
                    summary.fee,
                    collector.as_deref(),
                    summary.nonce,
                )
                .await
                {
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

        let send_now = move |_| {
            let Some(transfer) = signed.get() else {
                return;
            };
            busy.set(true);
            spawn_local(async move {
                let outcome =
                    bridge::broadcast(&state.node_url.get_untracked(), &transfer.raw_hex).await;
                match outcome {
                    Ok(txid) => {
                        let _ = bridge::record_outcome(&transfer.txid, true, None).await;
                        state.inform(format!("Broadcast. Transaction {}", short(&txid)));
                        if let Ok(history) = bridge::history().await {
                            state.history.set(history);
                        }
                        state.screen.set(Screen::Wallet);
                    }
                    Err(error) => {
                        let _ = bridge::record_outcome(&transfer.txid, false, Some(error.clone()))
                            .await;
                        state.fail(error);
                    }
                }
                busy.set(false);
            });
        };

        let fee_buttons = move || {
            fee_terms
                .get()
                .options
                .into_iter()
                .map(|option| {
                    let value = option.fee;
                    view! {
                        <button
                            class=move || if fee.get() == value.to_string() {
                                "chip selected"
                            } else {
                                "chip"
                            }
                            on:click=move |_| fee.set(value.to_string())
                        >
                            {option.label.clone()}
                            <span class="chip-value">{value}</span>
                        </button>
                    }
                })
                .collect::<Vec<_>>()
        };

        view! {
            <section class="card">
                <h1>"Send"</h1>

                <Show when={let label = label.clone(); move || label.is_some()}>
                    <p class="hint">
                        {
                            let label = label.clone().unwrap_or_default();
                            format!("Requested by \u{201c}{label}\u{201d} — this name is \
                                     supplied by the payee and is not verified.")
                        }
                    </p>
                </Show>

                <label>"Recipient"</label>
                <input
                    class="mono"
                    prop:value=move || recipient.get()
                    on:input=move |ev| recipient.set(event_target_value(&ev))
                />

                <label>"Amount"</label>
                <input
                    type="number"
                    min="0"
                    prop:value=move || amount.get()
                    on:input=move |ev| amount.set(event_target_value(&ev))
                />

                <label>"Network fee"</label>
                <div class="chips">{fee_buttons}</div>
                <input
                    type="number"
                    min="0"
                    prop:value=move || fee.get()
                    on:input=move |ev| fee.set(event_target_value(&ev))
                />
                <p class="hint">
                    {move || if fee_terms.get().collector.is_some() {
                        "The network burns the base fee; anything above it tips the                          validators. Below Economy, the node refuses the transfer."
                    } else {
                        "This network charges no fee. Any fee you set is burned."
                    }}
                </p>

                <Show
                    when=move || preview.get().is_some()
                    fallback=move || view! {
                        <button class="primary wide" on:click=build_preview>"Review"</button>
                    }
                >
                    {move || {
                        let summary = preview.get().expect("checked");
                        let is_signed = signed.get().is_some();
                        let fee_is_large = summary.fee > summary.amount;
                        view! {
                            <div class="summary">
                                <div><span>"From"</span><span class="mono">{short(&summary.sender)}</span></div>
                                <div><span>"To"</span><span class="mono">{short(&summary.recipient)}</span></div>
                                <div><span>"Amount"</span><span>{summary.amount}</span></div>
                                <div><span>"Fee"</span><span>{summary.fee}</span></div>
                                <div class="total"><span>"Total"</span><span>{summary.total}</span></div>
                                <div><span>"Nonce"</span><span>{summary.nonce}</span></div>
                            </div>
                            // The collector itself is checked in the wallet core against
                            // the chain's fixed address; the size of the fee is the user's
                            // to judge, so an unusual one is called out before signing.
                            <Show when=move || fee_is_large>
                                <p class="hint warning">
                                    "The fee is larger than the amount you are sending.                                      Check it before signing."
                                </p>
                            </Show>

                            <Show
                                when=move || is_signed
                                fallback=move || view! {
                                    <button
                                        class="primary wide"
                                        disabled=move || busy.get()
                                        on:click=sign
                                    >
                                        "Sign offline"
                                    </button>
                                }
                            >
                                <div class="raw">
                                    <label>"Signed transaction"</label>
                                    <textarea class="mono" rows="4" readonly>
                                        {move || signed.get().map(|t| t.raw_hex).unwrap_or_default()}
                                    </textarea>
                                    <p class="hint">
                                        "This hex is a complete transaction. It can be copied to \
                                         an online device and broadcast there, so this machine \
                                         never needs a network connection."
                                    </p>
                                </div>
                                <button
                                    class="primary wide"
                                    disabled=move || busy.get()
                                    on:click=send_now
                                >
                                    "Broadcast now"
                                </button>
                            </Show>
                        }
                    }}
                </Show>

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
}
