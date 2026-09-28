//! The MPC dark pool's aggregation made **malicious-secure against the
//! servers**: information-theoretic MACs in the style of SPDZ (Damgård,
//! Pastro, Smart, Zakarias, 2012).
//!
//! [`crate::mpc_darkpool`] is semi-honest: a server that adds anything to its
//! share shifts the clearing curves and nobody can tell. Here every shared
//! value `v` also carries shares of `α·v`, where the MAC key `α` is itself
//! shared so that no server knows it. Opening an aggregate `A` is followed by
//! a check that the servers' MAC shares sum to `α·A`, run as commit-then-
//! reveal so no server can choose its answer after seeing the others'. A
//! server that tampers is caught except with probability `1/p`,
//! `p = 2^61 - 1`.
//!
//! **Inputs** go through masks from an offline phase: the dealer gives each
//! trader a random vector `r` and the servers authenticated shares of it; the
//! trader publishes `ε = v - r`, which reveals nothing, and each server
//! derives its share of `v`.
//!
//! **Trusted, stated:**
//! - The dealer that prepares `α` and the masks. SPDZ's offline phase
//!   replaces it with homomorphic encryption or oblivious transfer; that is
//!   not built.
//! - The traders, for well-formedness. A MAC proves the servers did not
//!   change what a trader input; it does not prove a trader's curve is a
//!   legal order, which needs a per-order proof (not built).
//!
//! Output is detection and abort, not correction: a caught batch does not
//! clear.

use rand_core::RngCore;

use crate::mpc_darkpool::{Clearing, MAX_ORDERS, TICKS};

/// The field modulus, the Mersenne prime `2^61 - 1`. Every aggregate the dark
/// pool forms stays below `2^60` (its size and count caps), so none wraps.
pub const P: u64 = (1 << 61) - 1;

/// Why a step was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SpdzError {
    /// A server's revealed MAC share does not match its commitment.
    #[error("server {0} revealed something other than it committed to")]
    BadOpening(usize),
    /// The MAC check failed: some server changed its shares.
    #[error("MAC check failed: the opened value was tampered with")]
    MacCheck,
    /// Mismatched shapes.
    #[error("{0}")]
    Shape(&'static str),
    /// More than [`MAX_ORDERS`] inputs: aggregates could leave the range in
    /// which they are exact.
    #[error("the batch is full")]
    BatchFull,
    /// Fewer than two servers, or not every server answered. A missing
    /// server is an abort, never a smaller check.
    #[error("{got} of {expected} servers answered (at least two are needed)")]
    Servers {
        /// How many answered.
        got: usize,
        /// How many there are.
        expected: usize,
    },
    /// A step out of order: a second commitment on one sum (which would
    /// reveal the server's key share), or a reveal before every commitment
    /// is in.
    #[error("{0}")]
    Order(&'static str),
}

fn add(a: u64, b: u64) -> u64 {
    (a + b) % P
}

fn sub(a: u64, b: u64) -> u64 {
    (a + P - b) % P
}

fn mul(a: u64, b: u64) -> u64 {
    let product = u128::from(a) * u128::from(b) % u128::from(P);
    // Reduced modulo P < 2^61, so the value fits in 64 bits exactly.
    #[allow(clippy::cast_possible_truncation)]
    let reduced = product as u64;
    reduced
}

fn random(rng: &mut impl RngCore) -> u64 {
    // 61 random bits, rejecting only P itself: uniform on [0, P).
    loop {
        let v = rng.next_u64() >> 3;
        if v < P {
            return v;
        }
    }
}

/// Additive shares of `v` among `n` parties (`n` at least two, checked by
/// every caller).
fn share(v: u64, n: usize, rng: &mut impl RngCore) -> Vec<u64> {
    let mut out: Vec<u64> = (1..n).map(|_| random(rng)).collect();
    let rest = out.iter().fold(v % P, |acc, s| sub(acc, *s));
    out.push(rest);
    out
}

fn enough(got: usize, expected: usize) -> Result<(), SpdzError> {
    if expected < 2 || got != expected {
        return Err(SpdzError::Servers { got, expected });
    }
    Ok(())
}

/// One server's authenticated share of a vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthShare {
    /// Shares of the values.
    pub value: Vec<u64>,
    /// Shares of `α ·` the values.
    pub mac: Vec<u64>,
}

impl AuthShare {
    fn zero(len: usize) -> Self {
        Self {
            value: vec![0; len],
            mac: vec![0; len],
        }
    }
}

/// What the dealer gives a trader for one input: the mask itself.
#[derive(Clone, Debug)]
pub struct Mask {
    /// The random vector the trader subtracts from its input.
    pub r: Vec<u64>,
}

/// The offline phase: the MAC key, shared, and masks on request.
pub struct Dealer {
    alpha: u64,
    servers: usize,
}

impl Dealer {
    /// A dealer for `servers` servers. Returns it and each server's share of
    /// the MAC key.
    ///
    /// # Errors
    ///
    /// Fewer than two servers: one "share" would be the key itself.
    pub fn new(servers: usize, rng: &mut impl RngCore) -> Result<(Self, Vec<u64>), SpdzError> {
        enough(servers, servers)?;
        let alpha = random(rng);
        Ok((Self { alpha, servers }, share(alpha, servers, rng)))
    }

    /// A fresh mask of `len` values: the trader's copy, and each server's
    /// authenticated share of it.
    pub fn mask(&self, len: usize, rng: &mut impl RngCore) -> (Mask, Vec<AuthShare>) {
        let r: Vec<u64> = (0..len).map(|_| random(rng)).collect();
        let mut shares = vec![AuthShare::zero(len); self.servers];
        for (t, value) in r.iter().enumerate() {
            let (vs, ms) = (
                share(*value, self.servers, rng),
                share(mul(self.alpha, *value), self.servers, rng),
            );
            for (s, server) in shares.iter_mut().enumerate() {
                server.value[t] = vs[s];
                server.mac[t] = ms[s];
            }
        }
        (Mask { r }, shares)
    }
}

/// The trader's public message: `ε = v - r`. Uniform, so it reveals nothing.
#[must_use]
pub fn masked_input(v: &[u64], mask: &Mask) -> Vec<u64> {
    v.iter()
        .zip(&mask.r)
        .map(|(v, r)| sub(*v % P, *r))
        .collect()
}

/// A server's residue, committed and waiting to be revealed.
#[derive(Clone, Debug)]
struct Committed {
    sigma: Vec<u64>,
    salt: [u8; 32],
    commitment: [u8; 32],
}

/// One server's state: its MAC-key share, its running authenticated sum, and
/// where it is in the opening.
#[derive(Clone, Debug)]
pub struct SpdzServer {
    /// Its index; server 0 adds the public `ε` to its value share.
    pub index: usize,
    alpha: u64,
    sum: AuthShare,
    inputs: u64,
    committed: Option<Committed>,
}

impl SpdzServer {
    /// A server holding `alpha_share`, summing vectors of `len`.
    #[must_use]
    pub fn new(index: usize, alpha_share: u64, len: usize) -> Self {
        Self {
            index,
            alpha: alpha_share,
            sum: AuthShare::zero(len),
            inputs: 0,
            committed: None,
        }
    }

    /// Adds one input: its mask share and the trader's public `ε`.
    ///
    /// # Errors
    ///
    /// Vectors of the wrong length, a full batch, or a batch already being
    /// opened.
    pub fn receive(&mut self, mask: &AuthShare, epsilon: &[u64]) -> Result<(), SpdzError> {
        if self.committed.is_some() {
            return Err(SpdzError::Order("the batch is being opened"));
        }
        let len = self.sum.value.len();
        if mask.value.len() != len || mask.mac.len() != len || epsilon.len() != len {
            return Err(SpdzError::Shape("input of the wrong length"));
        }
        if self.inputs >= MAX_ORDERS {
            return Err(SpdzError::BatchFull);
        }
        self.inputs += 1;
        let own = |t: usize| if self.index == 0 { epsilon[t] } else { 0 };
        let (value, mac): (Vec<u64>, Vec<u64>) = (0..len)
            .map(|t| {
                (
                    add(self.sum.value[t], add(mask.value[t], own(t))),
                    add(
                        self.sum.mac[t],
                        add(mask.mac[t], mul(self.alpha, epsilon[t])),
                    ),
                )
            })
            .unzip();
        self.sum = AuthShare { value, mac };
        Ok(())
    }

    /// Step one of opening: the value share this server publishes.
    #[must_use]
    pub fn value_share(&self) -> &[u64] {
        &self.sum.value
    }

    /// Step two: commits to this server's MAC residue `σ = mac − α·A` for
    /// the opened vector `A`. Once per batch: two residues on one sum for two
    /// different `A` would reveal this server's key share.
    ///
    /// # Errors
    ///
    /// [`SpdzError::Order`] on a second call; [`SpdzError::Shape`] for an
    /// opened vector of the wrong length.
    pub fn commit_residue(
        &mut self,
        opened: &[u64],
        rng: &mut impl RngCore,
    ) -> Result<[u8; 32], SpdzError> {
        if self.committed.is_some() {
            return Err(SpdzError::Order(
                "a residue was already committed on this sum",
            ));
        }
        if opened.len() != self.sum.mac.len() {
            return Err(SpdzError::Shape("opened vector of the wrong length"));
        }
        let sigma: Vec<u64> = opened
            .iter()
            .zip(&self.sum.mac)
            .map(|(a, m)| sub(*m, mul(self.alpha, *a)))
            .collect();
        let mut salt = [0u8; 32];
        rng.fill_bytes(&mut salt);
        let commitment = commit(&sigma, &salt);
        self.committed = Some(Committed {
            sigma,
            salt,
            commitment,
        });
        Ok(commitment)
    }

    /// Step three: reveals the residue, but only once this server holds
    /// every server's commitment, its own at its own index — so it cannot be
    /// asked to reveal before the others are bound.
    ///
    /// # Errors
    ///
    /// [`SpdzError::Order`] before committing or without its own commitment
    /// in place; [`SpdzError::Servers`] without every server's commitment.
    pub fn reveal(
        &self,
        commitments: &[[u8; 32]],
        servers: usize,
    ) -> Result<(Vec<u64>, [u8; 32]), SpdzError> {
        enough(commitments.len(), servers)?;
        let own = self
            .committed
            .as_ref()
            .ok_or(SpdzError::Order("reveal before commit"))?;
        if commitments.get(self.index) != Some(&own.commitment) {
            return Err(SpdzError::Order(
                "this server's commitment is not the one collected",
            ));
        }
        Ok((own.sigma.clone(), own.salt))
    }

    /// Simulates a malicious server changing its sum, so tests can show the
    /// MAC check catching it. An honest deployment never calls this.
    pub fn tamper(&mut self, position: usize, value_delta: u64, mac_delta: u64) {
        if let (Some(v), Some(m)) = (
            self.sum.value.get_mut(position),
            self.sum.mac.get_mut(position),
        ) {
            *v = add(*v, value_delta % P);
            *m = add(*m, mac_delta % P);
        }
    }
}

/// The commitment to a residue: BLAKE3 over the salt and the values.
#[must_use]
pub fn commit(sigma: &[u64], salt: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("maya2c spdz mac residue v1");
    h.update(salt);
    for s in sigma {
        h.update(&s.to_le_bytes());
    }
    *h.finalize().as_bytes()
}

/// Step one: the sum of every server's published value share.
///
/// # Errors
///
/// Not every server's share, or shares of different lengths.
pub fn open(value_shares: &[&[u64]], servers: usize) -> Result<Vec<u64>, SpdzError> {
    enough(value_shares.len(), servers)?;
    let len = value_shares[0].len();
    if value_shares.iter().any(|s| s.len() != len) {
        return Err(SpdzError::Shape("value shares of different lengths"));
    }
    Ok((0..len)
        .map(|t| value_shares.iter().fold(0, |acc, s| add(acc, s[t])))
        .collect())
}

/// Step four: every server's reveal matches its commitment, and the
/// residues sum to zero at every position of the opened vector.
///
/// # Errors
///
/// Not every server, a reveal of the wrong length, a reveal that does not
/// match its commitment ([`SpdzError::BadOpening`]), or a failed MAC check.
pub fn check(
    opened_len: usize,
    commitments: &[[u8; 32]],
    reveals: &[(Vec<u64>, [u8; 32])],
    servers: usize,
) -> Result<(), SpdzError> {
    enough(commitments.len(), servers)?;
    enough(reveals.len(), servers)?;
    for (i, ((sigma, salt), commitment)) in reveals.iter().zip(commitments).enumerate() {
        if sigma.len() != opened_len {
            return Err(SpdzError::Shape("a residue of the wrong length"));
        }
        if commit(sigma, salt) != *commitment {
            return Err(SpdzError::BadOpening(i));
        }
    }
    let zero = (0..opened_len).all(|t| reveals.iter().fold(0, |acc, r| add(acc, r.0[t])) == 0);
    if zero {
        Ok(())
    } else {
        Err(SpdzError::MacCheck)
    }
}

/// An order's two curves as one vector: demand ticks, then supply ticks.
#[must_use]
pub fn order_vector(side: crate::darkpool::Side, price: usize, size: u64) -> Vec<u64> {
    let mut v = vec![0u64; 2 * TICKS];
    match side {
        crate::darkpool::Side::Buy => v[..=price.min(TICKS - 1)].fill(size),
        crate::darkpool::Side::Sell => v[TICKS + price.min(TICKS - 1)..].fill(size),
    }
    v
}

/// The uniform clearing price from opened curves (demand ticks, then supply
/// ticks), by the rule [`crate::darkpool::clear`] uses: most volume, lowest
/// price on a tie. Only call it on a vector that passed [`check`].
///
/// # Errors
///
/// [`SpdzError::Shape`] for a vector that is not two curves.
pub fn clear_opened(opened: &[u64]) -> Result<Clearing, SpdzError> {
    if opened.len() != 2 * TICKS {
        return Err(SpdzError::Shape("not a demand and a supply curve"));
    }
    let (demand, supply) = opened.split_at(TICKS);
    let best = (0..TICKS)
        .map(|p| (demand[p].min(supply[p]), std::cmp::Reverse(p)))
        .max()
        .filter(|(v, _)| *v > 0);
    Ok(match best {
        Some((volume, std::cmp::Reverse(p))) => Clearing {
            price: u64::try_from(p).ok(),
            volume,
            demand: demand[p],
            supply: supply[p],
        },
        None => Clearing {
            price: None,
            volume: 0,
            demand: 0,
            supply: 0,
        },
    })
}
