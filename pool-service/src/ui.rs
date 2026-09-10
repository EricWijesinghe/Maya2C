//! Leptos server-rendered views for the miner dashboard.
//!
//! ## SSR without a client bundle
//!
//! The same choice `explorer/src/ui.rs` makes, for the same reason: a hydrating
//! build needs `cargo-leptos` and `wasm-bindgen` to produce a WASM bundle, and
//! that is a build-tooling change rather than a rewrite. Signals still drive
//! the render; liveness comes from the WebSocket at `/ws`, which the page
//! subscribes to and uses to refresh in place.
//!
//! ## What a miner is shown, and what they are not
//!
//! Every rig figure is labelled by where it came from. **Measured** hash rate
//! is the pool's own arithmetic over accepted share weight. **Reported** power,
//! temperature, and fan speed are what the rig said about itself and cannot be
//! checked — [`crate::telemetry`] explains why they are kept apart, and the
//! page says so in words rather than only in a class name.
//!
//! Balances are shown in three columns and never as one total. `immature` can
//! still be taken away by a reorg; presenting it beside `unpaid` as a single
//! number would make an ordinary reversal look like theft.

use leptos::prelude::*;

use crate::ledger::{FoundBlock, MinerBalance};
use crate::model::{PayoutBatch, WorkerStats};

/// Renders a component tree to an HTML string.
fn render(view: impl IntoView + 'static) -> String {
    view.into_view().to_html()
}

/// Formats a hash rate with a unit, so the page is readable at any scale.
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

/// Formats milliwatts as watts.
#[must_use]
pub fn format_power(milliwatts: u64) -> String {
    if milliwatts == 0 {
        return "—".to_string();
    }
    format!("{:.1} W", milliwatts as f64 / 1_000.0)
}

/// Formats millidegrees as degrees.
#[must_use]
pub fn format_temperature(millicelsius: i32) -> String {
    if millicelsius == 0 {
        return "—".to_string();
    }
    format!("{:.1} °C", f64::from(millicelsius) / 1_000.0)
}

/// Formats an efficiency ratio as a percentage.
///
/// A worker that has submitted nothing shows a dash rather than 100%: a dead
/// rig must not read as a perfect one.
#[must_use]
pub fn format_efficiency(stats: &WorkerStats) -> String {
    match stats.efficiency() {
        Some(ratio) => format!("{:.2}%", ratio * 100.0),
        None => "—".to_string(),
    }
}

/// Shortens a hex identifier for display.
#[must_use]
pub fn short_hex(value: &str) -> String {
    if value.len() <= 16 {
        return value.to_string();
    }
    format!("{}…{}", &value[..8], &value[value.len() - 6..])
}

/// Describes how long ago something happened.
#[must_use]
pub fn format_age(seconds: Option<u64>) -> String {
    match seconds {
        None => "never".to_string(),
        Some(seconds) if seconds < 60 => format!("{seconds}s ago"),
        Some(seconds) if seconds < 3_600 => format!("{}m ago", seconds / 60),
        Some(seconds) => format!("{}h ago", seconds / 3_600),
    }
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
                <title>{format!("{title} · Maya2C Pool")}</title>
                <style>{STYLES}</style>
            </head>
            <body>
                <nav class="nav">
                    <a class="brand" href="/">"Maya2C Pool"</a>
                    <a href="/">"Overview"</a>
                    <a href="/blocks">"Blocks"</a>
                    <a href="/payouts">"Payouts"</a>
                </nav>
                <main>{children()}</main>
                <script>{LIVE_SCRIPT}</script>
            </body>
        </html>
    }
}

/// Pool-wide summary tiles.
#[component]
pub fn PoolSummary(
    /// Measured pool hash rate.
    hashrate: f64,
    /// Open channels.
    channels: usize,
    /// Rigs with statistics.
    workers: usize,
    /// Sum of rig-reported power, in milliwatts.
    reported_power_milliwatts: u64,
) -> impl IntoView {
    view! {
        <section class="tiles">
            <div class="tile">
                <span class="label">"Measured hashrate"</span>
                <span class="value">{format_hashrate(hashrate)}</span>
                <span class="note">"from accepted share weight"</span>
            </div>
            <div class="tile">
                <span class="label">"Channels"</span>
                <span class="value">{channels.to_string()}</span>
            </div>
            <div class="tile">
                <span class="label">"Rigs"</span>
                <span class="value">{workers.to_string()}</span>
            </div>
            <div class="tile">
                <span class="label">"Reported power"</span>
                <span class="value">{format_power(reported_power_milliwatts)}</span>
                <span class="note reported">"self-reported, unverified"</span>
            </div>
        </section>
    }
}

/// A miner's balances, in three columns.
#[component]
pub fn BalancePanel(
    /// The miner's standing.
    balance: MinerBalance,
) -> impl IntoView {
    view! {
        <section class="tiles">
            <div class="tile">
                <span class="label">"Unpaid"</span>
                <span class="value">{balance.unpaid.to_string()}</span>
                <span class="note">"awaiting the next batch"</span>
            </div>
            <div class="tile">
                <span class="label">"Immature"</span>
                <span class="value">{balance.immature.to_string()}</span>
                <span class="note">"can still be reversed by a reorg"</span>
            </div>
            <div class="tile">
                <span class="label">"Paid"</span>
                <span class="value">{balance.paid.to_string()}</span>
            </div>
        </section>
    }
}

/// A table of rigs.
#[component]
pub fn WorkerTable(
    /// Rigs to list.
    workers: Vec<WorkerStats>,
) -> impl IntoView {
    let rows: Vec<_> = workers
        .into_iter()
        .map(|worker| {
            let status = if worker.connected {
                "online"
            } else {
                "offline"
            };
            view! {
                <tr class=status>
                    <td>{worker.worker.clone()}</td>
                    <td>{status}</td>
                    <td>{format_hashrate(worker.measured_hashrate)}</td>
                    <td class="reported">{format_hashrate(worker.reported_hashrate)}</td>
                    <td>{worker.accepted_shares.to_string()}</td>
                    <td>{worker.rejected_shares.to_string()}</td>
                    <td>{worker.stale_shares.to_string()}</td>
                    <td>{format_efficiency(&worker)}</td>
                    <td>{worker.target_bits.to_string()}</td>
                    <td class="reported">{format_power(worker.reported_power_milliwatts)}</td>
                    <td class="reported">
                        {format_temperature(worker.reported_temperature_millicelsius)}
                    </td>
                    <td>{format_age(worker.seconds_since_share)}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <section>
            <h2>"Rigs"</h2>
            <p class="caption">
                "Columns marked "
                <span class="reported">"in amber"</span>
                " are reported by the rig and cannot be verified by the pool."
            </p>
            <table>
                <thead>
                    <tr>
                        <th>"Worker"</th>
                        <th>"Status"</th>
                        <th>"Measured"</th>
                        <th class="reported">"Reported"</th>
                        <th>"Accepted"</th>
                        <th>"Rejected"</th>
                        <th>"Stale"</th>
                        <th>"Efficiency"</th>
                        <th>"Target bits"</th>
                        <th class="reported">"Power"</th>
                        <th class="reported">"Temp"</th>
                        <th>"Last share"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </section>
    }
}

/// A table of found blocks.
#[component]
pub fn BlockTable(
    /// Blocks to list.
    blocks: Vec<FoundBlock>,
) -> impl IntoView {
    let rows: Vec<_> = blocks
        .into_iter()
        .map(|block| {
            let state = format!("{:?}", block.state).to_lowercase();
            let state_label = state.clone();
            view! {
                <tr>
                    <td>{block.height.to_string()}</td>
                    <td class="mono">{short_hex(&block.id)}</td>
                    <td class="mono">{short_hex(&hex::encode(block.finder))}</td>
                    <td>{block.reward.to_string()}</td>
                    <td class=state>{state_label}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <section>
            <h2>"Blocks found"</h2>
            <table>
                <thead>
                    <tr>
                        <th>"Height"</th>
                        <th>"Block"</th>
                        <th>"Finder"</th>
                        <th>"Reward"</th>
                        <th>"State"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </section>
    }
}

/// A table of payout batches.
#[component]
pub fn PayoutTable(
    /// Batches to list.
    batches: Vec<PayoutBatch>,
) -> impl IntoView {
    let rows: Vec<_> = batches
        .into_iter()
        .map(|batch| {
            let state = format!("{:?}", batch.state).to_lowercase();
            let state_label = state.clone();
            let txid = batch.txid.clone().unwrap_or_else(|| "—".to_string());
            view! {
                <tr>
                    <td>{batch.id.to_string()}</td>
                    <td>{batch.nonce.to_string()}</td>
                    <td>{batch.entries.len().to_string()}</td>
                    <td>{batch.total().to_string()}</td>
                    <td class="mono">{short_hex(&txid)}</td>
                    <td class=state>{state_label}</td>
                </tr>
            }
        })
        .collect();

    view! {
        <section>
            <h2>"Payouts"</h2>
            <p class="caption">
                "Payouts are ordinary signed transfers from the pool treasury. \
                 This chain mints no block reward, so the treasury is funded by \
                 the operator."
            </p>
            <table>
                <thead>
                    <tr>
                        <th>"Batch"</th>
                        <th>"Nonce"</th>
                        <th>"Recipients"</th>
                        <th>"Value"</th>
                        <th>"Transaction"</th>
                        <th>"State"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </section>
    }
}

/// The pool overview page.
#[must_use]
pub fn overview_page(
    hashrate: f64,
    channels: usize,
    workers: Vec<WorkerStats>,
    reported_power_milliwatts: u64,
    blocks: Vec<FoundBlock>,
) -> String {
    let worker_count = workers.len();
    render(view! {
        <Shell title="Overview".to_string()>
            <PoolSummary
                hashrate=hashrate
                channels=channels
                workers=worker_count
                reported_power_milliwatts=reported_power_milliwatts
            />
            <WorkerTable workers=workers/>
            <BlockTable blocks=blocks/>
        </Shell>
    })
}

/// One miner's page.
#[must_use]
pub fn miner_page(address: String, balance: MinerBalance, workers: Vec<WorkerStats>) -> String {
    render(view! {
        <Shell title=format!("Miner {}", short_hex(&address))>
            <h1 class="mono">{address}</h1>
            <BalancePanel balance=balance/>
            <WorkerTable workers=workers/>
        </Shell>
    })
}

/// The found-blocks page.
#[must_use]
pub fn blocks_page(blocks: Vec<FoundBlock>) -> String {
    render(view! {
        <Shell title="Blocks".to_string()>
            <BlockTable blocks=blocks/>
        </Shell>
    })
}

/// The payouts page.
#[must_use]
pub fn payouts_page(batches: Vec<PayoutBatch>) -> String {
    render(view! {
        <Shell title="Payouts".to_string()>
            <PayoutTable batches=batches/>
        </Shell>
    })
}

/// A minimal error page.
#[must_use]
pub fn error_page(title: &str, detail: &str) -> String {
    let title = title.to_string();
    let detail = detail.to_string();
    render(view! {
        <Shell title=title.clone()>
            <h1>{title}</h1>
            <p>{detail}</p>
        </Shell>
    })
}

/// Page styles.
///
/// Inline rather than a served asset: the dashboard is one page of chrome, and
/// a second request for 3 KB of CSS is a second thing that can 404 in a
/// deployment.
const STYLES: &str = "\
:root { color-scheme: light dark; --fg: #16181d; --bg: #fbfbfd; --muted: #6b7280;
  --line: #e3e5ea; --accent: #2f6f4f; --reported: #9a6b00; }
@media (prefers-color-scheme: dark) { :root { --fg: #e6e8ec; --bg: #14161a;
  --muted: #9aa1ac; --line: #2a2e36; --accent: #79c79b; --reported: #d8a83a; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
  font: 15px/1.5 ui-sans-serif, system-ui, -apple-system, sans-serif; }
.nav { display: flex; gap: 1.25rem; align-items: center; padding: 0.9rem 1.5rem;
  border-bottom: 1px solid var(--line); }
.nav a { color: var(--fg); text-decoration: none; }
.nav .brand { font-weight: 650; color: var(--accent); margin-right: 0.5rem; }
main { max-width: 1200px; margin: 0 auto; padding: 1.5rem; }
h1 { font-size: 1.3rem; } h2 { font-size: 1.05rem; margin-top: 2rem; }
.tiles { display: grid; gap: 1rem; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr)); }
.tile { border: 1px solid var(--line); border-radius: 10px; padding: 0.9rem 1rem;
  display: flex; flex-direction: column; gap: 0.2rem; }
.tile .label { color: var(--muted); font-size: 0.78rem; text-transform: uppercase;
  letter-spacing: 0.04em; }
.tile .value { font-size: 1.5rem; font-weight: 620; font-variant-numeric: tabular-nums; }
.tile .note { color: var(--muted); font-size: 0.75rem; }
.caption { color: var(--muted); font-size: 0.85rem; }
table { width: 100%; border-collapse: collapse; margin-top: 0.75rem; font-size: 0.9rem; }
th, td { text-align: left; padding: 0.5rem 0.6rem; border-bottom: 1px solid var(--line);
  font-variant-numeric: tabular-nums; white-space: nowrap; }
th { color: var(--muted); font-weight: 550; font-size: 0.8rem; }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
.reported { color: var(--reported); }
tr.offline td { opacity: 0.55; }
td.confirmed, td.mature { color: var(--accent); }
td.orphaned, td.failed { color: #c0392b; }
";

/// The live-update client.
///
/// Deliberately tiny. It subscribes to the event stream and reloads the page on
/// a change rather than patching the DOM: without a WASM bundle there is no
/// client-side view to patch, and a page that quietly diverges from the server
/// is worse than one that reloads.
const LIVE_SCRIPT: &str = "\
(function () {
  var url = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws';
  var pending = false;
  function connect() {
    var socket = new WebSocket(url);
    socket.onmessage = function () {
      // Coalesced: at a share a second a reload per event would be a reload per
      // second, and the page would never finish rendering.
      if (pending) { return; }
      pending = true;
      setTimeout(function () { location.reload(); }, 3000);
    };
    socket.onclose = function () { setTimeout(connect, 5000); };
  }
  connect();
})();
";

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(connected: bool) -> WorkerStats {
        WorkerStats {
            miner: "aa".repeat(32),
            worker: "rig-1".to_string(),
            measured_hashrate: 1_500_000.0,
            reported_hashrate: 40_000_000.0,
            accepted_shares: 90,
            rejected_shares: 10,
            stale_shares: 4,
            accepted_weight: 92_160,
            target_bits: 20,
            reported_power_milliwatts: 1_450_000,
            reported_temperature_millicelsius: 62_500,
            reported_fan_percent: 80,
            seconds_since_share: Some(45),
            connected,
        }
    }

    #[test]
    fn hash_rates_are_scaled_to_a_readable_unit() {
        assert_eq!(format_hashrate(999.0), "999.00 H/s");
        assert_eq!(format_hashrate(1_500_000.0), "1.50 MH/s");
        assert_eq!(format_hashrate(0.0), "0 H/s");
        // A NaN reaching the page would render as "NaN H/s" without this.
        assert_eq!(format_hashrate(f64::NAN), "0 H/s");
    }

    #[test]
    fn a_rig_that_has_submitted_nothing_shows_a_dash_not_a_hundred_percent() {
        let stats = WorkerStats::default();
        assert_eq!(format_efficiency(&stats), "—");
    }

    #[test]
    fn a_sub_zero_temperature_renders_with_its_sign() {
        assert_eq!(format_temperature(-18_000), "-18.0 °C");
    }

    #[test]
    fn the_overview_marks_reported_figures_as_unverified() {
        // The one thing this page must not do is present a rig's claim about
        // itself as a measurement.
        let html = overview_page(1_500_000.0, 2, vec![worker(true)], 1_450_000, Vec::new());

        assert!(html.contains("self-reported, unverified"));
        assert!(html.contains("cannot be verified by the pool"));
        assert!(html.contains("class=\"reported\""));
    }

    #[test]
    fn the_overview_shows_measured_and_reported_side_by_side() {
        let html = overview_page(1_500_000.0, 1, vec![worker(true)], 0, Vec::new());
        assert!(html.contains("1.50 MH/s"), "measured rate missing");
        assert!(html.contains("40.00 MH/s"), "reported rate missing");
    }

    #[test]
    fn a_miner_page_never_totals_immature_with_unpaid() {
        // Presenting them as one number makes an ordinary reorg reversal look
        // like the pool taking money back.
        let html = miner_page(
            "aa".repeat(32),
            MinerBalance {
                unpaid: 500,
                immature: 300,
                paid: 1_200,
            },
            vec![worker(true)],
        );

        assert!(html.contains("Unpaid"));
        assert!(html.contains("Immature"));
        assert!(html.contains("can still be reversed by a reorg"));
        assert!(!html.contains("800"), "the two were summed");
    }

    #[test]
    fn an_offline_rig_is_still_listed() {
        // The operator looking for it has just been paged about it.
        let html = overview_page(0.0, 0, vec![worker(false)], 0, Vec::new());
        assert!(html.contains("offline"));
        assert!(html.contains("rig-1"));
    }

    #[test]
    fn the_payouts_page_says_where_the_money_comes_from() {
        let html = payouts_page(Vec::new());
        assert!(html.contains("mints no block reward"));
    }

    #[test]
    fn every_page_carries_the_live_update_client() {
        for html in [
            overview_page(0.0, 0, Vec::new(), 0, Vec::new()),
            blocks_page(Vec::new()),
            payouts_page(Vec::new()),
            error_page("Not found", "no such miner"),
        ] {
            assert!(html.contains("/ws"), "a page shipped without live updates");
        }
    }
}
