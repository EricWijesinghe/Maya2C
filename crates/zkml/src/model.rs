//! The one model shape this crate proves, and its integer semantics.
//!
//! ```text
//! acc    = x · W1 + b1                        int8 × int8 → int32
//! hidden = min(max(acc, 0) >> shift, 127)     ReLU, requantize, saturate
//! logits = hidden · W2 + b2                   int8 × int8 → int32
//! class  = argmax(logits)                     the first maximum wins a tie
//! ```
//!
//! [`QuantizedMlp::evaluate`] is the definition. The circuit in
//! [`crate::circuit`] constrains exactly this computation and nothing wider,
//! and the tests check both against a numpy evaluation and a tract evaluation
//! of the same ONNX file — three implementations, so no one of them can be
//! wrong alone.
//!
//! # Why the bounds are compiled in
//!
//! The circuit's range checks are 27 bits wide. That width is sound only if
//! every intermediate value the circuit handles is far below it, and "far
//! below" is a function of the layer widths and the bias magnitude. The limits
//! here keep the worst case under `2^20`, which leaves a margin of 128× — so a
//! value that went negative in the field (and therefore became a number near
//! the 254-bit modulus) can never be mistaken for a small positive one.
//! Widening a limit without redoing that arithmetic is how a range check stops
//! checking anything.

use crate::error::{Result, ZkmlError};

/// Widest input layer.
pub const MAX_INPUTS: usize = 16;
/// Widest hidden layer.
pub const MAX_HIDDEN: usize = 16;
/// Most classes.
pub const MAX_CLASSES: usize = 8;
/// Largest bias magnitude, `2^12`.
pub const BIAS_BOUND: i32 = 1 << 12;
/// Largest requantization shift.
pub const MAX_SHIFT: u32 = 16;
/// The saturation ceiling for a hidden activation.
pub const HIDDEN_CEILING: i64 = 127;

/// A two-layer integer classifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantizedMlp {
    inputs: usize,
    hidden: usize,
    classes: usize,
    /// `[inputs][hidden]`, row-major — the layout ONNX `MatMulInteger` uses.
    w1: Vec<i8>,
    b1: Vec<i32>,
    /// `[hidden][classes]`, row-major.
    w2: Vec<i8>,
    b2: Vec<i32>,
    shift: u32,
}

/// Every intermediate of one evaluation — the circuit's witness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    /// First-layer accumulators, before ReLU.
    pub acc: Vec<i64>,
    /// Hidden activations, in `0..=127`.
    pub hidden: Vec<i64>,
    /// Output logits.
    pub logits: Vec<i64>,
    /// The predicted class.
    pub class: usize,
}

impl QuantizedMlp {
    /// Builds a model, refusing anything the circuit's bounds do not cover.
    ///
    /// # Errors
    ///
    /// [`ZkmlError::ModelOutOfBounds`] naming the first violated limit.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        inputs: usize,
        hidden: usize,
        classes: usize,
        w1: Vec<i8>,
        b1: Vec<i32>,
        w2: Vec<i8>,
        b2: Vec<i32>,
        shift: u32,
    ) -> Result<Self> {
        let refuse = |what: &'static str| Err(ZkmlError::ModelOutOfBounds(what));
        if inputs == 0 || inputs > MAX_INPUTS {
            return refuse("input width");
        }
        if hidden == 0 || hidden > MAX_HIDDEN {
            return refuse("hidden width");
        }
        // One class is a constant, not a classifier, and argmax over it proves
        // nothing a verifier could not have assumed.
        if !(2..=MAX_CLASSES).contains(&classes) {
            return refuse("class count");
        }
        if w1.len() != inputs * hidden || b1.len() != hidden {
            return refuse("first layer shape");
        }
        if w2.len() != hidden * classes || b2.len() != classes {
            return refuse("second layer shape");
        }
        if b1
            .iter()
            .chain(&b2)
            .any(|b| b.unsigned_abs() > BIAS_BOUND as u32)
        {
            return refuse("bias magnitude");
        }
        if shift > MAX_SHIFT {
            return refuse("requantization shift");
        }
        Ok(Self {
            inputs,
            hidden,
            classes,
            w1,
            b1,
            w2,
            b2,
            shift,
        })
    }

    /// Input width.
    #[must_use]
    pub fn inputs(&self) -> usize {
        self.inputs
    }

    /// Hidden width.
    #[must_use]
    pub fn hidden(&self) -> usize {
        self.hidden
    }

    /// Class count.
    #[must_use]
    pub fn classes(&self) -> usize {
        self.classes
    }

    /// Requantization shift.
    #[must_use]
    pub fn shift(&self) -> u32 {
        self.shift
    }

    /// `W1[i][j]`.
    #[must_use]
    pub fn w1(&self, i: usize, j: usize) -> i8 {
        self.w1[i * self.hidden + j]
    }

    /// `b1[j]`.
    #[must_use]
    pub fn b1(&self, j: usize) -> i32 {
        self.b1[j]
    }

    /// `W2[j][k]`.
    #[must_use]
    pub fn w2(&self, j: usize, k: usize) -> i8 {
        self.w2[j * self.classes + k]
    }

    /// `b2[k]`.
    #[must_use]
    pub fn b2(&self, k: usize) -> i32 {
        self.b2[k]
    }

    /// Runs the model. The definition every other implementation is tested
    /// against.
    ///
    /// # Errors
    ///
    /// [`ZkmlError::WrongInputWidth`] if `x` is not `inputs` long.
    pub fn evaluate(&self, x: &[i8]) -> Result<Evaluation> {
        let wide: Vec<i64> = x.iter().map(|&v| i64::from(v)).collect();
        self.evaluate_wide(&wide)
    }

    /// [`Self::evaluate`] over `i64` inputs, with no int8 check.
    ///
    /// Exists for one caller: [`crate::circuit::Witness::honest`], so that a
    /// test can build a witness that is *consistent* for an out-of-range input
    /// and prove the circuit still refuses it. The arithmetic is the same
    /// function, not a copy of it — two copies of the model's semantics is one
    /// more than can be kept in step by eye.
    pub(crate) fn evaluate_wide(&self, x: &[i64]) -> Result<Evaluation> {
        if x.len() != self.inputs {
            return Err(ZkmlError::WrongInputWidth {
                expected: self.inputs,
                found: x.len(),
            });
        }

        let acc: Vec<i64> = (0..self.hidden)
            .map(|j| {
                (0..self.inputs)
                    .map(|i| x[i] * i64::from(self.w1(i, j)))
                    .sum::<i64>()
                    + i64::from(self.b1(j))
            })
            .collect();

        // `max(acc, 0)` is non-negative, so the shift is a floor division and
        // agrees with ONNX `Div` on int32, which truncates toward zero.
        let hidden: Vec<i64> = acc
            .iter()
            .map(|&a| (a.max(0) >> self.shift).min(HIDDEN_CEILING))
            .collect();

        let logits: Vec<i64> = (0..self.classes)
            .map(|k| {
                (0..self.hidden)
                    .map(|j| hidden[j] * i64::from(self.w2(j, k)))
                    .sum::<i64>()
                    + i64::from(self.b2(k))
            })
            .collect();

        // First maximum, as ONNX ArgMax with `select_last_index = 0` and numpy
        // both define it. `max_by_key` would return the *last* maximum.
        let mut class = 0;
        for (k, &logit) in logits.iter().enumerate() {
            if logit > logits[class] {
                class = k;
            }
        }

        Ok(Evaluation {
            acc,
            hidden,
            logits,
            class,
        })
    }
}
