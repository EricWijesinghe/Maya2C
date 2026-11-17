//! Wallet screens. Send is in `send.rs`, Receive and Sent in `receive.rs`.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::amount;
use crate::bridge;
use crate::widgets::{PausedCard, SecretField, TestnetTag};
use crate::{AppState, Screen};

/// Shortens a hex string for display.
pub fn short(hash: &str) -> String {
    if hash.len() <= 18 {
        return hash.to_string();
    }
    format!("{}…{}", &hash[..10], &hash[hash.len() - 6..])
}

/// Groups digits in threes: `1234567` → `1,234,567`.
///
/// For counts only (heights, nonces). Amounts go through `show_amount`, which
/// formats whole coins at 24 decimals; this would drop the decimal point.
pub fn group_digits(n: impl std::fmt::Display) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A base-unit decimal string from IPC as the whole-coin amount a person
/// reads. Empty means "not loaded yet" and reads as zero, as it always has.
pub fn show_amount(base: &str) -> String {
    match amount::parse_base_units(base) {
        Ok(value) => amount::format_display(value),
        Err(amount::AmountError::Empty) => amount::format_display(primitive_types::U256::zero()),
        Err(_) => "invalid amount".to_string(),
    }
}

/// A display amount without its trailing zeros: `1,000.500000` → `1,000.5`,
/// `2.000000` → `2`. Exact: only zeros after the point are dropped.
pub fn trim_display(display: &str) -> String {
    if !display.contains('.') {
        return display.to_string();
    }
    display
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// [`show_amount`] without trailing zeros: what a hero figure shows.
pub fn pretty_amount(base: &str) -> String {
    trim_display(&show_amount(base))
}

/// A base-unit decimal string as a plain value an input field can hold.
pub fn input_amount(base: &str) -> String {
    amount::parse_base_units(base).map_or_else(|_| String::new(), amount::format_input)
}

/// What someone typed, a whole-coin decimal, as the base-unit string IPC takes.
/// Rejected text is an error the form shows, never a rounded value.
pub fn typed_amount(text: &str) -> Result<String, amount::AmountError> {
    amount::parse_display(text.trim()).map(|value| value.to_string())
}

/// An address in groups of four, so a person can compare it group by group.
pub fn chunks(address: &str) -> Vec<String> {
    address
        .chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|group| group.iter().collect())
        .collect()
}

// ---------------------------------------------------------------------------
// setup
// ---------------------------------------------------------------------------

/// First run: a welcome with two choices, then create or recover.
pub mod setup {
    use super::*;

    /// Which part of setup is showing.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Step {
        Welcome,
        Create,
        Recover,
    }

    /// Setup; `from_unlock` opens it on the recovery form.
    #[component]
    pub fn Setup(#[prop(optional)] from_unlock: bool) -> impl IntoView {
        let state = expect_context::<AppState>();
        let step = RwSignal::new(if from_unlock {
            Step::Recover
        } else {
            Step::Welcome
        });
        let passphrase = RwSignal::new(String::new());
        let confirm = RwSignal::new(String::new());
        let phrase = RwSignal::new(String::new());
        let phrase_shown = RwSignal::new(false);
        let busy = RwSignal::new(false);
        let mismatch =
            Signal::derive(move || !confirm.get().is_empty() && passphrase.get() != confirm.get());

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

        let create = move || {
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

        let recover = move || {
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

        let go = move |next: Step| {
            state.clear();
            step.set(next);
        };

        let welcome = move || {
            view! {
                <section class="welcome enter">
                    // Decorative: the heading beside it names the app.
                    <img class="mark" src="assets/emblem.png" alt="" width="96" height="96"/>
                    <h1>"Maya2C Wallet"</h1>
                    <p class="lede">
                        "Your keys are encrypted with a passphrase you choose and kept in \
                         this device's keychain. They never leave it."
                    </p>
                    <TestnetTag/>
                    <div class="choices">
                        <button class="primary" on:click=move |_| go(Step::Create)>
                            "Create a new wallet"
                        </button>
                        <button class="secondary" on:click=move |_| go(Step::Recover)>
                            "I have a recovery phrase"
                        </button>
                    </div>
                </section>
            }
        };

        let form = move || {
            let recovering = step.get() == Step::Recover;
            view! {
                <section class="card enter">
                    <h1>{if recovering { "Recover a wallet" } else { "Create a wallet" }}</h1>
                    <p class="muted">
                        "The passphrase encrypts your keys on this device. It is not \
                         sent anywhere, and nobody can reset it for you."
                    </p>
                    <TestnetTag/>

                    <Show when=move || recovering>
                        <div class="field">
                            <label for="setup-phrase">"Recovery phrase"</label>
                            <div class="secret">
                                <textarea
                                    id="setup-phrase"
                                    rows="4"
                                    class=move || if phrase_shown.get() { "mono" } else { "mono concealed" }
                                    autocomplete="off"
                                    spellcheck="false"
                                    autocapitalize="off"
                                    aria-describedby="setup-phrase-hint"
                                    placeholder="twenty four words, separated by spaces"
                                    prop:value=move || phrase.get()
                                    on:input=move |ev| phrase.set(event_target_value(&ev))
                                ></textarea>
                                <button
                                    type="button"
                                    class="reveal"
                                    aria-pressed=move || if phrase_shown.get() { "true" } else { "false" }
                                    aria-label=move || if phrase_shown.get() {
                                        "Hide recovery phrase"
                                    } else {
                                        "Show recovery phrase"
                                    }
                                    on:click=move |_| phrase_shown.update(|s| *s = !*s)
                                >
                                    {move || if phrase_shown.get() { "Hide" } else { "Show" }}
                                </button>
                            </div>
                            <p
                                id="setup-phrase-hint"
                                class=move || if phrase_ok.get() { "hint ok" } else { "hint" }
                                aria-live="polite"
                            >
                                {move || {
                                    let count = phrase.get().split_whitespace().count();
                                    if count == 0 {
                                        "Enter your recovery phrase.".to_string()
                                    } else if phrase_ok.get() {
                                        format!("{count} words \u{b7} checksum valid")
                                    } else {
                                        format!("{count} words \u{b7} not yet a valid phrase")
                                    }
                                }}
                            </p>
                        </div>
                    </Show>

                    <SecretField
                        id="setup-pass"
                        label="Passphrase"
                        value=passphrase
                        autocomplete="new-password"
                        show_strength=true
                    />
                    <SecretField
                        id="setup-confirm"
                        label="Confirm passphrase"
                        value=confirm
                        autocomplete="new-password"
                        invalid=mismatch
                        on_enter=Callback::new(move |()| {
                            if recovering { recover() } else { create() }
                        })
                    />
                    <Show when=move || mismatch.get()>
                        <p class="hint warning" role="status">"The passphrases do not match yet."</p>
                    </Show>

                    {if recovering {
                        view! {
                            <button
                                class="primary wide"
                                disabled=move || busy.get() || !phrase_ok.get()
                                on:click=move |_| recover()
                            >
                                "Recover wallet"
                            </button>
                        }
                        .into_any()
                    } else {
                        view! {
                            <button
                                class="primary wide"
                                disabled=move || busy.get()
                                on:click=move |_| create()
                            >
                                {move || if busy.get() { "Creating…" } else { "Create wallet" }}
                            </button>
                        }
                        .into_any()
                    }}
                    {if from_unlock {
                        view! {
                            <button
                                class="ghost wide"
                                on:click=move |_| {
                                    state.clear();
                                    state.screen.set(Screen::Unlock);
                                }
                            >
                                "Back to unlock"
                            </button>
                        }
                        .into_any()
                    } else {
                        view! {
                            <button class="ghost wide" on:click=move |_| go(Step::Welcome)>
                                "Back"
                            </button>
                        }
                        .into_any()
                    }}
                </section>
            }
        };

        move || {
            if step.get() == Step::Welcome {
                welcome().into_any()
            } else {
                form().into_any()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// backup
// ---------------------------------------------------------------------------

/// One-time display of a new recovery phrase, then a check of three words.
pub mod backup {
    use super::*;

    /// How many words the check asks for.
    const CHECKED_WORDS: usize = 3;

    /// `count` distinct word positions out of `total`, in ascending order.
    /// Not secret, so `Math.random` is enough: the point is that the user
    /// cannot know in advance which words will be asked.
    fn pick(total: usize, count: usize) -> Vec<usize> {
        let mut chosen: Vec<usize> = Vec::with_capacity(count);
        while chosen.len() < count.min(total) {
            // Truncation is the intent: a uniform index in 0..total.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let index = (js_sys::Math::random() * total as f64) as usize;
            if index < total && !chosen.contains(&index) {
                chosen.push(index);
            }
        }
        chosen.sort_unstable();
        chosen
    }

    /// Shows the phrase once, asks for an acknowledgement, then for three of
    /// its words before the wallet opens.
    #[component]
    pub fn Backup(phrase: String) -> impl IntoView {
        let state = expect_context::<AppState>();
        let acknowledged = RwSignal::new(false);
        let checking = RwSignal::new(false);
        let words: Vec<String> = phrase.split_whitespace().map(str::to_string).collect();
        let asked = pick(words.len(), CHECKED_WORDS);
        let answers: Vec<RwSignal<String>> =
            asked.iter().map(|_| RwSignal::new(String::new())).collect();

        let expected: Vec<String> = asked.iter().map(|&i| words[i].clone()).collect();
        let all_right = {
            let answers = answers.clone();
            Signal::derive(move || {
                answers
                    .iter()
                    .zip(&expected)
                    .all(|(answer, word)| answer.get().trim().eq_ignore_ascii_case(word))
            })
        };

        let numbered: Vec<_> = words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                view! {
                    <li><span class="index">{index + 1}</span>{word.clone()}</li>
                }
            })
            .collect();

        let check_fields: Vec<_> = asked
            .iter()
            .zip(answers)
            .map(|(&index, answer)| {
                let id = format!("quiz-{}", index + 1);
                let label = format!("Word {}", index + 1);
                view! {
                    <div class="field">
                        <label for=id.clone()>{label}</label>
                        <input
                            id=id
                            class="mono"
                            autocomplete="off"
                            spellcheck="false"
                            autocapitalize="off"
                            prop:value=move || answer.get()
                            on:input=move |ev| answer.set(event_target_value(&ev))
                        />
                    </div>
                }
            })
            .collect();

        let show_phrase = view! {
            <section class="card enter">
                <ol class="steps" aria-label="Step 1 of 2">
                    <li class="done"></li><li></li>
                </ol>
                <h1>"Write down your recovery phrase"</h1>
                <TestnetTag/>
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
                    on:click=move |_| checking.set(true)
                >
                    "Continue"
                </button>
            </section>
        };

        let check = view! {
            <section class="card enter">
                <ol class="steps" aria-label="Step 2 of 2">
                    <li class="done"></li><li class="done"></li>
                </ol>
                <h1>"Check your backup"</h1>
                <TestnetTag/>
                <p class="muted">
                    "Type these three words from what you wrote down. The wallet \
                     opens once all three match."
                </p>
                <div class="quiz">{check_fields}</div>
                <button
                    class="primary wide"
                    disabled=move || !all_right.get()
                    on:click=move |_| {
                        state.inform("Wallet created.");
                        state.screen.set(Screen::Wallet);
                    }
                >
                    "Open wallet"
                </button>
                <button class="ghost wide" on:click=move |_| checking.set(false)>
                    "Show the words again"
                </button>
            </section>
        };

        // Both halves stay mounted and one is hidden, so typed answers and
        // the phrase survive a look back.
        view! {
            <div class:hidden=move || checking.get()>{show_phrase}</div>
            <div class:hidden=move || !checking.get()>{check}</div>
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
            <section class="card enter">
                <h1>"Unlock"</h1>
                <SecretField
                    id="unlock-pass"
                    label="Passphrase"
                    value=passphrase
                    autocomplete="current-password"
                    on_enter=Callback::new(move |()| submit())
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
                // Without this, a stored wallet hides recovery entirely: Setup
                // is shown only when no wallet exists.
                <button
                    class="ghost wide"
                    on:click=move |_| {
                        state.clear();
                        state.screen.set(Screen::Recover);
                    }
                >
                    "Forgot passphrase? Recover with your recovery phrase"
                </button>
            </section>
        }
    }
}

// ---------------------------------------------------------------------------
// wallet
// ---------------------------------------------------------------------------

/// The main wallet view: accounts, balance, history, settings.
pub mod wallet {
    use super::*;
    use crate::theme::{self, Theme};

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
                    Ok(info) => {
                        state.account_state.set(info);
                        state.node_error.set(None);
                    }
                    // An unreachable or paused node is an ordinary state: the
                    // calm card, with the raw error under its Details.
                    Err(error) => state.node_error.set(Some(error)),
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
        // The header re-reads the network every few seconds; once it answers
        // again, so does the balance, without the user pressing anything.
        Effect::new(move |_| {
            if state.network.get().is_some() && state.node_error.get_untracked().is_some() {
                refresh();
            }
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
                            aria-pressed=move || if state.selected.get() == index { "true" } else { "false" }
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
                    <div class="empty">
                        <p>"No activity yet."</p>
                        <p class="muted small">
                            "Use Receive to get your address, or ask the faucet on \
                             maya2c.dev for testnet coins."
                        </p>
                    </div>
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
                                        "\u{2212}{} (+{} fee)",
                                        pretty_amount(&entry.transfer.amount),
                                        pretty_amount(&entry.transfer.fee),
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

        let selected_address = move || {
            let index = state.selected.get();
            state
                .accounts
                .get()
                .into_iter()
                .find(|a| a.index == index)
                .map(|a| a.address)
                .unwrap_or_default()
        };
        let go = move |screen: Screen| {
            state.clear();
            state.screen.set(screen);
        };
        // Never a made-up zero: until the node has answered, the balance is
        // a dash.
        let balance_known = move || !state.account_state.get().balance.is_empty();

        let theme_buttons = Theme::ALL
            .into_iter()
            .map(|choice| {
                view! {
                    <button
                        type="button"
                        aria-pressed=move || if state.theme.get() == choice { "true" } else { "false" }
                        on:click=move |_| {
                            theme::save(choice);
                            state.theme.set(choice);
                        }
                    >
                        {choice.label()}
                    </button>
                }
            })
            .collect::<Vec<_>>();

        view! {
            <section class="hero-card enter">
                <div class="hero-top">
                    <span class="label">{move || format!("Account {}", state.selected.get())}</span>
                    <span class="mono faint">{move || short(&selected_address())}</span>
                </div>
                <div class="hero-balance">
                    <span class=move || if balance_known() { "value" } else { "value missing" }>
                        {move || if balance_known() {
                            pretty_amount(&state.account_state.get().balance)
                        } else {
                            "\u{2014}".to_string()
                        }}
                    </span>
                    <span class="unit">{amount::TICKER}</span>
                </div>
                <TestnetTag/>
                <div class="actions">
                    <button
                        class="action"
                        on:click=move |_| go(Screen::Send(bridge::PaymentRequest::default()))
                    >
                        <span class="icon" aria-hidden="true">"\u{2197}"</span>
                        "Send"
                    </button>
                    <button class="action" on:click=move |_| go(Screen::Receive)>
                        <span class="icon" aria-hidden="true">"\u{2199}"</span>
                        "Receive"
                    </button>
                    <button class="action" on:click=move |_| go(Screen::Scan)>
                        <span class="icon" aria-hidden="true">"\u{2b1a}"</span>
                        "Scan"
                    </button>
                </div>
            </section>

            <PausedCard
                title="The network is not answering"
                body="The public testnet may be paused. Your keys stay on this device, \
                      and the balance refreshes by itself when the node answers."
                error=state.node_error
            />

            <section class="card enter">
                <h2>"Accounts"</h2>
                <div class="accounts">{account_rows}</div>
                <button
                    class="ghost wide"
                    disabled=move || adding.get()
                    on:click=add_account
                >
                    {move || if adding.get() { "Deriving…" } else { "Add account" }}
                </button>
            </section>

            <section class="card enter">
                <h2>"Recent activity"</h2>
                {history_rows}
            </section>

            <details class="card settings">
                <summary>"Settings"</summary>
                <span class="field-label" id="theme-label">"Theme"</span>
                <div class="segmented" role="group" aria-labelledby="theme-label">
                    {theme_buttons}
                </div>
                <div class="field">
                    <label for="node-url">"Node endpoint"</label>
                    <input
                        id="node-url"
                        class="mono"
                        autocomplete="off"
                        spellcheck="false"
                        aria-describedby="node-url-hint"
                        prop:value=move || state.node_url.get()
                        on:input=move |ev| state.node_url.set(event_target_value(&ev))
                    />
                </div>
                <p class="hint" id="node-url-hint">
                    "The Maya2C public testnet by default. For your own node, use \
                     http://127.0.0.1:8545."
                </p>
                <button class="ghost wide" on:click=move |_| refresh()>"Refresh"</button>
            </details>
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
            <section class="card enter">
                <h1>"Scan a payment code"</h1>
                <div class="viewfinder">
                    <video id=VIDEO_ID autoplay playsinline muted></video>
                    <canvas id=CANVAS_ID class="hidden"></canvas>
                    <div class="reticle"></div>
                </div>

                <button class="primary wide" on:click=capture>"Capture"</button>

                <div class="field">
                    <label for="scan-manual">"Or paste an address"</label>
                    <input
                        id="scan-manual"
                        class="mono"
                        autocomplete="off"
                        spellcheck="false"
                        placeholder="maya:… or a 64-character address"
                        prop:value=move || manual.get()
                        on:input=move |ev| manual.set(event_target_value(&ev))
                    />
                </div>
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trimming_drops_only_zeros_after_the_point() {
        assert_eq!(trim_display("1,000.500000"), "1,000.5");
        assert_eq!(trim_display("2.000000"), "2");
        assert_eq!(trim_display("1,000"), "1,000");
        assert_eq!(trim_display("0.000000"), "0");
    }

    #[test]
    fn an_address_splits_into_groups_of_four() {
        assert_eq!(chunks("abcdefghij"), vec!["abcd", "efgh", "ij"]);
        assert_eq!(chunks(&"a".repeat(64)).len(), 16);
    }
}
