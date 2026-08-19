//! The nested backend — omoya running as a window inside an existing session.
//!
//! This is the DEV LOOP host, not a shipping surface: `theory/OMOYA.md` M2 uses
//! it because it needs no DRM device, no root and no VT, so the question *"does
//! omoya composite at all?"* can be answered without risking the machine's only
//! console. M4 is the same compositor on `backend_drm`.
//!
//! The renderer here is whatever `backend_winit` provides (GLES). That is **not**
//! a commitment against OMOYA.md §5a's pixman-first decision, which is about the
//! **scanout** path on real hardware — nested rendering does not scan out at
//! all, it hands a buffer to the host compositor.

use std::time::Duration;

use smithay::{
    backend::{
        renderer::{
            damage::OutputDamageTracker, element::surface::WaylandSurfaceRenderElement,
            gles::GlesRenderer,
        },
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Rectangle, Transform},
};

use crate::{CalloopData, state::Omoya, theme};

pub fn init_winit(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
) -> Result<(), Box<dyn std::error::Error>> {
    let display_handle = &mut data.display_handle;
    let state = &mut data.state;

    let (mut backend, winit) = winit::init()?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "omoya-nested".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "pleme-io".into(),
            model: "omoya".into(),
        },
    );
    let _global = output.create_global::<Omoya>(display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.space.map_output(&output, (0, 0));

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    // Clients find us here. Set before any client is spawned.
    //
    // SAFETY: single-threaded startup, before the event loop runs.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", &state.socket_name);
    }

    // ★ The background is Nord, sourced from `irodori` via `crate::theme` —
    // there is no colour literal in this file.
    //
    // The ENCODING is asked of the surface rather than assumed. `PixelFormat`
    // is what smithay itself logs at startup ("Selected color format: … srgb:
    // false"), and it is reachable through the public `egl_surface()` accessor,
    // so the compositor reads the same fact the log prints instead of carrying
    // a second belief about it. On this nested X11 path it is false; on a DRM
    // scanout target it is usually true — see `theme::background_for_surface`
    // for the measured pixel that made this a query.
    let format_is_srgb = backend.egl_surface().pixel_format().srgb;
    let background = theme::background_for_surface(format_is_srgb);
    tracing::info!(
        srgb_framebuffer = format_is_srgb,
        clear = ?background,
        "seat background resolved"
    );

    event_loop
        .handle()
        .insert_source(winit, move |event, _, data| {
            let display = &mut data.display_handle;
            let state = &mut data.state;

            match event {
                WinitEvent::Resized { size, .. } => {
                    output.change_current_state(
                        Some(Mode {
                            size,
                            refresh: 60_000,
                        }),
                        None,
                        None,
                        None,
                    );
                }
                WinitEvent::Input(event) => state.process_input_event(event),
                WinitEvent::Redraw => {
                    let size = backend.window_size();
                    let damage = Rectangle::from_size(size);

                    {
                        let (renderer, mut framebuffer) =
                            backend.bind().expect("could not bind the winit surface");
                        smithay::desktop::space::render_output::<
                            _,
                            WaylandSurfaceRenderElement<GlesRenderer>,
                            _,
                            _,
                        >(
                            &output,
                            renderer,
                            &mut framebuffer,
                            1.0,
                            0,
                            [&state.space],
                            &[],
                            &mut damage_tracker,
                            background,
                        )
                        .expect("render_output failed");
                    }
                    backend.submit(Some(&[damage])).expect("submit failed");

                    state.space.elements().for_each(|window| {
                        window.send_frame(
                            &output,
                            state.start_time.elapsed(),
                            Some(Duration::ZERO),
                            |_, _| Some(output.clone()),
                        );
                    });

                    state.space.refresh();
                    state.popups.cleanup();
                    let _ = display.flush_clients();

                    backend.window().request_redraw();
                }
                WinitEvent::CloseRequested => {
                    state.loop_signal.stop();
                }
                // `WinitEvent` has exactly five variants in smithay 0.7 —
                // Resized, Focus, Input, CloseRequested, Redraw. Matching them
                // all EXHAUSTIVELY (rather than with a `_` arm) is deliberate:
                // a new variant in a future smithay should be a compile error
                // here, not something omoya silently ignores.
                WinitEvent::Focus(_) => {}
            }
        })?;

    Ok(())
}
