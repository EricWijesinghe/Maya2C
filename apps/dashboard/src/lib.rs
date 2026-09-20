//! The network dashboard.
//!
//! # It renders the collector's own type
//!
//! [`Snapshot`] is imported from `maya-telemetry`, not redeclared here. A
//! dashboard with its own copy of that struct is a dashboard where adding a
//! field to the collector silently stops it being displayed — the failure
//! shows up as a chart that is merely missing, and nothing anywhere errors.
//! Sharing the type makes it a compile error instead, which is why the
//! telemetry crate's wire types build for `wasm32-unknown-unknown` at all.
//!
//! # What the page says about its own numbers
//!
//! Every figure except difficulty is a claim by whoever reported it. The
//! header says so, in the page, not in a footnote — a network dashboard that
//! presents unauthenticated hash rate as measured fact is how "the network has
//! N exahash" becomes something people repeat.
//!
//! # Live, with a fallback that is honest about being stale
//!
//! The page opens a WebSocket and renders whatever arrives. If the socket
//! closes it says **disconnected** and keeps showing the last snapshot with
//! its timestamp, rather than blanking or silently freezing. A stale number
//! labelled stale is useful; a stale number labelled nothing is a lie that
//! looks like data.

use leptos::prelude::*;
use maya_telemetry::snapshot::Snapshot;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::closure::Closure;

/// Connection state, as the page shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// Opening, and nothing rendered yet.
    Connecting,
    /// Receiving.
    Live,
    /// Closed or failed. The last snapshot is still shown, marked stale.
    Disconnected,
}

impl Link {
    /// The words on the badge.
    const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Live => "live",
            Self::Disconnected => "disconnected — showing the last snapshot",
        }
    }

    const fn class(self) -> &'static str {
        match self {
            Self::Connecting => "badge connecting",
            Self::Live => "badge live",
            Self::Disconnected => "badge stale",
        }
    }
}

/// Mounts the dashboard.
pub fn mount() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// The whole page.
#[component]
pub fn App() -> impl IntoView {
    let (snapshot, set_snapshot) = signal(None::<Snapshot>);
    let (link, set_link) = signal(Link::Connecting);

    connect(set_snapshot, set_link);

    view! {
        <main>
            <header>
                <img
                    class="mark"
                    src="assets/logo.png"
                    srcset="assets/logo.png 1x, assets/logo@2x.png 2x"
                    // Decorative: the `<h1>` beside it already carries the
                    // name, and a screen reader announcing it twice is worse
                    // than not announcing the image at all.
                    alt=""
                    width="250"
                    height="100"
                />
                <h1>"Maya2C network"</h1>
                <span class=move || link.get().class()>{move || link.get().label()}</span>
            </header>

            <p class="caveat">
                "Every figure below except difficulty is what a node or miner "
                <em>"claims"</em>
                ". Nothing here is verified against the chain, because there is no proof a \
                 machine can offer that it is hashing. Heights and propagation times are \
                 medians, so one wrong reporter cannot set them; hash rate is a sum, so one \
                 dishonest reporter can inflate it."
            </p>

            {move || match snapshot.get() {
                None => view! { <p class="empty">"Waiting for the first snapshot…"</p> }.into_any(),
                Some(snapshot) => view! { <Body snapshot=snapshot/> }.into_any(),
            }}
        </main>
    }
}

/// Everything below the header, once a snapshot has arrived.
#[component]
fn Body(snapshot: Snapshot) -> impl IntoView {
    let regions = snapshot.regions.clone();
    let backends = snapshot.backends.clone();
    let total = snapshot.hash_rate;

    view! {
        <section class="figures">
            <Figure label="hash rate" value=format_rate(snapshot.hash_rate) note="claimed, summed"/>
            <Figure
                label="height"
                value=snapshot.height.map_or_else(|| "—".to_string(), |h| group(h))
                note="median of reporting nodes"
            />
            <Figure
                label="propagation"
                value=snapshot
                    .propagation_ms
                    .map_or_else(|| "—".to_string(), |ms| format!("{ms} ms"))
                note="median; header timestamps are miner-written"
            />
            <Figure
                label="peers"
                value=snapshot.peers.map_or_else(|| "—".to_string(), |p| p.to_string())
                note="median per node"
            />
            <Figure label="miners" value=snapshot.miners.to_string() note="reporting now"/>
            <Figure label="nodes" value=snapshot.nodes.to_string() note="reporting now"/>
        </section>

        <section>
            <h2>"Where the hash rate is"</h2>
            <p class="caveat">
                "Country granularity. No coordinates are collected, and a country with fewer \
                 than "
                {maya_telemetry::region::MIN_REPORTERS}
                " reporters is folded into ZZ so that a lone operator is not identified by \
                 their flag. ?? is reporters the collector could not place."
            </p>
            <table>
                <thead>
                    <tr><th>"region"</th><th>"reporters"</th><th>"hash rate"</th><th></th></tr>
                </thead>
                <tbody>
                    {regions
                        .into_iter()
                        .map(|row| {
                            let share = share_of(row.hash_rate, total);
                            view! {
                                <tr>
                                    <td>{row.region.as_str().to_string()}</td>
                                    <td>{row.reporters}</td>
                                    <td>{format_rate(row.hash_rate)}</td>
                                    <td class="bar">
                                        <span style=format!("width:{share:.1}%")></span>
                                    </td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </section>

        <section>
            <h2>"What it is running on"</h2>
            <table>
                <thead>
                    <tr><th>"backend"</th><th>"miners"</th><th>"hash rate"</th><th></th></tr>
                </thead>
                <tbody>
                    {backends
                        .into_iter()
                        .map(|row| {
                            let share = share_of(row.hash_rate, total);
                            view! {
                                <tr>
                                    <td>{row.backend}</td>
                                    <td>{row.miners}</td>
                                    <td>{format_rate(row.hash_rate)}</td>
                                    <td class="bar">
                                        <span style=format!("width:{share:.1}%")></span>
                                    </td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </section>

        <footer>"snapshot taken at " {snapshot.taken_at} " (unix seconds)"</footer>
    }
}

/// One headline number.
#[component]
fn Figure(
    label: &'static str,
    value: String,
    /// What the number actually is, in small print. Every figure gets one:
    /// "median of reporting nodes" is the difference between a number a reader
    /// can use and a number a reader will misquote.
    note: &'static str,
) -> impl IntoView {
    view! {
        <div class="figure">
            <div class="label">{label}</div>
            <div class="value">{value}</div>
            <div class="note">{note}</div>
        </div>
    }
}

/// Opens the feed and pushes every snapshot into the signal.
fn connect(set_snapshot: WriteSignal<Option<Snapshot>>, set_link: WriteSignal<Link>) {
    let Some(url) = websocket_url() else {
        set_link.set(Link::Disconnected);
        return;
    };

    let socket = match web_sys::WebSocket::new(&url) {
        Ok(socket) => socket,
        Err(_) => {
            set_link.set(Link::Disconnected);
            return;
        }
    };

    let on_message =
        Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            let Some(text) = event.data().as_string() else {
                return;
            };
            // A frame that does not parse is dropped rather than blanking the
            // page. Snapshots are absolute, so the next one is a full repair —
            // there is no accumulated state to have corrupted.
            if let Ok(snapshot) = serde_json::from_str::<Snapshot>(&text) {
                set_snapshot.set(Some(snapshot));
                set_link.set(Link::Live);
            }
        });
    socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    let on_close = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| {
        // The last snapshot stays on screen, labelled stale. Blanking would
        // discard information that is still true a second ago; showing it
        // unlabelled would be worse than blanking.
        set_link.set(Link::Disconnected);
    });
    socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    socket.set_onerror(Some(on_close.as_ref().unchecked_ref()));
    on_close.forget();
}

/// The collector's WebSocket URL, derived from where the page was served.
///
/// Derived rather than configured, so a deployment does not need a build-time
/// constant that can be wrong. `https` becomes `wss`: a page on `https` cannot
/// open a plain `ws`, and getting this backwards is a dashboard that works in
/// development and is blank in production.
fn websocket_url() -> Option<String> {
    let location = web_sys::window()?.location();
    let protocol = location.protocol().ok()?;
    let host = location.host().ok()?;
    let scheme = if protocol == "https:" { "wss" } else { "ws" };
    Some(format!("{scheme}://{host}/api/ws"))
}

/// A hash rate with a unit a person can read.
fn format_rate(rate: u64) -> String {
    const UNITS: [(&str, u64); 5] = [
        ("TH/s", 1_000_000_000_000),
        ("GH/s", 1_000_000_000),
        ("MH/s", 1_000_000),
        ("kH/s", 1_000),
        ("H/s", 1),
    ];

    for (unit, scale) in UNITS {
        if rate >= scale {
            // One decimal place at every scale. More would imply a precision
            // that summed self-reported figures do not have.
            return format!("{:.1} {unit}", rate as f64 / scale as f64);
        }
    }
    "0 H/s".to_string()
}

/// `part` as a percentage of `whole`, with a zero whole reading as zero.
fn share_of(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    (part as f64 / whole as f64) * 100.0
}

/// A number with thousands separators.
fn group(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_rate_is_shown_at_a_readable_scale() {
        assert_eq!(format_rate(0), "0 H/s");
        assert_eq!(format_rate(999), "999.0 H/s");
        assert_eq!(format_rate(1_500), "1.5 kH/s");
        assert_eq!(format_rate(2_400_000_000), "2.4 GH/s");
        assert_eq!(format_rate(10_000_000_000_000), "10.0 TH/s");
    }

    #[test]
    fn a_share_of_nothing_is_zero_and_not_a_nan() {
        // A NaN reaches the page as `width:NaN%`, which renders as a bar of
        // unpredictable length rather than as an error.
        assert_eq!(share_of(0, 0), 0.0);
        assert_eq!(share_of(5, 0), 0.0);
        assert!((share_of(25, 100) - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_height_is_grouped() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(4_200_000), "4,200,000");
    }

    #[test]
    fn the_dashboard_deserializes_the_collectors_own_snapshot() {
        // The reason the type is shared rather than mirrored. If the collector
        // adds a field, this still parses; if it renames one the dashboard
        // reads, this fails to compile — which is the point.
        let json = r#"{
            "reporters": 3, "miners": 2, "nodes": 1,
            "hash_rate": 1500, "height": 4200, "peers": 8,
            "propagation_ms": 950,
            "regions": [{"region":"US","reporters":2,"hash_rate":1500}],
            "backends": [{"backend":"wgpu","miners":2,"hash_rate":1500}],
            "taken_at": 1700000000
        }"#;

        let snapshot: Snapshot = serde_json::from_str(json).expect("the collector's shape");
        assert_eq!(snapshot.hash_rate, 1_500);
        assert_eq!(snapshot.regions[0].region.as_str(), "US");
    }
}
