//! Delivering and collecting through one relay: the steps a user's client
//! takes, shared by the one-shot CLI commands and the interactive `chat`.

use libp2p::Multiaddr;

use crate::client::{Client, Received};
use crate::identity::PrekeyBundle;
use crate::net::{self, Request, Response};
use crate::relay::fetch_bytes;
use crate::{Address, ChatError};

async fn ask(relay: &Multiaddr, request: Request) -> Result<Response, ChatError> {
    match net::call(relay, request).await? {
        Response::Refused(why) => Err(ChatError::Network(format!("the relay refused: {why}"))),
        other => Ok(other),
    }
}

fn unexpected(what: &str) -> ChatError {
    ChatError::Network(format!("unexpected answer to a {what} request"))
}

/// Publishes a prekey bundle at `relay`.
///
/// # Errors
///
/// A refusal or a network failure.
pub async fn publish(relay: &Multiaddr, bundle: PrekeyBundle) -> Result<(), ChatError> {
    ask(relay, Request::Publish(bundle)).await.map(|_| ())
}

/// Seals `text` for `to`, stamps it with the relay's postage and deposits
/// it. Fetches `to`'s prekey bundle first when there is no session yet.
///
/// On error the client may already have advanced its sending chain, so a
/// caller that keeps state on disk must not save it: reloading the saved
/// state discards a message the peer will never see.
///
/// # Errors
///
/// No prekey published for `to`, a refusal, or a network failure.
pub async fn deliver(
    client: &mut Client,
    relay: &Multiaddr,
    to: Address,
    text: &[u8],
    now: u64,
) -> Result<(), ChatError> {
    let bundle = if client.has_session(&to) {
        None
    } else {
        match ask(relay, Request::Bundle(to)).await? {
            Response::Bundle(Some(bundle)) => Some(bundle),
            Response::Bundle(None) => return Err(ChatError::NoSession),
            _ => return Err(unexpected("bundle")),
        }
    };
    let Response::Postage(bits) = ask(relay, Request::Postage).await? else {
        return Err(unexpected("postage"));
    };
    let envelope = client.seal(to, bundle.as_ref(), text, now)?.mint(bits)?;
    ask(relay, Request::Deposit(envelope)).await?;
    Ok(())
}

/// Empties this identity's mailbox at `relay` and opens each envelope. One
/// envelope that fails to open does not stop the others.
///
/// # Errors
///
/// A refusal or a network failure; per-envelope failures are in the list.
pub async fn collect(
    client: &mut Client,
    relay: &Multiaddr,
    now: u64,
) -> Result<Vec<Result<Received, ChatError>>, ChatError> {
    let me = client.identity();
    let address = me.address();
    let Response::Challenge(nonce) = ask(relay, Request::Challenge(address)).await? else {
        return Err(unexpected("challenge"));
    };
    let request = Request::Fetch {
        address,
        identity_key: me.public_key().to_vec(),
        signature: me.sign(&fetch_bytes(&address, &nonce))?,
    };
    let Response::Envelopes(envelopes) = ask(relay, request).await? else {
        return Err(unexpected("fetch"));
    };
    Ok(envelopes.iter().map(|e| client.open(e, now)).collect())
}
