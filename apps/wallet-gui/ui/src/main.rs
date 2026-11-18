//! Maya2C Wallet frontend.
//!
//! Client-side Leptos with fine-grained signals. Every privileged action is a
//! call into the Rust core through [`bridge`]; nothing here can reach a key,
//! because the core exposes no command that returns one.
//!
//! ## Screens
//!
//! - **Setup** — welcome, then create a wallet or recover one from a BIP-39 phrase
//! - **Backup** — show the recovery phrase once, then ask for three of its words
//! - **Wallet** — accounts, balance, send, and pending history
//! - **Scan** — camera QR capture, decoded in Rust
//! - **Send** — recipient, amount, and manual fee control
//! - **Receive** — the account's address and a QR code of its `maya:` URI
//! - **Sent** — the moment a transfer leaves, with its id

// The core's amount parser and formatter, shared by path: the UI accepts and
// shows exactly the strings the core and the IPC boundary accept.
#[path = "../../core/src/amount.rs"]
mod amount;
mod bridge;
mod camera;
mod receive;
mod screens;
mod send;
mod theme;
mod widgets;

use leptos::prelude::*;

/// Where balances are read and transfers sent unless the user changes it.
const DEFAULT_NODE_URL: &str = "https://rpc.maya2c.dev/rpc";

/// How often the header re-reads the network's height. A block lands every
/// few seconds on the testnet; faster polling would only load the gateway.
const NETWORK_POLL: std::time::Duration = std::time::Duration::from_secs(5);
/// Pending-link check. Polled, not evented: a wasm page cannot listen to a
/// Tauri event without JS glue. One cheap IPC call a second, Wallet screen only.
const PENDING_PAY_POLL: std::time::Duration = std::time::Duration::from_secs(1);
/// How long a success notice stays: long enough to read one line twice.
const NOTICE_FOR: std::time::Duration = std::time::Duration::from_secs(6);

/// Which screen is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// First run: create or recover.
    Setup,
    /// Recover from a phrase over a stored wallet (forgotten passphrase,
    /// or a reinstall that found the old wallet).
    Recover,
    /// One-time display of a new recovery phrase.
    Backup(String),
    /// Unlock a stored wallet.
    Unlock,
    /// The main wallet view.
    Wallet,
    /// Camera scanner.
    Scan,
    /// Compose and sign a transfer.
    Send(bridge::PaymentRequest),
    /// Show an address and its QR code.
    Receive,
    /// A transfer was broadcast.
    Sent(receive::SentTransfer),
}

/// Application state shared across screens.
///
/// Each field is its own signal, so updating a balance re-renders the balance
/// and nothing else — the reason for fine-grained reactivity rather than a
/// single state blob.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Current screen.
    pub screen: RwSignal<Screen>,
    /// Derived accounts.
    pub accounts: RwSignal<Vec<bridge::Account>>,
    /// Selected account index.
    pub selected: RwSignal<u32>,
    /// Balance and nonce for the selected account.
    pub account_state: RwSignal<bridge::AccountState>,
    /// Signed transfers this session.
    pub history: RwSignal<Vec<bridge::PendingTransfer>>,
    /// Node endpoint used for balance reads and broadcast.
    pub node_url: RwSignal<String>,
    /// Last error, shown as a banner.
    pub error: RwSignal<Option<String>>,
    /// Last success message.
    pub notice: RwSignal<Option<String>>,
    /// The network the node reports; `None` until it answers or when it
    /// cannot be reached.
    pub network: RwSignal<Option<bridge::NetworkStatus>>,
    /// Why the node did not answer the last account read, raw. Shown in the
    /// calm paused card's Details, never as a red banner: an unreachable or
    /// paused node is an ordinary state, not something the user did wrong.
    pub node_error: RwSignal<Option<String>>,
    /// System, Light or Dark (Settings).
    pub theme: RwSignal<theme::Theme>,
}

impl AppState {
    fn new() -> Self {
        Self {
            screen: RwSignal::new(Screen::Setup),
            accounts: RwSignal::new(Vec::new()),
            selected: RwSignal::new(0),
            account_state: RwSignal::new(bridge::AccountState::default()),
            history: RwSignal::new(Vec::new()),
            // The public testnet's gateway (ADR-032), which speaks the node's
            // JSON-RPC at /rpc behind an allowlist. Editable in Settings for
            // a local node (http://127.0.0.1:8545) or another network.
            node_url: RwSignal::new(DEFAULT_NODE_URL.to_string()),
            error: RwSignal::new(None),
            notice: RwSignal::new(None),
            network: RwSignal::new(None),
            node_error: RwSignal::new(None),
            theme: RwSignal::new(theme::load()),
        }
    }

    /// Reports a failure to the user.
    pub fn fail(&self, message: impl Into<String>) {
        self.notice.set(None);
        self.error.set(Some(message.into()));
    }

    /// Reports a success. It goes by itself after [`NOTICE_FOR`] (an error
    /// stays until dismissed), unless a newer notice has replaced it.
    pub fn inform(&self, message: impl Into<String>) {
        let message = message.into();
        self.error.set(None);
        self.notice.set(Some(message.clone()));
        let notice = self.notice;
        set_timeout(
            move || {
                if notice.get_untracked().as_deref() == Some(message.as_str()) {
                    notice.set(None);
                }
            },
            NOTICE_FOR,
        );
    }

    /// Clears both banners.
    pub fn clear(&self) {
        self.error.set(None);
        self.notice.set(None);
    }
}

/// The application root.
#[component]
fn App() -> impl IntoView {
    let state = AppState::new();
    provide_context(state);
    Effect::new(move |_| theme::apply(state.theme.get()));

    // Decide the opening screen from whether a wallet is already stored.
    let existing = LocalResource::new(move || async move {
        bridge::wallet_exists("primary").await.unwrap_or(false)
    });

    Effect::new(move |_| {
        if existing.get().unwrap_or(false) {
            state.screen.set(Screen::Unlock);
        }
    });

    // The header's live height. Failures clear it rather than raising a
    // banner: an unreachable node is shown as "offline", not as an error.
    let poll_network = move || {
        leptos::task::spawn_local(async move {
            let status = bridge::network_status(&state.node_url.get_untracked()).await;
            state.network.set(status.ok());
        });
    };
    poll_network();
    let _ = set_interval_with_handle(poll_network, NETWORK_POLL);

    // A payment link arrives while the app runs; it is applied only on the
    // Wallet screen, so it never interrupts an unlock, backup or signing step.
    let poll_pending_pay = move || {
        if !matches!(state.screen.get_untracked(), Screen::Wallet) {
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(Some(request)) = bridge::take_pending_pay().await {
                state.screen.set(Screen::Send(request));
            }
        });
    };
    let _ = set_interval_with_handle(poll_pending_pay, PENDING_PAY_POLL);

    view! {
        <div class="app">
            <header class="titlebar">
                <img
                    class="logo"
                    src="assets/emblem.png"
                    // The emblem is the mark; the `.name` span beside it is the
                    // accessible name, so empty alt keeps it from being read
                    // out twice.
                    alt=""
                    // The emblem's square intrinsic size, so the titlebar does
                    // not reflow when the image finishes loading.
                    width="40"
                    height="40"
                />
                <span class="name">"Maya2C Wallet"</span>
                <span class="spacer"></span>
                // No network pill before a wallet exists: the first thing a
                // new user would read is a grey "offline" about a node they
                // have not used yet.
                <Show when=move || !matches!(state.screen.get(), Screen::Setup | Screen::Backup(_))>
                    <NetworkPill/>
                </Show>
                <Show when=move || matches!(state.screen.get(), Screen::Wallet)>
                    <button
                        class="ghost lock"
                        aria-label="Lock"
                        on:click=move |_| {
                            leptos::task::spawn_local(async move {
                                let _ = bridge::lock().await;
                                state.accounts.set(Vec::new());
                                state.screen.set(Screen::Unlock);
                            });
                        }
                    >
                        <widgets::Icon glyph=widgets::Glyph::Lock/>
                        // Hidden at phone width, where the button is the icon.
                        <span class="lock-text" aria-hidden="true">"Lock"</span>
                    </button>
                </Show>
            </header>

            <Banner/>

            // Short screens sit in the middle of the window rather than under
            // the header with an empty half below.
            <main
                class="body"
                class:center=move || matches!(
                    state.screen.get(),
                    Screen::Setup | Screen::Recover | Screen::Backup(_) | Screen::Unlock | Screen::Sent(_)
                )
            >
                // Components take generated Props structs, so they are invoked
                // through `view!` rather than called as plain functions.
                {move || match state.screen.get() {
                    Screen::Setup => view! { <screens::setup::Setup/> }.into_any(),
                    Screen::Recover => {
                        view! { <screens::setup::Setup from_unlock=true/> }.into_any()
                    }
                    Screen::Backup(phrase) => {
                        view! { <screens::backup::Backup phrase=phrase/> }.into_any()
                    }
                    Screen::Unlock => view! { <screens::unlock::Unlock/> }.into_any(),
                    Screen::Wallet => view! { <screens::wallet::WalletView/> }.into_any(),
                    Screen::Scan => view! { <screens::scan::Scan/> }.into_any(),
                    Screen::Send(request) => {
                        view! { <send::Send request=request/> }.into_any()
                    }
                    Screen::Receive => view! { <receive::Receive/> }.into_any(),
                    Screen::Sent(sent) => view! { <receive::Sent sent=sent/> }.into_any(),
                }}
            </main>
        </div>
    }
}

/// Which network, and that it is alive: a testnet badge nobody can mistake
/// for mainnet, and the height ticking up.
#[component]
fn NetworkPill() -> impl IntoView {
    let state = expect_context::<AppState>();
    // Memos, so the pill redraws only when what it shows changes: the poll
    // sets the status every few seconds, but the height span is replaced
    // (and its tick plays) only when a new block has arrived.
    let look = Memo::new(move |_| {
        state.network.get().map(|status| {
            let name = status
                .network
                .clone()
                .unwrap_or_else(|| "unnamed network".into());
            let title = format!("{name} \u{b7} genesis {}", screens::short(&status.genesis));
            let (class, kind) = if status.is_impostor() {
                ("net-pill impostor", "NOT MAINNET")
            } else if status.is_test_network() {
                ("net-pill test", "TESTNET")
            } else {
                ("net-pill main", "MAINNET")
            };
            (class, kind, title)
        })
    });
    let height = Memo::new(move |_| state.network.get().map(|status| status.height));
    view! {
        {move || match look.get() {
            Some((class, kind, title)) => view! {
                <span class=class title=title>
                    <span class="dot"></span>
                    <span class="net-kind">{kind}</span>
                    <span class="net-height">
                        {move || height.get().map(|h| view! {
                            <span class="tick">{format!("#{}", screens::group_digits(h))}</span>
                        })}
                    </span>
                </span>
            }
            .into_any(),
            None => view! {
                <span class="net-pill offline"><span class="dot"></span>"offline"</span>
            }
            .into_any(),
        }}
    }
}

/// Error and success banners: an error is an alert (announced at once), a
/// notice is a status (announced when the reader is idle).
#[component]
fn Banner() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <Show when=move || state.error.get().is_some()>
            <div class="banner error" role="alert">
                <span>{move || state.error.get().unwrap_or_default()}</span>
                <button
                    class="dismiss"
                    aria-label="Dismiss"
                    on:click=move |_| state.error.set(None)
                >
                    <span aria-hidden="true">"×"</span>
                </button>
            </div>
        </Show>
        <Show when=move || state.notice.get().is_some()>
            <div class="banner notice" role="status">
                <span>{move || state.notice.get().unwrap_or_default()}</span>
                <button
                    class="dismiss"
                    aria-label="Dismiss"
                    on:click=move |_| state.notice.set(None)
                >
                    <span aria-hidden="true">"×"</span>
                </button>
            </div>
        </Show>
    }
}

fn main() {
    // Without this a panic in the WASM bundle is an unexplained blank window.
    console_error_panic_hook::set_once();
    // Before the first paint, so a remembered Light never flashes dark.
    theme::apply(theme::load());
    leptos::mount::mount_to_body(App);
}
