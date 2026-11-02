//! Delivering and collecting through one relay: the steps a user's client
//! takes, shared by the one-shot CLI commands and the interactive `chat`.

use libp2p::Multiaddr;

use crate::client::{Client, Frame, Received};
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

/// The envelope version for a message to `relay`: the client's, lowered to
/// the relay's maximum. A relay that predates the request refuses it and
/// reads version 1 only, so an error here means version 1.
async fn version_for(client: &Client, relay: &Multiaddr) -> u8 {
    match ask(relay, Request::Version).await {
        Ok(Response::Version(relay_max)) => relay_max.min(client.send_version()),
        _ => 1,
    }
}

/// Seals `text` for `to` and puts it in the outbox, without sending it. Save
/// the client after this and before [`flush`], so a crash between the two
/// leaves the message queued rather than lost.
///
/// # Errors
///
/// No prekey published for `to`, a refusal, or a full outbox.
pub async fn prepare(
    client: &mut Client,
    relay: &Multiaddr,
    to: Address,
    text: &[u8],
    now: u64,
) -> Result<[u8; 32], ChatError> {
    let bundle = if client.has_session(&to) {
        None
    } else {
        match ask(relay, Request::Bundle(to)).await? {
            Response::Bundle(Some(bundle)) => Some(bundle),
            Response::Bundle(None) => return Err(ChatError::NoSession),
            _ => return Err(unexpected("bundle")),
        }
    };
    let version = version_for(client, relay).await;
    client.queue(to, bundle.as_ref(), text, now, version)
}

/// Attempts every due outbox entry once: the relay's postage, then a deposit
/// of each entry's same bytes. Each outcome is recorded in the outbox, so a
/// failure is retried later with backoff. Save the client afterwards.
///
/// # Errors
///
/// Only when the relay cannot quote postage; per-entry failures are in the
/// list.
pub async fn flush(
    client: &mut Client,
    relay: &Multiaddr,
    now: u64,
) -> Result<Vec<([u8; 32], Result<(), ChatError>)>, ChatError> {
    client.sweep(now);
    let due = client.due(now);
    if due.is_empty() {
        return Ok(Vec::new());
    }
    let Response::Postage(bits) = ask(relay, Request::Postage).await? else {
        return Err(unexpected("postage"));
    };
    let mut results = Vec::with_capacity(due.len());
    for id in due {
        let outcome = attempt(client, relay, &id, bits, now).await;
        results.push((id, outcome));
    }
    Ok(results)
}

async fn attempt(
    client: &mut Client,
    relay: &Multiaddr,
    id: &[u8; 32],
    bits: u32,
    now: u64,
) -> Result<(), ChatError> {
    let envelope = client.envelope_for_send(id, bits)?;
    match ask(relay, Request::Deposit(envelope)).await {
        Ok(_) => client.sent(id, now),
        Err(e) => {
            client.failed(id, now)?;
            Err(e)
        }
    }
}

/// [`prepare`] then [`flush`] in one call, for callers that save only at the
/// end. The message stays queued whatever the outcome; the error is the
/// outcome of this message's first attempt.
///
/// # Errors
///
/// As [`prepare`], or this message's deposit failure.
pub async fn deliver(
    client: &mut Client,
    relay: &Multiaddr,
    to: Address,
    text: &[u8],
    now: u64,
) -> Result<(), ChatError> {
    let id = prepare(client, relay, to, text, now).await?;
    let results = flush(client, relay, now).await?;
    match results.into_iter().find(|(due, _)| *due == id) {
        Some((_, outcome)) => outcome,
        None => Err(ChatError::Refused("not in the outbox")),
    }
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
    let mut out = Vec::new();
    for envelope in &envelopes {
        match client.open_frame(envelope, now) {
            Ok(Frame::Text(received)) => out.push(Ok(received)),
            Ok(Frame::Receipt { .. }) => {}
            Err(e) => out.push(Err(e)),
        }
    }
    if let Err(e) = client.end_pass(now) {
        out.push(Err(e));
    }
    Ok(out)
}
