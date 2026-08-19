{
  description = "omoya (母屋) — the pleme-io-native Wayland compositor";

  inputs = {
    # substrate carries the whole Rust build surface (crate2nix, fenix,
    # flake-utils, gen) — a consumer stops pinning them individually.
    substrate.url = "github:pleme-io/substrate";
  };

  # ── PACKAGED, as of 2026-08-19 ─────────────────────────────────────────────
  # This flake used to be devShell-only, carrying
  # `pending-substrate-flake: omoya has no releasable artifact yet`. That was
  # true when written and is now false: the compositor runs, serves a Wayland
  # socket, paints Nord0 and composites a real client. A compositor nothing can
  # install is a compositor no seat can use, so the package is the missing
  # delivery link rather than ceremony.
  #
  # ★ LINUX ONLY, and that is structural rather than a gap to close. smithay
  # does not build on darwin and a Wayland compositor has no meaning there;
  # `omoya-spec` alone is portable. So `systems` names the two Linux targets
  # instead of taking substrate's four-system default, which would fail on the
  # darwin arms for a reason no amount of fixing would remove.
  #
  # ★ AND THE PACKAGE IS THE WRAPPED `host-tool`, NOT `packages.default`.
  # substrate's default Linux output is pkgsStatic/musl, and a compositor
  # dlopens libwayland-client.so.0, libxkbcommon.so.0 and libEGL.so.1 at
  # runtime — a static binary cannot dlopen them at all, and even the glibc
  # build carries no RPATH for them. mado hit exactly this and solved it with a
  # makeWrapper LD_LIBRARY_PATH wrap; omoya inherits that measured lesson
  # rather than rediscovering it. Keep this list in step with
  # substrate/lib/build/rust/eframe.nix::linuxRuntimeLibs.
  outputs =
    { substrate, ... }:
    let
      nixpkgs = substrate.inputs.nixpkgs;
      flake-utils = substrate.inputs.flake-utils;

      base = substrate.rust.tool {
        src = ./.;
        repo = "pleme-io/omoya";
        # Two workspace members, so the binary crate must be named. Without it
        # the builder's "defaults to single member" path has nothing to default
        # to and picks wrong.
        member = "omoya";
        # ★ EXACTLY ONE build input, and the list is measured rather than
        # guessed — it took two failed builds to get here, each narrowing it.
        #
        # Attempt 1 passed `[ "wayland" "libxkbcommon" "libGL" ]`, and rio
        # refused it with `build input wayland does not exist`: the crate2nix
        # layer resolves these as NAMES against its own package set, not as
        # nixpkgs attributes, so a plausible name is not necessarily a valid
        # one.
        #
        # Attempt 2 dropped all three, on the reasoning that smithay's
        # `wayland_frontend` uses the pure-Rust wayland-backend and links no C
        # libwayland. That reasoning was right about wayland and wrong about
        # xkbcommon — the link failed with:
        #
        #   rust-lld: error: unable to find library -lxkbcommon
        #
        # So the honest split is: the `xkbcommon` crate LINKS libxkbcommon (it
        # is in the rustc invocation as `-lxkbcommon`), while libwayland and
        # libGL are DLOPENED and belong in the LD_LIBRARY_PATH wrap below.
        # Link-time and dlopen-time are different mechanisms for different
        # problems, and treating them as one list is what cost both builds.
        # ★ `nativeBuildInputs`, NOT `buildInputs`, and the reason is in
        # substrate's own signature rather than in convention:
        #
        #   buildInputs       = ... ++ buildInputs                    # DERIVATIONS
        #   nativeBuildInputs = ... ++ (map (n: targetPkgs.${n}) ...) # NAMES
        #
        # (substrate/lib/build/rust/tool-release.nix:232-236.) Only
        # nativeBuildInputs is name-addressed, which is why passing the string
        # "libxkbcommon" to buildInputs failed with `build input libxkbcommon
        # does not exist` — buildInputs wanted a package and got a string.
        #
        # The name is resolved from **targetPkgs**, i.e. the package set being
        # built FOR, so despite the "native" in its name this is the correct
        # hook for a library the target links against. omoya is Linux-only and
        # never cross-compiled, so there is no host/target split to get wrong.
        nativeBuildInputs = [
          "pkg-config"
          "libxkbcommon"
        ];
      };

      devOutputs = flake-utils.lib.eachDefaultSystem (
        system:
        let
          pkgs = import nixpkgs { inherit system; };

          # ── THE M2 SET, AND ONLY THE M2 SET ────────────────────────────────
          # M2 is a NESTED compositor (smithay `backend_winit`): it runs as a
          # window inside an existing session and never touches a DRM device, a
          # seat, or evdev. So it needs the Wayland/X11 CLIENT stack and nothing
          # else.
          #
          # ★ This list was originally the M4 (DRM backend) set, and that was a
          # measured mistake, not a stylistic one: `libinput` drags
          # libwacom → libgudev → umockdev → libpcap → libnl, and on plo that
          # whole subtree missed the binary cache and went to SOURCE. The build
          # spent 40+ minutes compiling network libraries and a device-mocking
          # framework for a compositor that reads its input from winit. Trimming
          # it deletes the entire failing subtree rather than working around it.
          #
          # libinput / seatd / libdrm / mesa / libgbm / udev come back with M4,
          # where they are actually used — see theory/OMOYA.md's phase ladder.
          compositorDeps = with pkgs; [
            # Wayland itself + the protocol XML the scanner reads.
            wayland
            wayland-protocols
            wayland-scanner
            # xkbcommon turns keycodes into symbols. This is also where omoya's
            # `awase::Key` adapter attaches (OMOYA.md §5), so it is M2-relevant
            # rather than deferred with the rest of the input stack.
            libxkbcommon
            # winit's X11 arm — winit auto-detects at runtime and needs both
            # present. Names track mado's proven Linux GUI wrap.
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
            xorg.libxcb
            # EGL/GL + Vulkan loader for the renderer smithay's winit backend
            # brings up.
            libGL
            vulkan-loader
          ];
          # ── THE M4 SET — DRM/KMS on real hardware ──────────────────────────
          # Deliberately a SEPARATE shell, not an addition to the default one.
          # The M2 trim was measured, not stylistic: libinput drags
          # libwacom → libgudev → umockdev → libpcap → libnl, and on plo that
          # whole subtree missed the binary cache and compiled for 40+ minutes —
          # a device-mocking framework and a packet-capture library, built for a
          # compositor that reads its input from winit. Keeping the sets apart
          # means the M2 dev loop never pays for M4's hardware access.
          #
          # `nix develop .#drm` is what M4 builds in. Everything here is used by
          # `backend_drm` + `backend_session_libseat` + `backend_udev` +
          # `backend_libinput`, and nothing here is used before then.
          # ★ SCANOUT ONLY, and the split is MEASURED rather than tidy-minded.
          # M4's first done-predicate is "drives the monitor at its native mode",
          # which needs no input at all. Dry-run on plo, 2026-08-19:
          #
          #   libdrm + libgbm + udev + seatd + mesa  ->  nothing to build
          #   the same set PLUS libinput             ->  11 derivations
          #
          # Those 11 are the libinput chain (libnl, libpcap, umockdev, libgudev,
          # libwacom) plus graphviz and source-highlight pulled in to build them.
          # So scanout is FREE and input is the thing that costs — which makes
          # "M4a scanout, M4b input" a real boundary rather than a bookkeeping
          # one. libinput joins this list when M4b starts; the chain has been
          # pre-built on plo so that day costs nothing either.
          drmDeps = with pkgs; [
            libdrm # the KMS ioctls themselves
            libgbm # buffer allocation for the scanout path
            udev # device discovery + hotplug
            seatd # libseat — session/VT arbitration (talks to logind here)
            mesa # the GL/EGL userspace, for when the dumb-buffer path grows one
          ];

          # ── THE WITNESS SET ────────────────────────────────────────────────
          # Answering "does omoya composite?" needs a display to composite ONTO
          # and a way to read the pixels back. Xvfb supplies the first without a
          # DRM device, a VT, or root — which is the whole reason M2 can be
          # measured on a machine whose only console is the thing being replaced.
          #
          # ★ These are DEV-SHELL inputs, not a `nix shell` invocation, and the
          # difference is load-bearing: `nix shell nixpkgs#xorg.xorgserver …`
          # REPLACES the environment, dropping the `LD_LIBRARY_PATH` the
          # compositor's dlopen'd libEGL is found through. Measured 2026-08-19 —
          # omoya reached the winit backend, opened an X connection, and then
          # panicked with `Failed to load LibEGL: libEGL.so.1: cannot open shared
          # object file`. The X tools and the GL libraries must be in ONE
          # environment, so the flake is where they meet.
          witnessTools = with pkgs; [
            xorg.xorgserver # Xvfb — a display with no hardware behind it
            xorg.xwininfo # what windows actually exist, and how big
            imagemagick # `import` the root window; read pixels back out
          ];
        in
        {
          # `nix develop .#witness` — the environment the M2 done-predicate is
          # measured in. Same libraries as the dev shell, plus the means to look.
          devShells.witness = pkgs.mkShell {
            name = "omoya-witness";
            nativeBuildInputs =
              (with pkgs; [
                rustc
                cargo
                pkg-config
              ])
              ++ witnessTools;
            buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux compositorDeps;
            LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux (
              pkgs.lib.makeLibraryPath compositorDeps
            );
          };

          # `nix develop .#drm` — M4a's environment (scanout). See `drmDeps` for
          # why it is separate from the default shell, and for the measurement
          # that makes scanout-vs-input the right place to split it.
          devShells.drm = pkgs.mkShell {
            name = "omoya-drm";
            nativeBuildInputs = with pkgs; [
              rustc
              cargo
              clippy
              pkg-config
            ];
            buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (compositorDeps ++ drmDeps);
            LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux (
              pkgs.lib.makeLibraryPath (compositorDeps ++ drmDeps)
            );
          };

          devShells.default = pkgs.mkShell {
            name = "omoya-dev";

            nativeBuildInputs = with pkgs; [
              rustc
              cargo
              clippy
              rustfmt
              pkg-config
            ];

            buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux compositorDeps;

            # smithay's crates dlopen some of these rather than linking them, so
            # the loader needs the path at RUN time too — the same wrap mado's
            # flake applies for exactly this reason.
            LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux (
              pkgs.lib.makeLibraryPath compositorDeps
            );

            shellHook = ''
              echo "omoya dev shell — $(rustc --version)"
              ${pkgs.lib.optionalString (!pkgs.stdenv.hostPlatform.isLinux) ''
                echo "NOTE: this is not Linux. \`omoya-spec\` builds and tests here;"
                echo "      the compositor itself does not — see theory/OMOYA.md M2."
              ''}
            '';
          };
        }
      );

      # ── THE SHIPPING PACKAGE ────────────────────────────────────────────
      # substrate's `host-tool` (glibc) wrapped so the compositor's dlopen'd
      # runtime libraries resolve. See the header for why `packages.default`
      # (pkgsStatic/musl) cannot be the answer here.
      guiOutputs = flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          runtimeLibs = with pkgs; [
            # Wayland — what omoya SERVES on, and what winit's Wayland arm
            # loads when omoya itself runs nested inside another compositor.
            wayland
            libxkbcommon
            # winit's X11 arm. Present because the nested backend picks its
            # platform at RUNTIME: the same binary talks X11 under Xvfb (which
            # is how M2 was witnessed) and Wayland under a host compositor.
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
            xorg.libxcb
            # EGL/GL for the renderer smithay's winit backend brings up.
            libGL
            vulkan-loader
          ];
          wrapped = pkgs.symlinkJoin {
            name = "omoya-linux";
            paths = [ base.packages.${system}.host-tool ];
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postBuild = ''
              for f in $out/bin/*; do
                [ -L "$f" ] || continue
                wrapProgram "$f" \
                  --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath runtimeLibs}
              done
            '';
            meta = (base.packages.${system}.host-tool.meta or { }) // {
              mainProgram = "omoya";
            };
          };
        in
        {
          packages.default = wrapped;
          packages.omoya = wrapped;
          apps.default = {
            type = "app";
            program = "${wrapped}/bin/omoya";
          };
        }
      );
    in
    nixpkgs.lib.recursiveUpdate (nixpkgs.lib.recursiveUpdate base devOutputs) guiOutputs;
}
