//! Lookup pages for the v4 records (ADR-135 I8): names, assets, NFT
//! collections and items, and scheduled payments. Each is read live from the
//! node, so an owner or a balance is never an indexed copy that went stale.

use leptos::prelude::*;

use super::{Shell, render};
use custom_l1_node::rpc::{AssetInfo, NameInfo, NftCollectionInfo, NftItemInfo, ScheduleInfo};

/// One `<dt>`/`<dd>` row. An account links to its page; anything else is text.
enum Row {
    Text(&'static str, String),
    Account(&'static str, String),
}

fn rows(rows: Vec<Row>) -> impl IntoView {
    let items = rows
        .into_iter()
        .map(|row| match row {
            Row::Text(dt, dd) => view! { <dt>{dt}</dt><dd class="mono">{dd}</dd> }.into_any(),
            Row::Account(dt, address) => view! {
                <dt>{dt}</dt>
                <dd class="mono"><a href=format!("/account?address={address}")>{address.clone()}</a></dd>
            }
            .into_any(),
        })
        .collect::<Vec<_>>();
    view! { <dl class="detail">{items}</dl> }
}

/// What a lookup page shows: an error, a record, a "not found", or the prompt.
fn outcome<T>(
    record: Option<T>,
    error: Option<String>,
    asked: bool,
    missing: String,
    prompt: &'static str,
    show: impl FnOnce(T) -> AnyView,
) -> AnyView {
    match (record, error) {
        (_, Some(text)) => view! { <p class="error">{text}</p> }.into_any(),
        (Some(r), None) => show(r),
        (None, None) if asked => view! { <p class="muted">{missing}</p> }.into_any(),
        (None, None) => view! { <p class="muted">{prompt}</p> }.into_any(),
    }
}

/// The banner for a name that no longer resolves, or `None` while it does.
#[must_use]
pub fn name_banner(n: &NameInfo) -> Option<String> {
    match n.status.as_str() {
        "active" => None,
        "grace" => Some(format!(
            "Expired at block {}. In its grace period: only the owner may renew it. It does not resolve; do not pay to it.",
            n.expires_at
        )),
        _ => Some(format!(
            "Expired at block {} and lapsed: anyone may register it again. It does not resolve; do not pay to it.",
            n.expires_at
        )),
    }
}

/// The name lookup page (ADR-135 P-3): who owns `label.maya` and, while it is
/// active, where it points. `error` explains a missing answer (no node, a node
/// without `get_name`, or a label the chain could never hold).
#[must_use]
pub fn name_page(query: String, info: Option<NameInfo>, error: Option<String>) -> String {
    let asked = !query.is_empty();
    let body = outcome(
        info,
        error,
        asked,
        format!("{query} is not registered."),
        "Enter a name to look up, e.g. alice.maya.",
        |n| {
            let banner = name_banner(&n).map(|text| view! { <p class="error">{text}</p> });
            let mut list = Vec::new();
            if let Some(target) = n.target.clone() {
                list.push(Row::Account("Points at", target));
            }
            list.push(Row::Account("Owner", n.owner.clone()));
            list.push(Row::Text(
                "Registered at block",
                n.registered_at.to_string(),
            ));
            list.push(Row::Text("Expires after block", n.expires_at.to_string()));
            list.push(Row::Text(
                "Status",
                format!("{} (at block {})", n.status, n.checked_at),
            ));
            view! {
                <section class="panel">
                    <h2>{format!("{}.maya", n.label)}</h2>
                    {banner}
                    {rows(list)}
                </section>
            }
            .into_any()
        },
    );
    page(
        "Names",
        "Name lookup",
        "/name",
        vec![("label", "name, e.g. alice.maya", query)],
        body,
    )
}

/// The asset page (ADR-135 P-1): supply, cap and who may mint.
#[must_use]
pub fn asset_page(query: String, info: Option<AssetInfo>, error: Option<String>) -> String {
    let body = outcome(
        info,
        error,
        !query.is_empty(),
        format!("No asset {query}."),
        "Enter an asset id (64 hex characters).",
        |a| {
            let authority = match a.mint_authority {
                Some(who) => Row::Account("Mint authority", who),
                None => Row::Text("Mint authority", "none: the supply can only fall".into()),
            };
            view! {
                <section class="panel">
                    <h2>{format!("Asset {}", a.symbol)}</h2>
                    <p class="muted">"Tickers are not unique; compare the id."</p>
                    {rows(vec![
                        Row::Text("Id", a.id),
                        Row::Account("Creator", a.creator),
                        Row::Text("Units in existence", crate::plain::group_digits(a.total_supply)),
                        Row::Text("Cap", crate::plain::group_digits(a.cap)),
                        authority,
                    ])}
                </section>
            }
            .into_any()
        },
    );
    page(
        "Assets",
        "Asset lookup",
        "/asset",
        vec![("id", "asset id, hex", query)],
        body,
    )
}

/// The NFT page (ADR-135 P-2): a collection and, given an index, one item.
#[must_use]
pub fn nft_page(
    query: (String, String),
    found: Option<(NftCollectionInfo, Option<NftItemInfo>)>,
    error: Option<String>,
) -> String {
    let (collection, index) = query;
    let body = outcome(
        found,
        error,
        !collection.is_empty(),
        format!("No collection {collection}."),
        "Enter a collection id (64 hex characters), and an item index to see one item.",
        |(c, item)| {
            let item = match (item, index.is_empty()) {
                (Some(i), _) => view! {
                    <section class="panel">
                        <h2>{format!("Item {}", i.index)}</h2>
                        {rows(vec![Row::Account("Held by", i.owner), Row::Text("Metadata hash", i.metadata_hash)])}
                    </section>
                }
                .into_any(),
                (None, false) => view! {
                    <p class="muted">{format!("Item {index} does not exist: never minted, or burnt.")}</p>
                }
                .into_any(),
                (None, true) => ().into_any(),
            };
            view! {
                <section class="panel">
                    <h2>"Collection"</h2>
                    {rows(vec![
                        Row::Text("Id", c.id),
                        Row::Account("Creator, the only minter", c.creator),
                        Row::Text("Minted", format!("{} of at most {}", c.minted, c.max_items)),
                        Row::Text("Metadata hash", c.metadata_hash),
                    ])}
                </section>
                {item}
            }
            .into_any()
        },
    );
    page(
        "NFTs",
        "NFT lookup",
        "/nft",
        vec![
            ("collection", "collection id, hex", collection.clone()),
            ("index", "item index", index.clone()),
        ],
        body,
    )
}

/// The scheduled-payment page (ADR-135 P-7).
#[must_use]
pub fn schedule_page(query: String, info: Option<ScheduleInfo>, error: Option<String>) -> String {
    let body = outcome(
        info,
        error,
        !query.is_empty(),
        format!("No schedule {query}: never created, cancelled, or paid out."),
        "Enter a schedule id (64 hex characters).",
        |s| {
            view! {
                <section class="panel">
                    <h2>"Scheduled payment"</h2>
                    {rows(vec![
                        Row::Text("Id", s.id),
                        Row::Account("Payer", s.payer),
                        Row::Account("Pays", s.payee),
                        Row::Text("Per run", format!("{} base units", crate::plain::group_digits(s.amount))),
                        Row::Text("Runs left", s.remaining.to_string()),
                        Row::Text("Next run at block", s.next_due.to_string()),
                        Row::Text("Every", format!("{} blocks", s.interval)),
                        Row::Text("Held in escrow, run fees included", format!("{} base units", crate::plain::group_digits(s.escrow))),
                        Row::Text("Runs missed in a row", s.missed.to_string()),
                    ])}
                </section>
            }
            .into_any()
        },
    );
    page(
        "Schedules",
        "Scheduled payment lookup",
        "/schedule",
        vec![("id", "schedule id, hex", query)],
        body,
    )
}

fn page(
    title: &'static str,
    heading: &'static str,
    action: &'static str,
    inputs: Vec<(&'static str, &'static str, String)>,
    body: AnyView,
) -> String {
    let inputs = inputs
        .into_iter()
        .map(|(name, placeholder, value)| view! { <input name=name placeholder=placeholder value=value/> })
        .collect::<Vec<_>>();
    render(view! {
        <Shell title=title.to_string()>
            <h1>{heading}</h1>
            <form class="search" method="get" action=action>
                {inputs}
                <button type="submit">"Look up"</button>
            </form>
            {body}
        </Shell>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use primitive_types::U256;

    fn name(status: &str, target: Option<String>) -> NameInfo {
        NameInfo {
            label: "alice".into(),
            owner: "aa".repeat(32),
            target,
            registered_at: 10,
            expires_at: 31_536_010,
            status: status.into(),
            checked_at: 20,
        }
    }

    #[test]
    fn an_active_name_shows_its_target_and_owner_in_full() {
        let info = name("active", Some("bb".repeat(32)));
        let html = name_page("alice.maya".into(), Some(info), None);
        assert!(html.contains("alice.maya"), "{html}");
        assert!(html.contains("Points at"), "{html}");
        assert!(html.contains(&format!("/account?address={}", "bb".repeat(32))));
        assert!(html.contains(&"aa".repeat(32)));
        assert!(html.contains("31536010"));
        assert!(!html.contains("does not resolve"), "{html}");
    }

    #[test]
    fn an_expired_name_says_it_does_not_resolve_instead_of_where_it_points() {
        let cases = [
            ("grace", "only the owner may renew it"),
            ("lapsed", "anyone may register it again"),
        ];
        for (status, words) in cases {
            let html = name_page("alice.maya".into(), Some(name(status, None)), None);
            assert!(
                html.contains("Expired at block 31536010"),
                "{status}: {html}"
            );
            assert!(html.contains(words), "{status}: {html}");
            assert!(html.contains("It does not resolve; do not pay to it."));
            assert!(!html.contains("Points at"), "{status}: {html}");
        }
    }

    #[test]
    fn a_free_name_and_a_refused_label_say_so() {
        let free = name_page("bob.maya".into(), None, None);
        assert!(free.contains("bob.maya is not registered."), "{free}");
        let why = Some("Not a name the chain can hold.".to_string());
        let refused = name_page("<b>".into(), None, why);
        assert!(refused.contains("Not a name the chain can hold."));
        assert!(!refused.contains("<b>"), "the query is escaped");
    }

    #[test]
    fn an_asset_shows_its_cap_and_who_may_mint() {
        let asset = AssetInfo {
            id: "51".repeat(32),
            symbol: "GOLD".into(),
            creator: "aa".repeat(32),
            total_supply: U256::from(1_000),
            cap: U256::from(5_000_000),
            mint_authority: None,
        };
        let html = asset_page("51".repeat(32), Some(asset), None);
        assert!(html.contains("Asset GOLD"), "{html}");
        assert!(html.contains("5,000,000"), "{html}");
        assert!(html.contains("none: the supply can only fall"), "{html}");
        assert!(html.contains(&format!("/account?address={}", "aa".repeat(32))));
    }

    #[test]
    fn an_nft_page_shows_the_collection_and_says_when_an_item_is_gone() {
        let collection = NftCollectionInfo {
            id: "52".repeat(32),
            creator: "ac".repeat(32),
            max_items: 10,
            minted: 2,
            metadata_hash: "01".repeat(32),
        };
        let item = NftItemInfo {
            collection: "52".repeat(32),
            index: 1,
            owner: "ad".repeat(32),
            metadata_hash: "02".repeat(32),
        };
        let query = ("52".repeat(32), "1".to_string());
        let held = nft_page(query, Some((collection.clone(), Some(item))), None);
        assert!(held.contains("2 of at most 10"), "{held}");
        assert!(held.contains("Item 1"), "{held}");
        assert!(held.contains(&format!("/account?address={}", "ad".repeat(32))));
        let query = ("52".repeat(32), "7".to_string());
        let gone = nft_page(query, Some((collection, None)), None);
        assert!(gone.contains("Item 7 does not exist"), "{gone}");
    }

    #[test]
    fn a_schedule_shows_its_runs_and_escrow() {
        let schedule = ScheduleInfo {
            id: "53".repeat(32),
            payer: "ae".repeat(32),
            payee: "af".repeat(32),
            amount: U256::from(1_500_000),
            next_due: 40,
            interval: 20,
            remaining: 3,
            escrow: U256::from(4_500_003),
            missed: 0,
        };
        let html = schedule_page("53".repeat(32), Some(schedule), None);
        assert!(html.contains("1,500,000 base units"), "{html}");
        assert!(html.contains("4,500,003 base units"), "{html}");
        assert!(html.contains(&format!("/account?address={}", "af".repeat(32))));
        let none = schedule_page("53".repeat(32), None, None);
        assert!(
            none.contains("never created, cancelled, or paid out"),
            "{none}"
        );
    }
}
