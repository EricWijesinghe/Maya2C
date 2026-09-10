//! The wgpu pipeline: adapters, buffers, dispatch, readback.
//!
//! Compiled only under the `gpu` feature. With no feature selected this crate
//! is the host pipeline and the CPU reference, which is what keeps
//! `cargo build --workspace` green on a runner with no adapter — the same rule
//! `cuda-miner`'s `cuda` feature follows.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use custom_l1_node::crypto::dag::MIX_WORDS;
use maya_telemetry::meter::HashMeter;
use wgpu::util::DeviceExt as _;

use crate::error::MinerError;
use crate::reference::{ITEM_WORDS, PAGE_WORDS};

/// Dataset chunks the shader declares.
///
/// Four bindings, because WGSL has no array-of-bindings and they must be
/// written out individually. Four against a 1 GiB binding limit covers the
/// 4 GiB mainnet dataset; an adapter that would need more is refused at
/// startup rather than silently mining a truncated dataset.
pub const CHUNKS: usize = 4;

/// The uniform the shader reads its job from.
///
/// `repr(C)` and 16-byte sized: WGSL uniform buffers require the struct to be
/// laid out predictably, and four `u32` is exactly one 16-byte block, so no
/// padding has to be reasoned about.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Job {
    pages: u32,
    nonce_count: u32,
    chunk_words: u32,
    chunk_count: u32,
}

/// What an adapter reports about itself.
#[derive(Clone, Debug)]
pub struct AdapterInfo {
    /// Index in the enumeration, which is what `--device` selects.
    pub index: usize,
    /// Human name, e.g. "NVIDIA GeForce RTX 5070 Laptop GPU".
    pub name: String,
    /// Backend: Vulkan, Dx12, Metal, Gl.
    pub backend: String,
    /// Largest single storage buffer binding, in bytes.
    pub max_storage_binding: u64,
    /// Largest buffer of any kind, in bytes.
    pub max_buffer_size: u64,
}

impl AdapterInfo {
    /// Chunks a dataset of `bytes` would need on this adapter.
    ///
    /// Reported rather than assumed. The limit varies by backend and driver,
    /// and designing against a guessed number is how a miner ends up reading
    /// past a binding on somebody else's machine.
    #[must_use]
    pub fn chunks_for(&self, bytes: u64) -> u64 {
        if self.max_storage_binding == 0 {
            return u64::MAX;
        }
        bytes.div_ceil(self.max_storage_binding)
    }
}

/// Enumerates every adapter wgpu can reach.
///
/// # Errors
///
/// [`MinerError::NoAdapter`] if there are none.
pub fn adapters() -> Result<Vec<AdapterInfo>, MinerError> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let found: Vec<_> = instance.enumerate_adapters(wgpu::Backends::all());

    if found.is_empty() {
        return Err(MinerError::NoAdapter);
    }

    Ok(found
        .iter()
        .enumerate()
        .map(|(index, adapter)| {
            let info = adapter.get_info();
            let limits = adapter.limits();
            AdapterInfo {
                index,
                name: info.name,
                backend: format!("{:?}", info.backend),
                max_storage_binding: u64::from(limits.max_storage_buffer_binding_size),
                max_buffer_size: limits.max_buffer_size,
            }
        })
        .collect())
}

/// A device with the shader compiled and the dataset resident.
pub struct GpuMiner {
    /// Hashes this device has completed, as a rate.
    ///
    /// Owned by the miner rather than passed in, because the number worth
    /// reporting is per-adapter: a two-card machine that reported one combined
    /// figure could not tell an operator which card died.
    meter: Arc<HashMeter>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    chunks: Vec<wgpu::Buffer>,
    chunk_words: u32,
    chunk_count: u32,
    pages: u32,
    workgroup_size: u32,
}

impl GpuMiner {
    /// Builds a miner on adapter `index` with `dataset` resident.
    ///
    /// # Errors
    ///
    /// [`MinerError::NoAdapter`] for a bad index, [`MinerError::DatasetTooLarge`]
    /// if the dataset needs more than [`CHUNKS`] bindings, or
    /// [`MinerError::Device`] if the device cannot be created.
    pub fn new(
        index: usize,
        dataset: &[u32],
        pages: u32,
        workgroup_size: u32,
    ) -> Result<Self, MinerError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let all: Vec<_> = instance.enumerate_adapters(wgpu::Backends::all());
        let adapter = all.get(index).ok_or(MinerError::NoAdapter)?;

        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("maya-wgpu-miner"),
            required_features: wgpu::Features::empty(),
            required_limits: limits.clone(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .map_err(|e| MinerError::Device(e.to_string()))?;

        // Split the dataset across at most CHUNKS bindings, on a page
        // boundary so no page ever straddles two buffers. A straddling page
        // would need the shader to stitch a read across bindings, which is the
        // kind of code that works on one backend.
        let binding_words =
            (limits.max_storage_buffer_binding_size as usize / 4) / PAGE_WORDS * PAGE_WORDS;
        if binding_words == 0 {
            return Err(MinerError::DatasetTooLarge {
                needed: u64::MAX,
                available: CHUNKS as u64,
            });
        }

        let needed = dataset.len().div_ceil(binding_words);
        if needed > CHUNKS {
            return Err(MinerError::DatasetTooLarge {
                needed: needed as u64,
                available: CHUNKS as u64,
            });
        }

        let mut chunks = Vec::with_capacity(CHUNKS);
        for slot in 0..CHUNKS {
            let start = (slot * binding_words).min(dataset.len());
            let end = ((slot + 1) * binding_words).min(dataset.len());
            // Empty bindings are not allowed, so an unused slot gets one word.
            // The shader never reads it: `chunk_count` tells it how many are
            // real, and `read_word` clamps regardless.
            let slice: &[u32] = if start < end {
                &dataset[start..end]
            } else {
                &[0u32]
            };

            chunks.push(
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("dataset-chunk"),
                    contents: bytemuck::cast_slice(slice),
                    usage: wgpu::BufferUsages::STORAGE,
                }),
            );
        }

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hashimoto"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/hashimoto.wgsl").into()),
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hashimoto-layout"),
            entries: &bind_group_entries(),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hashimoto-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("hashimoto-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("mine"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            layout,
            chunks,
            chunk_words: binding_words as u32,
            chunk_count: needed as u32,
            pages,
            workgroup_size,
            meter: Arc::new(HashMeter::new()),
        })
    }

    /// This device's hash rate meter.
    ///
    /// Shared rather than copied, so a telemetry reporter reads the same
    /// counter the dispatch loop writes.
    #[must_use]
    pub fn meter(&self) -> Arc<HashMeter> {
        Arc::clone(&self.meter)
    }

    /// Computes the mix for each seed, in one dispatch.
    ///
    /// # Errors
    ///
    /// [`MinerError::Readback`] if the result buffer cannot be mapped.
    pub fn mixes(&self, seeds: &[[u32; ITEM_WORDS]]) -> Result<Vec<[u32; MIX_WORDS]>, MinerError> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }

        // Recorded before the dispatch rather than after. A dispatch that
        // fails readback still consumed the GPU time, and a meter that only
        // counted successes would report a card that is erroring as a card
        // that is idle — the opposite of what an operator needs to see.
        self.meter.record(seeds.len() as u64);

        let flat: Vec<u32> = seeds.iter().flat_map(|s| s.iter().copied()).collect();
        let seed_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("seeds"),
                contents: bytemuck::cast_slice(&flat),
                usage: wgpu::BufferUsages::STORAGE,
            });

        let out_bytes = (seeds.len() * MIX_WORDS * 4) as u64;
        let out_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mixes"),
            size: out_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // A separate buffer for readback: a STORAGE buffer cannot also be
        // MAP_READ on most backends.
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mixes-staging"),
            size: out_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let job = Job {
            pages: self.pages,
            nonce_count: seeds.len() as u32,
            chunk_words: self.chunk_words,
            chunk_count: self.chunk_count,
        };
        let job_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("job"),
                contents: bytemuck::bytes_of(&job),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hashimoto-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: job_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: seed_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: out_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.chunks[0].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.chunks[1].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.chunks[2].as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: self.chunks[3].as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mine"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("mine-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            // Rounded up to whole workgroups. The shader returns early past
            // `nonce_count`, so the surplus invocations cost a branch and
            // nothing else.
            let groups = (seeds.len() as u32).div_ceil(self.workgroup_size);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out_buffer, 0, &staging, 0, out_bytes);
        self.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            // Wait for the most recent submission rather than a named index:
            // there is exactly one in flight per call.
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|e| MinerError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| MinerError::Readback(e.to_string()))?
            .map_err(|e| MinerError::Readback(e.to_string()))?;

        let data = slice.get_mapped_range();
        let words: &[u32] = bytemuck::cast_slice(&data);
        // `as_chunks` rather than `chunks_exact`: the chunk size is a constant,
        // so this yields `[u32; MIX_WORDS]` directly and the copy disappears.
        let (mixes, _) = words.as_chunks::<MIX_WORDS>();
        let out = mixes.to_vec();
        drop(data);
        staging.unmap();

        Ok(out)
    }
}

fn bind_group_entries() -> [wgpu::BindGroupLayoutEntry; 3 + CHUNKS] {
    let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };

    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        storage(1, true),
        storage(2, false),
        storage(3, true),
        storage(4, true),
        storage(5, true),
        storage(6, true),
    ]
}
