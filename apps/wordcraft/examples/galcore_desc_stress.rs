//! Stress-tests the descriptor pool allocator against quirky Vulkan drivers
//! (Vivante galcore on Phytium ftg340 in particular), which return
//! VK_ERROR_OUT_OF_POOL_MEMORY from pools that still report free capacity.
//!
//! Run with the same environment as ~/bin/wordcraft (see README at the bottom).
//!
//! Phase 1 keeps thousands of bind groups alive from one layout, forcing many
//! pool creations and growth; phase 2 creates/drops rapidly to exercise the
//! free path; dropping the device then exercises the pool teardown asserts
//! that the vendored wgpu-hal patch relaxed.

use std::sync::Arc;

use eframe::egui_wgpu::wgpu;

/// Minimal logger so wgpu-hal's "poisoning pool and retrying" warnings show up
/// without pulling in env_logger.
struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            eprintln!("{} [{}] {}", record.level(), record.target(), record.args());
        }
    }
    fn flush(&self) {}
}

fn main() {
    log::set_boxed_logger(Box::new(StderrLogger)).ok();
    log::set_max_level(log::LevelFilter::Warn);

    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = wgpu::Backends::VULKAN;
    let instance = wgpu::Instance::new(instance_desc);
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .expect("no Vulkan adapter found");
    let info = adapter.get_info();
    println!("adapter: {} ({:?})", info.name, info.backend);

    // Same clamp as main.rs: adopt the adapter's own limits when wgpu's
    // defaults exceed the hardware (ftg340 caps compute workgroup size at 128).
    let mut limits = wgpu::Limits::default();
    if !limits.check_limits(&adapter.limits()) {
        limits = adapter.limits();
    }
    let (device, _queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("galcore-stress"),
        required_features: wgpu::Features::empty(),
        required_limits: limits,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .expect("device creation failed");
    device.on_uncaptured_error(Arc::new(|err| {
        eprintln!("UNCAPTURED ERROR: {err:?}");
        std::process::exit(2);
    }));

    let started = std::time::Instant::now();

    // ---- Phase 1: one layout, thousands of long-lived bind groups → many pools.
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
        label: Some("stress-layout"),
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut buffers: Vec<wgpu::Buffer> = Vec::new();
    let mut groups: Vec<wgpu::BindGroup> = Vec::new();
    const PHASE1: usize = 3000;
    for i in 0..PHASE1 {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: None,
        });
        buffers.push(buf);
        groups.push(group);
        if (i + 1) % 1000 == 0 {
            println!("phase 1: {}/{} bind groups alive ({:?})", i + 1, PHASE1, started.elapsed());
        }
    }

    // ---- Phase 2: rapid create/drop churn from several layouts (free path).
    for round in 0..10 {
        for variant in 0..4 {
            let churn_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: variant != 0,
                            min_binding_size: wgpu::BufferSize::new(16 * (variant as u64 + 1)),
                        },
                        count: None,
                    },
                ],
                label: Some("churn-layout"),
            });
            for _ in 0..64 {
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 128,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &churn_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buf.as_entire_binding(),
                    }],
                    label: None,
                });
                drop(group);
                buffers.push(buf);
            }
        }
        if (round + 1) % 5 == 0 {
            println!("phase 2: round {} done ({:?})", round + 1, started.elapsed());
        }
    }

    // ---- Phase 3: wide layouts (many descriptors per set) — the shape real
    // document rendering uses; wide pools multiplied by 512-set capacity are
    // where galcore reportedly returns OOPM with room still available.
    {
        const WIDE_SAMPLERS: u32 = 8;
        const WIDE_IMAGES: u32 = 128;
        let mut wide_entries = Vec::new();
        for b in 0..WIDE_IMAGES {
            wide_entries.push(wgpu::BindGroupLayoutEntry {
                binding: b,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
        }
        for b in 0..WIDE_SAMPLERS {
            wide_entries.push(wgpu::BindGroupLayoutEntry {
                binding: WIDE_IMAGES + b,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let wide_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &wide_entries,
            label: Some("wide-layout"),
        });
        let mut views = Vec::new();
        for i in 0..8 {
            let t = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 32,
                    height: 32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            views.push(t.create_view(&wgpu::TextureViewDescriptor::default()));
        }
        let mut wide_groups = Vec::new();
        for i in 0..300 {
            let entries: Vec<wgpu::BindGroupEntry> = (0..WIDE_IMAGES)
                .map(|b| wgpu::BindGroupEntry {
                    binding: b,
                    resource: wgpu::BindingResource::TextureView(&views[i % views.len()]),
                })
                .chain((0..WIDE_SAMPLERS).map(|b| wgpu::BindGroupEntry {
                    binding: WIDE_IMAGES + b,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                }))
                .collect();
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &wide_layout,
                entries: &entries,
                label: None,
            });
            wide_groups.push(group);
            if (i + 1) % 100 == 0 {
                println!("phase 3: {}/300 wide bind groups ({:?})", i + 1, started.elapsed());
            }
        }
        drop(wide_groups);
        drop(wide_layout);
        drop(views);
        println!("phase 3 done ({:?})", started.elapsed());
    }

    // ---- Tear down everything; device drop runs the pool teardown asserts.
    drop(groups);
    drop(layout);
    drop(buffers);
    device.poll(wgpu::PollType::wait_indefinitely()).expect("poll failed");
    drop(device);

    println!(
        "PASS: descriptor stress completed in {:?} without panic",
        started.elapsed()
    );
}
