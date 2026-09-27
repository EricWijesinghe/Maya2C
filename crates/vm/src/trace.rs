//! Recording a contract call for the time-travel debugger (Master Prompt 24).
//!
//! [`Traced`] wraps any [`HostState`] and records every interaction a call
//! has with the chain — storage reads and writes, events, balance and height
//! reads — in order. A [`Timeline`] then answers "what did storage and the
//! event log look like after step *k*?" for any *k*, forward or backward,
//! by folding the recorded writes up to it: time travel without re-executing.
//!
//! Granularity is the **host call**, not the instruction or source line: the
//! VM exposes no per-instruction hook, and mapping wasm offsets to source
//! lines needs DWARF the contracts are not built with. Gas is known for the
//! whole call ([`crate::Outcome::gas_used`]), not per step.
//!
//! Tracing changes nothing a contract can observe: every read returns what
//! the wrapped state returns. It is a development tool and is never on a
//! consensus path.

use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::host::{Address, ContractId, Event, HostState, OracleValue, RANDOMNESS_LEN};
use crate::zkml::{ZKML_MODEL_ID_LEN, ZkmlVerdict};

/// One interaction with the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// `balance_of`.
    Balance {
        /// Whose.
        address: Address,
        /// What it returned.
        value: u64,
    },
    /// `caller`.
    Caller(Option<Address>),
    /// `block_height`.
    Height(u64),
    /// `block_randomness`.
    Randomness(Option<[u8; RANDOMNESS_LEN]>),
    /// `oracle_feed`.
    Oracle {
        /// Feed id.
        feed: [u8; 32],
        /// The value, if the feed exists.
        value: Option<OracleValue>,
    },
    /// `verify_zkml`.
    Zkml(ZkmlVerdict),
    /// `storage_get`.
    Read {
        /// Contract.
        contract: ContractId,
        /// Key.
        key: Vec<u8>,
        /// Value read.
        value: Option<Vec<u8>>,
    },
    /// `storage_set`, with the value it replaced.
    Write {
        /// Contract.
        contract: ContractId,
        /// Key.
        key: Vec<u8>,
        /// Value before this write.
        before: Option<Vec<u8>>,
        /// Value written.
        after: Vec<u8>,
    },
    /// `emit`.
    Event(Event),
}

/// A [`HostState`] that records every interaction.
#[derive(Debug)]
pub struct Traced<S> {
    inner: S,
    steps: RefCell<Vec<Step>>,
}

impl<S: HostState> Traced<S> {
    /// Wraps `inner`.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            steps: RefCell::new(Vec::new()),
        }
    }

    /// The wrapped state and the recorded timeline.
    #[must_use]
    pub fn finish(self) -> (S, Timeline) {
        (
            self.inner,
            Timeline {
                steps: self.steps.into_inner(),
            },
        )
    }

    fn record(&self, step: Step) {
        self.steps.borrow_mut().push(step);
    }
}

impl<S: HostState> HostState for Traced<S> {
    fn balance_of(&self, address: &Address) -> u64 {
        let value = self.inner.balance_of(address);
        self.record(Step::Balance {
            address: *address,
            value,
        });
        value
    }

    fn caller(&self) -> Option<Address> {
        let caller = self.inner.caller();
        self.record(Step::Caller(caller));
        caller
    }

    fn block_height(&self) -> u64 {
        let height = self.inner.block_height();
        self.record(Step::Height(height));
        height
    }

    fn block_randomness(&self) -> Option<[u8; RANDOMNESS_LEN]> {
        let value = self.inner.block_randomness();
        self.record(Step::Randomness(value));
        value
    }

    fn oracle_feed(&self, feed_id: &[u8; 32]) -> Option<OracleValue> {
        let value = self.inner.oracle_feed(feed_id);
        self.record(Step::Oracle {
            feed: *feed_id,
            value,
        });
        value
    }

    fn verify_zkml(
        &self,
        model_id: &[u8; ZKML_MODEL_ID_LEN],
        vk: &[u8],
        public: &[i64],
        proof: &[u8],
    ) -> ZkmlVerdict {
        let verdict = self.inner.verify_zkml(model_id, vk, public, proof);
        self.record(Step::Zkml(verdict.clone()));
        verdict
    }

    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        let value = self.inner.storage_get(contract, key);
        self.record(Step::Read {
            contract: *contract,
            key: key.to_vec(),
            value: value.clone(),
        });
        value
    }

    fn storage_set(&mut self, contract: &ContractId, key: Vec<u8>, value: Vec<u8>) {
        let before = self.inner.storage_get(contract, &key);
        self.record(Step::Write {
            contract: *contract,
            key: key.clone(),
            before,
            after: value.clone(),
        });
        self.inner.storage_set(contract, key, value);
    }

    fn emit(&mut self, event: Event) {
        self.record(Step::Event(event.clone()));
        self.inner.emit(event);
    }
}

/// Storage as seen at some step: `(contract, key) -> value`, only keys the
/// call touched. `None` is "absent".
pub type StorageView = BTreeMap<(ContractId, Vec<u8>), Option<Vec<u8>>>;

/// A recorded call, navigable in both directions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    steps: Vec<Step>,
}

impl Timeline {
    /// Every step, in order.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// Storage of every touched key before any step ran.
    #[must_use]
    pub fn initial_storage(&self) -> StorageView {
        let mut view = StorageView::new();
        for step in &self.steps {
            let (contract, key, value) = match step {
                Step::Read {
                    contract,
                    key,
                    value,
                } => (contract, key, value),
                Step::Write {
                    contract,
                    key,
                    before,
                    ..
                } => (contract, key, before),
                _ => continue,
            };
            view.entry((*contract, key.clone()))
                .or_insert_with(|| value.clone());
        }
        view
    }

    /// Storage of every touched key after the first `upto` steps (0 is the
    /// initial state, `steps().len()` the final one).
    #[must_use]
    pub fn storage_after(&self, upto: usize) -> StorageView {
        let mut view = self.initial_storage();
        for step in self.steps.iter().take(upto) {
            if let Step::Write {
                contract,
                key,
                after,
                ..
            } = step
            {
                view.insert((*contract, key.clone()), Some(after.clone()));
            }
        }
        view
    }

    /// Events emitted within the first `upto` steps.
    #[must_use]
    pub fn events_after(&self, upto: usize) -> Vec<&Event> {
        self.steps
            .iter()
            .take(upto)
            .filter_map(|s| match s {
                Step::Event(e) => Some(e),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::host::MemoryState;

    const C: ContractId = [1; 32];

    #[test]
    fn storage_can_be_seen_at_every_step_forward_and_back() {
        let mut state = MemoryState::at_height(9);
        state.storage.insert((C, b"n".to_vec()), vec![1]);
        let mut traced = Traced::new(state);
        assert_eq!(traced.storage_get(&C, b"n"), Some(vec![1]));
        traced.storage_set(&C, b"n".to_vec(), vec![2]);
        traced.emit(Event {
            contract: C,
            topic: b"t".to_vec(),
            data: vec![],
        });
        traced.storage_set(&C, b"m".to_vec(), vec![7]);
        assert_eq!(traced.block_height(), 9);
        let (state, timeline) = traced.finish();

        assert_eq!(timeline.steps().len(), 5);
        let n = (C, b"n".to_vec());
        let m = (C, b"m".to_vec());
        assert_eq!(timeline.storage_after(0)[&n], Some(vec![1]));
        assert_eq!(
            timeline.storage_after(0)[&m],
            None,
            "absent before its write"
        );
        assert_eq!(timeline.storage_after(2)[&n], Some(vec![2]));
        assert_eq!(timeline.storage_after(5)[&m], Some(vec![7]));
        // Backward is just a smaller index: nothing is re-executed.
        assert_eq!(timeline.storage_after(1)[&n], Some(vec![1]));
        assert_eq!(timeline.events_after(2).len(), 0);
        assert_eq!(timeline.events_after(3).len(), 1);
        // The wrapped state saw exactly the same writes.
        assert_eq!(state.storage_get_for_test(&C, b"m"), Some(vec![7]));
    }
}
