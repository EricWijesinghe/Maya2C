//! Vault accounts (ADR-030): the rules, executed by the node's own state
//! transition.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::htlc_payload::HtlcLock;
use custom_l1_node::core::vault_payload::{RECONFIGURE_ID, VaultAction, VaultConfig};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_htlc_lattice::{HashFunction, Lock, Preimage};
use tempfile::TempDir;

mod common;

const FUNDS: u64 = 10_000;
const DELAY: u64 = 10;
const LIMIT: u64 = 100;

struct Chain {
    db: StateDB,
    height: u64,
    _dir: TempDir,
}

fn key() -> (HybridSigningKey, Address) {
    let k = generate_signing_key().unwrap();
    let a = k.address();
    (k, a)
}

impl Chain {
    fn new(funded: &[Address]) -> Self {
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        common::bind(&db);
        for a in funded {
            db.put_account(
                a,
                &Account {
                    balance: FUNDS,
                    nonce: 0,
                },
            )
            .unwrap();
        }
        Self {
            db,
            height: 0,
            _dir: dir,
        }
    }

    /// One transaction in the next block; `Err` if the block is refused.
    fn send(&mut self, tx: Transaction) -> Result<(), String> {
        self.height += 1;
        let block = Block::new(
            BlockHeader {
                prev_hash: [0; 32],
                state_root: [0; 32],
                timestamp: 1_790_000_000 + self.height,
                nonce: 0,
                difficulty_target: target_from_leading_zero_bits(0),
                tx_root: [0; 32],
            },
            vec![tx],
        );
        self.db
            .apply_block(&block, BlockContext::at_height(self.height))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn idle_to(&mut self, height: u64) {
        self.height = self.height.max(height);
    }

    fn balance(&self, a: &Address) -> u64 {
        self.db.get_account(a).unwrap().balance
    }

    fn nonce(&self, a: &Address) -> u64 {
        self.db.get_account(a).unwrap().nonce
    }

    fn escrowed(&self, owner: &Address) -> u64 {
        self.db
            .committed_withdrawals(owner)
            .unwrap()
            .iter()
            .map(|(_, w)| w.escrowed())
            .sum()
    }
}

fn vault(chain: &Chain, key: &HybridSigningKey, action: VaultAction) -> Transaction {
    let mut tx =
        Transaction::with_kind(TxKind::Vault(Box::new(action)), chain.nonce(&key.address()));
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn transfer(chain: &Chain, key: &HybridSigningKey, to: Address, amount: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: to,
        }],
        chain.nonce(&key.address()),
    );
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn configure(guardian: Address) -> VaultAction {
    VaultAction::Configure(VaultConfig {
        delay_blocks: DELAY,
        limit: LIMIT,
        guardians: vec![guardian],
    })
}

#[test]
fn a_stolen_key_waits_and_a_guardian_cancels() {
    let (owner_key, owner) = key();
    let (guardian_key, guardian) = key();
    let (thief_payee, bob) = ([0x7E; 32], [0xB0; 32]);
    let mut chain = Chain::new(&[owner, guardian]);
    chain
        .send(vault(&chain, &owner_key, configure(guardian)))
        .unwrap();

    // In-limit transfers stay instant; above the limit they are refused.
    chain
        .send(transfer(&chain, &owner_key, bob, LIMIT))
        .unwrap();
    let refused = chain
        .send(transfer(&chain, &owner_key, thief_payee, LIMIT + 1))
        .unwrap_err();
    assert!(refused.contains("instant limit"), "{refused}");

    // Other doors out are closed: an HTLC lock from a vault is refused.
    let mut lock = Transaction::with_kind(
        TxKind::HtlcLock(Box::new(HtlcLock {
            recipient: thief_payee,
            amount: 5_000,
            expiry_height: 1_000,
            lock: Lock::hash(HashFunction::Sha256, &Preimage::new([1; 32])),
        })),
        chain.nonce(&owner),
    );
    lock.sign(&owner_key, &common::test_chain()).unwrap();
    assert!(chain.send(lock).unwrap_err().contains("may not send"));

    // The thief, holding the owner's key, requests everything. It leaves the
    // balance into escrow, and cannot be executed before the delay.
    let before = chain.balance(&owner);
    chain
        .send(vault(
            &chain,
            &owner_key,
            VaultAction::Request {
                to: thief_payee,
                amount: 9_000,
            },
        ))
        .unwrap();
    assert_eq!(chain.balance(&owner), before - 9_000);
    assert_eq!(chain.escrowed(&owner), 9_000);
    let early = chain
        .send(vault(
            &chain,
            &guardian_key,
            VaultAction::Execute { owner, id: 0 },
        ))
        .unwrap_err();
    assert!(early.contains("matures"), "{early}");

    // The guardian cancels; the money is back and the thief got nothing.
    chain
        .send(vault(
            &chain,
            &guardian_key,
            VaultAction::Cancel { owner, id: 0 },
        ))
        .unwrap();
    assert_eq!(chain.balance(&owner), before);
    assert_eq!(chain.escrowed(&owner), 0);
    assert_eq!(chain.balance(&thief_payee), 0);
    assert_eq!(
        chain.balance(&bob),
        LIMIT,
        "supply accounted for: nothing created or lost"
    );
}

#[test]
fn a_matured_request_pays_and_only_once() {
    let (owner_key, owner) = key();
    let (guardian_key, guardian) = key();
    let (stranger_key, stranger) = key();
    let bob = [0xB1; 32];
    let mut chain = Chain::new(&[owner, guardian, stranger]);
    chain
        .send(vault(&chain, &owner_key, configure(guardian)))
        .unwrap();
    chain
        .send(vault(
            &chain,
            &owner_key,
            VaultAction::Request {
                to: bob,
                amount: 2_000,
            },
        ))
        .unwrap();
    let requested_at = chain.height;

    // A stranger cannot cancel.
    let refused = chain
        .send(vault(
            &chain,
            &stranger_key,
            VaultAction::Cancel { owner, id: 0 },
        ))
        .unwrap_err();
    assert!(refused.contains("guardian"), "{refused}");

    chain.idle_to(requested_at + DELAY - 1);
    // Anyone may execute a matured request.
    chain
        .send(vault(
            &chain,
            &stranger_key,
            VaultAction::Execute { owner, id: 0 },
        ))
        .unwrap();
    assert_eq!(chain.balance(&bob), 2_000);
    assert_eq!(chain.balance(&owner), FUNDS - 2_000);
    let again = chain
        .send(vault(
            &chain,
            &guardian_key,
            VaultAction::Execute { owner, id: 0 },
        ))
        .unwrap_err();
    assert!(again.contains("no open request"), "{again}");
}

#[test]
fn reconfiguration_waits_out_the_current_delay_and_can_be_cancelled() {
    let (owner_key, owner) = key();
    let (guardian_key, guardian) = key();
    let mut chain = Chain::new(&[owner, guardian]);
    chain
        .send(vault(&chain, &owner_key, configure(guardian)))
        .unwrap();

    // A thief tries to lift the limit: it is queued, not applied.
    let open = VaultAction::Configure(VaultConfig {
        delay_blocks: 1,
        limit: u64::MAX,
        guardians: vec![guardian],
    });
    chain.send(vault(&chain, &owner_key, open)).unwrap();
    assert!(
        chain
            .send(transfer(&chain, &owner_key, [0x7F; 32], 5_000))
            .is_err(),
        "the old limit still binds"
    );
    chain
        .send(vault(
            &chain,
            &guardian_key,
            VaultAction::Cancel {
                owner,
                id: RECONFIGURE_ID,
            },
        ))
        .unwrap();
    let record = chain.db.committed_vault(&owner).unwrap().unwrap();
    assert!(record.pending.is_none());

    // An honest reconfiguration takes effect by height, with nothing sent.
    let raise = VaultAction::Configure(VaultConfig {
        delay_blocks: DELAY,
        limit: 1_000,
        guardians: vec![guardian],
    });
    chain.send(vault(&chain, &owner_key, raise)).unwrap();
    let queued_at = chain.height;
    assert!(
        chain
            .send(transfer(&chain, &owner_key, [0x7F; 32], 500))
            .is_err()
    );
    chain.idle_to(queued_at + DELAY);
    chain
        .send(transfer(&chain, &owner_key, [0x7F; 32], 500))
        .unwrap();
}

#[test]
fn bad_policies_and_requests_are_refused() {
    let (owner_key, owner) = key();
    let (_, guardian) = key();
    let mut chain = Chain::new(&[owner]);
    let zero_delay = VaultAction::Configure(VaultConfig {
        delay_blocks: 0,
        limit: 1,
        guardians: vec![guardian],
    });
    assert!(chain.send(vault(&chain, &owner_key, zero_delay)).is_err());
    let no_guardian = VaultAction::Configure(VaultConfig {
        delay_blocks: 5,
        limit: 1,
        guardians: vec![],
    });
    assert!(chain.send(vault(&chain, &owner_key, no_guardian)).is_err());
    // A request from an account with no vault is refused.
    assert!(
        chain
            .send(vault(
                &chain,
                &owner_key,
                VaultAction::Request {
                    to: guardian,
                    amount: 1
                }
            ))
            .is_err()
    );
    // An ordinary account is untouched: a large transfer is instant.
    chain
        .send(transfer(&chain, &owner_key, guardian, 9_000))
        .unwrap();
}

#[test]
fn the_fee_collector_is_not_a_way_around_the_limit() {
    // Review finding C1: fee outputs were excluded from the limit and have no
    // upper bound, so a stolen key could send the whole balance to the
    // collector at once. Above the fee rule's headroom they now count.
    let (owner_key, owner) = key();
    let (_, guardian) = key();
    let mut chain = Chain::new(&[owner]);
    chain
        .send(vault(&chain, &owner_key, configure(guardian)))
        .unwrap();
    let collector = custom_l1_node::state::fees::FEE_COLLECTOR;
    let drain = transfer(&chain, &owner_key, collector, 9_000);
    let refused = chain.send(drain).unwrap_err();
    assert!(refused.contains("instant limit"), "{refused}");
    assert_eq!(chain.balance(&owner), FUNDS);
    // An in-limit amount to the collector is just a transfer under the limit.
    chain
        .send(transfer(&chain, &owner_key, collector, LIMIT))
        .unwrap();
}

#[test]
fn the_limit_bounds_a_window_not_a_transaction() {
    // Review: a per-transaction limit let a stolen key drain a vault through
    // many in-limit transfers at once. The limit is outflow per window of
    // `delay_blocks`.
    let (owner_key, owner) = key();
    let (_, guardian) = key();
    let bob = [0xB2; 32];
    let mut chain = Chain::new(&[owner]);
    chain
        .send(vault(&chain, &owner_key, configure(guardian)))
        .unwrap();
    chain.send(transfer(&chain, &owner_key, bob, 60)).unwrap();
    let refused = chain
        .send(transfer(&chain, &owner_key, bob, 60))
        .unwrap_err();
    assert!(refused.contains("already used"), "{refused}");
    chain.send(transfer(&chain, &owner_key, bob, 40)).unwrap();
    assert_eq!(chain.balance(&bob), 100);
    let window_start = chain.height;
    chain.idle_to(window_start + DELAY);
    chain.send(transfer(&chain, &owner_key, bob, 100)).unwrap();
    assert_eq!(chain.balance(&bob), 200);
}
