//! Page-weight budgets (Master Prompt 29 §4).
//!
//! What CI can hold the explorer to without a phone: the bytes a page ships
//! and the requests it needs before first paint. The explorer renders on the
//! server with inline CSS and one small inline script, so a first paint needs
//! only the HTML document itself; these tests keep it that way. Lighthouse on
//! a mid-range Android over slow 4G is the real measurement and has not been
//! run (`reports/29-interface.md`).

use maya_explorer::model::IndexedTx;
use maya_explorer::ui::{account_page, transaction_page};

/// Uncompressed HTML budget for an account page with 50 transactions. At the
/// Lighthouse slow-4G profile (~1.6 Mbit/s, 150 ms RTT) 64 KiB is ~0.33 s of
/// transfer before compression.
const ACCOUNT_PAGE_BUDGET: usize = 64 * 1024;
/// A single-transaction page.
const TX_PAGE_BUDGET: usize = 24 * 1024;

fn tx(i: i64) -> IndexedTx {
    IndexedTx {
        txid: format!("{i:064x}"),
        height: 1_000 + i,
        sender: "ab".repeat(32),
        nonce: i,
        output_count: 2,
        total_out: 1_000_000 + i,
        signed: true,
    }
}

fn render_blocking_requests(html: &str) -> usize {
    html.matches("<script src").count() + html.matches("rel=\"stylesheet\"").count()
}

#[test]
fn pages_stay_within_their_byte_budgets() {
    let account = account_page(
        "ab".repeat(32),
        Some(5),
        Some(50),
        (0..50).map(tx).collect(),
        None,
    );
    let single = transaction_page(Some(tx(1)), "01".into());
    println!(
        "account page (50 txs): {} B of {ACCOUNT_PAGE_BUDGET}; transaction page: {} B of {TX_PAGE_BUDGET}",
        account.len(),
        single.len()
    );
    assert!(
        account.len() <= ACCOUNT_PAGE_BUDGET,
        "account page {} B",
        account.len()
    );
    assert!(
        single.len() <= TX_PAGE_BUDGET,
        "transaction page {} B",
        single.len()
    );
}

#[test]
fn first_paint_needs_no_request_beyond_the_document() {
    let page = transaction_page(Some(tx(1)), "01".into());
    assert_eq!(render_blocking_requests(&page), 0);
    assert!(
        page.contains("data-theme=\"command\""),
        "the shared tokens are in use"
    );
    assert!(
        page.contains("--danger:"),
        "tokens are inlined, not fetched"
    );
}
