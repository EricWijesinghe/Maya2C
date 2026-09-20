//! The first trained network. **Generated — do not edit.**
//!
//! `cargo run --release -p maya-neural-gas-trainer` writes this file and
//! `-- --check` fails if a fresh run differs. Trained on the synthetic
//! simulator in `bins/neural-gas-trainer/src/simulator.rs`: every figure below
//! is about that simulator, not about any real chain.
//!
//! - training seed `0x4d41594132430001`, 40000 blocks, 40000 labelled, 12 epochs
//! - float gain MSE 0.141388; largest quantization error 8 bps
//! - evaluation seed `0x4d41594132430002`, 20000 blocks, closed loop:
//!
//! | Rule | mean \|size − target\| / target | mean \|Δfee\| / fee | blocks at cap | envelope violations |
//! |---|---|---|---|---|
//! | linear | 0.1427 | 0.0174 | 2.41% | 0 |
//! | neural | 0.1394 | 0.0207 | 1.85% | 0 |

use super::Model;

/// The first trained network.
pub const MODEL_V1: Model = Model {
    hidden_weights: [
        [-9886, 2682, 275, -672, 803, -982],
        [2032, -751, -247, 403, -1545, -2139],
        [3575, -3052, -297, -964, 1417, 1242],
        [4767, -1269, 2054, -241, -1035, 1536],
        [-2895, 490, 409, 2316, -843, 1110],
        [-23, 1898, -1556, 1042, -385, 519],
        [-3889, -587, 2436, 2186, 1029, 67],
        [3073, -747, -571, 1029, -2543, 95],
        [-281, -1181, 1809, 1097, 1253, -508],
        [-1878, 370, -1364, -1581, -1730, 1307],
        [-10821, 3042, 654, -796, 427, -970],
        [-246, 1400, -626, -2226, 1133, 1611],
        [-371, -1876, 1154, -1698, -163, 189],
        [-955, 1724, 2321, 1208, 718, -1447],
        [-28, -1388, -287, 1522, -391, -134],
        [79, -1593, 1196, 1471, 928, 320],
    ],
    hidden_bias: [470123870, -51243857, -35436286, -218502440, 78944535, -618847, 116573887, -100165439, 8540315, 26843546, 597607604, 19101203, 26843546, 2535002, 18460456, 56628885],
    output_weights: [12178, 412, -3154, 2472, -2014, 893, -3040, 1148, 13, 175, -11694, 1837, -220, 1455, 136, -915],
    output_bias: 431306606,
};
