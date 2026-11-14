//! The wgpu backend. Compiled only under the `gpu` feature.
//!
//! Every failure is an error, never a verdict: no adapter, a device that
//! cannot be made or is lost, a dispatch that outlives the bounded wait, or an
//! output that does not carry this batch's nonce (threat model G-8, G-9).
//! After the first failure the backend stays unavailable for its lifetime.

use std::time::Duration;

use wgpu::util::DeviceExt as _;

use crate::error::VerifyError;
use crate::lattice::{
    LatticeBackend, LatticeInput, LatticeOutput, MAX_DISPATCH_ITEMS, WORDS_OUT, decode_outputs,
    encode_inputs,
};
use crate::shader;

/// How long the host waits for one dispatch before declaring the GPU
/// unavailable. A liveness bound, not a target. What keeps a dispatch inside
/// an operating system's GPU watchdog is [`MAX_DISPATCH_ITEMS`]; this wait is
/// longer so a software adapter (CI's lavapipe) is not misread as lost.
pub const DISPATCH_WAIT: Duration = Duration::from_secs(10);

/// What an adapter says about itself, for the harness's report.
#[derive(Clone, Debug)]
pub struct AdapterInfo {
    /// e.g. "NVIDIA GeForce RTX 5070 Laptop GPU".
    pub name: String,
    /// Vulkan, Dx12, Metal, Gl.
    pub backend: String,
    /// Driver name and version as the driver reports them.
    pub driver: String,
    /// Whether wgpu calls this a CPU (software) adapter.
    pub software: bool,
}

/// The wgpu lattice backend.
pub struct GpuLattice {
    info: AdapterInfo,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    next_nonce: u32,
    failed: bool,
}

impl GpuLattice {
    /// Every adapter wgpu can reach, by index.
    #[must_use]
    pub fn adapters() -> Vec<AdapterInfo> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        instance
            .enumerate_adapters(wgpu::Backends::PRIMARY)
            .iter()
            .map(|a| describe(&a.get_info()))
            .collect()
    }

    /// Opens adapter `index` and compiles the shader.
    ///
    /// # Errors
    ///
    /// [`VerifyError::NoAdapter`] for a missing adapter, [`VerifyError::Device`]
    /// if the device cannot be created.
    pub fn new(index: usize) -> Result<Self, VerifyError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let all = instance.enumerate_adapters(wgpu::Backends::PRIMARY);
        let adapter = all.get(index).ok_or(VerifyError::NoAdapter)?;
        let info = describe(&adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("maya-wgpu-verify"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .map_err(|e| VerifyError::Device(e.to_string()))?;

        // A shader the driver refuses is an error to report, not wgpu's
        // default panic.
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ml-dsa-65-lattice"),
            source: wgpu::ShaderSource::Wgsl(shader::source().into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lattice-layout"),
            entries: &[entry(0, None), entry(1, Some(true)), entry(2, Some(false))],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lattice-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("lattice-pipeline"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(shader::ENTRY_POINT),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(VerifyError::Device(error.to_string()));
        }
        Ok(Self {
            info,
            device,
            queue,
            pipeline,
            layout,
            next_nonce: 1,
            failed: false,
        })
    }

    /// The adapter this backend runs on.
    #[must_use]
    pub fn info(&self) -> &AdapterInfo {
        &self.info
    }

    fn dispatch(&mut self, inputs: &[LatticeInput]) -> Result<Vec<LatticeOutput>, VerifyError> {
        let count =
            u32::try_from(inputs.len()).map_err(|_| VerifyError::BatchTooLarge(inputs.len()))?;
        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.wrapping_add(1).max(1);

        let job = [count, nonce, 0, 0];
        let job_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("job"),
                contents: bytemuck::cast_slice(&job[..]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let input_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("lattice-inputs"),
                contents: bytemuck::cast_slice(&encode_inputs(inputs)),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let out_bytes = (inputs.len() * WORDS_OUT * 4) as u64;
        // Fresh buffers per dispatch, so a result can only be this batch's;
        // the nonce check below is the backstop if that ever changes.
        let output = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lattice-outputs"),
            size: out_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lattice-staging"),
            size: out_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lattice-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: job_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lattice"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("lattice-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(count, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &staging, 0, out_bytes);
        self.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(DISPATCH_WAIT),
            })
            .map_err(|e| match e {
                wgpu::PollError::Timeout => VerifyError::Timeout,
                other => VerifyError::Device(other.to_string()),
            })?;
        rx.recv_timeout(DISPATCH_WAIT)
            .map_err(|_| VerifyError::Timeout)?
            .map_err(|e| VerifyError::Device(e.to_string()))?;

        let data = slice.get_mapped_range();
        let words: &[u32] = bytemuck::cast_slice(&data);
        let decoded = decode_outputs(words, nonce, inputs.len());
        drop(data);
        staging.unmap();
        decoded
    }
}

impl LatticeBackend for GpuLattice {
    fn name(&self) -> String {
        format!(
            "{} ({}, driver {})",
            self.info.name, self.info.backend, self.info.driver
        )
    }

    fn run(&mut self, inputs: &[LatticeInput]) -> Result<Vec<LatticeOutput>, VerifyError> {
        if self.failed {
            return Err(VerifyError::Device(
                "marked unavailable after an earlier failure".to_owned(),
            ));
        }
        if inputs.len() > MAX_DISPATCH_ITEMS {
            return Err(VerifyError::BatchTooLarge(inputs.len()));
        }
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let result = self.dispatch(inputs);
        self.failed = result.is_err();
        result
    }
}

fn describe(info: &wgpu::AdapterInfo) -> AdapterInfo {
    AdapterInfo {
        name: info.name.clone(),
        backend: format!("{:?}", info.backend),
        driver: format!("{} {}", info.driver, info.driver_info),
        software: info.device_type == wgpu::DeviceType::Cpu,
    }
}

fn entry(binding: u32, storage_read_only: Option<bool>) -> wgpu::BindGroupLayoutEntry {
    let ty = match storage_read_only {
        None => wgpu::BufferBindingType::Uniform,
        Some(read_only) => wgpu::BufferBindingType::Storage { read_only },
    };
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
