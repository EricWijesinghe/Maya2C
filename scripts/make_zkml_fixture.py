"""Builds the quantized classifier the zkML tests prove inference for.

Run from the repository root:

    uv venv .zkml-venv && uv pip install --python .zkml-venv onnx numpy
    .zkml-venv/Scripts/python scripts/make_zkml_fixture.py

Writes two files under tests/fixtures/zkml/:

  classifier.onnx           the model
  classifier.expected.json  inputs, and the logits and class a plain numpy
                            evaluation assigns them

# Why the model is integer-only, end to end

The circuit in zkml/src/circuit.rs proves exactly this computation:

    acc    = x @ W1 + b1                       int8 x int8 -> int32
    hidden = min(max(acc, 0) // 2**SHIFT, 127) ReLU, requantize, saturate
    logits = hidden @ W2 + b2                  int8 x int8 -> int32
    class  = argmax(logits)                    first index wins a tie

No float appears anywhere, because a float in a proven computation is a
rounding mode the prover and the verifier have to agree on, and ONNX, numpy
and a halo2 circuit do not share one.

The graph is spelled with Max/Div/Min rather than Relu/Clip so that every op
is defined for int32 in every opset tract implements -- the point of the
fixture is that three independent evaluators (numpy here, tract in the
prover, and the circuit's own witness) agree on it.

# Why the expected outputs are computed here, in numpy

A test that compared the circuit against itself would pass for any circuit.
The JSON is the external reference: tests/zkml assert that tract's evaluation
of the .onnx file and the circuit's claimed class both match it.

# The bounds are part of the contract

zkml/src/model.rs refuses weights outside these ranges, because the circuit's
range checks are sized from them. Changing a bound here without changing it
there produces a model the importer rejects, which is the intended failure.
"""

import json
import pathlib

import numpy as np
import onnx
from onnx import TensorProto, helper, numpy_helper

SEED = 7331
INPUTS, HIDDEN, CLASSES = 4, 8, 3
SHIFT = 6
BIAS_BOUND = 1 << 12

OUT = pathlib.Path("tests/fixtures/zkml")


def weights(rng):
    w1 = rng.integers(-127, 128, size=(INPUTS, HIDDEN), dtype=np.int8)
    b1 = rng.integers(-BIAS_BOUND, BIAS_BOUND + 1, size=(HIDDEN,), dtype=np.int32)
    w2 = rng.integers(-127, 128, size=(HIDDEN, CLASSES), dtype=np.int8)
    b2 = rng.integers(-BIAS_BOUND, BIAS_BOUND + 1, size=(CLASSES,), dtype=np.int32)
    return w1, b1, w2, b2


def reference(x, w1, b1, w2, b2):
    """The computation, in integers, with no ONNX involved."""
    acc = x.astype(np.int64) @ w1.astype(np.int64) + b1
    hidden = np.minimum(np.maximum(acc, 0) // (1 << SHIFT), 127)
    logits = hidden @ w2.astype(np.int64) + b2
    # np.argmax returns the first maximum, matching ONNX select_last_index=0.
    return logits.astype(int).tolist(), int(np.argmax(logits))


def graph(w1, b1, w2, b2):
    c = lambda name, value: numpy_helper.from_array(value, name)
    initializers = [
        c("W1", w1),
        c("b1", b1),
        c("zero", np.array(0, dtype=np.int32)),
        c("divisor", np.array(1 << SHIFT, dtype=np.int32)),
        c("ceiling", np.array(127, dtype=np.int32)),
        c("W2", w2),
        c("b2", b2),
    ]
    nodes = [
        helper.make_node("MatMulInteger", ["x", "W1"], ["mm1"]),
        helper.make_node("Add", ["mm1", "b1"], ["acc"]),
        helper.make_node("Max", ["acc", "zero"], ["relu"]),
        helper.make_node("Div", ["relu", "divisor"], ["scaled"]),
        helper.make_node("Min", ["scaled", "ceiling"], ["saturated"]),
        helper.make_node("Cast", ["saturated"], ["hidden"], to=TensorProto.INT8),
        helper.make_node("MatMulInteger", ["hidden", "W2"], ["mm2"]),
        helper.make_node("Add", ["mm2", "b2"], ["logits"]),
        helper.make_node("ArgMax", ["logits"], ["class"], axis=1, keepdims=0),
    ]
    g = helper.make_graph(
        nodes,
        "maya2c_quantized_classifier",
        [helper.make_tensor_value_info("x", TensorProto.INT8, [1, INPUTS])],
        [
            helper.make_tensor_value_info("class", TensorProto.INT64, [1]),
            helper.make_tensor_value_info("logits", TensorProto.INT32, [1, CLASSES]),
        ],
        initializers,
    )
    model = helper.make_model(g, opset_imports=[helper.make_opsetid("", 17)])
    # Pinned low: a newer IR version is a reader the prover's tract may not have.
    model.ir_version = 8
    model.producer_name = "maya2c scripts/make_zkml_fixture.py"
    onnx.checker.check_model(model)
    return model


def write_unsupported(w1, b1, w2, b2):
    """Two models the importer must refuse, each one step from the real one.

    Close on purpose. An importer tested only against garbage would pass while
    accepting a model that is *almost* the circuit's -- which is the dangerous
    case, because the circuit would then prove a computation the file does not
    describe, under a model id that names the file.
    """
    # Relu where the circuit expects Max(acc, 0). Same function; a different
    # graph. The importer checks graphs, not functions, and says so.
    model = graph(w1, b1, w2, b2)
    relu = model.graph.node[2]
    relu.op_type = "Relu"
    del relu.input[1]
    onnx.checker.check_model(model)
    onnx.save(model, OUT / "unsupported-relu.onnx")

    # A zero point on the first MatMulInteger. The circuit has no zero points;
    # accepting this would silently drop one.
    model = graph(w1, b1, w2, b2)
    model.graph.initializer.append(
        numpy_helper.from_array(np.array(3, dtype=np.int8), "x_zero_point")
    )
    model.graph.node[0].input.append("x_zero_point")
    onnx.checker.check_model(model)
    onnx.save(model, OUT / "unsupported-zero-point.onnx")


def main():
    rng = np.random.default_rng(SEED)
    w1, b1, w2, b2 = weights(rng)

    # Enough inputs to reach every class and both sides of every ReLU and the
    # saturation ceiling, found by search rather than hoped for.
    per_class = {c: 0 for c in range(CLASSES)}
    cases = []
    probe = np.random.default_rng(SEED + 1)
    extremes = [[127] * INPUTS, [-128] * INPUTS, [0] * INPUTS]
    for x in extremes + probe.integers(-128, 128, size=(4000, INPUTS)).tolist():
        arr = np.array([x], dtype=np.int8)
        logits, cls = reference(arr, w1, b1, w2, b2)
        # The extremes always go in: they are the saturation and all-negative
        # cases. Beyond them, up to three inputs per class.
        if x in extremes or per_class[cls] < 3:
            cases.append({"x": x, "logits": logits[0], "class": cls})
            per_class[cls] += 1
        if min(per_class.values()) >= 3:
            break
    seen = {c for c, n in per_class.items() if n}
    assert seen == set(range(CLASSES)), f"inputs reached only classes {seen}"

    OUT.mkdir(parents=True, exist_ok=True)
    onnx.save(graph(w1, b1, w2, b2), OUT / "classifier.onnx")
    write_unsupported(w1, b1, w2, b2)
    (OUT / "classifier.expected.json").write_text(
        json.dumps({"shift": SHIFT, "cases": cases}, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(cases)} cases covering classes {sorted(seen)}")


if __name__ == "__main__":
    main()
