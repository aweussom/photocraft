//! #4831: the app builds the compositor on a thread of its own at launch and composites with it
//! on the UI thread. That holds only while a `Compositor` is `Send` and one built on another
//! thread renders as one built here. Skips without an adapter, or on one the compositor refuses.

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, Size};
use photocraft_geom::Rect;
use photocraft_gpu::{Compositor, Unsupported, render_to_vec};

use crate::GPU_LOCK;

/// A deterministic 48x32 RGBA8 document with one noisy, half-transparent layer.
fn doc() -> Document {
    let rect = Rect::from_xywh(0, 0, 48, 32);
    let mut d = Document::new("off-thread", Size::new(48, 32), ColorMode::Rgb, SampleType::U8);
    let mut layer = Layer::raster("noise", PixelFormat::RGBA8);
    let n = rect.width() as usize * rect.height() as usize;
    let data: Vec<f32> = (0..n * 4).map(|i| if i % 4 == 3 { 0.5 + ((i * 7919) % 101) as f32 / 202.0 } else { ((i * 2_654_435) % 997) as f32 / 997.0 }).collect();
    layer.surface_mut().expect("raster layer").write_region(rect, &data);
    d.layers.push(layer);
    d
}

fn assert_send<T: Send>() {}

#[test]
fn a_compositor_built_on_another_thread_renders_on_this_one() {
    assert_send::<Result<Compositor, Unsupported>>();
    let _lock = GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())) else {
        eprintln!("skipping: no adapter");
        return;
    };
    let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) else {
        eprintln!("skipping: no device");
        return;
    };
    let acc = Compositor::preferred_acc_format(&adapter);
    let on = device.clone();
    let build = std::thread::Builder::new().name("gpu-compositor".into()).spawn(move || Compositor::try_new_with_format(&on, acc)).expect("spawn");
    let mut comp = match build.join().expect("the build thread panicked") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let d = doc();
    let gpu = render_to_vec(&mut comp, &device, &queue, &d, d.bounds()).expect("render with a compositor built on another thread");
    let cpu = photocraft_compose::render(&d, d.bounds());
    assert_eq!(gpu.len(), cpu.px.len());
    let worst = gpu.iter().zip(&cpu.px).flat_map(|(g, c)| (0..4).map(move |k| (g[k] * g[3] - c[k] * c[3]).abs())).fold(0.0f32, f32::max);
    assert!(worst <= 2.0 / 255.0, "max premultiplied difference {:.2}/255", worst * 255.0);
}
