//! The wgpu compositor can be built on a thread of its own and used from another (#4831).
//!
//! `GpuCanvas` starts the build at start-up on a worker so that the first document of a session
//! does not wait one to a few seconds for the pipelines on the UI thread. That holds only while a
//! `Compositor` built elsewhere is `Send` and composites correctly afterwards; a non-`Send` field
//! would fail the compile, and a thread-bound resource would fail the render here.
//!
//! Skips without an adapter, or on one the compositor refuses (then the app uses the CPU
//! compositor and nothing is built on any thread).

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, Size};
use photocraft_geom::Rect;
use photocraft_gpu::{Compositor, render_to_vec};

/// A deterministic 48x32 RGBA8 document with one noisy layer.
fn doc() -> Document {
    let rect = Rect::from_xywh(0, 0, 48, 32);
    let mut d = Document::new("off-thread", Size::new(48, 32), ColorMode::Rgb, SampleType::U8);
    let mut layer = Layer::raster("noise", PixelFormat::RGBA8);
    let n = rect.width() as usize * rect.height() as usize;
    let data: Vec<f32> = (0..n * 4).map(|i| if i % 4 == 3 { 0.5 + ((i * 7919) % 101) as f32 / 202.0 } else { ((i * 2_654_435) % 997) as f32 / 997.0 }).collect();
    layer.surface_mut().unwrap().write_region(rect, &data);
    d.layers.push(layer);
    d
}

fn assert_send<T: Send>() {}

#[test]
fn built_on_a_worker_renders_on_the_caller() {
    assert_send::<Result<Compositor, photocraft_gpu::Unsupported>>();
    let adapter = match pollster::block_on(wgpu::Instance::default().request_adapter(&wgpu::RequestAdapterOptions::default())) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipping compositor off-thread test: no adapter ({e})");
            return;
        }
    };
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) else {
        eprintln!("skipping compositor off-thread test: no device");
        return;
    };
    let acc = Compositor::preferred_acc_format(&adapter);
    let worker_device = device.clone();
    let worker = std::thread::Builder::new().name("gpu-compositor".into()).spawn(move || Compositor::try_new_with_format(&worker_device, acc)).expect("spawn");
    let mut comp = match worker.join().expect("the build thread panicked") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping compositor off-thread test: {e}");
            return;
        }
    };
    let d = doc();
    let gpu = render_to_vec(&mut comp, &device, &queue, &d, d.bounds()).expect("render with a compositor built on another thread");
    let cpu = photocraft_compose::render(&d, d.bounds());
    assert_eq!(gpu.len(), cpu.px.len());
    let worst = gpu.iter().zip(&cpu.px).flat_map(|(g, c)| (0..4).map(move |k| (g[k] * g[3] - c[k] * c[3]).abs())).fold(0.0f32, f32::max);
    assert!(worst <= 2.0 / 255.0, "max premultiplied diff {:.2}/255", worst * 255.0);
}
