//! The address book and the new-recipient check (ADR-086 §6.2, §7).
//!
//! Every judgement here — is it saved, does it look like a saved one, is the
//! name confusable — is made by the wallet core. This file shows the answers
//! and refuses to let the send form move on without them.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::bridge;
use crate::receive::CopyButton;
use crate::{AppState, Screen};

/// The address book screen: saved addresses, adding one, export and import.
#[component]
pub fn Contacts() -> impl IntoView {
    let state = expect_context::<AppState>();
    let list = RwSignal::new(Vec::<bridge::Contact>::new());
    let address = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let note = RwSignal::new(String::new());
    let transfer = RwSignal::new(String::new());

    let reload = move || {
        spawn_local(async move {
            match bridge::contacts().await {
                Ok(saved) => list.set(saved),
                Err(error) => state.fail(error),
            }
        });
    };
    reload();

    let add = move |_| {
        state.clear();
        let (a, n, t) = (address.get(), name.get(), note.get());
        spawn_local(async move {
            match bridge::add_contact(&a, &n, &t, false, &state.genesis()).await {
                Ok(check) if check.warnings.is_empty() => {
                    state.inform(format!("Saved as \u{201c}{}\u{201d}.", check.name));
                }
                Ok(check) => state.fail(format!(
                    "Saved as \u{201c}{}\u{201d}, but check it: {}",
                    check.name,
                    check.warnings.join("; ")
                )),
                Err(error) => state.fail(error),
            }
            address.set(String::new());
            name.set(String::new());
            note.set(String::new());
            reload();
        });
    };

    let export = move |_| {
        spawn_local(async move {
            match bridge::export_contacts().await {
                Ok(sealed) => transfer.set(sealed),
                Err(error) => state.fail(error),
            }
        });
    };
    let import = move |_| {
        let data = transfer.get();
        spawn_local(async move {
            match bridge::import_contacts(&data).await {
                Ok(added) => state.inform(format!("Imported {added} new contacts.")),
                Err(error) => state.fail(error),
            }
            reload();
        });
    };

    let rows = move || {
        list.get()
            .into_iter()
            .map(|c| {
                let pay_to = c.address.clone();
                let remove_it = c.address.clone();
                let copy_text = Signal::derive({
                    let a = c.address.clone();
                    move || a.clone()
                });
                view! {
                    <div class="contact">
                        <div class="contact-name">{c.name.clone()}</div>
                        <code class="mono address">{c.grouped.clone()}</code>
                        <Show when={let n = c.note.clone(); move || !n.is_empty()}>
                            <p class="hint">{c.note.clone()}</p>
                        </Show>
                        <div class="copy-row">
                            <button
                                class="primary"
                                on:click=move |_| state.screen.set(Screen::Send(bridge::PaymentRequest {
                                    recipient: pay_to.clone(),
                                    ..bridge::PaymentRequest::default()
                                }))
                            >
                                "Pay"
                            </button>
                            <CopyButton text=copy_text label="Copy"/>
                            <button
                                class="ghost"
                                on:click=move |_| {
                                    let a = remove_it.clone();
                                    spawn_local(async move {
                                        if let Err(error) = bridge::remove_contact(&a).await {
                                            state.fail(error);
                                        }
                                        reload();
                                    });
                                }
                            >
                                "Remove"
                            </button>
                        </div>
                    </div>
                }
            })
            .collect_view()
    };

    view! {
        <section class="card contacts enter">
            <h1>"Contacts"</h1>
            <p class="lede">
                "Names here are the ones you typed. A name in a payment request is the \
                 payee's claim and is never saved for you."
            </p>
            <div class="contact-list">{rows}</div>

            <h2>"Add a contact"</h2>
            <label>"Address (paste it)"</label>
            <input class="mono" prop:value=move || address.get()
                on:input=move |ev| address.set(event_target_value(&ev))/>
            <label>"Name"</label>
            <input prop:value=move || name.get()
                on:input=move |ev| name.set(event_target_value(&ev))/>
            <label>"Note (optional)"</label>
            <input prop:value=move || note.get()
                on:input=move |ev| note.set(event_target_value(&ev))/>
            <button class="primary wide" on:click=add>"Save"</button>

            <h2>"Export or import"</h2>
            <p class="hint">
                "An export is encrypted. It opens only in a wallet restored from the \
                 same recovery phrase."
            </p>
            <textarea class="mono" rows="3" prop:value=move || transfer.get()
                on:input=move |ev| transfer.set(event_target_value(&ev))></textarea>
            <div class="copy-row">
                <button class="ghost" on:click=export>"Export"</button>
                <button class="ghost" on:click=import>"Import"</button>
            </div>

            <button class="ghost wide" on:click=move |_| {
                state.clear();
                state.screen.set(Screen::Wallet);
            }>"Back"</button>
        </section>
    }
}

/// The recipient read-back the send form shows before Review. For an address
/// never saved and never paid it is mandatory: Review stays disabled until
/// `acknowledged` is set by the person ticking the box.
#[component]
pub fn RecipientCheck(
    #[prop(into)] check: Signal<Option<bridge::PasteCheck>>,
    acknowledged: RwSignal<bool>,
) -> impl IntoView {
    move || {
        check.get().map(|c| {
            let saved = c.contact.clone();
            let look_alike = c.look_alike_of.clone();
            let new_recipient = c.new_recipient;
            view! {
                <div class=if new_recipient { "recipient-check new" } else { "recipient-check" }>
                    {match saved {
                        Some(n) => format!("Saved contact: {n}"),
                        None if new_recipient => "New recipient: you have never paid or saved this address.".to_string(),
                        None => "You have paid this address before.".to_string(),
                    }}
                    <code class="mono address readback">{c.grouped.clone()}</code>
                    <Show when={let l = look_alike.clone(); move || l.is_some()}>
                        <p class="hint warning">
                            {format!(
                                "This looks like your contact \u{201c}{}\u{201d} but is a different \
                                 address. Address poisoning works this way: compare every group.",
                                look_alike.clone().unwrap_or_default()
                            )}
                        </p>
                    </Show>
                    <Show when=move || new_recipient>
                        <label class="check">
                            <input type="checkbox"
                                prop:checked=move || acknowledged.get()
                                on:change=move |ev| acknowledged.set(event_target_checked(&ev))/>
                            "I compared every group above with the address I was given."
                        </label>
                    </Show>
                </div>
            }
        })
    }
}
