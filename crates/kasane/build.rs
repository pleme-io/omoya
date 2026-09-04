//! Compile the compositor's WGSL to SPIR-V, at build time, in pure Rust.
//!
//! ── ★ THE C THIS AVOIDS ──────────────────────────────────────────────────
//! Every ordinary route from shader source to SPIR-V is a C library —
//! `shaderc-sys` builds shaderc, `glslang` is C++, and both would arrive as a
//! `-sys` crate compiling C at build time. That is precisely the linkage
//! `docs/KASANE.md` §4 refuses, and refusing it in `Cargo.toml` while a
//! build script quietly compiled C would be the refusal in name only.
//!
//! `naga` is pure Rust. It is also already in the fleet's dependency closure
//! (garasu and engawa reach it through wgpu 25), so this is reuse rather than
//! a new supply-chain surface — the census move, not a new dependency.
//!
//! ── ★ BUILD TIME, NOT RUNTIME ────────────────────────────────────────────
//! naga is a `[build-dependencies]` entry, so it is absent from the shipped
//! binary entirely: the seat never parses a shader, and a WGSL syntax error
//! is a compile failure rather than a black screen at startup.
//!
//! ── ★ WHY `include_str!` AND NOT A `CARGO_MANIFEST_DIR` JOIN ─────────────
//! The obvious spelling is
//! `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders/composite.wgsl")`,
//! and it is WRONG under this fleet's Nix builder: substrate's crate2nix
//! builds set `CARGO_MANIFEST_DIR` to the WORKSPACE ROOT, so the join lands
//! one directory too high and the file is not found — a failure that appears
//! only in the Nix build and never in `cargo build`.
//!
//! `include_str!` resolves against THIS FILE's directory, which is correct
//! under both. It also gives rustc a real dependency edge, so editing the
//! shader re-runs this script without a `rerun-if-changed` path that would
//! have the same manifest-dir problem.

use std::path::Path;

/// The shader, read at compile time — see the header for why not a path join.
const COMPOSITE_WGSL: &str = include_str!("shaders/composite.wgsl");

fn main() {
    // Correct under any manifest dir, unlike a path into `shaders/`.
    println!("cargo:rerun-if-changed=build.rs");

    let module = naga::front::wgsl::parse_str(COMPOSITE_WGSL).unwrap_or_else(|e| {
        // naga's rendered diagnostic carries the line and a caret; the Debug
        // form does not. A shader error should read like a compiler error.
        panic!(
            "composite.wgsl failed to parse:\n{}",
            e.emit_to_string(COMPOSITE_WGSL)
        )
    });

    // ★ VALIDATE BEFORE EMITTING. naga will happily write SPIR-V for a module
    // it has not checked, and an invalid module does not fail at
    // `vkCreateShaderModule` — it fails inside the driver at pipeline
    // creation or, worse, renders wrong. Validating here turns that class
    // into a build failure.
    //
    // `PUSH_CONSTANT` is required because `var<push_constant>` is not in
    // WGSL's base capability set; without it the parse succeeds and
    // validation refuses.
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::PUSH_CONSTANT,
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("composite.wgsl failed validation: {e:?}"));

    let options = naga::back::spv::Options {
        // 1.0 is the floor every Vulkan 1.0 driver accepts. Nothing in this
        // shader needs a later SPIR-V, and asking for one would narrow the
        // set of machines that can run the seat for no gain.
        lang_version: (1, 0),
        ..Default::default()
    };

    // `None` pipeline options emits EVERY entry point into one module, which
    // is what lets one `vkCreateShaderModule` serve all three stages — the
    // stage picks its entry point by name at pipeline creation.
    let words = naga::back::spv::write_vec(&module, &info, &options, None)
        .unwrap_or_else(|e| panic!("composite.wgsl failed SPIR-V emission: {e:?}"));

    // SPIR-V is defined as a stream of 32-bit words; the file is those words
    // little-endian, which is what the magic number at word 0 tells the
    // driver to expect.
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();

    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"))
        .join("composite.spv");
    std::fs::write(&out, bytes).unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
}
