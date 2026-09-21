//! **RESEARCH** sources: named, typed, and deliberately not implemented.

use crate::{EntropyError, EntropySource, SourceClass};

/// Casimir-cavity fluctuations.
///
/// The proposal is to sample the fluctuating force between closely spaced
/// plates, driven by zero-point modes of the confined field. There is no
/// characterised device to model and no published min-entropy analysis to
/// model it *from*; a "simulation" would be arbitrary Gaussian noise with a
/// physics label, which is worse than nothing. So this exists as a type — so
/// configuration can name it and the pool can refuse it — and always reports
/// itself unavailable.
#[derive(Debug, Default)]
pub struct CasimirCavity;

impl EntropySource for CasimirCavity {
    fn name(&self) -> &'static str {
        "research-casimir-cavity"
    }

    fn class(&self) -> SourceClass {
        SourceClass::Research
    }

    fn min_entropy_millibits(&self) -> u32 {
        0
    }

    fn fill(&mut self, _out: &mut [u8]) -> Result<(), EntropyError> {
        Err(EntropyError::Unavailable(
            self.name().to_owned(),
            "RESEARCH: no device and no validated model",
        ))
    }
}
