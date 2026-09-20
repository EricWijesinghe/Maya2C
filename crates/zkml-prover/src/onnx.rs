//! Reading the model out of an ONNX file, and running it with tract.
//!
//! Off-chain only: this crate is never a dependency of the node.
//!
//! # The importer refuses anything it does not recognise
//!
//! [`load`] does not interpret arbitrary ONNX. It checks the graph is exactly
//! the nine-node integer classifier `scripts/make_zkml_fixture.py` builds —
//! same op sequence, each node fed by the previous one, the constants where
//! they are expected to be and equal to what the circuit hard-codes — and
//! refuses otherwise. An importer that accepted "something close" would produce
//! a circuit proving a model other than the one in the file, and the model id
//! would faithfully identify the wrong thing.
//!
//! # tract is the independent witness
//!
//! [`evaluate_with_tract`] runs the file through tract, which knows nothing
//! about the circuit. The tests require its answer to match both the circuit's
//! claimed class and a numpy evaluation of the same weights. That is the reason
//! tract is here at all.

use std::path::Path;

use tract_onnx::pb::{GraphProto, NodeProto, TensorProto};
use tract_onnx::prelude::*;

use maya_zkml::error::{Result, ZkmlError};
use maya_zkml::model::{HIDDEN_CEILING, QuantizedMlp};

/// The op sequence this importer accepts, in order.
const OPS: [&str; 9] = [
    "MatMulInteger",
    "Add",
    "Max",
    "Div",
    "Min",
    "Cast",
    "MatMulInteger",
    "Add",
    "ArgMax",
];

/// ONNX `TensorProto.DataType` values.
const INT8: i32 = 3;
const INT32: i32 = 6;

fn unsupported(why: impl Into<String>) -> ZkmlError {
    ZkmlError::UnsupportedOnnx(why.into())
}

/// Loads the classifier from an ONNX file.
///
/// # Errors
///
/// [`ZkmlError::UnsupportedOnnx`] for a file that does not parse or is not the
/// supported shape, and [`ZkmlError::ModelOutOfBounds`] for weights outside
/// the circuit's limits.
pub fn load(path: impl AsRef<Path>) -> Result<QuantizedMlp> {
    let proto = tract_onnx::onnx()
        .proto_model_for_path(path)
        .map_err(|e| unsupported(e.to_string()))?;
    let graph = proto.graph.ok_or_else(|| unsupported("no graph"))?;
    check_topology(&graph)?;

    let n = &graph.node;
    let (w1, w1_dims) = int_tensor(&graph, &n[0].input[1], INT8)?;
    let (b1, _) = int_tensor(&graph, &n[1].input[1], INT32)?;
    let zero = scalar(&graph, &n[2].input[1])?;
    let divisor = scalar(&graph, &n[3].input[1])?;
    let ceiling = scalar(&graph, &n[4].input[1])?;
    let (w2, w2_dims) = int_tensor(&graph, &n[6].input[1], INT8)?;
    let (b2, _) = int_tensor(&graph, &n[7].input[1], INT32)?;

    if zero != 0 {
        return Err(unsupported("ReLU threshold is not zero"));
    }
    if ceiling != HIDDEN_CEILING {
        return Err(unsupported("saturation ceiling is not 127"));
    }
    if divisor <= 0 || divisor & (divisor - 1) != 0 {
        return Err(unsupported("requantization divisor is not a power of two"));
    }
    if attribute(&n[5], "to") != Some(i64::from(INT8)) {
        return Err(unsupported("hidden activations are not cast to int8"));
    }
    if attribute(&n[8], "axis") != Some(1)
        || attribute(&n[8], "select_last_index").unwrap_or(0) != 0
    {
        return Err(unsupported(
            "ArgMax is not over the class axis, first index wins",
        ));
    }

    let [inputs, hidden] = two_dims(&w1_dims)?;
    let [hidden2, classes] = two_dims(&w2_dims)?;
    if hidden != hidden2 {
        return Err(unsupported("layer widths do not chain"));
    }

    QuantizedMlp::new(
        inputs,
        hidden,
        classes,
        w1.iter().map(|&v| v as i8).collect(),
        b1.iter().map(|&v| v as i32).collect(),
        w2.iter().map(|&v| v as i8).collect(),
        b2.iter().map(|&v| v as i32).collect(),
        divisor.trailing_zeros(),
    )
}

/// Every node is the expected op, has the expected arity, and is fed by the
/// node before it.
fn check_topology(graph: &GraphProto) -> Result<()> {
    let ops: Vec<&str> = graph.node.iter().map(|n| n.op_type.as_str()).collect();
    if ops != OPS {
        return Err(unsupported(format!(
            "op sequence {ops:?}, expected {OPS:?}"
        )));
    }
    let input = graph
        .input
        .first()
        .map(|v| v.name.as_str())
        .ok_or_else(|| unsupported("no graph input"))?;

    for (index, node) in graph.node.iter().enumerate() {
        // Unary ops take one input; everything else, exactly two. A third input
        // to MatMulInteger is a zero point, which the circuit does not model.
        let arity = if matches!(node.op_type.as_str(), "Cast" | "ArgMax") {
            1
        } else {
            2
        };
        if node.input.len() != arity || node.output.len() != 1 {
            return Err(unsupported(format!("node {index} has unexpected arity")));
        }
        let expected_source = match index {
            0 => input,
            _ => graph.node[index - 1].output[0].as_str(),
        };
        if node.input[0] != expected_source {
            return Err(unsupported(format!(
                "node {index} is not fed by its predecessor"
            )));
        }
    }
    Ok(())
}

fn initializer<'g>(graph: &'g GraphProto, name: &str) -> Result<&'g TensorProto> {
    graph
        .initializer
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| unsupported(format!("{name} is not a constant")))
}

/// Reads an integer initializer of the given type, from `raw_data` if present
/// and from `int32_data` otherwise — ONNX permits both.
fn int_tensor(graph: &GraphProto, name: &str, data_type: i32) -> Result<(Vec<i64>, Vec<i64>)> {
    let tensor = initializer(graph, name)?;
    if tensor.data_type != data_type {
        return Err(unsupported(format!(
            "{name} has data type {}",
            tensor.data_type
        )));
    }
    let values: Vec<i64> = if tensor.raw_data.is_empty() {
        tensor.int32_data.iter().map(|&v| i64::from(v)).collect()
    } else if data_type == INT8 {
        tensor
            .raw_data
            .iter()
            .map(|&b| i64::from(b as i8))
            .collect()
    } else {
        let (words, stray) = tensor.raw_data.as_chunks::<4>();
        // A stray byte would be dropped silently, and the element count below
        // could still match. Refused, rather than read as a different tensor.
        if !stray.is_empty() {
            return Err(unsupported(format!(
                "{name} is not a whole number of int32s"
            )));
        }
        words
            .iter()
            .map(|c| i64::from(i32::from_le_bytes(*c)))
            .collect()
    };
    let expected: i64 = tensor.dims.iter().product();
    if values.len() as i64 != expected {
        return Err(unsupported(format!(
            "{name} holds the wrong number of values"
        )));
    }
    Ok((values, tensor.dims.clone()))
}

fn scalar(graph: &GraphProto, name: &str) -> Result<i64> {
    let (values, _) = int_tensor(graph, name, INT32)?;
    match values.as_slice() {
        [v] => Ok(*v),
        _ => Err(unsupported(format!("{name} is not a scalar"))),
    }
}

fn two_dims(dims: &[i64]) -> Result<[usize; 2]> {
    match dims {
        [a, b] if *a > 0 && *b > 0 => Ok([*a as usize, *b as usize]),
        _ => Err(unsupported("weight matrix is not two-dimensional")),
    }
}

fn attribute(node: &NodeProto, name: &str) -> Option<i64> {
    node.attribute.iter().find(|a| a.name == name).map(|a| a.i)
}

/// Runs the file through tract and returns `(class, logits)`.
///
/// # Errors
///
/// [`ZkmlError::UnsupportedOnnx`] if tract cannot load or run the model.
pub fn evaluate_with_tract(path: impl AsRef<Path>, input: &[i8]) -> Result<(usize, Vec<i64>)> {
    let run = || -> TractResult<(usize, Vec<i64>)> {
        let model = tract_onnx::onnx()
            .model_for_path(path)?
            .into_optimized()?
            .into_runnable()?;
        let x = tract_ndarray::Array2::from_shape_vec((1, input.len()), input.to_vec())?;
        let outputs = model.run(tvec!(x.into_tensor().into()))?;
        let class = *outputs[0]
            .to_plain_array_view::<i64>()?
            .iter()
            .next()
            .ok_or_else(|| TractError::msg("empty class output"))?;
        let logits = outputs[1]
            .to_plain_array_view::<i32>()?
            .iter()
            .map(|&v| i64::from(v))
            .collect();
        Ok((class as usize, logits))
    };
    run().map_err(|e| unsupported(e.to_string()))
}
