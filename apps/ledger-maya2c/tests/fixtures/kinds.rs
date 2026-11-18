//! Reads `suite-0x10-kinds.txt`, written by the wallet core with the node's
//! encoder (`apps/wallet-gui/core/tests/ledger_kinds_fixture_tests.rs`).
//! Each line: `name verdict hex`, the device key written as `KEY`.

/// One shape: its name, the status word the device must answer (`0x9000`
/// for one it shows and signs), and its signing bytes for the given key.
pub struct Case {
    pub name: &'static str,
    pub status: u16,
    pub signing_bytes: Vec<u8>,
}

pub fn cases(key: &[u8]) -> Vec<Case> {
    let key = hex::encode(key);
    include_str!("suite-0x10-kinds.txt")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let mut parts = line.split(' ');
            let (name, verdict, bytes) = (
                parts.next().expect("name"),
                parts.next().expect("verdict"),
                parts.next().expect("bytes"),
            );
            let status = match verdict {
                "ok" => 0x9000,
                sw => u16::from_str_radix(sw, 16).expect("status word"),
            };
            Case {
                name,
                status,
                signing_bytes: hex::decode(bytes.trim().replace("KEY", &key)).expect("hex"),
            }
        })
        .collect()
}
