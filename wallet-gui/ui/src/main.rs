//! Maya wallet frontend.
//!
//! Client-side Leptos with fine-grained signals. Every privileged action is a
//! call into the Rust core through [`bridge`]; nothing here can reach a key,
//! because the core exposes no command that returns one.
//!
//! ## Screens
//!
//! - **Setup** — create a wallet or recover one from a BIP-39 phrase
//! - **Backup** — show the recovery phrase once, and require confirmation
//! - **Wallet** — accounts, balance, send, and pending history
//! - **Scan** — camera QR capture, decoded in Rust
//! - **Send** — recipient, amount, and manual fee control

mod bridge;
mod camera;
mod screens;

use leptos::prelude::*;

/// Which screen is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// First run: create or recover.
    Setup,
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
}

impl AppState {
    fn new() -> Self {
        Self {
            screen: RwSignal::new(Screen::Setup),
            accounts: RwSignal::new(Vec::new()),
            selected: RwSignal::new(0),
            account_state: RwSignal::new(bridge::AccountState::default()),
            history: RwSignal::new(Vec::new()),
            node_url: RwSignal::new("http://127.0.0.1:8545".to_string()),
            error: RwSignal::new(None),
            notice: RwSignal::new(None),
        }
    }

    /// Reports a failure to the user.
    pub fn fail(&self, message: impl Into<String>) {
        self.notice.set(None);
        self.error.set(Some(message.into()));
    }

    /// Reports a success.
    pub fn inform(&self, message: impl Into<String>) {
        self.error.set(None);
        self.notice.set(Some(message.into()));
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

    // Decide the opening screen from whether a wallet is already stored.
    let existing = LocalResource::new(move || async move {
        bridge::wallet_exists("primary").await.unwrap_or(false)
    });

    Effect::new(move |_| {
        if existing.get().unwrap_or(false) {
            state.screen.set(Screen::Unlock);
        }
    });

    view! {
        <div class="app">
            <header class="titlebar">
                <span class="logo">"◆"</span>
                <span class="name">"Maya Wallet"</span>
                <span class="spacer"></span>
                <Show when=move || matches!(state.screen.get(), Screen::Wallet)>
                    <button
                        class="ghost"
                        on:click=move |_| {
                            leptos::task::spawn_local(async move {
                                let _ = bridge::lock().await;
                                state.accounts.set(Vec::new());
                                state.screen.set(Screen::Unlock);
                            });
                        }
                    >
                        "Lock"
                    </button>
                </Show>
            </header>

            <Banner/>

            <main class="body">
                // Components take generated Props structs, so they are invoked
                // through `view!` rather than called as plain functions.
                {move || match state.screen.get() {
                    Screen::Setup => view! { <screens::setup::Setup/> }.into_any(),
                    Screen::Backup(phrase) => {
                        view! { <screens::backup::Backup phrase=phrase/> }.into_any()
                    }
                    Screen::Unlock => view! { <screens::unlock::Unlock/> }.into_any(),
                    Screen::Wallet => view! { <screens::wallet::WalletView/> }.into_any(),
                    Screen::Scan => view! { <screens::scan::Scan/> }.into_any(),
                    Screen::Send(request) => {
                        view! { <screens::send::Send request=request/> }.into_any()
                    }
                }}
            </main>
        </div>
    }
}

/// Error and success banners.
#[component]
fn Banner() -> impl IntoView {
    let state = expect_context::<AppState>();

    view! {
        <Show when=move || state.error.get().is_some()>
            <div class="banner error">
                {move || state.error.get().unwrap_or_default()}
                <button class="dismiss" on:click=move |_| state.error.set(None)>"×"</button>
            </div>
        </Show>
        <Show when=move || state.notice.get().is_some()>
            <div class="banner notice">
                {move || state.notice.get().unwrap_or_default()}
                <button class="dismiss" on:click=move |_| state.notice.set(None)>"×"</button>
            </div>
        </Show>
    }
}

fn main() {
    // Without this a panic in the WASM bundle is an unexplained blank window.
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}
