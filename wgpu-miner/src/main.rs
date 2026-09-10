//! The GPU miner binary.
//!
//! ```text
//! wgpu-miner --list
//! wgpu-miner --device 0 --workgroup-size 64 --rpc http://127.0.0.1:8545
//! ```
//!
//! Built without the `gpu` feature this compiles to a binary that can list
//! nothing and mine nothing, and says so. That is deliberate: the crate has to
//! build on a runner with no adapter, and a binary that pretended otherwise
//! would fail at dispatch instead of at startup.

use clap::Parser;

/// Cross-platform GPU miner for Maya2C's hashimoto proof of work.
#[derive(Parser, Debug)]
#[command(name = "wgpu-miner", version, about)]
struct Args {
    /// List the adapters wgpu can reach, with their limits, and exit.
    ///
    /// Run this first. The dataset chunking depends on
    /// `maxStorageBufferBindingSize`, which varies by backend and driver, and
    /// designing against a guessed value is how a miner reads past a binding
    /// on somebody else's machine.
    #[arg(long)]
    list: bool,

    /// Adapter index to mine on, as reported by `--list`.
    #[arg(long, default_value_t = 0)]
    device: usize,

    /// Invocations per workgroup.
    ///
    /// 64 by default: a multiple of both a 32-wide NVIDIA warp and a 64-wide
    /// AMD wavefront, so no lane idles on either. The best value is a property
    /// of the adapter rather than of the algorithm, which is why it is a flag.
    #[arg(long, default_value_t = 64)]
    workgroup_size: u32,

    /// Nonces per dispatch.
    ///
    /// Larger amortises submission overhead and costs readback memory:
    /// each nonce returns 128 bytes of mix.
    #[arg(long, default_value_t = 65_536)]
    batch: u32,

    /// JSON-RPC endpoint of the node to take work from and submit to.
    #[arg(long, env = "MAYA_RPC", default_value = "http://127.0.0.1:8545")]
    rpc: String,

    /// Measure this adapter's hash rate and exit, without mining.
    ///
    /// The only mode that currently does real, sustained GPU work: it
    /// dispatches the validated compute path in a loop and reports what the
    /// card manages. Use it to size a rig, and to check that a telemetry
    /// export reaches a collector.
    #[arg(long)]
    benchmark: bool,

    /// Seconds to run `--benchmark` for.
    #[arg(long, default_value_t = 30)]
    benchmark_seconds: u64,

    /// Collector to post the measured hash rate to.
    ///
    /// Off unless given. A miner that phoned home by default would be a miner
    /// that phoned home by default.
    #[arg(long, env = "MAYA_TELEMETRY")]
    telemetry: Option<String>,

    /// Name this rig reports under.
    ///
    /// Self-assigned and unverified — it exists so repeat reports replace
    /// rather than accumulate, and for nothing else.
    #[arg(long, default_value = "wgpu-miner")]
    telemetry_id: String,
}

fn main() {
    let args = Args::parse();

    #[cfg(not(feature = "gpu"))]
    {
        let _ = &args;
        eprintln!(
            "this binary was built without the `gpu` feature and cannot mine.\n\
             \n\
             The feature is off by default so that `cargo build --workspace`\n\
             succeeds on a machine with no adapter and no graphics driver —\n\
             the same rule `cuda-miner`'s `cuda` feature follows.\n\
             \n\
             Rebuild with:  cargo build -p maya-wgpu-miner --features gpu"
        );
        std::process::exit(1);
    }

    #[cfg(feature = "gpu")]
    {
        use maya_wgpu_miner::gpu;

        let adapters = match gpu::adapters() {
            Ok(found) => found,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        };

        if args.list {
            println!(
                "{:<3} {:<38} {:<8} {:>12} {:>12}",
                "idx", "adapter", "backend", "max bind", "max buffer"
            );
            for adapter in &adapters {
                println!(
                    "{:<3} {:<38} {:<8} {:>10} MiB {:>10} MiB",
                    adapter.index,
                    adapter.name,
                    adapter.backend,
                    adapter.max_storage_binding / (1024 * 1024),
                    adapter.max_buffer_size / (1024 * 1024),
                );
            }

            // The number that decides whether a mainnet dataset can be bound
            // at all. Printed here rather than discovered at dispatch.
            const MAINNET_DATASET: u64 = 4 * 1024 * 1024 * 1024;
            println!();
            println!(
                "Chunks needed for the 4 GiB mainnet dataset (shader declares {}):",
                gpu::CHUNKS
            );
            for adapter in &adapters {
                let chunks = adapter.chunks_for(MAINNET_DATASET);
                let verdict = if chunks <= gpu::CHUNKS as u64 {
                    "ok"
                } else {
                    "TOO LARGE"
                };
                println!("  [{}] {chunks} — {verdict}", adapter.index);
            }
            return;
        }

        println!(
            "wgpu-miner: device {}, workgroup {}, batch {}, rpc {}",
            args.device, args.workgroup_size, args.batch, args.rpc
        );
        eprintln!(
            "\nWork distribution is not implemented: this binary can enumerate\n\
             adapters and run the validated compute path, but it does not yet\n\
             poll `get_mining_candidate` or submit blocks. Use --list."
        );
        std::process::exit(1);
    }
}
