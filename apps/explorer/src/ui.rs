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
                <meta name="theme-color" content="#ffffff"/>

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
                        // Not decorative here: this is the only content of the
                        // link, so without alt text the home link announces
                        // itself as an unlabelled image.
                        <img
                            src="/assets/logo.png"
                            srcset="/assets/logo.png 1x, /assets/logo@2x.png 2x"
                            alt="Maya2C"
                            width="250"
                            height="100"
                        />
                    </a>
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
            <table>
                <thead>
                    <tr>
                        <th>"Height"</th><th>"Id"</th><th>"Txs"</th>
                        <th>"Time"</th><th>"State root"</th>
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
pub fn dashboard_page(stats: NetworkStats, blocks: Vec<IndexedBlock>) -> String {
    render(view! {
        <Shell title="Dashboard".to_string()>
            <h1>"maya-testnet-1"</h1>
            <p class="lead">"Post-quantum layer 1 · DAG-BFT finality · every block below is final"</p>
            <StatsPanel stats=stats/>
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

/// The shared design-system tokens, then the explorer's own rules. The
/// explorer renders the "command" theme, its signature dark look; the calm
/// theme is the same tokens under `data-theme="calm"`.
fn styles() -> String {
    format!("{}{STYLES}", maya_design_system::tokens::css())
}

const STYLES: &str = r#"
/* maya2c.dev's palette over the design-system tokens, so the explorer reads
   as the same product as the site it is linked from. */
:root, [data-theme="command"] { --accent:#22d3ee; --focus:#a78bfa; --on-accent:#04111a; }
* { box-sizing: border-box; }
body { margin:0; font:14px/1.5 "Inter",ui-sans-serif,system-ui,sans-serif; color:var(--fg);
       background:
         radial-gradient(50rem 24rem at 10% -10%, rgb(34 211 238 / 0.12), transparent 60%),
         radial-gradient(44rem 22rem at 100% 0%, rgb(167 139 250 / 0.12), transparent 60%),
         var(--bg); min-height:100vh; }
h1 { background:linear-gradient(100deg,#22d3ee,#a78bfa 60%,#f472b6);
     -webkit-background-clip:text; background-clip:text; color:transparent; }
.lead { color:var(--muted); margin:-0.5rem 0 1.25rem; }
.tile, .panel { background:linear-gradient(160deg, rgb(255 255 255 / 0.05), rgb(255 255 255 / 0.01)); }
.tile .value { color:#e0f2fe; }
.nav { display:flex; gap:1.25rem; align-items:center; padding:0.85rem 1.5rem;
       border-bottom:1px solid var(--border); background:var(--panel); }
.nav a { color:var(--muted); text-decoration:none; }
.nav a:hover { color:var(--fg); }
.nav .brand { display:flex; align-items:center; margin-right:0.5rem; }
/* Height-constrained, width auto, so the 5:2 wordmark is never stretched. The
   intrinsic size is on the element itself, so the nav does not reflow when the
   image finishes loading. */
.nav .brand img { height:1.6rem; width:auto; display:block; }
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
.error { color:var(--danger); }
.plain { font-size:1.05rem; margin:0 0 1rem; }
details.panel summary { cursor:pointer; color:var(--muted); margin-bottom:0.75rem; }
:focus-visible { outline:2px solid var(--focus); outline-offset:2px; }
.chart { display:flex; align-items:flex-end; gap:3px; height:120px;
         padding-top:0.5rem; }
.bar { flex:1; min-width:2px; background:linear-gradient(180deg,#22d3ee,#a78bfa); opacity:0.8;
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
.search button { padding:0.55rem 1.1rem; background:var(--accent); color:var(--on-accent);
                 min-height:var(--touch-target);
                 border:0; border-radius:6px; font-weight:600; cursor:pointer; }
@media (max-width:600px) { .detail { grid-template-columns:1fr; } }
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

  connect('/ws/blocks', function (block) {
    var height = document.getElementById('stat-height');
    if (height) height.textContent = block.height;
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
