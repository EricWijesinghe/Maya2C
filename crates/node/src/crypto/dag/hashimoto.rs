//! Hashimoto: the hash function the chain's proof of work actually is.
//!
//! One digest is [`ACCESSES`] pseudo-random 128-byte reads through the epoch's
//! dataset, folded into a 128-byte mix and squeezed to 32 bytes. Each lookup
//! index depends on the mix so far, so the reads are strictly sequential in
//! their dependency and cannot be issued ahead of time. Arithmetic between
//! reads is a handful of multiply-xors — deliberately trivial, so that the DRAM
//! round trip is the entire cost and the hash rate a machine achieves is a
//! direct reading of its memory bandwidth.
//!
//! ## Two implementations of one function
//!
//! [`hashimoto_full`] reads a materialised 4 GiB [`Dataset`]. [`hashimoto_light`]
//! recomputes each page it touches from the 64 MiB [`Cache`]. They must return
//! identical bytes — a miner uses the first, the node that validates the miner's
//! block uses the second — and `light_and_full_agree_on_every_nonce` asserts it
//! rather than trusting it.
//!
//! Light costs about a hundred times more per hash: 128 items recomputed at 256
//! cache reads each. That ratio is the design's load-bearing number. Too small
//! and mining from the cache becomes competitive, which deletes the memory
//! requirement the whole scheme rests on; too large and validation stops being
//! cheap. Around 100× is where Ethash sits and where this sits.
//!
//! ## Cost, measured against what it replaces
//!
//! A light hash is roughly 33,000 BLAKE3 compressions, on the order of 1–3 ms.
//! ArgonBlake, the pre-fork rule, is ~25.4 ms. Validation gets an order of
//! magnitude cheaper at the same time as mining gets bound to bandwidth.

use crate::crypto::argon_blake::HASH_LEN;
use crate::crypto::dag::cache::Cache;
use crate::crypto::dag::dataset::{Dataset, dataset_item};
use crate::crypto::dag::{
    ACCESSES, ITEM_BYTES, Item, MIX_WORDS, blake3_512, fnv, item_from_le_bytes, mix_key,
};

/// Words in the compressed mix, and so bytes in it divided by four.
const COMPRESSED_WORDS: usize = MIX_WORDS / 4;

/// The output of one hashimoto evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proof {
    /// The compressed mix: a commitment to the pages that were read.
    ///
    /// Not part of consensus — the header has no field for it — but it is what
    /// a parity test compares when a GPU and the node disagree, because it
    /// isolates the lookups from the final squeeze.
    pub mix: [u8; HASH_LEN],
    /// The digest compared against the difficulty target.
    pub result: [u8; HASH_LEN],
}

/// The seed a nonce mixes from: `BLAKE3-512(mix_key, header_hash ‖ nonce_le)`.
///
/// # Why this is public
///
/// A GPU miner needs it and needs nothing else. `hashimoto` derives the seed
/// once per nonce and then does 64 data-dependent dataset reads; the reads are
/// the entire cost, and they are the half worth moving to a device. Exposing
/// the seam lets an accelerator compute the mix and hand it back without
/// reimplementing BLAKE3 — see `maya-wgpu-miner`.
///
/// The alternative was for the miner to reproduce this derivation from the
/// domain string and the encoding. That is duplication of a consensus input,
/// and a miner whose seed drifted would produce blocks the network rejects
/// with nothing in its logs to explain why. One function here is cheaper than
/// a parity test there.
#[must_use]
pub fn seed_for(header_hash: &[u8; HASH_LEN], nonce: u64) -> [u8; ITEM_BYTES] {
    let mut seed_input = [0u8; HASH_LEN + 8];
    seed_input[..HASH_LEN].copy_from_slice(header_hash);
    seed_input[HASH_LEN..].copy_from_slice(&nonce.to_le_bytes());
    blake3_512(mix_key(), &seed_input)
}

/// Folds a completed mix into the digest, closing the seam [`seed_for`] opens.
///
/// `mix` is the 32-word accumulator an accelerator produces. Everything after
/// it — the four-to-one compression and the final BLAKE3 — happens here, so
/// there is one implementation of each rather than one per backend.
#[must_use]
pub fn finish(seed_bytes: &[u8; ITEM_BYTES], mix: &[u32; MIX_WORDS]) -> Proof {
    let mut compressed = [0u32; COMPRESSED_WORDS];
    for (index, slot) in compressed.iter_mut().enumerate() {
        let base = index * 4;
        *slot = fnv(
            fnv(fnv(mix[base], mix[base + 1]), mix[base + 2]),
            mix[base + 3],
        );
    }

    let mut mix_bytes = [0u8; HASH_LEN];
    let (chunks, _) = mix_bytes.as_chunks_mut::<4>();
    for (word, chunk) in compressed.iter().zip(chunks) {
        *chunk = word.to_le_bytes();
    }

    let mut hasher = blake3::Hasher::new_keyed(mix_key());
    hasher.update(seed_bytes);
    hasher.update(&mix_bytes);

    Proof {
        mix: mix_bytes,
        result: *hasher.finalize().as_bytes(),
    }
}

/// Evaluates hashimoto against a materialised dataset. The miner's path.
#[must_use]
pub fn hashimoto_full(dataset: &Dataset, header_hash: &[u8; HASH_LEN], nonce: u64) -> Proof {
    hashimoto(dataset.pages(), header_hash, nonce, |page| {
        dataset.page(page)
    })
}

/// Evaluates hashimoto by recomputing pages from the cache. The validator's
/// path.
///
/// Identical output to [`hashimoto_full`] over the same epoch, at roughly a
/// hundred times the cost and a sixty-fourth of the memory.
#[must_use]
pub fn hashimoto_light(cache: &Cache, header_hash: &[u8; HASH_LEN], nonce: u64) -> Proof {
    hashimoto(cache.dataset_pages(), header_hash, nonce, |page| {
        let first = page * 2;
        [
            dataset_item(cache, first),
            dataset_item(cache, first.wrapping_add(1)),
        ]
    })
}

/// The shared body. `lookup` is the only difference between the two paths, and
/// keeping it a parameter is what makes it impossible for them to drift.
fn hashimoto(
    pages: u32,
    header_hash: &[u8; HASH_LEN],
    nonce: u64,
    lookup: impl Fn(u32) -> [Item; 2],
) -> Proof {
    // The seed binds the header and the nonce. A miner hashes this once per
    // nonce and nothing else outside the loop, which is why the GPU kernel
    // needs only 32 bytes of job state.
    let mut seed_input = [0u8; HASH_LEN + 8];
    seed_input[..HASH_LEN].copy_from_slice(header_hash);
    seed_input[HASH_LEN..].copy_from_slice(&nonce.to_le_bytes());
    let seed_bytes = blake3_512(mix_key(), &seed_input);
    let seed = item_from_le_bytes(&seed_bytes);

    // The mix starts as the seed laid down twice: 128 bytes, one page wide, so
    // that a lookup can be folded in without any reshaping.
    let mut mix = [0u32; MIX_WORDS];
    for (slot, word) in mix.iter_mut().zip(seed.iter().cycle()) {
        *slot = *word;
    }

    for access in 0..ACCESSES as u32 {
        // Data-dependent: the mix decides where the next read lands, so the
        // reads cannot be batched, reordered or prefetched.
        let page = fnv(access ^ seed[0], mix[access as usize % MIX_WORDS]) % pages;
        let [first, second] = lookup(page);

        for (slot, word) in mix.iter_mut().zip(first.iter().chain(second.iter())) {
            *slot = fnv(*slot, *word);
        }
    }

    // Fold four words into one, 32 words down to 8, so the digest commits to
    // the whole mix at a quarter of the input length.
    let mut compressed = [0u32; COMPRESSED_WORDS];
    for (index, slot) in compressed.iter_mut().enumerate() {
        let base = index * 4;
        *slot = fnv(
            fnv(fnv(mix[base], mix[base + 1]), mix[base + 2]),
            mix[base + 3],
        );
    }

    let mut mix_bytes = [0u8; HASH_LEN];
    let (chunks, _) = mix_bytes.as_chunks_mut::<4>();
    for (word, chunk) in compressed.iter().zip(chunks) {
        *chunk = word.to_le_bytes();
    }

    let mut hasher = blake3::Hasher::new_keyed(mix_key());
    hasher.update(&seed_bytes);
    hasher.update(&mix_bytes);

    Proof {
        mix: mix_bytes,
        result: *hasher.finalize().as_bytes(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::crypto::dag::Params;

    const TEST: Params = Params::TESTING;

    fn fixtures(epoch: u64) -> (Cache, Dataset) {
        let cache = Cache::generate(epoch, TEST).expect("cache generation must succeed");
        let dataset = Dataset::generate(&cache).expect("dataset generation must succeed");
        (cache, dataset)
    }

    fn header(byte: u8) -> [u8; HASH_LEN] {
        [byte; HASH_LEN]
    }

    #[test]
    fn light_and_full_agree_on_every_nonce() {
        // The single most important property in this crate. A miner searches
        // with `hashimoto_full`; the network checks the winner with
        // `hashimoto_light`. Any disagreement means valid work is rejected and
        // the chain stalls.
        let (cache, dataset) = fixtures(0);
        let header = header(0xA5);

        for nonce in 0..24u64 {
            let light = hashimoto_light(&cache, &header, nonce);
            let full = hashimoto_full(&dataset, &header, nonce);
            assert_eq!(light, full, "light and full disagree at nonce {nonce}");
        }
    }

    #[test]
    fn evaluation_is_deterministic() {
        let (cache, dataset) = fixtures(0);
        let header = header(0x11);

        assert_eq!(
            hashimoto_light(&cache, &header, 7),
            hashimoto_light(&cache, &header, 7)
        );
        assert_eq!(
            hashimoto_full(&dataset, &header, 7),
            hashimoto_full(&dataset, &header, 7)
        );
    }

    #[test]
    fn a_different_nonce_gives_a_different_digest() {
        // Mining is a search over the nonce, so a digest that ignored it would
        // make the search unwinnable and the chain would halt.
        let (_, dataset) = fixtures(0);
        let header = header(0x22);

        let mut seen = std::collections::HashSet::new();
        for nonce in 0..64u64 {
            assert!(
                seen.insert(hashimoto_full(&dataset, &header, nonce).result),
                "nonce {nonce} repeated an earlier digest"
            );
        }
    }

    #[test]
    fn a_different_header_gives_a_different_digest() {
        let (_, dataset) = fixtures(0);
        let a = hashimoto_full(&dataset, &header(0x01), 0);
        let b = hashimoto_full(&dataset, &header(0x02), 0);

        assert_ne!(a.result, b.result);
        assert_ne!(a.mix, b.mix);
    }

    #[test]
    fn a_different_epoch_gives_a_different_digest() {
        // What "rotating the dataset" has to mean: the same header and nonce
        // must not stay solved across an epoch boundary.
        let (_, zero) = fixtures(0);
        let (_, one) = fixtures(1);
        let header = header(0x33);

        assert_ne!(
            hashimoto_full(&zero, &header, 5).result,
            hashimoto_full(&one, &header, 5).result
        );
    }

    #[test]
    fn the_mix_commits_to_the_pages_that_were_read() {
        // `mix` and `result` are distinct outputs; a bug that returned the
        // squeeze for both would make the parity test below vacuous.
        let (_, dataset) = fixtures(0);
        let proof = hashimoto_full(&dataset, &header(0x44), 1);
        assert_ne!(proof.mix, proof.result);
        assert_ne!(proof.mix, [0u8; HASH_LEN]);
    }

    #[test]
    fn the_lookups_are_spread_across_the_dataset() {
        // If the walk collapsed onto a few pages, a miner could cache those and
        // the 4 GiB requirement would evaporate. Recorded through the same
        // lookup path the real function uses.
        let cache = Cache::generate(0, TEST).expect("cache generation must succeed");
        let pages = cache.dataset_pages();

        let touched = std::cell::RefCell::new(std::collections::HashSet::new());
        for nonce in 0..16u64 {
            let _ = hashimoto(pages, &header(0x55), nonce, |page| {
                touched.borrow_mut().insert(page);
                let first = page * 2;
                [dataset_item(&cache, first), dataset_item(&cache, first + 1)]
            });
        }

        let touched = touched.into_inner();
        // 16 nonces × 64 accesses = 1024 lookups. Near-total distinctness is
        // what a uniform walk over tens of thousands of pages looks like; heavy
        // repetition is what a broken one looks like.
        assert!(
            touched.len() > 1000,
            "1024 lookups touched only {} distinct pages",
            touched.len()
        );
        assert!(touched.iter().all(|page| *page < pages));
    }

    #[test]
    fn the_mix_is_a_whole_page_wide() {
        // The reason a lookup is 128 bytes and not 64: a mix narrower than the
        // page would leave half of every memory transaction unused, and the
        // bandwidth binding would be half of what it claims.
        assert_eq!(MIX_WORDS, (crate::crypto::dag::ITEM_BYTES / 4) * 2);
        assert_eq!(COMPRESSED_WORDS * 4, MIX_WORDS);
    }
}
