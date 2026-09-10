//! The circuit: [`QuantizedMlp::evaluate`], as polynomial constraints.
//!
//! # One gate, one lookup, and copy constraints
//!
//! Every constraint in this file is an instance of a single arithmetic gate,
//!
//! ```text
//! arith · (fa·a + fb·b + fm·a·b + fc − c) = 0
//! ```
//!
//! plus one lookup that puts a value in `[0, 512)`. The coefficients are fixed
//! columns, so the model's weights live in the verifying key — the key *is* the
//! model, and [`crate::verify::model_id`] is a hash of it. There is no second
//! gate to audit and no custom gadget whose soundness has to be argued
//! separately. That is the property worth more than the rows it costs.
//!
//! # The four things the circuit must refuse
//!
//! | A prover who lies about… | Is stopped by |
//! |---|---|
//! | an input outside int8 | `x + 128` and `127 − x` both range-checked |
//! | the sign of an accumulator (ReLU) | `t = 2·bit·acc − acc + bit − 1` range-checked: it is `acc` when the bit says non-negative and `−acc − 1` when it says negative, and only the true sign makes it small |
//! | the requantization | `r = q·2^s + rem` with `q`, `rem` and `2^s − 1 − rem` range-checked: floor division has exactly one solution |
//! | the saturation | `c·(q − 127) + (1 − c)·(126 − q)` range-checked: only the true comparison makes it small |
//! | the class | a one-hot vector, and `m − logit_j − [j < class] ≥ 0` for every `j`, which is "the first maximum" and nothing else |
//!
//! Each row of that table has a negative test in `tests/circuit_tests.rs` that
//! hands the circuit a witness which lies in exactly that way — and lies
//! *consistently*, with everything downstream recomputed, so the only
//! constraint that can refuse it is the one the test is named after.
//!
//! That is checked, not assumed. Deleting each guard in turn (the ReLU sign
//! check and booleanity, the recomposition, the remainder's two range checks,
//! the saturation flag and gap, the input bounds, the one-hot count, index and
//! booleanity, and the argmax slack) makes a named test fail. The first
//! version of these tests passed with the ReLU sign check deleted, because a
//! lie that was not propagated forward was caught by some unrelated constraint
//! first. The one deletion nothing notices is the quotient's range check, which
//! is implied by three others — see the comment where it is laid out.
//!
//! # Why 27-bit range checks are sound here
//!
//! A range check says `v ∈ [0, 2^27)`. Arithmetic here is modulo a 254-bit
//! prime, so a "negative" intermediate is really a number near `2^254` and
//! fails the check. That argument holds only while every *honest* value is far
//! below `2^27`; `crate::model` bounds the worst case under `2^20`.

use halo2_axiom::circuit::{Cell, Layouter, Region, SimpleFloorPlanner, Value};
use halo2_axiom::halo2curves::bn256::Fr;
use halo2_axiom::halo2curves::ff::{Field, PrimeField};
use halo2_axiom::plonk::{
    Advice, Circuit, Column, ConstraintSystem, Error, Fixed, Instance, Selector, TableColumn,
};
use halo2_axiom::poly::Rotation;

use crate::model::{HIDDEN_CEILING, QuantizedMlp};

/// Bits per range-check limb. The lookup table holds `[0, 2^LIMB_BITS)`.
pub const LIMB_BITS: u32 = 9;
/// Limbs per range check.
pub const LIMBS: usize = 3;
/// Width of every range check: `LIMB_BITS × LIMBS = 27`.
pub const RANGE_BITS: u32 = LIMB_BITS * LIMBS as u32;

/// Every value the prover chooses freely.
///
/// Public, and deliberately so: the soundness tests construct witnesses that
/// lie, one field at a time, and check the constraints refuse each lie. A
/// circuit whose witness could only be produced by [`Witness::honest`] could
/// only be tested with honest witnesses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Witness {
    /// The input vector. Public in the proof; `i64` so a test can go out of
    /// int8 range.
    pub input: Vec<i64>,
    /// Per hidden unit: `1` if the accumulator is non-negative.
    pub relu_bit: Vec<i64>,
    /// Per hidden unit: `max(acc, 0) >> shift`.
    pub quotient: Vec<i64>,
    /// Per hidden unit: `max(acc, 0) mod 2^shift`.
    pub remainder: Vec<i64>,
    /// Per hidden unit: `1` if the quotient saturated at 127.
    pub saturated: Vec<i64>,
    /// One-hot encoding of the class.
    pub one_hot: Vec<i64>,
    /// The claimed class. Public in the proof.
    pub class: i64,
}

impl Witness {
    /// The witness an honest prover computes for `input`.
    ///
    /// Takes `i64` so a test can build a consistent witness for an input the
    /// circuit must still refuse.
    ///
    /// # Panics
    ///
    /// If `input` is not the model's input width — this is a test and prover
    /// helper, and a wrong width there is a bug in the caller.
    #[must_use]
    pub fn honest(model: &QuantizedMlp, input: &[i64]) -> Self {
        let evaluation = model
            .evaluate_wide(input)
            .expect("input width matches the model");
        let divisor_mask = (1i64 << model.shift()) - 1;
        let relu: Vec<i64> = evaluation.acc.iter().map(|&a| a.max(0)).collect();
        let quotient: Vec<i64> = relu.iter().map(|&r| r >> model.shift()).collect();

        Self {
            input: input.to_vec(),
            relu_bit: evaluation.acc.iter().map(|&a| i64::from(a >= 0)).collect(),
            remainder: relu.iter().map(|&r| r & divisor_mask).collect(),
            saturated: quotient
                .iter()
                .map(|&q| i64::from(q >= HIDDEN_CEILING))
                .collect(),
            quotient,
            one_hot: (0..model.classes())
                .map(|k| i64::from(k == evaluation.class))
                .collect(),
            class: evaluation.class as i64,
        }
    }

    /// The public inputs this witness claims: the input vector, then the class.
    #[must_use]
    pub fn public_inputs(&self) -> Vec<Fr> {
        public_inputs(&self.input, self.class)
    }
}

/// The instance column for a claim: `inputs` values, then the class.
#[must_use]
pub fn public_inputs(input: &[i64], class: i64) -> Vec<Fr> {
    input.iter().copied().chain([class]).map(fr).collect()
}

/// A signed integer as a field element.
#[must_use]
pub fn fr(value: i64) -> Fr {
    let magnitude = Fr::from(value.unsigned_abs());
    if value < 0 { -magnitude } else { magnitude }
}

/// The circuit's columns.
#[derive(Clone, Debug)]
pub struct MlpConfig {
    a: Column<Advice>,
    b: Column<Advice>,
    c: Column<Advice>,
    fa: Column<Fixed>,
    fb: Column<Fixed>,
    fm: Column<Fixed>,
    fc: Column<Fixed>,
    arith: Selector,
    range: Selector,
    table: TableColumn,
    instance: Column<Instance>,
}

/// The model, and — when proving — a witness.
///
/// Without a witness this is what key generation sees: the weights are fixed
/// columns and so part of the key, and nothing else is known.
#[derive(Clone, Debug)]
pub struct MlpCircuit {
    model: QuantizedMlp,
    witness: Option<Witness>,
}

impl MlpCircuit {
    /// The shape of the circuit, for key generation.
    #[must_use]
    pub fn for_keygen(model: QuantizedMlp) -> Self {
        Self {
            model,
            witness: None,
        }
    }

    /// A circuit carrying a witness, for proving.
    #[must_use]
    pub fn with_witness(model: QuantizedMlp, witness: Witness) -> Self {
        Self {
            model,
            witness: Some(witness),
        }
    }
}

impl Circuit<Fr> for MlpCircuit {
    type Config = MlpConfig;
    type FloorPlanner = SimpleFloorPlanner;
    type Params = ();

    fn without_witnesses(&self) -> Self {
        Self::for_keygen(self.model.clone())
    }

    fn configure(meta: &mut ConstraintSystem<Fr>) -> MlpConfig {
        let a = meta.advice_column();
        let b = meta.advice_column();
        let c = meta.advice_column();
        let instance = meta.instance_column();
        for column in [a, b, c] {
            meta.enable_equality(column);
        }
        meta.enable_equality(instance);

        let fa = meta.fixed_column();
        let fb = meta.fixed_column();
        let fm = meta.fixed_column();
        let fc = meta.fixed_column();
        let arith = meta.selector();
        // Complex, because a lookup input may not contain a simple selector.
        let range = meta.complex_selector();
        let table = meta.lookup_table_column();

        meta.create_gate("arith", |m| {
            let s = m.query_selector(arith);
            let (av, bv, cv) = (
                m.query_advice(a, Rotation::cur()),
                m.query_advice(b, Rotation::cur()),
                m.query_advice(c, Rotation::cur()),
            );
            let (qa, qb, qm, qc) = (
                m.query_fixed(fa, Rotation::cur()),
                m.query_fixed(fb, Rotation::cur()),
                m.query_fixed(fm, Rotation::cur()),
                m.query_fixed(fc, Rotation::cur()),
            );
            vec![s * (qa * av.clone() + qb * bv.clone() + qm * av * bv + qc - cv)]
        });

        // When `range` is off the input is 0, which is in the table, so
        // unselected rows impose nothing.
        meta.lookup("limb", |m| {
            let s = m.query_selector(range);
            let v = m.query_advice(a, Rotation::cur());
            vec![(s * v, table)]
        });

        MlpConfig {
            a,
            b,
            c,
            fa,
            fb,
            fm,
            fc,
            arith,
            range,
            table,
            instance,
        }
    }

    fn synthesize(&self, config: MlpConfig, mut layouter: impl Layouter<Fr>) -> Result<(), Error> {
        layouter.assign_table(
            || "limbs",
            |mut table| {
                for value in 0..(1usize << LIMB_BITS) {
                    table.assign_cell(
                        || "limb",
                        config.table,
                        value,
                        || Value::known(Fr::from(value as u64)),
                    )?;
                }
                Ok(())
            },
        )?;

        let public = layouter.assign_region(
            || "mlp",
            |mut region| {
                let mut builder = Builder {
                    region: &mut region,
                    config: &config,
                    row: 0,
                };
                builder.model(&self.model, self.witness.as_ref())
            },
        )?;

        for (row, cell) in public.into_iter().enumerate() {
            layouter.constrain_instance(cell, config.instance, row);
        }
        Ok(())
    }
}

/// Reads one value out of the witness, or `unknown` at key generation, when
/// there is no witness to read.
type Known<'a> = &'a dyn Fn(&dyn Fn(&Witness) -> i64) -> Value<Fr>;

/// An assigned value and where it lives.
#[derive(Clone, Copy, Debug)]
struct Wire {
    cell: Cell,
    value: Value<Fr>,
}

/// Lays the model out one row at a time, in a single region.
struct Builder<'a, 'r> {
    region: &'a mut Region<'r, Fr>,
    config: &'a MlpConfig,
    row: usize,
}

const ZERO: Fr = Fr::ZERO;
const ONE: Fr = Fr::ONE;

impl Builder<'_, '_> {
    /// A value the prover chooses, constrained only by where it is later used.
    fn free(&mut self, value: Value<Fr>) -> Wire {
        let cell = self
            .region
            .assign_advice(self.config.c, self.row, value)
            .cell();
        self.row += 1;
        Wire { cell, value }
    }

    /// One row of the gate: `c = fa·a + fb·b + fm·a·b + fc`.
    ///
    /// An absent operand is assigned zero and left unconstrained, which is sound
    /// only while its coefficients are zero — so that is asserted rather than
    /// trusted. It is a layout fact, identical at key generation and at proving.
    fn arith(
        &mut self,
        a: Option<Wire>,
        b: Option<Wire>,
        [fa, fb, fm, fc]: [Fr; 4],
    ) -> Result<Wire, Error> {
        assert!(
            a.is_some() || (fa == ZERO && fm == ZERO),
            "an absent `a` operand must not carry a coefficient"
        );
        assert!(
            b.is_some() || (fb == ZERO && fm == ZERO),
            "an absent `b` operand must not carry a coefficient"
        );

        let row = self.row;
        self.row += 1;
        let av = a.map_or(Value::known(ZERO), |w| w.value);
        let bv = b.map_or(Value::known(ZERO), |w| w.value);

        let a_cell = self.region.assign_advice(self.config.a, row, av).cell();
        if let Some(w) = a {
            self.region.constrain_equal(a_cell, w.cell);
        }
        let b_cell = self.region.assign_advice(self.config.b, row, bv).cell();
        if let Some(w) = b {
            self.region.constrain_equal(b_cell, w.cell);
        }

        self.region.assign_fixed(self.config.fa, row, fa);
        self.region.assign_fixed(self.config.fb, row, fb);
        self.region.assign_fixed(self.config.fm, row, fm);
        self.region.assign_fixed(self.config.fc, row, fc);
        self.config.arith.enable(self.region, row)?;

        let value = av.zip(bv).map(|(x, y)| fa * x + fb * y + fm * x * y + fc);
        let cell = self.region.assign_advice(self.config.c, row, value).cell();
        Ok(Wire { cell, value })
    }

    fn equal(&mut self, left: Wire, right: Wire) {
        self.region.constrain_equal(left.cell, right.cell);
    }

    /// `w ∈ {0, 1}`: `w·w = w`.
    fn boolean(&mut self, w: Wire) -> Result<(), Error> {
        let square = self.arith(Some(w), Some(w), [ZERO, ZERO, ONE, ZERO])?;
        self.equal(square, w);
        Ok(())
    }

    /// A constant, fixed by the key.
    fn constant(&mut self, value: Fr) -> Result<Wire, Error> {
        self.arith(None, None, [ZERO, ZERO, ZERO, value])
    }

    /// `w ∈ [0, 2^27)`: three looked-up 9-bit limbs that recompose to `w`.
    fn range(&mut self, w: Wire) -> Result<(), Error> {
        let limbs = w.value.map(decompose);
        let mut cells = [None; LIMBS];
        for (i, slot) in cells.iter_mut().enumerate() {
            let value = limbs.map(|l| Fr::from(l[i]));
            let cell = self
                .region
                .assign_advice(self.config.a, self.row, value)
                .cell();
            self.config.range.enable(self.region, self.row)?;
            self.row += 1;
            *slot = Some(Wire { cell, value });
        }
        let low = self.arith(
            cells[0],
            cells[1],
            [ONE, Fr::from(1 << LIMB_BITS), ZERO, ZERO],
        )?;
        let whole = self.arith(
            Some(low),
            cells[2],
            [ONE, Fr::from(1 << (2 * LIMB_BITS)), ZERO, ZERO],
        )?;
        self.equal(whole, w);
        Ok(())
    }

    /// Lays out the whole model and returns the cells bound to the instance.
    fn model(
        &mut self,
        model: &QuantizedMlp,
        witness: Option<&Witness>,
    ) -> Result<Vec<Cell>, Error> {
        let known = |pick: &dyn Fn(&Witness) -> i64| {
            witness.map_or(Value::unknown(), |w| Value::known(fr(pick(w))))
        };

        let inputs: Vec<Wire> = (0..model.inputs())
            .map(|i| self.free(known(&|w| w.input[i])))
            .collect();
        for &x in &inputs {
            // x + 128 ≥ 0 and 127 − x ≥ 0, i.e. x ∈ [−128, 127].
            let above = self.arith(Some(x), None, [ONE, ZERO, ZERO, fr(128)])?;
            self.range(above)?;
            let below = self.arith(Some(x), None, [-ONE, ZERO, ZERO, fr(127)])?;
            self.range(below)?;
        }

        let mut hidden = Vec::with_capacity(model.hidden());
        for j in 0..model.hidden() {
            let weights: Vec<i64> = (0..model.inputs())
                .map(|i| i64::from(model.w1(i, j)))
                .collect();
            let acc = self.dot(&inputs, &weights, i64::from(model.b1(j)))?;
            hidden.push(self.relu_requantize(acc, model.shift(), j, &known)?);
        }

        let logits: Vec<Wire> = (0..model.classes())
            .map(|k| {
                let weights: Vec<i64> = (0..model.hidden())
                    .map(|j| i64::from(model.w2(j, k)))
                    .collect();
                self.dot(&hidden, &weights, i64::from(model.b2(k)))
            })
            .collect::<Result<_, _>>()?;

        let class = self.free(known(&|w| w.class));
        self.argmax(&logits, class, &known)?;

        Ok(inputs.iter().map(|w| w.cell).chain([class.cell]).collect())
    }

    /// `Σ weights[i]·values[i] + bias`, one row per term.
    fn dot(&mut self, values: &[Wire], weights: &[i64], bias: i64) -> Result<Wire, Error> {
        let mut acc = self.arith(
            Some(values[0]),
            None,
            [fr(weights[0]), ZERO, ZERO, fr(bias)],
        )?;
        for (&value, &weight) in values.iter().zip(weights).skip(1) {
            acc = self.arith(Some(value), Some(acc), [fr(weight), ONE, ZERO, ZERO])?;
        }
        Ok(acc)
    }

    /// `min(max(acc, 0) >> shift, 127)`.
    fn relu_requantize(
        &mut self,
        acc: Wire,
        shift: u32,
        unit: usize,
        known: Known<'_>,
    ) -> Result<Wire, Error> {
        // ReLU. `bit` claims the sign; `t` is small only if the claim is true.
        let bit = self.free(known(&|w| w.relu_bit[unit]));
        self.boolean(bit)?;
        let relu = self.arith(Some(bit), Some(acc), [ZERO, ZERO, ONE, ZERO])?;
        let twice_less_acc = self.arith(Some(relu), Some(acc), [fr(2), -ONE, ZERO, ZERO])?;
        let t = self.arith(Some(twice_less_acc), Some(bit), [ONE, ONE, ZERO, -ONE])?;
        self.range(t)?;

        // Requantize. Floor division has exactly one (q, rem) with 0 ≤ rem < 2^s.
        let divisor = 1i64 << shift;
        let q = self.free(known(&|w| w.quotient[unit]));
        let rem = self.free(known(&|w| w.remainder[unit]));
        let recomposed = self.arith(Some(q), Some(rem), [fr(divisor), ONE, ZERO, ZERO])?;
        self.equal(recomposed, relu);
        // Implied, and kept anyway. `relu ∈ [0, 2^27)` (the sign check) and
        // `rem ∈ [0, 2^s)` make `q·2^s = relu − rem` a small integer or a value
        // with no small preimage at all, and the saturation gap check refuses
        // the latter. The mutation sweep confirms it: deleting this line is the
        // one deletion no test notices. It stays because the argument is three
        // constraints long and this is one row.
        self.range(q)?;
        self.range(rem)?;
        let headroom = self.arith(Some(rem), None, [-ONE, ZERO, ZERO, fr(divisor - 1)])?;
        self.range(headroom)?;

        // Saturate. `c` claims q ≥ 127; `gap` is small only if the claim is true.
        let c = self.free(known(&|w| w.saturated[unit]));
        self.boolean(c)?;
        let ceiling = HIDDEN_CEILING;
        let slope = self.arith(Some(q), None, [fr(2), ZERO, ZERO, fr(-(2 * ceiling - 1))])?;
        let selected = self.arith(Some(c), Some(slope), [ZERO, ZERO, ONE, ZERO])?;
        let gap = self.arith(Some(selected), Some(q), [ONE, -ONE, ZERO, fr(ceiling - 1)])?;
        self.range(gap)?;

        // hidden = q + c·(127 − q)
        let shortfall = self.arith(Some(q), None, [-ONE, ZERO, ZERO, fr(ceiling)])?;
        let lift = self.arith(Some(c), Some(shortfall), [ZERO, ZERO, ONE, ZERO])?;
        self.arith(Some(q), Some(lift), [ONE, ONE, ZERO, ZERO])
    }

    /// `class` is the index of the first maximum of `logits`.
    fn argmax(&mut self, logits: &[Wire], class: Wire, known: Known<'_>) -> Result<(), Error> {
        let one_hot: Vec<Wire> = (0..logits.len())
            .map(|k| self.free(known(&|w| w.one_hot[k])))
            .collect();
        for &e in &one_hot {
            self.boolean(e)?;
        }

        // Exactly one bit set.
        let mut count = self.arith(Some(one_hot[0]), None, [ONE, ZERO, ZERO, ZERO])?;
        for &e in &one_hot[1..] {
            count = self.arith(Some(e), Some(count), [ONE, ONE, ZERO, ZERO])?;
        }
        let one = self.constant(ONE)?;
        self.equal(count, one);

        // …and it is at `class`. Index 0 contributes nothing, and there are at
        // least two classes, so the sum starts at index 1.
        let mut index = self.arith(Some(one_hot[1]), None, [ONE, ZERO, ZERO, ZERO])?;
        for (k, &e) in one_hot.iter().enumerate().skip(2) {
            index = self.arith(Some(e), Some(index), [fr(k as i64), ONE, ZERO, ZERO])?;
        }
        self.equal(index, class);

        // The selected logit.
        let mut selected =
            self.arith(Some(one_hot[0]), Some(logits[0]), [ZERO, ZERO, ONE, ZERO])?;
        for (&e, &logit) in one_hot.iter().zip(logits).skip(1) {
            let term = self.arith(Some(e), Some(logit), [ZERO, ZERO, ONE, ZERO])?;
            selected = self.arith(Some(term), Some(selected), [ONE, ONE, ZERO, ZERO])?;
        }

        // selected − logit_j − [j < class] ≥ 0 for every j. For j after the
        // class this is "no larger"; for j before it, "strictly smaller" —
        // which is what makes the claim the *first* maximum.
        let mut later: Option<Wire> = None;
        for j in (0..logits.len()).rev() {
            let difference =
                self.arith(Some(selected), Some(logits[j]), [ONE, -ONE, ZERO, ZERO])?;
            let slack = match later {
                Some(l) => self.arith(Some(difference), Some(l), [ONE, -ONE, ZERO, ZERO])?,
                None => difference,
            };
            self.range(slack)?;
            later = Some(match later {
                Some(l) => self.arith(Some(one_hot[j]), Some(l), [ONE, ONE, ZERO, ZERO])?,
                None => one_hot[j],
            });
        }
        Ok(())
    }
}

/// Splits a field element into three 9-bit limbs.
///
/// A value that does not fit in 27 bits — which is every "negative" value, as
/// those are near the modulus — yields limbs that do not recompose to it, and
/// the recomposition constraint fails. The prover cannot help that, which is
/// the point.
fn decompose(value: Fr) -> [u64; LIMBS] {
    let repr = value.to_repr();
    let bytes = repr.as_ref();
    let fits = bytes[4..].iter().all(|&b| b == 0);
    let low = u64::from(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]));
    if !fits || low >= 1 << RANGE_BITS {
        return [0; LIMBS];
    }
    let mask = (1u64 << LIMB_BITS) - 1;
    [
        low & mask,
        (low >> LIMB_BITS) & mask,
        low >> (2 * LIMB_BITS),
    ]
}
