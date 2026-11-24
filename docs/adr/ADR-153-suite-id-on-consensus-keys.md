# ADR-153: A suite id on every consensus key

- **Status:** accepted, 2026-11-24. Lands before genesis with the re-freeze.
- **Decision owner:** Eric, 2026-11-24 ("start working on the gap items"; gap NEW-2).
- **Related:** ADR-007 (signature suites), ADR-051 option 3 (ML-DSA-87 validator keys), ADR-082 (upgrade levers), ADR-152, spec CRY-4 and CRY-7, invariant 29.

## Context

Transactions have been crypto-agile since ADR-007: each envelope names its suite, and the schedule (ADR-082) can sunset a suite at a height all nodes agree on. Consensus keys could not. A validator key was always 2,592 bytes of ML-DSA-87, and that held in four places: the `RegisterValidator` payload, the `k:key:<id>` record, the genesis validator list and the BFT wire bound (8 KiB, which an SLH-DSA signature exceeds). The security council's keys were 1,952 bytes of ML-DSA-65 in the council record and in every approval. If ML-DSA weakened, moving validators or the council to another family would need a new payload, new records and a new frame format, which means a hard fork during an emergency.

## Decision

**Every consensus key carries its suite byte, on the wire and in state. Which suites are accepted is a compiled list that an upgrade extends. No format change is needed to add one.**

| Where | Layout now | Accepted today |
|---|---|---|
| `RegisterValidator` payload | `0 suite:u8 key bond commission possession` | `0x11` (ML-DSA-87, validator context) |
| `k:key:<id>` record | `suite:u8 ‖ key`; the id is still `BLAKE3-derive-key("maya2c validator id v1", key)` | `0x11` |
| Council record | version 2: `… n (suite:u8 key)*n` | `0x10` (ML-DSA-65) |
| Council approval | `member:u8 suite:u8 sig` | `0x10` |
| BFT frame signature bound | `MAX_VALIDATOR_SIGNATURE_LEN`, the largest signature of any accepted validator suite (4,627 today), derived from the list rather than written down | — |

- **The lists.** `validator_keys::VALIDATOR_SUITES` and `council::COUNCIL_SUITES`. Any other byte is a decode error, the same rule as an unknown transaction suite (invariant 29). Adding a suite is a code change plus vectors, activated at a scheduled height (ADR-082). Old encodings stay valid, because the byte they carry is still on the list.
- **Certificates carry no suite byte.** A vote's suite is a property of the voter's key, which the committee record already states. A second copy in every certificate entry would add a field that could disagree with the key, and nothing would be gained: signatures on the BFT wire are already length-prefixed, and the bound grows with the list. This departs from the gap note, which proposed a byte per certificate entry. The departure is deliberate and is recorded here (Standing Order 9).
- **Council approvals do carry it.** Council payloads are decoded before any state is read, so the decoder needs the signature length from the payload itself.

## The guarantee

Every key the chain's safety rests on (validators, votes, attestations, the council) can move to a new signature family through a scheduled upgrade, without a hard fork and without re-registering under a new payload format. Together with the sunset floor (ADR-082 amendment 2026-11-24, CRY-5), a break of one hardness assumption is handled by an activation height, not by an emergency format change.

## What is not done

- A second validator suite is not compiled. Adding a hash-based vote suite means a validator-context SLH-DSA, the larger signature in the wire bound (which follows automatically), and a check of certificate sizes against the block byte cap. That needs its own ADR amendment when it is needed.
- A sunset rule for validator suites would apply only once there are two. The rule will be the same as CRY-5: one lattice and one hash-based suite always remain.
