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

use crate::model::{HashratePoint, IndexedBlock, IndexedTx, NetworkStats};

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
        <html>
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <title>{format!("{title} · Maya2C Explorer")}</title>
                <style>{STYLES}</style>
            </head>
            <body>
                <nav class="nav">
                    <a class="brand" href="/">"Maya2C"</a>
                    <a href="/">"Dashboard"</a>
                    <a href="/blocks">"Blocks"</a>
                    <a href="/tx">"Transactions"</a>
                    <a href="/account">"Accounts"</a>
                </nav>
                <main>{children()}</main>
                <script>{LIVE_SCRIPT}</script>
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
                <span class="label">"Height"</span>
                <span class="value" id="stat-height">{stats.height.to_string()}</span>
            </div>
            <div class="tile">
                <span class="label">"Hash rate"</span>
                <span class="value" id="stat-hashrate">{format_hashrate(stats.hashrate)}</span>
            </div>
            <div class="tile">
                <span class="label">"Avg block time"</span>
                <span class="value">{format!("{:.1}s", stats.average_block_time)}</span>
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

/// A sparkline-style table of recent hashrate samples.
#[component]
pub fn HashrateChart(
    /// Samples, oldest first.
    points: Vec<HashratePoint>,
) -> impl IntoView {
    // Scaled against the window maximum so the shape is visible regardless of
    // absolute magnitude — a chain at 200 H/s and one at 200 TH/s both read.
    let peak = points
        .iter()
        .map(|p| p.hashrate)
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let bars: Vec<_> = points
        .iter()
        .map(|point| {
            let height = ((point.hashrate / peak) * 100.0).clamp(1.0, 100.0);
            let label = format!(
                "height {} · {}",
                point.height,
                format_hashrate(point.hashrate)
            );
            view! {
                <div class="bar" style=format!("height:{height:.1}%") title=label></div>
            }
        })
        .collect();

    view! {
        <section class="panel">
            <h2>"Hash rate"</h2>
            <div class="chart">{bars}</div>
            <p class="muted">"Estimated from block targets and timestamps."</p>
        </section>
    }
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
                    <td>{block.timestamp.to_string()}</td>
                    <td class="mono">{short_hash(&block.difficulty_target)}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <section class="panel">
            <h2>"Latest blocks"</h2>
            <table>
                <thead>
                    <tr>
                        <th>"Height"</th><th>"Id"</th><th>"Txs"</th>
                        <th>"Timestamp"</th><th>"Target"</th>
                    </tr>
                </thead>
                <tbody id="block-rows">{rows}</tbody>
            </table>
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
                    <td>{tx.nonce.to_string()}</td>
                    <td>{tx.total_out.to_string()}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <table>
            <thead>
                <tr>
                    <th>"Txid"</th><th>"Height"</th><th>"Sender"</th>
                    <th>"Nonce"</th><th>"Value"</th>
                </tr>
            </thead>
            <tbody id="tx-rows">{rows}</tbody>
        </table>
    }
}

// ---------------------------------------------------------------------------
// pages
// ---------------------------------------------------------------------------

/// The dashboard.
#[must_use]
pub fn dashboard_page(
    stats: NetworkStats,
    points: Vec<HashratePoint>,
    blocks: Vec<IndexedBlock>,
) -> String {
    render(view! {
        <Shell title="Dashboard".to_string()>
            <h1>"Network"</h1>
            <StatsPanel stats=stats/>
            <HashrateChart points=points/>
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
                    <dt>"Timestamp"</dt><dd>{block.timestamp.to_string()}</dd>
                    <dt>"Nonce"</dt><dd>{block.nonce.to_string()}</dd>
                    <dt>"Target"</dt><dd class="mono">{block.difficulty_target.clone()}</dd>
                    <dt>"Work"</dt><dd class="mono">{block.work.clone()}</dd>
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
            <section class="panel">
                <dl class="detail">
                    <dt>"Txid"</dt><dd class="mono">{tx.txid.clone()}</dd>
                    <dt>"Block"</dt>
                    <dd><a href=format!("/blocks/{}", tx.height)>{tx.height.to_string()}</a></dd>
                    <dt>"Sender"</dt>
                    <dd class="mono">
                        <a href=format!("/account?address={}", tx.sender)>{tx.sender.clone()}</a>
                    </dd>
                    <dt>"Nonce"</dt><dd>{tx.nonce.to_string()}</dd>
                    <dt>"Outputs"</dt><dd>{tx.output_count.to_string()}</dd>
                    <dt>"Value"</dt><dd>{tx.total_out.to_string()}</dd>
                    <dt>"Signed"</dt><dd>{if tx.signed { "yes" } else { "no" }}</dd>
                </dl>
            </section>
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
    balance: Option<u64>,
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

const STYLES: &str = r#"
:root { color-scheme: light dark; --fg:#e6e6e6; --bg:#12141a; --muted:#8a91a0;
        --panel:#1a1d26; --accent:#5ad1a0; --border:#272b36; }
* { box-sizing: border-box; }
body { margin:0; font:14px/1.5 ui-sans-serif,system-ui,sans-serif;
       background:var(--bg); color:var(--fg); }
.nav { display:flex; gap:1.25rem; align-items:center; padding:0.85rem 1.5rem;
       border-bottom:1px solid var(--border); background:var(--panel); }
.nav a { color:var(--muted); text-decoration:none; }
.nav a:hover { color:var(--fg); }
.nav .brand { color:var(--accent); font-weight:600; margin-right:0.5rem; }
main { max-width:1100px; margin:0 auto; padding:1.5rem; }
h1 { font-size:1.4rem; margin:0 0 1rem; }
h2 { font-size:1rem; margin:0 0 0.75rem; color:var(--muted);
     text-transform:uppercase; letter-spacing:0.06em; }
.tiles { display:grid; grid-template-columns:repeat(auto-fit,minmax(150px,1fr));
         gap:0.75rem; margin-bottom:1.5rem; }
.tile { background:var(--panel); border:1px solid var(--border);
        border-radius:8px; padding:0.85rem 1rem; display:flex;
        flex-direction:column; gap:0.35rem; }
.tile .label { color:var(--muted); font-size:0.75rem; text-transform:uppercase;
               letter-spacing:0.06em; }
.tile .value { font-size:1.35rem; font-variant-numeric:tabular-nums; }
.panel { background:var(--panel); border:1px solid var(--border);
         border-radius:8px; padding:1rem; margin-bottom:1.5rem; }
table { width:100%; border-collapse:collapse; }
th,td { text-align:left; padding:0.5rem 0.6rem;
        border-bottom:1px solid var(--border); }
th { color:var(--muted); font-size:0.75rem; text-transform:uppercase;
     letter-spacing:0.06em; font-weight:500; }
td a { color:var(--accent); text-decoration:none; }
.mono { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:0.85em; }
.muted { color:var(--muted); }
.error { color:#ff8a8a; }
.chart { display:flex; align-items:flex-end; gap:3px; height:120px;
         padding-top:0.5rem; }
.bar { flex:1; min-width:2px; background:var(--accent); opacity:0.75;
       border-radius:2px 2px 0 0; }
.bar:hover { opacity:1; }
.detail { display:grid; grid-template-columns:150px 1fr; gap:0.4rem 1rem;
          margin:0; }
.detail dt { color:var(--muted); }
.detail dd { margin:0; overflow-wrap:anywhere; }
.search { display:flex; gap:0.5rem; margin-bottom:1.25rem; }
.search input { flex:1; padding:0.55rem 0.7rem; background:var(--panel);
                border:1px solid var(--border); border-radius:6px;
                color:var(--fg); font-family:ui-monospace,monospace; }
.search button { padding:0.55rem 1.1rem; background:var(--accent); color:#0b0d12;
                 border:0; border-radius:6px; font-weight:600; cursor:pointer; }
@media (max-width:600px) { .detail { grid-template-columns:1fr; } }
"#;

/// Live-update client.
///
/// Deliberately small and defensive. Without a hydrating WASM bundle this is
/// what makes the page live, and it must never break the server-rendered
/// content it patches: every lookup is guarded, and a closed socket retries
/// rather than leaving a silently stale page.
const LIVE_SCRIPT: &str = r#"
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

  function shorten(h) {
    return (typeof h === 'string' && h.length > 16)
      ? h.slice(0, 8) + '…' + h.slice(-6) : h;
  }
  function prepend(tbodyId, cells, limit) {
    var body = document.getElementById(tbodyId);
    if (!body) return;
    var row = document.createElement('tr');
    cells.forEach(function (text) {
      var cell = document.createElement('td');
      cell.textContent = text;
      row.appendChild(cell);
    });
    body.insertBefore(row, body.firstChild);
    while (body.children.length > limit) { body.removeChild(body.lastChild); }
  }

  connect('/ws/blocks', function (block) {
    var height = document.getElementById('stat-height');
    if (height) height.textContent = block.height;
    prepend('block-rows', [
      block.height, shorten(block.id), block.tx_count,
      block.timestamp, shorten(block.difficulty_target)
    ], 20);
  });

  connect('/ws/txs', function (tx) {
    prepend('tx-rows', [
      shorten(tx.txid), tx.height, shorten(tx.sender), tx.nonce, tx.total_out
    ], 20);
  });
})();
"#;
