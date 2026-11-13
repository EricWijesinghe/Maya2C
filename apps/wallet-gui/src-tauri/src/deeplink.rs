//! `maya2c-wallet://` links from other apps and the OS.
//!
//! A link is untrusted input: any web page or document can carry one. The
//! parser therefore accepts an exact allow-list of routes and nothing else.
//! `pay` only prefills the Send form; it signs and sends nothing. Signing
//! still needs the unlocked session and the native confirm dialog.

use std::sync::{Mutex, PoisonError};

use maya_address::{Network, decode_any};
use maya_wallet_core::amount::parse_display;
use maya_wallet_core::chat_pay::PaidRequests;
use maya_wallet_core::payment::{ChatRequest, PaymentRequest};
use primitive_types::U256;
use tauri::{AppHandle, Manager, Runtime, State};

/// Longest link accepted. A bech32m address is at most 90 characters, so a
/// valid `pay` link fits well inside this; anything longer is not a link.
const MAX_URL_LEN: usize = 512;

/// Longest decimal amount accepted. 2^256 has 78 digits; 24 fractional
/// places plus a point fit in 40 with room to spare.
const MAX_AMOUNT_LEN: usize = 40;

const PREFIX: &str = "maya2c-wallet://";

/// A route the app will act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// Bring the window forward. Carries nothing.
    Open,
    /// Prefill the Send form. `recipient` is lowercase hex of the 32-byte id.
    /// `amount` is in base units (ADR-068), parsed from a whole-coin decimal.
    /// `chat` is the chat request it pays and its chain, from a link the
    /// chat made (`request` and `chain`, both or neither; ADR-088).
    Pay {
        recipient: String,
        amount: Option<U256>,
        chat: Option<ChatRequest>,
    },
}

/// The paid chat request ids, in the app's data directory.
const PAID_FILE: &str = "chat-paid-requests.json";

/// Why a link was refused. Carries no input text, so it is safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteError {
    TooLong,
    NotAscii,
    Scheme,
    Route,
    Query,
    Address,
    Amount,
    /// A `request` or `chain` that is not 64 lowercase hex characters, or
    /// one without the other.
    ChatRequest,
    /// A chat request this wallet has already paid.
    AlreadyPaid,
}

/// Parses one link. Every rule is strict: a surprise is a refusal, not a guess.
///
/// # Errors
///
/// [`RouteError`] for an over-long, non-ASCII or foreign-scheme link, an
/// unknown route or query key, a duplicated or missing key, a value outside
/// its character set, an address that fails its checksum, or an amount that
/// is not a positive whole-coin decimal.
pub fn parse(url: &str) -> Result<Route, RouteError> {
    if url.len() > MAX_URL_LEN {
        return Err(RouteError::TooLong);
    }
    // Printable ASCII only: no controls, spaces, or percent-encoding tricks.
    if !url.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(RouteError::NotAscii);
    }
    // Case-sensitive on purpose: one spelling, one meaning.
    let body = url.strip_prefix(PREFIX).ok_or(RouteError::Scheme)?;
    let (path, query) = match body.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (body, None),
    };
    match (path, query) {
        ("open", None) => Ok(Route::Open),
        ("pay", Some(query)) => parse_pay(query),
        ("pay" | "open", _) => Err(RouteError::Query),
        _ => Err(RouteError::Route),
    }
}

/// Reads `to` (required), `amount` (optional), and `request` with `chain`
/// (both or neither), each at most once.
fn parse_pay(query: &str) -> Result<Route, RouteError> {
    let mut to = None;
    let mut amount = None;
    let mut request = None;
    let mut chain = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').ok_or(RouteError::Query)?;
        if value.is_empty() {
            return Err(RouteError::Query);
        }
        let slot = match key {
            "to" => &mut to,
            "amount" => &mut amount,
            "request" => &mut request,
            "chain" => &mut chain,
            _ => return Err(RouteError::Query),
        };
        if slot.replace(value).is_some() {
            return Err(RouteError::Query);
        }
    }
    let to = to.ok_or(RouteError::Query)?;
    let recipient = recipient_hex(to)?;
    let amount = amount.map(parse_whole_coins).transpose()?;
    let chat = match (request, chain) {
        (None, None) => None,
        (Some(request), Some(chain)) if is_hex32(request) && is_hex32(chain) => {
            Some(ChatRequest {
                request: request.to_owned(),
                chain: chain.to_owned(),
            })
        }
        _ => return Err(RouteError::ChatRequest),
    };
    Ok(Route::Pay {
        recipient,
        amount,
        chat,
    })
}

/// 64 lowercase hex characters: one spelling, as the chat writes it.
fn is_hex32(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The paid list, read from disk. A file that cannot be read refuses every
/// chat request rather than forgetting what was paid.
///
/// # Errors
///
/// No data directory, or an unreadable file.
pub fn paid_list<R: Runtime>(app: &AppHandle<R>) -> Result<PaidRequests, String> {
    PaidRequests::load(&paid_path(app)?).map_err(|e| e.to_string())
}

/// Marks chat request `id` paid at `now`, on disk.
///
/// # Errors
///
/// No data directory, an unreadable or unwritable file, or an id already
/// paid.
pub fn record_paid<R: Runtime>(app: &AppHandle<R>, id: &[u8; 32], now: u64) -> Result<(), String> {
    let path = paid_path(app)?;
    let mut paid = PaidRequests::load(&path).map_err(|e| e.to_string())?;
    paid.record(id, now).map_err(|e| e.to_string())?;
    paid.save(&path).map_err(|e| e.to_string())
}

fn paid_path<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(PAID_FILE))
}

/// Whether the wallet has already paid the chat request `chat` names.
fn already_paid<R: Runtime>(app: &AppHandle<R>, chat: &ChatRequest) -> bool {
    let Ok(bytes) = hex::decode(&chat.request) else {
        return true;
    };
    let Ok(id) = <[u8; 32]>::try_from(bytes) else {
        return true;
    };
    paid_list(app).map_or(true, |paid| paid.check(&id).is_err())
}

/// Checks a bech32m address and returns its 32-byte id as hex.
///
/// Any known network is accepted; the Send screen's preview names the chain,
/// so a testnet id is still shown as one before anything is signed.
fn recipient_hex(text: &str) -> Result<String, RouteError> {
    // Lowercase bech32m only: the charset check runs before the checksum.
    if !text
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return Err(RouteError::Address);
    }
    let (_network, id) = decode_any(text).map_err(|_| RouteError::Address)?;
    Ok(hex::encode(id))
}

/// A whole-coin decimal such as `1.5`, exact, in base units. Zero is refused.
fn parse_whole_coins(text: &str) -> Result<U256, RouteError> {
    if text.len() > MAX_AMOUNT_LEN || !text.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err(RouteError::Amount);
    }
    let value = parse_display(text).map_err(|_| RouteError::Amount)?;
    if value.is_zero() {
        return Err(RouteError::Amount);
    }
    Ok(value)
}

/// The pay request waiting for the UI to pick it up.
#[derive(Default)]
pub struct PendingPay(Mutex<Option<PaymentRequest>>);

/// Applies links the OS or a second launch delivered. Refused links are
/// dropped with a static log line that names only the reason.
pub fn handle<R: Runtime, U: std::fmt::Display>(app: &AppHandle<R>, urls: &[U]) {
    for url in urls {
        match parse(&url.to_string()) {
            Ok(Route::Open) => {}
            Ok(Route::Pay {
                recipient,
                amount,
                chat,
            }) => {
                if chat.as_ref().is_some_and(|c| already_paid(app, c)) {
                    eprintln!("deep link refused: {:?}", RouteError::AlreadyPaid);
                    continue;
                }
                let request = PaymentRequest {
                    recipient,
                    amount,
                    label: None,
                    chat,
                };
                let pending: State<'_, PendingPay> = app.state();
                *pending.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(request);
            }
            Err(reason) => eprintln!("deep link refused: {reason:?}"),
        }
    }
    focus(app);
}

/// Brings the main window forward. Best effort: a failed focus must not
/// stop a valid link from being applied.
pub fn focus<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Returns and clears the pending pay request. Called by the UI only while
/// the Wallet screen is showing, so a link never yanks the user off a
/// signing flow.
#[tauri::command]
pub fn take_pending_pay(pending: State<'_, PendingPay>) -> Option<PaymentRequest> {
    pending
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real checksummed address, built at test time so it cannot go stale.
    fn address(network: Network) -> String {
        maya_address::encode(network, &[7u8; 32])
    }

    fn pay(to: &str) -> String {
        format!("{PREFIX}pay?to={to}")
    }

    #[test]
    fn open_route_is_accepted() {
        assert_eq!(parse("maya2c-wallet://open"), Ok(Route::Open));
    }

    #[test]
    fn pay_with_amount_prefills_exact_base_units() {
        let url = pay(&address(Network::Mainnet)) + "&amount=1.5";
        let expected = U256::from(15) * U256::exp10(23);
        assert_eq!(
            parse(&url),
            Ok(Route::Pay {
                recipient: hex::encode([7u8; 32]),
                amount: Some(expected),
                chat: None,
            })
        );
    }

    #[test]
    fn pay_without_amount_leaves_it_empty() {
        let url = pay(&address(Network::Testnet));
        assert_eq!(
            parse(&url),
            Ok(Route::Pay {
                recipient: hex::encode([7u8; 32]),
                amount: None,
                chat: None,
            })
        );
    }

    /// The link `maya-chat`'s `pay_link::pay_uri` writes.
    #[test]
    fn a_chat_pay_link_carries_its_request_and_chain() {
        let request = "ab".repeat(32);
        let chain = "c1".repeat(32);
        let url = format!(
            "{}&amount=2.5&request={request}&chain={chain}",
            pay(&address(Network::Testnet))
        );
        let Ok(Route::Pay { chat, .. }) = parse(&url) else {
            panic!("refused: {url}");
        };
        assert_eq!(chat, Some(ChatRequest { request, chain }));
    }

    #[test]
    fn a_chat_request_needs_both_keys_in_their_one_spelling() {
        let to = address(Network::Mainnet);
        let id = "ab".repeat(32);
        let cases = [
            format!("{}&request={id}", pay(&to)),
            format!("{}&chain={id}", pay(&to)),
            format!("{}&request={}&chain={id}", pay(&to), id.to_uppercase()),
            format!("{}&request={}&chain={id}", pay(&to), &id[1..]),
            format!("{}&request={id}0&chain={id}", pay(&to)),
            format!("{}&request={id}&chain={}", pay(&to), "zz".repeat(32)),
        ];
        for url in cases {
            assert_eq!(parse(&url), Err(RouteError::ChatRequest), "accepted: {url}");
        }
        let twice = format!("{}&request={id}&request={id}&chain={id}", pay(&to));
        assert_eq!(parse(&twice), Err(RouteError::Query));
    }

    #[test]
    fn oversize_link_is_refused_before_parsing() {
        let url = format!("{PREFIX}open{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(parse(&url), Err(RouteError::TooLong));
    }

    #[test]
    fn foreign_or_mis_cased_scheme_is_refused() {
        let to = address(Network::Mainnet);
        assert_eq!(parse("https://pay?to=x"), Err(RouteError::Scheme));
        assert_eq!(parse("maya2c-chat://open"), Err(RouteError::Scheme));
        assert_eq!(parse("MAYA2C-WALLET://open"), Err(RouteError::Scheme));
        assert_eq!(parse(&format!("maya2c-wallet:/pay?to={to}")), Err(RouteError::Scheme));
    }

    #[test]
    fn unknown_route_is_refused() {
        assert_eq!(parse("maya2c-wallet://send"), Err(RouteError::Route));
        assert_eq!(parse("maya2c-wallet://"), Err(RouteError::Route));
        assert_eq!(parse("maya2c-wallet://open/../pay"), Err(RouteError::Route));
        assert_eq!(parse("maya2c-wallet://OPEN"), Err(RouteError::Route));
    }

    #[test]
    fn open_takes_no_query() {
        assert_eq!(parse("maya2c-wallet://open?x=1"), Err(RouteError::Query));
    }

    #[test]
    fn malformed_queries_are_refused() {
        let to = address(Network::Mainnet);
        let cases = [
            "maya2c-wallet://pay".to_string(),
            "maya2c-wallet://pay?".to_string(),
            format!("{PREFIX}pay?to={to}&"),
            format!("{PREFIX}pay?to={to}&to={to}"),
            format!("{PREFIX}pay?to={to}&amount=1&amount=2"),
            format!("{PREFIX}pay?to={to}&from=evil"),
            format!("{PREFIX}pay?to={to}&amount"),
            format!("{PREFIX}pay?to={to}&amount="),
            format!("{PREFIX}pay?to="),
            format!("{PREFIX}pay?amount=1"),
        ];
        for url in cases {
            assert_eq!(parse(&url), Err(RouteError::Query), "accepted: {url}");
        }
    }

    #[test]
    fn injection_shaped_input_is_refused() {
        let to = address(Network::Mainnet);
        let cases = [
            format!("{PREFIX}pay?to={to}%0a"),
            format!("{PREFIX}pay?to=<script>"),
            format!("{PREFIX}pay?to=../../etc/passwd"),
            format!("{PREFIX}pay?to={to}#frag"),
            format!("{PREFIX}pay?to={to}\u{0}"),
            format!("{PREFIX}pay?to={to} "),
            format!("{PREFIX}pay?to=MAYA1Q{}", "A".repeat(10)),
            format!("{PREFIX}pay?to=\u{e9}{}", &to[1..]),
        ];
        for url in cases {
            assert!(parse(&url).is_err(), "accepted: {url:?}");
        }
    }

    #[test]
    fn checksum_error_is_refused() {
        let mut to = address(Network::Mainnet).into_bytes();
        let last = to.len() - 1;
        to[last] = if to[last] == b'q' { b'p' } else { b'q' };
        let to = String::from_utf8(to).expect("ascii");
        assert_eq!(parse(&pay(&to)), Err(RouteError::Address));
    }

    #[test]
    fn non_address_text_is_refused() {
        assert_eq!(parse(&pay("maya1")), Err(RouteError::Address));
        assert_eq!(parse(&pay("0000000000000000")), Err(RouteError::Address));
    }

    #[test]
    fn amount_must_be_positive_whole_coins() {
        let to = address(Network::Mainnet);
        let cases = ["0", "0.0", "-1", "1e3", "0x10", "1.5.5", "1,5", ".5", "1.", "+1"];
        for amount in cases {
            let url = format!("{}&amount={amount}", pay(&to));
            assert_eq!(parse(&url), Err(RouteError::Amount), "accepted: {amount}");
        }
        let too_long = format!("{}&amount={}", pay(&to), "1".repeat(MAX_AMOUNT_LEN + 1));
        assert_eq!(parse(&too_long), Err(RouteError::Amount));
    }

    #[test]
    fn too_many_fractional_digits_are_refused_not_rounded() {
        let to = address(Network::Mainnet);
        let url = format!("{}&amount=0.0000000000000000000000001", pay(&to));
        assert_eq!(parse(&url), Err(RouteError::Amount));
    }
}
