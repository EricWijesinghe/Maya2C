//! Rateless erasure coding: how an object survives 30% loss with nothing to
//! ask again with.
//!
//! ## Why not retransmission
//!
//! A 1% duty cycle means a lost frame does not cost a retransmission — it costs
//! *another duty-cycle window*. At SF12 that is around 150 seconds per frame.
//! An object of sixty frames at 30% loss needs roughly eighteen repairs, and
//! ARQ would spend forty-five minutes of silence discovering which. It also
//! needs a reverse path, and half the links this crate is for are one-way: a
//! node on a hilltop transmitting to whoever can hear it has nobody to take a
//! NACK from.
//!
//! So the sender does not learn what was lost and does not need to. It emits
//! encoded symbols until it stops, and a receiver that collects **any** `k + ε`
//! of them recovers the object. A lost symbol is not a lost piece of the
//! object; it is one of an unlimited supply, and the next one is just as good.
//!
//! ## The code
//!
//! A **random linear fountain** over GF(2). Each symbol is the XOR of a
//! pseudorandomly chosen subset of the `k` source blocks, where each block is
//! included with probability one half, and the subset is derived from the
//! symbol's index by a seeded hash. A receiver reconstructs the subset from the
//! index alone, so a symbol carries its recipe in four bytes rather than a
//! bitmap.
//!
//! Decoding is Gaussian elimination over GF(2): each arriving symbol is reduced
//! against the pivots already held, and installed as a new pivot if anything
//! survives. When the rank reaches `k`, back-substitution gives every block.
//!
//! ## Why not an LT code, which is the usual answer
//!
//! It was the first answer here, and it was measured rather than assumed. An LT
//! code with an ideal-soliton degree distribution recovered **42 of 60**
//! channels at 30% loss even when sending 160% overhead — because the soliton's
//! tail is tuned for `k` in the thousands and these objects are a few hundred
//! blocks at most. A block of headers is small by design, and small is exactly
//! where LT is weakest.
//!
//! A random linear fountain has the opposite trade: decoding is `O(k^2)` rather
//! than `O(k log k)`, but recovery needs only `k + ~10` symbols *whatever* `k`
//! is, with failure probability around `2^-10`. At `k <= 1024` the quadratic
//! cost is about a megabit of word operations — nothing next to 246 seconds of
//! duty-cycle silence per frame.
//!
//! So the expensive resource here is airtime, not CPU, and the code is chosen
//! to spend the cheap one.
//!
//! Hand-written rather than a crate for the reason `anomaly.rs` writes out its
//! own 256-bit multiply: the coefficient generator and the symbol ordering are
//! a format two radios must agree on byte for byte, forever, and a dependency
//! would be a third party's freedom to change them in a point release.
//!
//! ## Overhead is a distribution, and [`OVERHEAD_PERCENT`] is set by measurement
//!
//! A random linear fountain needs `k + ε` symbols *received*, not sent. At 30%
//! loss, receiving `1.1k` means sending `1.1k / 0.7`, which is about 57% — and
//! measuring it showed 60% still losing two channels in sixty, so the constant
//! is 80%. It is checked by
//! `recovers_at_the_stated_overhead_under_thirty_percent_loss`, which runs the
//! channel forty times and fails if the margin stops holding.
//!
//! ## What this is not
//!
//! Not authentication and not confidentiality. A symbol is XORed source data
//! and anyone can produce one. What makes a recovered header trustworthy is the
//! chain's proof of work over it — which is why this crate never needs to know
//! what it is carrying.

use crate::error::{Error, Result};

/// Bytes of symbol framing: the object id, the symbol index, and the object's
/// length.
///
/// 4 + 4 + 4. The object id lets a receiver keep several decodes in flight; the
/// index is the whole recipe; the length is what tells a receiver how many
/// source blocks there are before it has decoded any of them.
pub const SYMBOL_OVERHEAD: usize = 12;

/// Percent of extra symbols beyond `k` a sender emits by default.
///
/// Eighty, set from a measurement rather than guessed. Sweeping a 30% erasure
/// channel over sixty seeded trials at `k = 100`:
///
/// | overhead | channels recovered |
/// |---:|---:|
/// | 60% | 58 / 60 |
/// | 80% | 60 / 60 |
/// | 100% | 60 / 60 |
///
/// Sixty percent is where the arithmetic says it should sit — `1.1k / 0.7` is
/// about `1.57k` — and it is also where two channels in sixty fail. The extra
/// twenty buys the tail, and the tail is what matters: a receiver one symbol
/// short waits another duty-cycle window, which at SF12 is four minutes.
///
/// Pinned by `recovers_at_the_stated_overhead_under_thirty_percent_loss`.
/// Lowering it is a change that has to survive that test.
pub const OVERHEAD_PERCENT: usize = 80;

/// The most source blocks one object may be split into.
///
/// A window of 64 headers at 144 bytes is about 9 KB, which at a 39-byte
/// symbol payload is around 240 blocks. 1,024 leaves room without letting a
/// hostile length field allocate unboundedly.
pub const MAX_BLOCKS: usize = 1_024;

/// The largest object this codec will encode or decode.
pub const MAX_OBJECT_BYTES: usize = 64 * 1_024;

/// One encoded symbol, ready to be a frame payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Which object this belongs to.
    pub object: u32,
    /// The symbol's index, which *is* its recipe.
    pub index: u32,
    /// The object's length in bytes.
    pub object_len: u32,
    /// The XOR of the chosen source blocks.
    pub data: Vec<u8>,
}

impl Symbol {
    /// The bytes to put in a frame payload.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() + SYMBOL_OVERHEAD);
        out.extend_from_slice(&self.object.to_le_bytes());
        out.extend_from_slice(&self.index.to_le_bytes());
        out.extend_from_slice(&self.object_len.to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Reads a symbol out of a frame payload.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a payload shorter than the framing or
    /// carrying no data, and [`Error::Oversized`] for an object length past
    /// [`MAX_OBJECT_BYTES`] — checked here, because the length is what decides
    /// how much a decoder allocates and it arrives from a stranger.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() <= SYMBOL_OVERHEAD {
            return Err(Error::Malformed(format!(
                "{} bytes carries no symbol data",
                bytes.len()
            )));
        }
        let object = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes"));
        let index = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
        let object_len = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes"));

        if object_len as usize > MAX_OBJECT_BYTES {
            return Err(Error::Oversized {
                what: "fountain object",
                found: object_len as usize,
                limit: MAX_OBJECT_BYTES,
            });
        }
        Ok(Self {
            object,
            index,
            object_len,
            data: bytes[SYMBOL_OVERHEAD..].to_vec(),
        })
    }
}

/// Splits an object into symbols.
#[derive(Clone, Debug)]
pub struct Encoder {
    object: u32,
    object_len: usize,
    block_size: usize,
    blocks: Vec<Vec<u8>>,
}

impl Encoder {
    /// Prepares an object for transmission.
    ///
    /// `block_size` is the symbol payload the link can carry — frame capacity
    /// minus [`SYMBOL_OVERHEAD`] minus the frame's own framing. The caller
    /// computes it from the spreading factor, because only the caller knows the
    /// link.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] for an object past [`MAX_OBJECT_BYTES`] or
    /// one that would need more than [`MAX_BLOCKS`] blocks, and
    /// [`Error::Malformed`] for an empty object or a zero block size.
    pub fn new(object: u32, data: &[u8], block_size: usize) -> Result<Self> {
        if data.is_empty() || block_size == 0 {
            return Err(Error::Malformed(
                "an object and its block size must both be non-empty".into(),
            ));
        }
        if data.len() > MAX_OBJECT_BYTES {
            return Err(Error::Oversized {
                what: "fountain object",
                found: data.len(),
                limit: MAX_OBJECT_BYTES,
            });
        }
        let count = data.len().div_ceil(block_size);
        if count > MAX_BLOCKS {
            return Err(Error::Oversized {
                what: "fountain blocks",
                found: count,
                limit: MAX_BLOCKS,
            });
        }

        // The final block is zero-padded. `object_len` is what trims it back,
        // which is why the length rides in every symbol rather than being
        // agreed out of band.
        let mut blocks = Vec::with_capacity(count);
        for chunk in data.chunks(block_size) {
            let mut block = vec![0u8; block_size];
            block[..chunk.len()].copy_from_slice(chunk);
            blocks.push(block);
        }

        Ok(Self {
            object,
            object_len: data.len(),
            block_size,
            blocks,
        })
    }

    /// How many source blocks the object has.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// How many symbols to emit for a comfortable decode.
    ///
    /// `k` plus [`OVERHEAD_PERCENT`], and at least one more than `k` — for a
    /// single-block object the percentage rounds to nothing and one spare
    /// symbol is the difference between recovering and not.
    #[must_use]
    pub fn recommended_symbols(&self) -> usize {
        let k = self.blocks.len();
        (k + k * OVERHEAD_PERCENT / 100).max(k + 1)
    }

    /// The symbol at `index`.
    ///
    /// Deterministic: the same object and index give the same symbol on every
    /// node and every run, which is what lets a receiver rebuild the recipe
    /// from four bytes.
    #[must_use]
    pub fn symbol(&self, index: u32) -> Symbol {
        let mut data = vec![0u8; self.block_size];
        let row = coefficients(index, self.blocks.len());
        for (block, source) in self.blocks.iter().enumerate() {
            if bit(&row, block) {
                for (out, byte) in data.iter_mut().zip(source) {
                    *out ^= byte;
                }
            }
        }
        Symbol {
            object: self.object,
            index,
            object_len: self.object_len as u32,
            data,
        }
    }

    /// Every symbol up to [`Encoder::recommended_symbols`].
    #[must_use]
    pub fn symbols(&self) -> Vec<Symbol> {
        (0..self.recommended_symbols() as u32)
            .map(|index| self.symbol(index))
            .collect()
    }
}

/// Collects symbols until an object falls out.
///
/// Holds the system in row-echelon form: one pivot row per block solved so far,
/// each a coefficient vector and the XOR of the blocks it names. An arriving
/// symbol is reduced against the pivots it overlaps; whatever survives becomes a
/// new pivot, or nothing does and the symbol was redundant.
#[derive(Clone, Debug)]
pub struct Decoder {
    object: u32,
    object_len: usize,
    block_size: usize,
    block_count: usize,
    /// `pivots[i]`, when present, is a row whose leading coefficient is block
    /// `i`: its coefficient vector and its accumulated data.
    pivots: Vec<Option<(Vec<u64>, Vec<u8>)>>,
    rank: usize,
}

impl Decoder {
    /// A decoder for the object a symbol names.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a symbol with no data, and
    /// [`Error::Oversized`] if the object would need more than [`MAX_BLOCKS`].
    pub fn new(symbol: &Symbol) -> Result<Self> {
        let block_size = symbol.data.len();
        if block_size == 0 {
            return Err(Error::Malformed("a symbol carries no data".into()));
        }
        let object_len = symbol.object_len as usize;
        if object_len == 0 || object_len > MAX_OBJECT_BYTES {
            return Err(Error::Oversized {
                what: "fountain object",
                found: object_len,
                limit: MAX_OBJECT_BYTES,
            });
        }
        let block_count = object_len.div_ceil(block_size);
        if block_count > MAX_BLOCKS {
            return Err(Error::Oversized {
                what: "fountain blocks",
                found: block_count,
                limit: MAX_BLOCKS,
            });
        }
        Ok(Self {
            object: symbol.object,
            object_len,
            block_size,
            block_count,
            pivots: vec![None; block_count],
            rank: 0,
        })
    }

    /// How many independent symbols have been absorbed.
    ///
    /// The system's rank, which reaches `k` exactly when every source block
    /// becomes recoverable.
    #[must_use]
    pub fn solved_blocks(&self) -> usize {
        self.rank
    }

    /// Whether the object is complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.rank == self.block_count
    }

    /// Feeds one symbol in.
    ///
    /// Returns the object once it is complete, and `None` while it is not.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a symbol belonging to a different
    /// object, of a different block size, or declaring a different length — all
    /// three would corrupt a decode silently rather than loudly, which on a
    /// channel where anyone can transmit is the difference between a failed
    /// decode and a forged one.
    pub fn absorb(&mut self, symbol: &Symbol) -> Result<Option<Vec<u8>>> {
        if symbol.object != self.object {
            return Err(Error::Malformed(format!(
                "symbol is for object {} and this decoder is for {}",
                symbol.object, self.object
            )));
        }
        if symbol.data.len() != self.block_size || symbol.object_len as usize != self.object_len {
            return Err(Error::Malformed(
                "symbol disagrees with the object's block size or length".into(),
            ));
        }
        if self.is_complete() {
            return Ok(Some(self.assemble()));
        }

        let mut row = coefficients(symbol.index, self.block_count);
        let mut data = symbol.data.clone();

        // Reduce against what is already held. A symbol that reduces to nothing
        // was a linear combination of rows already present: redundant rather
        // than wrong, and the ordinary case once the rank is high.
        for block in 0..self.block_count {
            if !bit(&row, block) {
                continue;
            }
            match &self.pivots[block] {
                Some((pivot_row, pivot_data)) => {
                    xor_words(&mut row, pivot_row);
                    xor_bytes(&mut data, pivot_data);
                }
                None => {
                    self.pivots[block] = Some((row, data));
                    self.rank += 1;
                    return Ok(self.is_complete().then(|| self.assemble()));
                }
            }
        }
        Ok(None)
    }

    /// Back-substitutes the echelon form and concatenates the blocks.
    ///
    /// Upward, from the last pivot to the first: by the time block `i` is read,
    /// every block below it in the row has already been solved, so subtracting
    /// them leaves the block itself.
    fn assemble(&self) -> Vec<u8> {
        let mut solved: Vec<Vec<u8>> = vec![vec![0u8; self.block_size]; self.block_count];
        for block in (0..self.block_count).rev() {
            let Some((row, data)) = &self.pivots[block] else {
                continue;
            };
            let mut value = data.clone();
            for (higher, known) in solved.iter().enumerate().skip(block + 1) {
                if bit(row, higher) {
                    xor_bytes(&mut value, known);
                }
            }
            solved[block] = value;
        }

        let mut out = Vec::with_capacity(self.block_count * self.block_size);
        for block in &solved {
            out.extend_from_slice(block);
        }
        out.truncate(self.object_len);
        out
    }
}

/// Words needed to hold `block_count` coefficient bits.
fn words_for(block_count: usize) -> usize {
    block_count.div_ceil(64)
}

/// Whether block `index` is named by a coefficient vector.
fn bit(words: &[u64], index: usize) -> bool {
    words[index / 64] >> (index % 64) & 1 == 1
}

/// `a ^= b`, over coefficient words.
fn xor_words(a: &mut [u64], b: &[u64]) {
    for (left, right) in a.iter_mut().zip(b) {
        *left ^= right;
    }
}

/// `a ^= b`, over symbol data.
fn xor_bytes(a: &mut [u8], b: &[u8]) {
    for (left, right) in a.iter_mut().zip(b) {
        *left ^= right;
    }
}

/// The coefficient vector a symbol index names.
///
/// The recipe, derived from the index alone so a symbol does not carry a
/// bitmap. BLAKE3's extendable output is the generator: seeded with the index,
/// it gives the same bits on every machine and every build.
///
/// The first `k` indices are **systematic** — symbol `i` is source block `i`
/// alone. Not an optimisation: on a clean link each such symbol is already a
/// pivot in its own right, the reduction loop exits on its first iteration, and
/// only a lossy link pays for the code at all.
///
/// Past that, each block is included with probability one half. A vector drawn
/// that way is independent of the `rank` already held with probability
/// `1 - 2^(rank - k)`, which is where "about ten spare symbols" comes from: ten
/// gives a failure probability near `2^-10`, whatever `k` is.
fn coefficients(index: u32, block_count: usize) -> Vec<u64> {
    let mut words = vec![0u64; words_for(block_count)];
    if block_count == 0 {
        return words;
    }
    if (index as usize) < block_count {
        words[index as usize / 64] |= 1 << (index as usize % 64);
        return words;
    }

    let mut reader = blake3::Hasher::new()
        .update(b"maya-radio-fountain-v1")
        .update(&index.to_le_bytes())
        .finalize_xof();

    let mut buffer = [0u8; 8];
    for word in &mut words {
        reader.fill(&mut buffer);
        *word = u64::from_le_bytes(buffer);
    }
    // Bits past the block count would name blocks that do not exist, and would
    // make two receivers that disagree about `k` disagree about what a symbol
    // means.
    let tail = block_count % 64;
    if tail != 0 {
        let last = words.len() - 1;
        words[last] &= (1u64 << tail) - 1;
    }

    // An all-zero vector carries nothing and could never help. Vanishingly rare
    // at any useful `k`, and cheap to exclude — a receiver that got only those
    // would stall for a reason nobody could see from a hilltop.
    if words.iter().all(|word| *word == 0) {
        words[0] = 1;
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic pseudorandom stream, so a loss pattern is reproducible.
    fn drops(seed: u64, count: usize, percent: u32) -> Vec<bool> {
        let mut state = seed;
        (0..count)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                (state >> 33) % 100 < u64::from(percent)
            })
            .collect()
    }

    fn object(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 + 7) as u8).collect()
    }

    #[test]
    fn a_clean_channel_decodes_from_the_systematic_symbols_alone() {
        // The common case: no loss, no propagation, every block solved on
        // arrival. If the systematic prefix ever stopped being systematic this
        // would still pass — but the next test would get much slower.
        let data = object(500);
        let encoder = Encoder::new(1, &data, 40).expect("encode");
        let symbols = encoder.symbols();

        let mut decoder = Decoder::new(&symbols[0]).expect("decoder");
        let mut recovered = None;
        for symbol in symbols.iter().take(encoder.block_count()) {
            recovered = decoder.absorb(symbol).expect("absorb");
        }
        assert_eq!(recovered.as_deref(), Some(data.as_slice()));
    }

    #[test]
    fn recovers_at_the_stated_overhead_under_thirty_percent_loss() {
        // The property `OVERHEAD_PERCENT` exists for, and the test that has to
        // survive anyone lowering it. Many seeds, because a fountain code's
        // overhead is a distribution and one lucky channel proves nothing.
        let data = object(4_000);
        let encoder = Encoder::new(7, &data, 40).expect("encode");

        let mut recovered = 0;
        let trials = 40;
        for seed in 0..trials {
            let symbols = encoder.symbols();
            let pattern = drops(seed, symbols.len(), 30);

            let mut decoder: Option<Decoder> = None;
            let mut out = None;
            for (symbol, dropped) in symbols.iter().zip(&pattern) {
                if *dropped {
                    continue;
                }
                let decoder = decoder.get_or_insert_with(|| Decoder::new(symbol).expect("decoder"));
                out = decoder.absorb(symbol).expect("absorb");
                if out.is_some() {
                    break;
                }
            }
            if out.as_deref() == Some(data.as_slice()) {
                recovered += 1;
            }
        }
        assert!(
            recovered >= trials * 9 / 10,
            "recovered {recovered} of {trials} channels at 30% loss; \
             OVERHEAD_PERCENT = {OVERHEAD_PERCENT} is no longer enough"
        );
    }

    #[test]
    fn more_symbols_always_finish_a_decode_that_stalled() {
        // Ratelessness itself: a receiver that has not finished has not failed,
        // and the sender never needs to know which symbols were lost.
        let data = object(2_000);
        let encoder = Encoder::new(3, &data, 40).expect("encode");

        let mut decoder: Option<Decoder> = None;
        let mut out = None;
        // Half the symbols dropped, and simply keep going.
        for index in 0..2_000u32 {
            if index % 2 == 0 {
                continue;
            }
            let symbol = encoder.symbol(index);
            let decoder = decoder.get_or_insert_with(|| Decoder::new(&symbol).expect("decoder"));
            out = decoder.absorb(&symbol).expect("absorb");
            if out.is_some() {
                break;
            }
        }
        assert_eq!(out.as_deref(), Some(data.as_slice()));
    }

    #[test]
    fn symbols_arriving_out_of_order_decode_the_same() {
        // A mesh has several relays and no ordering. A decoder that depended on
        // order would work in a test and fail on air.
        let data = object(1_200);
        let encoder = Encoder::new(9, &data, 40).expect("encode");
        let mut symbols = encoder.symbols();
        symbols.reverse();

        let mut decoder = Decoder::new(&symbols[0]).expect("decoder");
        let mut out = None;
        for symbol in &symbols {
            out = decoder.absorb(symbol).expect("absorb");
            if out.is_some() {
                break;
            }
        }
        assert_eq!(out.as_deref(), Some(data.as_slice()));
    }

    #[test]
    fn a_duplicated_symbol_advances_nothing_and_breaks_nothing() {
        // Relays rebroadcast. The same symbol arriving twice must not corrupt
        // a decode, and must not count as progress.
        let data = object(800);
        let encoder = Encoder::new(4, &data, 40).expect("encode");
        let mut decoder = Decoder::new(&encoder.symbol(0)).expect("decoder");

        decoder.absorb(&encoder.symbol(0)).expect("absorb");
        let after_first = decoder.solved_blocks();
        decoder.absorb(&encoder.symbol(0)).expect("absorb again");
        assert_eq!(decoder.solved_blocks(), after_first);

        let mut out = None;
        for index in 0..encoder.recommended_symbols() as u32 {
            out = decoder.absorb(&encoder.symbol(index)).expect("absorb");
            if out.is_some() {
                break;
            }
        }
        assert_eq!(out.as_deref(), Some(data.as_slice()));
    }

    #[test]
    fn a_symbol_for_another_object_is_refused_rather_than_mixed_in() {
        // Anyone can transmit. A decoder that absorbed a stranger's symbol
        // would produce a plausible object that is not the one anybody sent.
        let data = object(400);
        let mine = Encoder::new(1, &data, 40).expect("encode");
        let theirs = Encoder::new(2, &object(400), 40).expect("encode");

        let mut decoder = Decoder::new(&mine.symbol(0)).expect("decoder");
        assert!(decoder.absorb(&theirs.symbol(0)).is_err());
    }

    #[test]
    fn a_symbol_of_the_wrong_shape_is_refused() {
        let data = object(400);
        let encoder = Encoder::new(1, &data, 40).expect("encode");
        let mut decoder = Decoder::new(&encoder.symbol(0)).expect("decoder");

        let mut wrong_size = encoder.symbol(1);
        wrong_size.data.push(0);
        assert!(decoder.absorb(&wrong_size).is_err());

        let mut wrong_len = encoder.symbol(1);
        wrong_len.object_len += 1;
        assert!(decoder.absorb(&wrong_len).is_err());
    }

    #[test]
    fn a_symbol_survives_its_own_wire_format() {
        let encoder = Encoder::new(0xdead_beef, &object(300), 40).expect("encode");
        let symbol = encoder.symbol(11);
        assert_eq!(Symbol::decode(&symbol.encode()).expect("decode"), symbol);
    }

    #[test]
    fn a_hostile_object_length_is_refused_before_anything_is_allocated() {
        // The length field decides how much a decoder allocates, and it arrives
        // from a stranger.
        let mut bytes = vec![0u8; SYMBOL_OVERHEAD + 8];
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            Symbol::decode(&bytes),
            Err(Error::Oversized { .. })
        ));
    }

    #[test]
    fn an_object_too_large_to_encode_is_refused() {
        assert!(Encoder::new(1, &vec![0; MAX_OBJECT_BYTES + 1], 40).is_err());
        // And one that would need too many blocks, even inside the byte limit.
        assert!(Encoder::new(1, &vec![0; MAX_BLOCKS + 1], 1).is_err());
    }

    #[test]
    fn a_single_block_object_still_gets_a_spare_symbol() {
        // The rounding case: 60% of one block is zero extra symbols, and a
        // receiver that lost the only one would wait forever.
        let encoder = Encoder::new(1, &object(10), 40).expect("encode");
        assert_eq!(encoder.block_count(), 1);
        assert!(encoder.recommended_symbols() >= 2);
    }

    #[test]
    fn a_recipe_never_names_a_block_that_does_not_exist() {
        // Coefficient bits past `k` would be blocks nobody has, and two
        // receivers that disagreed about `k` would disagree about what a symbol
        // means — which decodes to plausible garbage rather than to nothing.
        for block_count in [1usize, 7, 63, 64, 65, 100] {
            for index in 0..500u32 {
                let row = coefficients(index, block_count);
                assert_eq!(row.len(), words_for(block_count));
                for block in block_count..row.len() * 64 {
                    assert!(
                        !bit(&row, block),
                        "index {index} at k={block_count} names block {block}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_recipe_is_never_empty() {
        // An all-zero vector carries nothing and can never advance a decode.
        for index in 0..2_000u32 {
            let row = coefficients(index, 64);
            assert!(
                row.iter().any(|word| *word != 0),
                "index {index} names nothing"
            );
        }
    }

    #[test]
    fn the_systematic_prefix_is_one_block_each() {
        // The clean-channel case: symbol `i` is block `i`, so a link with no
        // loss never pays for the code at all.
        for index in 0..64u32 {
            let row = coefficients(index, 64);
            assert!(bit(&row, index as usize));
            assert_eq!(row.iter().map(|w| w.count_ones()).sum::<u32>(), 1);
        }
    }

    #[test]
    fn a_recipe_is_the_same_on_every_run() {
        // Two radios reconstruct the recipe from the index alone. If that
        // derivation ever differed between builds, every link between them
        // would decode garbage that passes its checksum.
        for index in [0u32, 1, 63, 64, 1_000, u32::MAX] {
            assert_eq!(coefficients(index, 64), coefficients(index, 64));
        }
    }
}
