//! A real round trip against a real kubo daemon.
//!
//! `KuboStore`'s unit tests drive a mock of the two endpoints, which proves the
//! client's own logic and nothing about kubo. This proves the rest: that kubo
//! accepts the CAR this crate writes, pins the root, and exports a DAG that
//! still verifies against the same root.
//!
//! Ignored by default, because it needs a daemon. To run it:
//!
//! ```text
//! ipfs daemon --offline
//! MAYA_KUBO_API=http://127.0.0.1:5001 cargo test -p maya-archive --test kubo_live -- --ignored
//! ```
//!
//! `--offline` is worth keeping: an archive is the chain's history, and a
//! default daemon announces every block it holds to the public DHT.

use maya_archive::kubo::KuboStore;
use maya_archive::store::ArchiveStore;
use maya_archive::{ArchivedBlock, build_archive, open_archive};

fn batch(first: u64, count: u64) -> Vec<ArchivedBlock> {
    (first..first + count)
        .map(|height| ArchivedBlock {
            height,
            id: [height as u8; 32],
            // Big enough that the CAR spans more than one buffer, and
            // compressible, which is what an archive at rest looks like.
            bytes: format!("block {height} ").repeat(4_096).into_bytes(),
        })
        .collect()
}

#[test]
#[ignore = "needs a kubo daemon; set MAYA_KUBO_API"]
fn an_archive_imports_pins_and_exports_back_from_a_real_daemon() {
    let api = std::env::var("MAYA_KUBO_API").expect("MAYA_KUBO_API must name a kubo RPC API");
    let store = KuboStore::new(api.clone()).expect("client");

    let blocks = batch(900_000, 12);
    let archive = build_archive("maya-kubo-live", &blocks).expect("build");

    let locator = store.put(&archive).expect("kubo accepted the CAR");
    assert_eq!(locator.kind, "ipfs");
    assert_eq!(
        locator.reference,
        archive.root.to_string(),
        "kubo must report the root this crate computed"
    );

    // Pinned, not merely imported: an unpinned block is garbage collected, and
    // the whole point of the manifest linking every block is that pinning the
    // root keeps the batch.
    let pinned = reqwest::blocking::Client::new()
        .post(format!("{api}/api/v0/pin/ls?arg={}", archive.root))
        .send()
        .expect("pin/ls")
        .text()
        .expect("body");
    assert!(
        pinned.contains(&archive.root.to_string()),
        "kubo did not pin the root: {pinned}"
    );

    // And the export verifies against the root, section by section.
    let car = store.get(&locator, &archive.root).expect("export");
    assert_eq!(
        open_archive(&car, &archive.root).expect("the export verifies"),
        blocks
    );
}
