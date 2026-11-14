# Prior art: Mind-link safety (consent and logging for brain-computer interfaces)

**Search status:** searched 2026-11-10 (web search and page fetch; sources below). Neuralink's own materials were not searched. IEEE P2731's current status could not be confirmed.

Feature: CATALOG §I row `mind-link-safety`, from `hal/bci-sim`.

## Who else does something similar

| Project or work | What it covers | Source |
|---|---|---|
| Chile, Law 21.383 (Oct 2021) | Constitutional amendment protecting "brain activity" and derived information; implementing bill Boletín 13.828-19 reported still in the legislature. In Aug 2023 the Supreme Court ordered Emotiv to delete a former senator's brain data | [Neurorights Foundation](https://neurorightsfoundation.org/chile), [IEEE Spectrum](https://spectrum.ieee.org/neurotech-neurorights) |
| UNESCO Recommendation on the Ethics of Neurotechnology | Adopted Nov 2025 (date per secondary coverage; verify); non-binding | [Inside Privacy](https://www.insideprivacy.com/emerging-technologies/unesco-adopts-first-global-framework-on-neurotechnology-ethics/) |
| IEEE P2731 | "Unified Terminology for Brain-Computer Interfaces", PAR approved 2018-09-27; a terminology project, not a consent or safety standard | [IEEE SA group](https://sagroups.ieee.org/2731), [functional model paper](https://pmc.ncbi.nlm.nih.gov/articles/PMC8901026) |
| FDA final guidance on implanted BCIs (May 2021) | Non-clinical and clinical considerations for implanted BCIs; excludes non-implanted devices | [RAPS](https://www.raps.org/resource/fda-finalizes-brain-computer-interface-guidance-is.html), [FDA neuro overview](https://www.fda.gov/medical-devices/neurological-devices/regulatory-overview-neurological-devices) |
| California 2024 brain-data law | Safeguards neural data held by neurotech firms (one secondary source; text not read) | [IAS Gyan summary](https://www.iasgyan.in/daily-current-affairs/unescos-global-framework-on-neurotechnology) |

Not found: an on-chain consent or access-log scheme for neural data. Colorado's law and the OECD recommendation were searched for but not confirmed.

## How a Maya2C version would differ (each backed by the repo)

- **What exists is authentication, not consent.** `hal/bci-sim` is SIM (`hal/bci-sim/src/lib.rs:1-11`): synthetic EEG and an EDF parser. No consent record or access log exists.
- **The crate states its limits:** nothing has run on human EEG; EEG stability for authentication is open (`lib.rs:7-9, 19-24`); the stimulus check needs attested hardware, which does not exist.
- **Status SIM** in `features.toml` and `docs/architecture-vision.md`; Standing Order 3 keeps it off production paths.
- A possible difference worth testing: consent and access events anchored on chain with raw neural data off chain (erasure rights forbid on-chain neural data). A hypothesis, not a feature.

## Legal and ethical gates

- Medical-device law (FDA, EU MDR) applies to any clinical device; nothing in the repo is one or may be presented as one.
- Neural data is specially protected (Chile, UNESCO). Real-data work needs ethics-board review and informed consent; synthetic data only until then.
- No neural data on chain; no real subject data, no human trials, no medical claims.

## What may be said publicly

- Accurate: "Maya2C includes a simulator for EEG-based authentication research; it has never been run on human data."
- **forbidden**: "first brain-computer interface blockchain", "only chain with neurorights", "unprecedented", "mind-safe", "neural consent protocol".

**Verdict:** SIM, stays SIM. Promotion would need consent-record code with tests, legal review per jurisdiction, and ethics oversight.

