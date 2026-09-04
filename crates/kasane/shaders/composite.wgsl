// The compositor's entire shader surface — three entry points, one module.
//
// ── ★ WHY THIS FILE IS WGSL AND NOT GLSL ─────────────────────────────────
// GLSL would mean glslang or shaderc: C libraries, compiled at build time,
// exactly the linkage `docs/KASANE.md` §4 exists to refuse. WGSL is compiled
// here by `naga`, which is pure Rust and already in the fleet's closure
// (garasu and engawa both reach it through wgpu 25). It runs in `build.rs`,
// so nothing parses a shader at seat startup and naga is absent from the
// shipped closure entirely.
//
// ── ★ NO VERTEX BUFFER ───────────────────────────────────────────────────
// The quad is generated from `vertex_index` and a push constant. A compositor
// draws one rectangle per surface with different numbers, not different
// geometry, so a vertex buffer would be four corners re-uploaded to say the
// same thing every frame. This costs one push-constant write per draw and no
// allocation, no binding, no staging.
//
// ── ★ PREMULTIPLIED ALPHA IS THE WAYLAND CONTRACT ────────────────────────
// `wl_shm` and `zwp_linux_dmabuf_v1` both deliver ARGB8888 with premultiplied
// alpha. So the fragment output is premultiplied too and the blend equation
// is ONE * ONE_MINUS_SRC_ALPHA — never SRC_ALPHA * ONE_MINUS_SRC_ALPHA, which
// would darken every translucent edge on the seat by multiplying alpha twice.
// The opacity knob multiplies all four channels for the same reason.

struct Params {
    /// Destination rectangle in clip space: `xy` = top-left, `zw` = size.
    /// Pre-transformed on the CPU, so the shader has no matrix to apply.
    dst: vec4<f32>,
    /// Source rectangle in texture UV: `xy` = top-left, `zw` = size.
    /// Carrying the source rect here is what makes cropping and scaling one
    /// draw rather than a pre-pass.
    src: vec4<f32>,
    /// Colour for the solid entry point; `a` is the opacity multiplier the
    /// textured path uses. One struct for both, so the pipeline layout is one
    /// layout and a mismatched push-constant range cannot arise.
    tint: vec4<f32>,
};

var<push_constant> params: Params;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Corners of a triangle strip, in the order Vulkan issues them for a 4-vertex
// non-indexed draw: (0,0) (1,0) (0,1) (1,1).
// ── ★★ THE Y NEGATION IS LOAD-BEARING, AND IT WAS MEASURED ──────────────
// WGSL's normalised device coordinates put +Y UP, following WebGPU. Vulkan's
// put +Y DOWN. naga emits the shader as written and does not reconcile the
// two — wgpu, its usual host, compensates by setting a NEGATIVE-HEIGHT
// viewport, and kasane does not.
//
// So without this negation a rectangle at pixel y=0 is drawn at the BOTTOM of
// the framebuffer, and every surface on the seat is upside down.
//
// ★ IT SURVIVED FOUR TESTS. A full-screen quad looks identical either way; the
// solid-placement test only varied X; the source-rectangle test only varied
// `src.x`; and the dmabuf and upload tests used single-colour textures. It was
// found by a partial texture update landing in the wrong corner, and
// `a_rect_at_pixel_y_zero_is_drawn_at_the_top_of_the_framebuffer` now pins it
// directly.
//
// ★ ONE NEGATION FIXES BOTH position AND uv, because they share `corner`: the
// vertex with `v = 0` (the texture's first row) is the one that moves to the
// top of the screen.
//
// The alternative is a negative-height viewport, which is the idiomatic Vulkan
// fix and what wgpu does. It is rejected here because it is action at a
// distance — a reader of this shader would have no way to know — and because
// it also changes how scissor rectangles are interpreted.
@vertex
fn vs_quad(@builtin(vertex_index) idx: u32) -> VsOut {
    let corner = vec2<f32>(f32(idx & 1u), f32((idx >> 1u) & 1u));
    let ndc = params.dst.xy + corner * params.dst.zw;
    var out: VsOut;
    out.pos = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = params.src.xy + corner * params.src.zw;
    return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// A client surface. The sample is already premultiplied, so scaling it by the
// opacity keeps it premultiplied.
@fragment
fn fs_texture(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv) * params.tint.a;
}

// A solid rectangle — the bar background, a border, a damage overlay. Callers
// pass a premultiplied colour so this needs no conversion and both entry
// points can share one blend state.
@fragment
fn fs_solid(in: VsOut) -> @location(0) vec4<f32> {
    return params.tint;
}
