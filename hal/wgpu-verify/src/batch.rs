//! The host API: verify a batch, the lattice step on a backend, every answer
//! checked against the CPU.

use crate::lattice::{
    LatticeBackend, LatticeInput, LatticeOutput, MAX_DISPATCH_ITEMS, Stage, lattice_on_cpu,
};
use crate::mldsa::{self, Item, Prepared};

/// What the backend-assisted path said about one item, next to the CPU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assisted {
    /// The CPU refused before the lattice step (length, context, hint
    /// encoding) and `maya-crypto-pq` refused too. Nothing was sent.
    NotSent,
    /// Every intermediate equalled the CPU reference's and the finished
    /// verdict equalled `maya-crypto-pq`'s.
    Agrees,
    /// The backend's intermediates differed from the CPU reference's.
    IntermediateMismatch(Stage),
    /// The split path finished with `assisted` where `maya-crypto-pq` said the
    /// opposite: a bug in this crate's decomposition, whatever the backend.
    VerdictMismatch {
        /// What the split path concluded.
        assisted: bool,
    },
    /// The backend failed (no device, lost device, timeout, stale buffer).
    Unavailable,
}

/// One item's result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemResult {
    /// `maya-crypto-pq`'s verdict. The only verdict (ADR-137 O-6 A).
    pub verdict: bool,
    /// The comparison.
    pub assisted: Assisted,
}

/// Verifies `items`, running the lattice step on `backend` in dispatches of at
/// most [`MAX_DISPATCH_ITEMS`]. Results are per item and in order.
///
/// A backend error makes every item of that dispatch and every later one
/// [`Assisted::Unavailable`]; the verdicts are unaffected, because they never
/// came from the backend.
pub fn batch_verify(backend: &mut dyn LatticeBackend, items: &[Item]) -> Vec<ItemResult> {
    let verdicts: Vec<bool> = items.iter().map(mldsa::cpu_verdict).collect();
    let prepared: Vec<Option<Prepared>> = items.iter().map(|i| mldsa::prepare(i).ok()).collect();
    let sent: Vec<&Prepared> = prepared.iter().flatten().collect();
    let inputs: Vec<LatticeInput> = sent.iter().map(|p| (*p.input).clone()).collect();
    let answers = run_bounded(backend, &inputs);

    let mut answer_iter = sent.iter().zip(&inputs).zip(answers);
    prepared
        .iter()
        .zip(verdicts)
        .map(|(maybe, verdict)| {
            let assisted = match maybe {
                None if verdict => Assisted::VerdictMismatch { assisted: false },
                None => Assisted::NotSent,
                Some(_) => match answer_iter.next() {
                    Some(((prep, input), Some(answer))) => compare(prep, input, &answer, verdict),
                    _ => Assisted::Unavailable,
                },
            };
            ItemResult { verdict, assisted }
        })
        .collect()
}

fn compare(
    prep: &Prepared,
    input: &LatticeInput,
    answer: &LatticeOutput,
    verdict: bool,
) -> Assisted {
    if let Some(stage) = lattice_on_cpu(input).first_difference(answer) {
        return Assisted::IntermediateMismatch(stage);
    }
    let assisted = mldsa::finish(prep, answer);
    if assisted == verdict {
        Assisted::Agrees
    } else {
        Assisted::VerdictMismatch { assisted }
    }
}

/// Bounded dispatches; the first failure ends the batch's GPU half.
fn run_bounded(
    backend: &mut dyn LatticeBackend,
    inputs: &[LatticeInput],
) -> Vec<Option<LatticeOutput>> {
    let mut out: Vec<Option<LatticeOutput>> = Vec::with_capacity(inputs.len());
    let mut failed = false;
    for chunk in inputs.chunks(MAX_DISPATCH_ITEMS) {
        let answers = if failed {
            None
        } else {
            backend.run(chunk).ok()
        };
        match answers {
            Some(answers) if answers.len() == chunk.len() => {
                out.extend(answers.into_iter().map(Some))
            }
            _ => {
                failed = true;
                out.extend(chunk.iter().map(|_| None));
            }
        }
    }
    out
}
