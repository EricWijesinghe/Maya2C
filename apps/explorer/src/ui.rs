//! Leptos server-rendered views.
//!
//! ## SSR without a client bundle
//!
//! These render to HTML on the server. Leptos signals still drive the render —
//! that is what `to_html` walks — but there is no WASM bundle rehydrating them
//! in the browser, so interactivity comes from the WebSocket endpoints instead.
//!
//! A hydrating build would need `cargo-leptos` and `wasm-bindgen` to produce a
//! client bundle. That is a build-tooling change, not a rewrite: the components
//! below are the same ones a hydrating build would use.

use leptos::prelude::*;

use crate::model::{IndexedBlock, IndexedTx, NetworkStats};

/// Renders a component tree to an HTML string.
fn render(view: impl IntoView + 'static) -> String {
    view.into_view().to_html()
}

/// Formats a hashrate with a unit, so a dashboard is readable at any scale.
#[must_use]
pub fn format_hashrate(rate: f64) -> String {
    const UNITS: [&str; 6] = ["H/s", "kH/s", "MH/s", "GH/s", "TH/s", "PH/s"];
    if !rate.is_finite() || rate <= 0.0 {
        return "0 H/s".to_string();
    }

    let mut value = rate;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

/// A Unix timestamp as a UTC time of day, `HH:MM:SS UTC`: at one block a
/// second, the time of day is what tells blocks apart, and raw seconds since
/// 1970 told a reader nothing.
#[must_use]
pub fn utc_time(unix_seconds: i64) -> String {
    let day = unix_seconds.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02} UTC",
        day / 3_600,
        day % 3_600 / 60,
        day % 60
    )
}

/// Blocks per minute from an average block time in seconds.
#[must_use]
pub fn blocks_per_minute(average_block_time: f64) -> String {
    if !average_block_time.is_finite() || average_block_time <= 0.0 {
        return "0".to_string();
    }
    format!("{:.1}", 60.0 / average_block_time)
}

/// Shortens a hex identifier for display.
#[must_use]
pub fn short_hash(hash: &str) -> String {
    if hash.len() <= 16 {
        return hash.to_string();
    }
    format!("{}…{}", &hash[..8], &hash[hash.len() - 6..])
}

// ---------------------------------------------------------------------------
// components
// ---------------------------------------------------------------------------

/// Page chrome: navigation, styles, and the live-update client.
#[component]
pub fn Shell(
    /// Page title.
    title: String,
    /// Page body.
    children: Children,
) -> impl IntoView {
    view! {
        <html data-theme="command">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>{format!("{title} · Maya2C Explorer")}</title>

                // `og:title` is the site name rather than the per-page title,
                // which the `<title>` above does carry. Leptos compiles a
                // static `property=` to an attribute but a dynamic one to a
                // `.property()` DOM call, which `<meta>` has no such thing for
                // — so a per-page value here does not build. Worth knowing
                // before someone tries again.
                //
                // Root-absolute paths, unlike the trunk-built frontends: the explorer
                // serves every page from its own origin root, and a relative
                // path would resolve against `/blocks/1234` on a detail page.
                <link rel="icon" type="image/png" sizes="48x48" href="/assets/favicon-48x48.png"/>
                <link rel="icon" type="image/png" sizes="32x32" href="/assets/favicon-32x32.png"/>
                <link rel="icon" type="image/png" sizes="16x16" href="/assets/favicon-16x16.png"/>
                <link rel="shortcut icon" href="/favicon.ico"/>
                <link rel="apple-touch-icon" sizes="180x180" href="/assets/apple-touch-icon.png"/>
                <link rel="manifest" href="/assets/site.webmanifest"/>
                <meta name="theme-color" content="#030716"/>

                <meta property="og:type" content="website"/>
                <meta property="og:site_name" content="Maya2C Explorer"/>
                <meta property="og:title" content="Maya2C Explorer"/>
                <meta property="og:image" content="/assets/og-image.png"/>
                <meta name="twitter:card" content="summary_large_image"/>
                <meta name="twitter:image" content="/assets/og-image.png"/>

                <style>{styles()}</style>
            </head>
            <body>
                <nav class="nav">
                    <a class="brand" href="/">
                        // The emblem is the mark; the wordmark span is the
                        // readable name beside it, matching the main site's
                        // header. Empty alt keeps the name from being read
                        // twice, since the span already carries it.
                        <img
                            class="emblem"
                            src="/assets/emblem.png"
                            alt=""
                            width="40"
                            height="40"
                        />
                        <span class="word">"Maya2C"</span>
                    </a>
                    <a href="/">"Dashboard"</a>
                    <a href="/blocks">"Blocks"</a>
                    <a href="/tx">"Transactions"</a>
                    <a href="/account">"Accounts"</a>
                    <a href="/validators">"Validators"</a>
                </nav>
                <main>{children()}</main>
                <script>{LIVE_SCRIPT}</script>
                // The HUD look (/assets/hud.js, which loads hud.css) can change
                // without a rebuild. Deferred, so first paint still needs only
                // this document; without it the inline theme above stands.
                <script src="/assets/hud.js" defer></script>
            </body>
        </html>
    }
}

/// Network summary tiles.
#[component]
pub fn StatsPanel(
    /// Current network statistics.
    stats: NetworkStats,
) -> impl IntoView {
    view! {
        <section class="tiles">
            <div class="tile">
                <span class="label">"Hashrate"</span>
                <span class="value" id="stat-hashrate">{format_hashrate(stats.hashrate)}</span>
            </div>
            <div class="tile">
                <span class="label">"Height"</span>
                <span class="value" id="stat-height">{stats.height.to_string()}</span>
            </div>
            <div class="tile">
                <span class="label">"Blocks / min"</span>
                <span class="value">{blocks_per_minute(stats.average_block_time)}</span>
            </div>
            <div class="tile">
                <span class="label">"Avg block time"</span>
                <span class="value">{format!("{:.1}s", stats.average_block_time)}</span>
            </div>
            <div class="tile">
                <span class="label">"Consensus"</span>
                <span class="value">"DAG-BFT"</span>
            </div>
            <div class="tile">
                <span class="label">"Blocks indexed"</span>
                <span class="value">{stats.blocks_indexed.to_string()}</span>
            </div>
            <div class="tile">
                <span class="label">"Transactions"</span>
                <span class="value">{stats.transactions_indexed.to_string()}</span>
            </div>
        </section>
    }
}

/// Seconds between consecutive recent blocks, oldest first: on a DAG-BFT
/// chain this is the pulse worth watching (a mining hash rate means nothing
/// there, since no block is mined).
#[component]
pub fn BlockTimeChart(
    /// Recent blocks, newest first.
    blocks: Vec<IndexedBlock>,
) -> impl IntoView {
    let mut times: Vec<(i64, u64)> = blocks
        .windows(2)
        .map(|w| {
            let gap = w[0].timestamp.saturating_sub(w[1].timestamp).max(0);
            (w[0].height, u64::try_from(gap).unwrap_or(0))
        })
        .collect();
    times.reverse();
    // Scaled against the window maximum, with a floor so a steady chain still
    // shows bars rather than a flat line.
    let peak = times.iter().map(|t| t.1).max().unwrap_or(1).max(2);
    let bars: Vec<_> = times
        .iter()
        .map(|(height, secs)| {
            #[allow(clippy::cast_precision_loss)]
            let pct = ((*secs as f64 / peak as f64) * 100.0).clamp(6.0, 100.0);
            let label = format!("block {height} · {secs}s after its parent");
            view! {
                <div class="bar" style=format!("height:{pct:.1}%") title=label></div>
            }
        })
        .collect();

    view! {
        <section class="panel">
            <h2>"Block time"</h2>
            <div class="chart">{bars}</div>
            <p class="muted">"Seconds between each recent block and its parent. Every block shown is final: DAG-BFT does not reorganise."</p>
        </section>
    }
}

/// The newest blocks as cubes on a slowly turning 3D ribbon. Each cube is
/// positioned in Rust (`--x`, `--z`, `--d`) so the layout needs no script, and
/// the live client re-lays the same grid when a new block lands.
#[component]
#[allow(clippy::cast_precision_loss)]
pub fn BlockRibbon(
    /// Blocks, newest first.
    blocks: Vec<IndexedBlock>,
) -> impl IntoView {
    let count = blocks.len().min(RIBBON_CUBES);
    let cubes: Vec<_> = blocks
        .iter()
        .take(RIBBON_CUBES)
        .enumerate()
        .map(|(i, block)| {
            // Newest sits at the right end; the ribbon arcs toward the viewer
            // in the middle and falls away at the ends.
            let slot = count.saturating_sub(1) - i;
            let (x, z) = ribbon_slot(slot, count);
            let style = format!("--x:{x:.0}px;--z:{z:.0}px;--d:-{:.2}s", slot as f64 * 1.1);
            let href = format!("/blocks/{}", block.height);
            let label = format!("block {} · {}", block.height, utc_time(block.timestamp));
            view! {
                <a class="cube" style=style href=href title=label>
                    <span class="f front">{block.height.to_string()}</span>
                    <span class="f back"></span>
                    <span class="f top"></span>
                    <span class="f bottom"></span>
                    <span class="f left"></span>
                    <span class="f right"></span>
                </a>
            }
        })
        .collect();

    view! {
        <section class="panel">
            <h2>"Ledger ribbon"</h2>
            <div class="ribbon" aria-label="Newest blocks">
                <div class="stage" id="ribbon">{cubes}</div>
            </div>
        </section>
    }
}

/// How many cubes the ribbon shows.
pub const RIBBON_CUBES: usize = 12;

/// Horizontal spacing (px) between ribbon cubes.
const RIBBON_SPACING: f64 = 72.0;

/// Position of a ribbon slot: `x` from the centre, `z` toward the viewer. The
/// ribbon bows forward in the middle, so it reads as 3D rather than a row.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn ribbon_slot(slot: usize, count: usize) -> (f64, f64) {
    let centre = (count.saturating_sub(1)) as f64 / 2.0;
    let offset = slot as f64 - centre;
    let x = offset * RIBBON_SPACING;
    let bow = (1.0 - (offset / (centre.max(1.0) + 0.5)).powi(2)).max(0.0);
    (x, 60.0 * bow)
}

/// A table of blocks.
#[component]
pub fn BlockTable(
    /// Blocks, newest first.
    blocks: Vec<IndexedBlock>,
) -> impl IntoView {
    let rows: Vec<_> = blocks
        .iter()
        .map(|block| {
            let href = format!("/blocks/{}", block.height);
            view! {
                <tr>
                    <td><a href=href>{block.height.to_string()}</a></td>
                    <td class="mono">{short_hash(&block.id)}</td>
                    <td>{block.tx_count.to_string()}</td>
                    <td>{utc_time(block.timestamp)}</td>
                    <td class="mono">{short_hash(&block.state_root)}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <section class="panel">
            <h2>"Latest blocks"</h2>
            <div class="tablewrap">
                <table>
                    <thead>
                        <tr>
                            <th>"Height"</th><th>"Id"</th><th>"Txs"</th>
                            <th>"Time"</th><th>"State root"</th>
                        </tr>
                    </thead>
                    <tbody id="block-rows">{rows}</tbody>
                </table>
            </div>
        </section>
    }
}

/// A table of transactions.
#[component]
pub fn TxTable(
    /// Transactions to show.
    transactions: Vec<IndexedTx>,
) -> impl IntoView {
    let rows: Vec<_> = transactions
        .iter()
        .map(|tx| {
            let href = format!("/tx/{}", tx.txid);
            view! {
                <tr>
                    <td class="mono"><a href=href>{short_hash(&tx.txid)}</a></td>
                    <td>{tx.height.to_string()}</td>
                    <td class="mono">{short_hash(&tx.sender)}</td>
                    <td>{kind_label(tx)}</td>
                    <td>{tx.nonce.to_string()}</td>
                    <td>{tx.total_out.to_string()}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <div class="tablewrap">
            <table>
                <thead>
                    <tr>
                        <th>"Txid"</th><th>"Height"</th><th>"Sender"</th>
                        <th>"Kind"</th><th>"Nonce"</th><th>"Value"</th>
                    </tr>
                </thead>
                <tbody id="tx-rows">{rows}</tbody>
            </table>
        </div>
    }
}

// ---------------------------------------------------------------------------
// pages
// ---------------------------------------------------------------------------

/// The dashboard.
#[must_use]
pub fn dashboard_page(stats: NetworkStats, blocks: Vec<IndexedBlock>) -> String {
    render(view! {
        <Shell title="Dashboard".to_string()>
            <h1>"maya-testnet-1"<span class="pulse" id="pulse" aria-hidden="true"></span></h1>
            <p class="lead">"Post-quantum layer 1 · DAG-BFT finality · every block below is final"</p>
            <StatsPanel stats=stats/>
            <BlockRibbon blocks=blocks.clone()/>
            <BlockTimeChart blocks=blocks.clone()/>
            <BlockTable blocks=blocks/>
        </Shell>
    })
}

/// The block list.
#[must_use]
pub fn blocks_page(blocks: Vec<IndexedBlock>) -> String {
    render(view! {
        <Shell title="Blocks".to_string()>
            <h1>"Blocks"</h1>
            <BlockTable blocks=blocks/>
        </Shell>
    })
}

/// The kind column: the transaction kind, marked when signing it needs care.
/// Blank for a row indexed from a node that predates `core::describe`.
fn kind_label(tx: &IndexedTx) -> String {
    match (tx.kind.is_empty(), tx.careful) {
        (true, _) => "-".to_string(),
        (false, true) => format!("{} (check first)", tx.kind),
        (false, false) => tx.kind.clone(),
    }
}

/// A single block with its transactions.
#[must_use]
pub fn block_page(block: IndexedBlock, transactions: Vec<IndexedTx>) -> String {
    render(view! {
        <Shell title=format!("Block {}", block.height)>
            <h1>{format!("Block {}", block.height)}</h1>
            <section class="panel">
                <dl class="detail">
                    <dt>"Id"</dt><dd class="mono">{block.id.clone()}</dd>
                    <dt>"Parent"</dt><dd class="mono">{block.prev_hash.clone()}</dd>
                    <dt>"State root"</dt><dd class="mono">{block.state_root.clone()}</dd>
                    <dt>"Time"</dt><dd>{format!("{} ({})", utc_time(block.timestamp), block.timestamp)}</dd>
                    <dt>"Finality"</dt><dd>"Final: derived from committed DAG-BFT certificates"</dd>
                </dl>
            </section>
            <section class="panel">
                <h2>{format!("{} transaction(s)", transactions.len())}</h2>
                <TxTable transactions=transactions/>
            </section>
        </Shell>
    })
}

/// The transaction inspector.
#[must_use]
pub fn transaction_page(transaction: Option<IndexedTx>, query: String) -> String {
    let body = match transaction {
        Some(tx) => view! {
            <p class="plain">{crate::plain::describe(&tx)}</p>
            <details class="panel">
                <summary>"Technical view"</summary>
                <dl class="detail">
                    <dt>"Txid"</dt><dd class="mono">{tx.txid.clone()}</dd>
                    <dt>"Block"</dt>
                    <dd><a href=format!("/blocks/{}", tx.height)>{tx.height.to_string()}</a></dd>
                    <dt>"Sender"</dt>
                    <dd class="mono">
                        <a href=format!("/account?address={}", tx.sender)>{tx.sender.clone()}</a>
                    </dd>
                    <dt>"Kind"</dt><dd>{kind_label(&tx)}</dd>
                    <dt>"Nonce"</dt><dd>{tx.nonce.to_string()}</dd>
                    <dt>"Outputs"</dt><dd>{tx.output_count.to_string()}</dd>
                    <dt>"Value"</dt><dd>{tx.total_out.to_string()}</dd>
                    <dt>"Signed"</dt><dd>{if tx.signed { "yes" } else { "no" }}</dd>
                </dl>
            </details>
        }
        .into_any(),
        None if query.is_empty() => view! {
            <p class="muted">"Enter a transaction id to inspect."</p>
        }
        .into_any(),
        None => view! {
            <p class="error">{format!("No transaction found for {query}")}</p>
        }
        .into_any(),
    };

    render(view! {
        <Shell title="Transaction".to_string()>
            <h1>"Transaction inspector"</h1>
            <form class="search" method="get" action="/tx">
                <input name="txid" placeholder="transaction id (hex)" value=query.clone()/>
                <button type="submit">"Inspect"</button>
            </form>
            {body}
        </Shell>
    })
}

/// The account lookup page.
#[must_use]
pub fn account_page(
    address: String,
    balance: Option<primitive_types::U256>,
    nonce: Option<u64>,
    transactions: Vec<IndexedTx>,
    error: Option<String>,
) -> String {
    let summary = match (balance, nonce) {
        (Some(balance), Some(nonce)) => view! {
            <section class="tiles">
                <div class="tile">
                    <span class="label">"Balance"</span>
                    <span class="value">{balance.to_string()}</span>
                </div>
                <div class="tile">
                    <span class="label">"Nonce"</span>
                    <span class="value">{nonce.to_string()}</span>
                </div>
                <div class="tile">
                    <span class="label">"Transactions"</span>
                    <span class="value">{transactions.len().to_string()}</span>
                </div>
            </section>
        }
        .into_any(),
        _ => view! { <span></span> }.into_any(),
    };

    let message = match error {
        Some(text) => view! { <p class="error">{text}</p> }.into_any(),
        None => view! { <span></span> }.into_any(),
    };

    render(view! {
        <Shell title="Account".to_string()>
            <h1>"Account"</h1>
            <form class="search" method="get" action="/account">
                <input name="address" placeholder="address (64 hex chars)" value=address.clone()/>
                <button type="submit">"Look up"</button>
            </form>
            {message}
            {summary}
            <section class="panel">
                <h2>"Transactions sent"</h2>
                <TxTable transactions=transactions/>
            </section>
        </Shell>
    })
}

/// The validator set: who is in this epoch's committee, who is jailed and
/// until when. `error` explains a missing answer (node without the RPC).
#[must_use]
pub fn validators_page(
    info: Option<custom_l1_node::rpc::validators::ValidatorsInfo>,
    error: Option<String>,
) -> String {
    let message = match error {
        Some(text) => view! { <p class="error">{text}</p> }.into_any(),
        None => view! { <span></span> }.into_any(),
    };
    let body = match info {
        None => view! { <span></span> }.into_any(),
        Some(info) => {
            let summary = format!(
                "Epoch {} · committee of {} · {} registered",
                info.epoch, info.committee, info.total
            );
            let rows: Vec<_> = info
                .validators
                .iter()
                .map(|v| {
                    let standing = match v.jailed_until {
                        Some(epoch) => format!("jailed, eligible again at epoch {epoch}"),
                        None => v.status.clone(),
                    };
                    let seat = if v.in_committee { "committee" } else { "-" };
                    view! {
                        <tr>
                            <td>{seat}</td>
                            <td class="mono">{short_hash(&v.id)}</td>
                            <td>{standing}</td>
                            <td>{v.self_bond.clone()}</td>
                            <td>{v.delegated.clone()}</td>
                            <td>{format!("{}%", f64::from(v.commission_bps) / 100.0)}</td>
                        </tr>
                    }
                })
                .collect();
            view! {
                <section class="panel">
                    <h2>{summary}</h2>
                    <div class="tablewrap">
                        <table>
                            <thead>
                                <tr>
                                    <th>"Seat"</th><th>"Validator"</th><th>"Standing"</th>
                                    <th>"Self bond"</th><th>"Delegated"</th><th>"Commission"</th>
                                </tr>
                            </thead>
                            <tbody>{rows}</tbody>
                        </table>
                    </div>
                </section>
            }
            .into_any()
        }
    };
    render(view! {
        <Shell title="Validators".to_string()>
            <h1>"Validators"</h1>
            {message}
            {body}
        </Shell>
    })
}

/// A 404 page.
#[must_use]
pub fn not_found_page(what: String) -> String {
    render(view! {
        <Shell title="Not found".to_string()>
            <h1>"Not found"</h1>
            <p class="error">{what}</p>
        </Shell>
    })
}

// ---------------------------------------------------------------------------
// assets
// ---------------------------------------------------------------------------

/// The shared design-system tokens, then the explorer's own rules. The
/// explorer renders the "command" theme, its signature dark look; the calm
/// theme is the same tokens under `data-theme="calm"`.
fn styles() -> String {
    format!("{}{STYLES}", maya_design_system::tokens::css())
}

const STYLES: &str = r#"
/* Living quantum ledger: deep-sea cyan and indigo. Blocks are cubes on a
   slow 3D ribbon and the finality pulse beats once per block. Every motion is
   held behind prefers-reduced-motion, which leaves a static ribbon. */
:root, [data-theme="command"] { --accent:#22d3ee; --focus:#818cf8; --on-accent:#021018; --deep:#030716; }
* { box-sizing:border-box; }
body { margin:0; font:14px/1.5 "Inter",ui-sans-serif,system-ui,sans-serif; color:var(--fg);
       background:var(--deep); min-height:100vh; position:relative; isolation:isolate; }
body::before { content:""; position:fixed; inset:0; z-index:-2; pointer-events:none;
       background:
         radial-gradient(60rem 30rem at 12% -8%, rgb(34 211 238 / 0.18), transparent 60%),
         radial-gradient(52rem 26rem at 92% 2%, rgb(99 102 241 / 0.20), transparent 60%),
         radial-gradient(46rem 30rem at 50% 115%, rgb(14 116 144 / 0.16), transparent 62%);
       background-size:200% 200%; animation:aurora 24s ease-in-out infinite; }
body::after { content:""; position:fixed; inset:0; z-index:-1; pointer-events:none; opacity:0.5;
       background-image:
         linear-gradient(rgb(34 211 238 / 0.05) 1px, transparent 1px),
         linear-gradient(90deg, rgb(99 102 241 / 0.06) 1px, transparent 1px);
       background-size:44px 44px;
       -webkit-mask-image:radial-gradient(120% 90% at 50% 0%, #000 35%, transparent 75%);
       mask-image:radial-gradient(120% 90% at 50% 0%, #000 35%, transparent 75%); }
@keyframes aurora { 0%,100% { background-position:0% 0%; } 50% { background-position:100% 100%; } }
h1 { font-size:1.4rem; margin:0 0 1rem; background:linear-gradient(100deg,#67e8f9,#818cf8 50%,#22d3ee);
     background-size:220% auto; -webkit-background-clip:text; background-clip:text; color:transparent;
     animation:sheen 8s linear infinite; }
@keyframes sheen { to { background-position:220% center; } }
h2 { font-size:0.95rem; margin:0 0 0.75rem; color:var(--muted);
     text-transform:uppercase; letter-spacing:0.06em; }
.lead { color:var(--muted); margin:-0.5rem 0 1.25rem; }
.nav { display:flex; flex-wrap:wrap; gap:0.6rem 1.25rem; align-items:center; padding:0.8rem 1.25rem;
       border-bottom:1px solid var(--border); background:rgb(3 7 22 / 0.72);
       backdrop-filter:blur(10px); -webkit-backdrop-filter:blur(10px);
       position:sticky; top:0; z-index:5; }
.nav::after { content:""; position:absolute; left:0; right:0; bottom:-1px; height:1px;
       background:linear-gradient(90deg, transparent, #22d3ee 35%, #818cf8 65%, transparent); opacity:0.7; }
.nav a { color:var(--muted); text-decoration:none; transition:color .18s ease; }
.nav a:hover { color:var(--fg); }
.nav .brand { display:flex; align-items:center; gap:0.55rem; margin-right:0.5rem; }
/* Square emblem with its size on the element, so the nav never reflows on load. */
.nav .brand .emblem { height:2rem; width:2rem; display:block;
       filter:drop-shadow(0 0 10px rgb(34 211 238 / 0.45)); }
.nav .brand .word { font-weight:800; font-size:1.15rem; letter-spacing:-0.01em;
       background:linear-gradient(100deg,#22d3ee,#818cf8 70%); -webkit-background-clip:text;
       background-clip:text; color:transparent; }
main { max-width:1100px; margin:0 auto; padding:1.5rem; }
.tiles { display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr));
         gap:0.75rem; margin-bottom:1.5rem; }
.tile, .panel { position:relative;
         background:linear-gradient(160deg, rgb(129 140 248 / 0.07), rgb(255 255 255 / 0.01)), var(--panel);
         border:1px solid var(--border); border-radius:12px;
         box-shadow:0 1px 0 rgb(255 255 255 / 0.04) inset, 0 10px 30px -18px rgb(0 0 0 / 0.9); }
.tile::before, .panel::before { content:""; position:absolute; inset:0; border-radius:inherit;
         padding:1px; pointer-events:none;
         background:linear-gradient(140deg, rgb(34 211 238 / 0.55), transparent 40%, transparent 60%, rgb(129 140 248 / 0.5));
         -webkit-mask:linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0);
         -webkit-mask-composite:xor; mask-composite:exclude; opacity:0; transition:opacity .25s ease; }
.tile { padding:0.85rem 1rem; display:flex; flex-direction:column; gap:0.35rem;
        transition:transform .2s ease, box-shadow .2s ease; }
.tile:hover { transform:translateY(-2px);
        box-shadow:0 1px 0 rgb(255 255 255 / 0.06) inset, 0 16px 40px -18px rgb(34 211 238 / 0.45); }
.tile:hover::before, .panel:hover::before { opacity:1; }
.tile .label { color:var(--muted); font-size:0.72rem; text-transform:uppercase; letter-spacing:0.06em; }
.tile .value { font-size:1.3rem; color:#e0f2fe; font-variant-numeric:tabular-nums; overflow-wrap:anywhere; }
#stat-height { text-shadow:0 0 18px rgb(34 211 238 / 0.55); animation:pulse 2.6s ease-in-out infinite; }
@keyframes pulse { 0%,100% { text-shadow:0 0 10px rgb(34 211 238 / 0.35); }
                   50% { text-shadow:0 0 22px rgb(34 211 238 / 0.8); } }
.panel { padding:1rem; margin-bottom:1.5rem; }
/* The 3D ribbon: each block is a cube on a shared stage that turns slowly. */
.ribbon { position:relative; height:190px; perspective:700px; overflow:hidden; margin-bottom:0.5rem; }
.stage { position:absolute; inset:0; transform-style:preserve-3d; transform:rotateX(-10deg); }
.cube { position:absolute; left:50%; top:50%; width:44px; height:44px; margin:-22px 0 0 -22px;
        transform-style:preserve-3d; transform:translate3d(var(--x),0,var(--z)) rotateX(-22deg);
        animation:turn 16s linear infinite; animation-delay:var(--d); }
.cube .f { position:absolute; inset:0; display:grid; place-items:center; border:1px solid rgb(103 232 249 / 0.75);
           background:linear-gradient(150deg, rgb(34 211 238 / 0.34), rgb(99 102 241 / 0.3));
           box-shadow:0 0 16px rgb(34 211 238 / 0.35) inset, 0 0 12px rgb(34 211 238 / 0.25);
           font:600 11px ui-monospace,SFMono-Regular,Menlo,monospace; color:#e0f7ff; }
.cube .front  { transform:translateZ(22px); }
.cube .back   { transform:rotateY(180deg) translateZ(22px); }
.cube .top    { transform:rotateX(90deg) translateZ(22px); }
.cube .bottom { transform:rotateX(-90deg) translateZ(22px); }
.cube .left   { transform:rotateY(-90deg) translateZ(22px); }
.cube .right  { transform:rotateY(90deg) translateZ(22px); }
.cube:hover .f, .cube:focus-visible .f { border-color:#fff; box-shadow:0 0 22px rgb(103 232 249 / 0.9); }
.cube:focus-visible { outline:none; }
@keyframes turn { from { transform:translate3d(var(--x),0,var(--z)) rotateX(-22deg) rotateY(0deg); }
                  to { transform:translate3d(var(--x),0,var(--z)) rotateX(-22deg) rotateY(360deg); } }
/* The finality pulse: one ring per block, beaten from the live socket. */
.pulse { position:relative; display:inline-block; width:12px; height:12px; margin-left:0.6rem;
         vertical-align:middle; border-radius:50%; background:var(--accent); box-shadow:0 0 14px var(--accent); }
.pulse::after { content:""; position:absolute; inset:-6px; border-radius:50%; border:2px solid var(--accent);
         animation:ping 2.4s ease-out infinite; }
.pulse.beat::before { content:""; position:absolute; inset:-2px; border-radius:50%; background:#fff;
         animation:flash .6s ease-out forwards; }
@keyframes ping { from { transform:scale(.6); opacity:.9; } to { transform:scale(2.6); opacity:0; } }
@keyframes flash { from { opacity:.9; } to { opacity:0; } }
.tablewrap { overflow-x:auto; -webkit-overflow-scrolling:touch; }
table { width:100%; border-collapse:collapse; }
th,td { text-align:left; padding:0.5rem 0.6rem; border-bottom:1px solid var(--border); white-space:nowrap; }
tbody tr { transition:background .15s ease; }
tbody tr:hover { background:rgb(34 211 238 / 0.05); }
tbody tr:first-child { animation:rowin .6s ease; }
@keyframes rowin { from { background:rgb(34 211 238 / 0.18); } to { background:transparent; } }
th { color:var(--muted); font-size:0.72rem; text-transform:uppercase; letter-spacing:0.06em; font-weight:500; }
td a { color:var(--accent); text-decoration:none; }
td a:hover { text-shadow:0 0 12px rgb(34 211 238 / 0.6); }
.mono { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:0.85em; }
.muted { color:var(--muted); }
.error { color:var(--danger); }
.plain { font-size:1.05rem; margin:0 0 1rem; }
details.panel summary { cursor:pointer; color:var(--muted); margin-bottom:0.75rem; }
:focus-visible { outline:2px solid var(--focus); outline-offset:2px; }
.chart { display:flex; align-items:flex-end; gap:3px; height:120px; padding-top:0.5rem; }
.bar { flex:1; min-width:2px; background:linear-gradient(180deg,#22d3ee,#6366f1); opacity:0.85;
       border-radius:3px 3px 0 0; box-shadow:0 0 10px rgb(34 211 238 / 0.25);
       transition:opacity .18s ease, box-shadow .18s ease; }
.bar:hover { opacity:1; box-shadow:0 0 16px rgb(34 211 238 / 0.7); }
.detail { display:grid; grid-template-columns:150px 1fr; gap:0.4rem 1rem; margin:0; }
.detail dt { color:var(--muted); }
.detail dd { margin:0; overflow-wrap:anywhere; }
.search { display:flex; gap:0.5rem; margin-bottom:1.25rem; }
.search input { flex:1; min-width:0; padding:0.55rem 0.7rem; background:var(--panel);
                border:1px solid var(--border); border-radius:8px; color:var(--fg); font-family:ui-monospace,monospace;
                transition:border-color .18s ease, box-shadow .18s ease; }
.search input:focus { border-color:var(--accent); box-shadow:0 0 0 3px rgb(34 211 238 / 0.18); outline:none; }
.search button { padding:0.55rem 1.1rem; background:var(--accent); color:var(--on-accent);
                 min-height:var(--touch-target); border:0; border-radius:8px; font-weight:600; cursor:pointer;
                 box-shadow:0 0 18px rgb(34 211 238 / 0.35); transition:filter .18s ease; }
.search button:hover { filter:brightness(1.1); }
@media (max-width:600px) {
  main { padding:1rem; }
  .detail { grid-template-columns:1fr; }
  .search { flex-direction:column; }
  .ribbon { height:170px; }
}
@media (prefers-reduced-motion: reduce) {
  body::before, h1, #stat-height, tbody tr:first-child, .cube, .pulse::after, .pulse.beat::before { animation:none; }
  .cube { transform:translate3d(var(--x),0,var(--z)) rotateX(-22deg); }
  .tile { transition:none; }
}
/* Site palette (maya2c.dev): abyss background, signal teal, bronze-gold, verified
   green. Flat panels, no neon or gradient text; the ribbon is a flat wave of
   chips (no spin, only a slow sine bob). Overrides the living-ledger rules above. */
:root, [data-theme="command"] { --accent:#1fb5c2; --focus:#e6b260; --on-accent:#08100f; --deep:#050b0f;
  --panel:rgba(14,30,36,0.62); --border:rgba(150,226,232,0.13); --fg:#edf3f2; --muted:#93a4aa; }
body::before { animation:none; background:
  radial-gradient(50rem 28rem at 8% -5%, rgb(31 181 194 / 0.28), transparent 62%),
  radial-gradient(44rem 26rem at 95% 5%, rgb(69 224 160 / 0.14), transparent 62%),
  radial-gradient(46rem 26rem at 75% 105%, rgb(230 178 96 / 0.18), transparent 62%),
  radial-gradient(40rem 24rem at 0% 90%, rgb(126 234 240 / 0.12), transparent 62%); }
body::after { opacity:0.6; background-image:radial-gradient(rgb(255 255 255 / 0.09) 1px, transparent 1px);
  background-size:22px 22px; -webkit-mask-image:linear-gradient(#000 20%, transparent 85%); mask-image:linear-gradient(#000 20%, transparent 85%); }
.nav::after, .tile::before, .panel::before, .pulse::after { display:none; }
h1 { background:none; color:var(--fg); animation:none; font-weight:650; letter-spacing:-0.01em; }
.nav { background:#0d1216; backdrop-filter:none; -webkit-backdrop-filter:none; }
.nav .brand .emblem { filter:none; }
.nav .brand .word { background:none; color:var(--fg); -webkit-text-fill-color:currentColor; }
.tile, .panel { background:var(--panel); border:1px solid var(--border); border-radius:6px; box-shadow:none; }
.tile { border-top:3px solid hsl(calc(186 - var(--n,0) * 18) 62% 56%); }
.tile:nth-child(1){--n:0} .tile:nth-child(2){--n:1} .tile:nth-child(3){--n:2} .tile:nth-child(4){--n:3}
.tile:nth-child(5){--n:4} .tile:nth-child(6){--n:5} .tile:nth-child(7){--n:6} .tile:nth-child(8){--n:7}
.tile:hover, .panel:hover { transform:none; box-shadow:none; border-color:#33424d; }
.tile .value { color:#fff; font-weight:650; }
.panel h2 { color:var(--fg); }
.panel h2::before { content:""; display:inline-block; width:3px; height:0.9em; margin-right:0.5rem;
  vertical-align:-0.1em; border-radius:2px; background:linear-gradient(#1fb5c2,#e6b260); }
.bar { background:var(--accent); opacity:0.8; border-radius:2px 2px 0 0; box-shadow:none; }
.bar:hover { opacity:1; box-shadow:none; background:var(--focus); }
.pulse { box-shadow:none; width:8px; height:8px; }
.search input { border-radius:4px; } .search input:focus { box-shadow:none; }
.search button { border-radius:4px; box-shadow:none; }
.ribbon { perspective:none; background:linear-gradient(90deg, transparent, rgb(31 181 194 / 0.08), transparent); border-radius:6px; }
.ribbon::before { content:""; position:absolute; left:4%; right:4%; top:50%; height:2px; opacity:0.5;
  background:linear-gradient(90deg, #1fb5c2, #7eeaf0 35%, #45e0a0 60%, #e6b260); }
.stage { transform:none; }
.cube { width:48px; height:34px; margin:-17px 0 0 -24px; transform:translateX(var(--x));
  animation:bob 6s ease-in-out infinite; animation-delay:var(--d); }
.cube .f:not(.front) { display:none; }
.cube .front { transform:none; border-radius:7px; font-size:11px; color:#fff; box-shadow:none;
  border:1px solid hsl(var(--h,186) 70% 72% / 0.8);
  background:linear-gradient(160deg, hsl(var(--h,186) 62% 48%), hsl(var(--h,186) 60% 30%)); }
.cube:nth-child(6n+1){--h:186} .cube:nth-child(6n+2){--h:170} .cube:nth-child(6n+3){--h:152}
.cube:nth-child(6n+4){--h:90} .cube:nth-child(6n+5){--h:45} .cube:nth-child(6n){--h:36}
.cube:last-child .front { border-color:#fff; }
@keyframes bob { 0%,100% { transform:translate(var(--x),-9px); } 50% { transform:translate(var(--x),9px); } }
@media (prefers-reduced-motion: reduce) { .cube { animation:none; } }
"#;

/// Live-update client.
///
/// Deliberately small and defensive. Without a hydrating WASM bundle this is
/// what makes the page live, and it must never break the server-rendered
/// content it patches: every lookup is guarded, and a closed socket retries
/// rather than leaving a silently stale page.
const LIVE_SCRIPT: &str = r"
(function () {
  var proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  function connect(path, onMessage) {
    var socket;
    try { socket = new WebSocket(proto + '//' + location.host + path); }
    catch (e) { return; }
    socket.onmessage = function (event) {
      try { onMessage(JSON.parse(event.data)); } catch (e) {}
    };
    // A dropped socket must not leave the page quietly frozen.
    socket.onclose = function () { setTimeout(function () { connect(path, onMessage); }, 3000); };
  }

  function utc(t) {
    var d = t % 86400, p = function (n) { return (n < 10 ? '0' : '') + n; };
    return p(Math.floor(d / 3600)) + ':' + p(Math.floor(d % 3600 / 60)) + ':' + p(d % 60) + ' UTC';
  }
  function shorten(h) {
    return (typeof h === 'string' && h.length > 16)
      ? h.slice(0, 8) + '…' + h.slice(-6) : h;
  }
  // Each cell is text, or {text, href, mono}, so a live row carries the same
  // links and fonts as the server-rendered rows beside it.
  function prepend(tbodyId, cells, limit) {
    var body = document.getElementById(tbodyId);
    if (!body) return;
    var row = document.createElement('tr');
    cells.forEach(function (spec) {
      var cell = document.createElement('td');
      var s = (spec !== null && typeof spec === 'object') ? spec : { text: spec };
      if (s.mono) cell.className = 'mono';
      if (s.href) {
        var a = document.createElement('a');
        a.href = s.href;
        a.textContent = s.text;
        cell.appendChild(a);
      } else {
        cell.textContent = s.text;
      }
      row.appendChild(cell);
    });
    body.insertBefore(row, body.firstChild);
    while (body.children.length > limit) { body.removeChild(body.lastChild); }
  }

  // Mirrors ribbon_slot in ui.rs: the same spacing, bow and delay, so a block
  // that arrives live sits exactly where a server render would put it.
  function layoutRibbon(ribbon) {
    var kids = ribbon.children, count = kids.length, centre = (count - 1) / 2;
    for (var i = 0; i < count; i++) {
      var slot = count - 1 - i, off = slot - centre;
      var bow = Math.max(0, 1 - Math.pow(off / (Math.max(centre, 1) + 0.5), 2));
      kids[i].style.setProperty('--x', Math.round(off * 72) + 'px');
      kids[i].style.setProperty('--z', Math.round(60 * bow) + 'px');
      kids[i].style.setProperty('--d', (-slot * 1.1).toFixed(2) + 's');
    }
  }
  function addCube(ribbon, block) {
    var a = document.createElement('a');
    a.className = 'cube';
    a.href = '/blocks/' + block.height;
    a.title = 'block ' + block.height + ' · ' + utc(block.timestamp);
    ['front', 'back', 'top', 'bottom', 'left', 'right'].forEach(function (face) {
      var s = document.createElement('span');
      s.className = 'f ' + face;
      if (face === 'front') s.textContent = block.height;
      a.appendChild(s);
    });
    ribbon.insertBefore(a, ribbon.firstChild);
    while (ribbon.children.length > 12) { ribbon.removeChild(ribbon.lastChild); }
    layoutRibbon(ribbon);
  }
  function beat() {
    var pulse = document.getElementById('pulse');
    if (!pulse) return;
    pulse.classList.remove('beat');
    void pulse.offsetWidth;
    pulse.classList.add('beat');
  }

  connect('/ws/blocks', function (block) {
    var height = document.getElementById('stat-height');
    if (height) height.textContent = block.height;
    var ribbon = document.getElementById('ribbon');
    if (ribbon) addCube(ribbon, block);
    beat();
    prepend('block-rows', [
      { text: block.height, href: '/blocks/' + block.height },
      { text: shorten(block.id), mono: true }, block.tx_count,
      utc(block.timestamp), { text: shorten(block.state_root), mono: true }
    ], 20);
  });

  connect('/ws/txs', function (tx) {
    prepend('tx-rows', [
      { text: shorten(tx.txid), href: '/tx/' + tx.txid, mono: true }, tx.height,
      { text: shorten(tx.sender), mono: true }, tx.nonce, tx.total_out
    ], 20);
  });
})();
";

#[cfg(test)]
mod ribbon_tests {
    use super::ribbon_slot;

    #[test]
    fn a_lone_cube_sits_centre_front() {
        assert_eq!(ribbon_slot(0, 1), (0.0, 60.0));
    }

    #[test]
    fn the_ribbon_is_centred_and_bows_toward_the_viewer() {
        let (left, _) = ribbon_slot(0, 12);
        let (right, _) = ribbon_slot(11, 12);
        assert!((left + right).abs() < 1e-9, "ribbon is not centred");
        let (_, edge_z) = ribbon_slot(0, 12);
        let (_, mid_z) = ribbon_slot(6, 12);
        assert!(mid_z > edge_z, "middle should sit nearer the viewer");
        assert!(edge_z >= 0.0, "depth never goes negative");
    }
}

#[cfg(test)]
mod validators_page_tests {
    use custom_l1_node::rpc::validators::{ValidatorInfo, ValidatorsInfo};

    use super::validators_page;

    fn row(id: u8, status: &str, until: Option<u64>, in_committee: bool) -> ValidatorInfo {
        ValidatorInfo {
            id: hex::encode([id; 32]),
            operator: hex::encode([0xAA; 32]),
            self_bond: "100000000".into(),
            delegated: "0".into(),
            commission_bps: 500,
            status: status.into(),
            jailed_until: until,
            in_committee,
        }
    }

    #[test]
    fn a_jail_is_explained_with_its_end_epoch() {
        let html = validators_page(
            Some(ValidatorsInfo {
                epoch: 65,
                epoch_blocks: 3600,
                committee: 1,
                total: 2,
                validators: vec![
                    row(1, "bonded", None, true),
                    row(2, "jailed", Some(67), false),
                ],
            }),
            None,
        );
        assert!(html.contains("Epoch 65"), "{html}");
        assert!(html.contains("jailed, eligible again at epoch 67"));
        assert!(html.contains("committee of 1"));
        assert!(html.contains("5%"));
    }

    #[test]
    fn a_node_without_the_rpc_gets_an_explanation_not_an_empty_page() {
        let html = validators_page(
            None,
            Some("This node does not serve get_validators.".into()),
        );
        assert!(html.contains("does not serve get_validators"));
        assert!(!html.contains("<table"));
    }
}
