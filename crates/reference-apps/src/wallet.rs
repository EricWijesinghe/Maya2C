//! The minimal wallet every app shares: one ML-DSA-65 key, its account, and
//! a nonce counter.

use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId};
use maya_smart_account::fees::FeeSpec;
use maya_smart_account::op::{Action, Op};
use maya_smart_account::{AccountId, Registry, key_hash};

/// An ML-DSA-65 signing key.
pub type SigningKey = <MlDsa65 as SignatureSuite>::SigningKey;

/// Fee cap every template op carries. The fee market is inactive, so this is
/// a ceiling the op states, not a price it pays.
pub const MAX_FEE: u64 = 1_000;

/// A key pair and the account it controls.
pub struct Wallet {
    /// The account's key; its public half is registered once in the account.
    pub key: Key,
    /// The account.
    pub id: AccountId,
    nonce: u64,
}

/// A key pair not yet registered anywhere (a session key, a recovery key).
pub struct Key {
    sk: SigningKey,
    /// The public key.
    pub pk: Vec<u8>,
}

impl Key {
    /// Derives a key from a 32-byte seed. Templates use fixed seeds so their
    /// tests are reproducible; a real app draws the seed from the OS RNG.
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let sk = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(seed));
        let pk = MlDsa65::public_key(&sk);
        Self { sk, pk }
    }

    /// Signs `op` with this key, replacing any signatures it carries.
    ///
    /// # Panics
    ///
    /// Never for a key built by [`Key::from_seed`]: ML-DSA signing of a
    /// bounded message does not fail.
    #[must_use]
    pub fn sign(&self, mut op: Op) -> Op {
        let msg = op.signing_bytes();
        let sig = MlDsa65::sign(&self.sk, &msg).expect("ML-DSA-65 signing");
        op.signatures = vec![(key_hash(SuiteId::MlDsa65, &self.pk), sig)];
        op
    }
}

impl Wallet {
    /// Creates the account in `registry` with `balance`.
    #[must_use]
    pub fn open(registry: &mut Registry, seed: [u8; 32], height: u64, balance: u64) -> Self {
        let key = Key::from_seed(seed);
        let id = registry.create(SuiteId::MlDsa65, &key.pk, height, balance);
        Self { key, id, nonce: 0 }
    }

    /// An unsigned op from this account with the next nonce.
    #[must_use]
    pub fn op(&mut self, action: Action) -> Op {
        let op = Op {
            account: self.id,
            nonce: self.nonce,
            action,
            fee: FeeSpec::Native { max_fee: MAX_FEE },
            signatures: vec![],
        };
        self.nonce += 1;
        op
    }

    /// An op signed by the account's own key.
    #[must_use]
    pub fn signed(&mut self, action: Action) -> Op {
        let op = self.op(action);
        self.key.sign(op)
    }

    /// Forgets the nonce just used, after an op was refused and not applied.
    pub fn refused(&mut self) {
        self.nonce -= 1;
    }
}
