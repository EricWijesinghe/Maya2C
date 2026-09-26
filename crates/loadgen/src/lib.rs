//! Workload generator (Master Prompt 12 §0).
//!
//! A [`Workload`] is a reproducible stream of [`Tx`] — plain transfers, token
//! transfers, contract calls that touch shared hot keys, and contract deploys
//! — drawn from a seeded generator. Two knobs set how hard it is to execute
//! in parallel:
//!
//! - **contention**: the share of transactions that touch one of `hot_keys`
//!   shared keys (a popular pool, a mint);
//! - **Zipf exponent**: how skewed ordinary account access is (s = 0 is
//!   uniform; s ≈ 1 is what real account activity looks like).
//!
//! The transactions are *programs over a key-value state* — reads and writes
//! of named keys — rather than signed chain transactions, so the same
//! workload drives the parallel executor (`crates/parallel-exec`), the econ
//! model, or a node adapter. Signatures are a separate cost measured
//! separately (Production Standing Orders: pre-verified work is labelled).

#![warn(missing_docs)]

mod zipf;

pub use zipf::Zipf;

/// A state key. Accounts are `0 .. accounts`; hot keys and contract storage
/// live in a disjoint range above `HOT_BASE`.
pub type Key = u64;

/// First key of the hot-key range.
pub const HOT_BASE: Key = 1 << 40;
/// First key of contract storage.
pub const CONTRACT_BASE: Key = 1 << 41;

/// What a transaction does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Native transfer between two accounts.
    Transfer,
    /// Token transfer: reads/writes two token-balance keys and the token's
    /// supply key (read only).
    TokenTransfer,
    /// Contract call that touches a shared hot key.
    ContractCall,
    /// Contract deploy: writes a fresh code key.
    Deploy,
}

/// One transaction as a key-value program: every key it may read and write,
/// and the amount it moves. Execution semantics live in the executor; the
/// generator only decides *which* keys, which is what contention is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tx {
    /// Position in the stream.
    pub id: u64,
    /// Kind.
    pub kind: Kind,
    /// Account paying (and nonce-bumped).
    pub sender: Key,
    /// Keys read.
    pub reads: Vec<Key>,
    /// Keys written (a subset of what an executor may touch).
    pub writes: Vec<Key>,
    /// Amount moved.
    pub amount: u64,
}

/// Share of each kind, in percent; must sum to 100.
#[derive(Clone, Copy, Debug)]
pub struct Mix {
    /// Native transfers.
    pub transfer: u8,
    /// Token transfers.
    pub token: u8,
    /// Contract calls.
    pub call: u8,
    /// Deploys.
    pub deploy: u8,
}

impl Mix {
    /// A realistic-looking default: mostly transfers.
    pub const DEFAULT: Self = Self {
        transfer: 60,
        token: 25,
        call: 14,
        deploy: 1,
    };
}

/// Workload parameters.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Seed.
    pub seed: u64,
    /// Accounts.
    pub accounts: u64,
    /// Shared hot keys.
    pub hot_keys: u64,
    /// Share of transactions touching a hot key, ppm.
    pub contention_ppm: u32,
    /// Zipf exponent × 100 for account selection.
    pub zipf_s_x100: u32,
    /// Kind mix.
    pub mix: Mix,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            seed: 1,
            accounts: 100_000,
            hot_keys: 8,
            contention_ppm: 100_000,
            zipf_s_x100: 100,
            mix: Mix::DEFAULT,
        }
    }
}

/// A seeded stream of transactions.
#[derive(Clone, Debug)]
pub struct Workload {
    params: Params,
    state: u64,
    next_id: u64,
    zipf: Zipf,
    deployed: u64,
}

impl Workload {
    /// A workload from parameters.
    pub fn new(params: Params) -> Self {
        Self {
            params,
            state: params.seed ^ 0x9E37_79B9_7F4A_7C15,
            next_id: 0,
            zipf: Zipf::new(params.accounts, params.zipf_s_x100),
            deployed: 0,
        }
    }

    fn next_u64(&mut self) -> u64 {
        // SplitMix64.
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn account(&mut self) -> Key {
        let u = self.next_u64();
        self.zipf.sample(u)
    }

    fn two_accounts(&mut self) -> (Key, Key) {
        let a = self.account();
        let mut b = self.account();
        if b == a {
            b = (a + 1) % self.params.accounts;
        }
        (a, b)
    }

    /// The next transaction.
    pub fn next_tx(&mut self) -> Tx {
        let id = self.next_id;
        self.next_id += 1;
        let pick = self.next_u64() % 100;
        let m = self.params.mix;
        let hot = self.params.hot_keys > 0
            && self.next_u64() % 1_000_000 < u64::from(self.params.contention_ppm);
        let hot_key = HOT_BASE + self.next_u64() % self.params.hot_keys.max(1);
        let amount = 1 + self.next_u64() % 1_000;
        let (kind, sender, mut reads, mut writes) = if pick < u64::from(m.transfer) {
            let (a, b) = self.two_accounts();
            (Kind::Transfer, a, vec![a, b], vec![a, b])
        } else if pick < u64::from(m.transfer + m.token) {
            let (a, b) = self.two_accounts();
            let token = CONTRACT_BASE + (self.next_u64() % 16) * 1_000_000;
            (
                Kind::TokenTransfer,
                a,
                vec![a, token + 1 + a % 999_999, token + 1 + b % 999_999, token],
                vec![a, token + 1 + a % 999_999, token + 1 + b % 999_999],
            )
        } else if pick < u64::from(m.transfer + m.token + m.call) {
            let a = self.account();
            let slot = CONTRACT_BASE + (1 << 30) + self.next_u64() % 10_000;
            (Kind::ContractCall, a, vec![a, slot], vec![a, slot])
        } else {
            let a = self.account();
            self.deployed += 1;
            let code = CONTRACT_BASE + (1 << 35) + self.deployed;
            (Kind::Deploy, a, vec![a], vec![a, code])
        };
        if hot {
            reads.push(hot_key);
            writes.push(hot_key);
        }
        Tx {
            id,
            kind,
            sender,
            reads,
            writes,
            amount,
        }
    }

    /// The next `n` transactions.
    pub fn take(&mut self, n: usize) -> Vec<Tx> {
        (0..n).map(|_| self.next_tx()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workload_is_a_function_of_its_seed() {
        let p = Params::default();
        assert_eq!(Workload::new(p).take(1_000), Workload::new(p).take(1_000));
        let other = Params { seed: 2, ..p };
        assert_ne!(Workload::new(p).take(100), Workload::new(other).take(100));
    }

    #[test]
    fn contention_controls_the_share_touching_hot_keys() {
        for ppm in [0u32, 100_000, 500_000, 900_000] {
            let txs = Workload::new(Params {
                contention_ppm: ppm,
                ..Params::default()
            })
            .take(20_000);
            let hot = txs
                .iter()
                .filter(|t| {
                    t.writes
                        .iter()
                        .any(|k| (HOT_BASE..CONTRACT_BASE).contains(k))
                })
                .count();
            let expected = 20_000 * ppm as usize / 1_000_000;
            assert!(
                hot.abs_diff(expected) < 20_000 / 50 + 1,
                "ppm {ppm}: {hot} vs {expected}"
            );
        }
    }

    #[test]
    fn the_mix_is_respected() {
        let txs = Workload::new(Params::default()).take(20_000);
        let transfers = txs.iter().filter(|t| t.kind == Kind::Transfer).count();
        assert!((11_000..13_000).contains(&transfers), "{transfers}");
    }
}
