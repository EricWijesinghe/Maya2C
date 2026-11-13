//! The host side of the mini-app bridge (ADR-138).
//!
//! A running mini-app sits in a sandboxed frame with no network and no host
//! API. Its only way out is `postMessage` to the chat window, which forwards
//! a [`Call`] here with the token it assigned that frame. A call with any
//! other token is dropped without a reply: it came from a frame this bridge
//! did not start, or from one that has since been closed.
//!
//! Each permission is asked for once, when first used, and only if the
//! manifest declared it. Every payment is asked for every time, with the
//! confirm text from [`pay_confirm_text`]; only a yes produces a wallet link,
//! and the wallet signs. Nothing here holds a key.

use std::collections::BTreeSet;

use maya_address::{Network, decode, encode};
use serde::{Deserialize, Serialize};

use crate::miniapp::{Permission, Verified};
use crate::pay::{self, ChainTag, check_amount, check_memo};
use crate::pay_link::{base_to_whole, pay_uri_to};
use crate::{Address, ChatError};

/// Identifies the frame a bridge was opened for.
pub type Token = [u8; 16];

/// Longest message a mini-app may send, in UTF-8 bytes. A policy bound.
pub const MAX_SEND_BYTES: usize = 2_000;

const INTENT_DOMAIN: &str = "maya-chat 2026-11 mini-app pay intent v1";

/// What a mini-app asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "call", rename_all = "kebab-case")]
pub enum Call {
    /// The user's address, as Bech32m text.
    Address,
    /// A payment the user's wallet should show.
    Pay {
        /// Bech32m address on the bridge's network.
        to: String,
        /// Decimal base units.
        amount: String,
        /// Text, never markup.
        memo: String,
    },
    /// A text message into this chat.
    Send {
        /// The message.
        text: String,
    },
}

/// What the host does next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reply", rename_all = "kebab-case")]
pub enum Reply {
    /// Return the address to the app.
    Address {
        /// Bech32m.
        address: String,
    },
    /// Show `text` and pass the user's answer to [`Bridge::answer`].
    Ask {
        /// Which prompt is being answered.
        prompt: u64,
        /// What to show, verbatim.
        text: String,
    },
    /// The user confirmed: hand this link to the wallet, which signs.
    OpenWallet {
        /// A `maya2c-wallet://pay?` link.
        link: String,
    },
    /// Send this text in the chat the app was opened in.
    Send {
        /// Already labelled with the app's name.
        text: String,
    },
    /// The user said no.
    Declined,
    /// The call broke a rule; nothing was asked.
    Refused {
        /// Why, in a fixed phrase.
        why: &'static str,
    },
}

/// A call waiting on the user.
#[derive(Clone, Debug)]
enum Pending {
    Grant(Permission, Call),
    Pay(String),
}

/// One running mini-app's bridge.
#[derive(Debug)]
pub struct Bridge {
    token: Token,
    app: Verified,
    network: Network,
    /// The chain a payment link names (ADR-088): the wallet refuses another.
    chain: ChainTag,
    user: Address,
    granted: BTreeSet<Permission>,
    pending: Option<(u64, Pending)>,
    next_prompt: u64,
}

/// The text the user confirms before the wallet is asked to pay. `asker`
/// says who asks: a mini-app and its publisher, or a chat contact.
///
/// shortcut: a format of this module's own, because PR #446's Bech32m
/// clear-sign renderer is not on this branch; switch to `maya-clear-sign`'s
/// renderer once #446 is merged so chat and wallet say the same words.
#[must_use]
pub fn pay_confirm_text(
    asker: &str,
    network: Network,
    to: &Address,
    amount: &str,
    memo: &str,
) -> String {
    let memo = if memo.is_empty() { "(none)" } else { memo };
    format!(
        "{asker} asks your wallet to pay\n\
         To: {to}\n\
         Amount: {amount} coins\n\
         Memo: {memo}\n\
         Your wallet shows this again and signs. Chat and its apps cannot sign.",
        to = encode(network, to),
        amount = base_to_whole(amount),
    )
}

impl Bridge {
    /// A bridge for `app`, run by `user` on `network` and `chain`, with a
    /// fresh token.
    ///
    /// # Errors
    ///
    /// [`ChatError::Entropy`].
    pub fn new(
        app: Verified,
        network: Network,
        chain: ChainTag,
        user: Address,
    ) -> Result<Self, ChatError> {
        let mut token = [0u8; 16];
        getrandom::fill(&mut token).map_err(|_| ChatError::Entropy)?;
        Ok(Self {
            token,
            app,
            network,
            chain,
            user,
            granted: BTreeSet::new(),
            pending: None,
            next_prompt: 1,
        })
    }

    /// The token the host gives the frame.
    #[must_use]
    pub fn token(&self) -> Token {
        self.token
    }

    /// The app this bridge runs.
    #[must_use]
    pub fn app(&self) -> &Verified {
        &self.app
    }

    /// Handles a call from the frame holding `from`. `None`: dropped.
    pub fn handle(&mut self, from: &Token, call: Call) -> Option<Reply> {
        if *from != self.token {
            return None;
        }
        if self.pending.is_some() {
            return Some(Reply::Refused {
                why: "a prompt is already open",
            });
        }
        Some(self.run(call))
    }

    /// The user's answer to `prompt`. `None`: dropped (wrong frame, or no such
    /// prompt open).
    pub fn answer(&mut self, from: &Token, prompt: u64, yes: bool) -> Option<Reply> {
        if *from != self.token || self.pending.as_ref().map(|(id, _)| *id) != Some(prompt) {
            return None;
        }
        let (_, pending) = self.pending.take()?;
        if !yes {
            return Some(Reply::Declined);
        }
        Some(match pending {
            Pending::Grant(permission, call) => {
                self.granted.insert(permission);
                self.run(call)
            }
            Pending::Pay(link) => Reply::OpenWallet { link },
        })
    }

    fn ask(&mut self, pending: Pending, text: String) -> Reply {
        let prompt = self.next_prompt;
        self.next_prompt = self.next_prompt.saturating_add(1);
        self.pending = Some((prompt, pending));
        Reply::Ask { prompt, text }
    }

    /// Checks the call, then the permission, then does it.
    fn run(&mut self, call: Call) -> Reply {
        let permission = match self.check(&call) {
            Ok(p) => p,
            Err(why) => return Reply::Refused { why },
        };
        if !self.app.permissions.contains(&permission) {
            return Reply::Refused {
                why: "the manifest does not declare that permission",
            };
        }
        if !self.granted.contains(&permission) {
            let text = format!("Allow {} to {}?", self.app.name, permission.describe());
            return self.ask(Pending::Grant(permission, call), text);
        }
        match call {
            Call::Address => Reply::Address {
                address: encode(self.network, &self.user),
            },
            Call::Send { text } => Reply::Send {
                text: format!("[{}] {text}", self.app.name),
            },
            Call::Pay { to, amount, memo } => match self.intent(&to, &amount, &memo) {
                Ok((payee, uri)) => {
                    let asker = format!(
                        "{} (app published by {})",
                        self.app.name,
                        encode(self.network, &self.app.publisher)
                    );
                    let text = pay_confirm_text(&asker, self.network, &payee, &amount, &memo);
                    self.ask(Pending::Pay(uri), text)
                }
                Err(_) => Reply::Refused {
                    why: "could not make a payment intent",
                },
            },
        }
    }

    /// The permission a call needs, if its fields pass.
    fn check(&self, call: &Call) -> Result<Permission, &'static str> {
        match call {
            Call::Address => Ok(Permission::ReadAddress),
            Call::Send { text } => {
                let ok = !text.is_empty()
                    && text.len() <= MAX_SEND_BYTES
                    && !text.contains('\0')
                    && !pay::is_marked(text.as_bytes());
                ok.then_some(Permission::SendMessage)
                    .ok_or("message is empty, too long or poses as a payment")
            }
            Call::Pay { to, amount, memo } => {
                decode(self.network, to).map_err(|_| "address is not for this network")?;
                check_amount(amount).map_err(|_| "amount is not canonical base units")?;
                check_memo(memo).map_err(|_| "memo is too long or has a hidden character")?;
                Ok(Permission::RequestPayment)
            }
        }
    }

    /// The payee and the wallet link for a checked payment.
    fn intent(&self, to: &str, amount: &str, memo: &str) -> Result<(Address, String), ChatError> {
        let payee = decode(self.network, to).map_err(|_| ChatError::Refused("bad address"))?;
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| ChatError::Entropy)?;
        let mut h = blake3::Hasher::new_derive_key(INTENT_DOMAIN);
        for part in [
            &self.app.publisher[..],
            &payee[..],
            amount.as_bytes(),
            memo.as_bytes(),
            &nonce[..],
        ] {
            h.update(&u64::try_from(part.len()).unwrap_or(u64::MAX).to_le_bytes());
            h.update(part);
        }
        let id = *h.finalize().as_bytes();
        Ok((
            payee,
            pay_uri_to(self.network, &payee, amount, &id, &self.chain),
        ))
    }
}
