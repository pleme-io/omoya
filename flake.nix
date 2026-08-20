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
          # M4a's software renderer. `renderer_pixman` links libpixman
          # (`-lpixman-1` in the rustc invocation), so it belongs here beside
          # xkbcommon rather than in the runtime wrap.
          #
          # ★ Worth noting what is NOT in this list: libdrm. smithay's
          # `backend_drm` uses the `drm` / `drm-ffi` / `drm-sys` crates, which
          # are pure-Rust ioctl bindings — they appear in the link line as
          # rlibs, with no `-ldrm` anywhere. That is why the scanout dependency
          # set measured as entirely cached: it adds no C library at all.
          "pixman"
          # M4c: libinput opens the evdev devices; libseat arbitrates the
          # session that is allowed to. Both are dynamically linked.
          "libinput"
          "seatd"
          "udev"
          # backend_gbm is a compile-time requirement of
          # DrmCompositor even though no GbmDevice is constructed.
          "libgbm"
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
            # ★ BOTH OF THESE WERE MISSING, and `cargo check` could not show
            # it: checking does not link. `cargo test` and `cargo build` do,
            # and failed with `cannot find -linput` / `cannot find -lpixman-1`.
            #
            # The crate has enabled `backend_libinput` and `renderer_pixman`
            # since M4, so the shell has been unable to produce a binary that
            # whole time. It went unnoticed because the PACKAGE build is a
            # different derivation with its own inputs — so `nix build` worked
            # while `nix develop` could not link, and only a test run tells
            # them apart.
            libinput # backend_libinput links -linput
            pixman # renderer_pixman links -lpixman-1
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
          # ── ★ RUNTIME LIBRARIES — FOR THE `nested` DEV BUILD ONLY ────────
          #
          # The SHIPPED binary needs none of these. Measured on rio at
          # `2f2aa2d`, on the actual artifact rather than the wrapper:
          #
          #   $ ldd .../rust_omoya-0.1.14/bin/omoya
          #     libc.so.6  libgcc_s.so.1  libm.so.6  linux-vdso.so.1  ld-linux
          #
          # Nothing else is linked, so wrapping the shipped binary in an
          # `LD_LIBRARY_PATH` would do nothing except drag five C libraries
          # back into its closure — which is exactly what it was doing, and
          # what made `nix path-info -r` contradict `ldd`. The closure is the
          # honest census: a library nothing links but the closure still
          # carries is a claim that has not actually been made good on.
          #
          # These stay because `--features nested` (winit) DOES dlopen them.
          # That build is a development tool and is never what ships.
          runtimeLibs = with pkgs; [
            # Wayland — what winit's Wayland arm loads when omoya runs nested
            # inside another compositor. omoya SERVES Wayland through
            # wayland-server's pure-Rust backend, which links nothing.
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
            # ★ pixman is in the BUILD inputs AND here, and that is not
            # redundancy. This file previously framed the split as "linked vs
            # dlopened", as though each library belonged to exactly one list.
            # A DYNAMICALLY linked library needs both: the build input so
            # `-lpixman-1` resolves at link time, and this entry so
            # `libpixman-1.so.0` resolves at exec time. Without it the binary
            # built cleanly and died instantly on plo with "cannot open shared
            # object file".
            pixman
            # libgbm joins pixman in the "both lists" row: enabling
            # `backend_gbm` (which DrmCompositor requires) links libgbm
            # dynamically, so it needs the build input AND this entry. No
            # GbmDevice is ever constructed — the runtime path is still dumb
            # buffers — but the .so must resolve at exec time regardless.
            libgbm
            # M4c, same both-lists rule as pixman and libgbm.
            libinput
            seatd
            udev
          ];
          # ── ★ NO WRAPPER ────────────────────────────────────────────────
          #
          # This used to be a `symlinkJoin` + `wrapProgram`, which made the
          # compositor's entry point a **generated bash script** in a repo
          # whose law is NO SHELL. It existed to put `LD_LIBRARY_PATH` in
          # front of libraries the binary no longer links.
          #
          # Both reasons are gone at once, which is why this is a deletion
          # rather than a rewrite: the shipped binary links only libc, libm
          # and libgcc_s, so there is no search path to set, so there is no
          # script to generate. The `.omoya-wrapped` indirection goes too —
          # `bin/omoya` is now the ELF binary itself.
          #
          # A `nested` build still needs the search path; that is what the
          # dev shells below are for.
          wrapped = base.packages.${system}.host-tool;
        in
        {
          packages.default = wrapped;
          packages.omoya = wrapped;
          apps.default = {
            type = "app";
            program = "${wrapped}/bin/omoya";
          };

          # ── ★ THE SEAT TEST — a REAL DRM device, in a VM, in CI ──────────
          # Every previous verification of this compositor needed a person at
          # plo. That is the reason a dropped session notifier and a mouse that
          # moved nothing both shipped: the DRM path had no automated exercise
          # at all, and the nested backend cannot show either bug.
          #
          # `vkms` — Virtual Kernel Mode Setting — is an in-tree kernel module
          # that presents a real DRM device with real connectors and real dumb
          # buffers, backed by nothing. So the whole scanout path runs here:
          # mode-setting, dumb-buffer allocation, the renderer, and the session.
          #
          # ★ THE PAMName TRICK IS THE LOAD-BEARING PART. `--session logind`
          # needs the process to be IN a logind session, and a test script's
          # root shell is not one — `GetSessionByPID` answers "does not belong
          # to any known session", which is exactly what it answers over ssh.
          # `systemd-run --property=PAMName=login` runs the unit through PAM,
          # which registers a session with logind. Without it this test would
          # exercise the error path and look like it passed.
          checks.vkms-seat = pkgs.testers.runNixOSTest {
            name = "omoya-vkms-seat";
            nodes.machine =
              { ... }:
              {
                boot.kernelModules = [ "vkms" ];
                # logind, and a user with a real account to hold the session.
                services.displayManager.enable = false;
                users.users.seat = {
                  isNormalUser = true;
                  password = "seat";
                  # ★ NO video/input GROUPS, DELIBERATELY — this is the
                  # assertion. The DRM device is opened through
                  # `Session::open` -> logind's `TakeDevice`, which grants
                  # access to the session rather than to a group. If this test
                  # passes without them, the device genuinely came from the
                  # session; if someone re-adds them "to fix a failure", the
                  # only thing being fixed is the evidence.
                  #
                  # They were here briefly, while the direct-open path was
                  # still in place, and their removal is the proof that it is
                  # gone.
                };
                environment.systemPackages = [
                  wrapped
                  pkgs.libinput
                  # For the capture assertion below: kanshou speaks
                  # length-prefixed JSON over a Unix socket, which needs a real
                  # client. `nc` cannot frame it and /dev/tcp cannot reach a
                  # Unix socket.
                  # Reads one pixel out of a P6 PPM. Used to prove the
                  # pointer is drawn AND drawn in the right encoding — a claim
                  # only the pixels can settle.
                  # A real Wayland shm client, so the gate can prove a
                  # CLIENT SURFACE composites — not merely that the compositor
                  # paints its own background and cursor.
                  pkgs.weston
                  (pkgs.writers.writePython3Bin "ppm-probe" { } (builtins.readFile ./nix/ppm-probe.py))
                  (pkgs.writers.writePython3Bin "ppm-colours" { } (builtins.readFile ./nix/ppm-colours.py))
                  (pkgs.writers.writePython3Bin "kanshou-get" { } (builtins.readFile ./nix/kanshou-get.py))
                  (pkgs.writers.writePython3Bin "ppm-region" { } (builtins.readFile ./nix/ppm-region.py))
                  (pkgs.writers.writePython3Bin "kanshou-capture" { } ''
                    import glob
                    import json
                    import socket
                    import struct
                    import sys
                    import time


                    def q(sock, path, args):
                        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                        s.settimeout(10)
                        s.connect(sock)
                        r = json.dumps({"path": path, "args": args}).encode()
                        s.sendall(struct.pack(">I", len(r)) + r)
                        n = struct.unpack(">I", s.recv(4))[0]
                        b = b""
                        while len(b) < n:
                            b += s.recv(n - len(b))
                        s.close()
                        return json.loads(b)


                    dest = sys.argv[1]
                    # The first socket that ANSWERS — see nix/kanshou-get.py.
                    # A kanshou socket outlives a process that dies hard, and
                    # picking a dead one reads as "the compositor is not
                    # answering" rather than "that file has no owner".
                    sock = None
                    for cand in sorted(
                        glob.glob("/run/user/*/kanshou/omoya-*.sock")
                    ):
                        try:
                            probe = socket.socket(
                                socket.AF_UNIX, socket.SOCK_STREAM
                            )
                            probe.settimeout(2)
                            probe.connect(cand)
                            probe.close()
                            sock = cand
                            break
                        except OSError:
                            continue
                    if sock is None:
                        print("no LIVE omoya kanshou socket found")
                        sys.exit(1)
                    print("socket:", sock)
                    print("request:", q(sock, ["capture"], [dest]))
                    for _ in range(50):
                        time.sleep(0.2)
                        r = q(sock, ["capture_result"], [])
                        if r.get("Ok"):
                            print("result:", r["Ok"])
                            sys.exit(0 if str(r["Ok"]).startswith("ok:") else 1)
                    print("capture never completed")
                    sys.exit(1)
                  '')
                ];
                # ★ A REAL LOGIN, NOT A SIMULATED ONE. Three attempts with
                # `systemd-run --property=PAMName=login` produced a session
                # that logind classed `manager` with `Seat=` empty and
                # `VTNr=0` — so `TakeDevice` was refused, and once it was not,
                # DRM master still was.
                #
                # The reason is ordering: pam_systemd reads XDG_SEAT and
                # XDG_VTNR from the PAM environment, and
                # `--property=Environment=` sets the SERVICE environment, which
                # is applied after PAM has already run. The variables were
                # therefore invisible to the code that needed them.
                #
                # getty autologin is the real thing: it establishes the session
                # through PAM on a VT, so logind assigns seat0 and a VTNr the
                # ordinary way. It also mirrors how the seat actually starts on
                # plo — greetd logs a user in, then execs the compositor.
                services.getty.autologinUser = "seat";
                virtualisation.memorySize = 2048;
              };
            testScript = ''
              import re

              machine.wait_for_unit("multi-user.target")

              # ── the virtual card ──
              machine.succeed("modprobe vkms")
              machine.wait_until_succeeds("test -e /dev/dri/card0")
              print(machine.succeed("ls -la /dev/dri/"))

              # ★ It must have CONNECTORS and a mode, or "scanout works" would
              # be vacuous — a card with nothing attached accepts a mode set
              # that displays nowhere.
              machine.succeed("test -d /sys/class/drm/card0-Virtual-1")
              machine.wait_until_succeeds(
                  "grep -q connected /sys/class/drm/card0-Virtual-1/status"
              )

              # ── the compositor, in a REAL logind session ──
              # PAMName=login is what makes GetSessionByPID succeed; see the
              # comment above the check.
              # ── the compositor, in a REAL seat session ──
              # getty has autologged `seat` in on tty1, which is a genuine PAM
              # session on seat0 with a VT. Launch omoya from inside it by
              # typing into the console — the only way to inherit the session
              # rather than construct one beside it.
              machine.wait_until_succeeds(
                  "loginctl list-sessions --no-legend | grep -q seat0"
              )
              print(machine.succeed("loginctl list-sessions --no-legend"))

              sid = machine.succeed(
                  "loginctl list-sessions --no-legend | awk '$4 == \"seat0\" {print $1; exit}'"
              ).strip()
              print(machine.succeed(
                  f"loginctl show-session {sid} -p Class -p Seat -p VTNr -p Active"
              ))

              machine.send_chars(
                  "exec ${wrapped}/bin/omoya --backend drm --session logind "
                  "-- ${pkgs.weston}/bin/weston-presentation-shm "
                  "> /tmp/omoya.log 2>&1\n"
              )

              machine.sleep(8)
              # ★ NOT named `log` — the test driver already binds that to its
              # AbstractLogger, and the type checker rejects the shadow. A
              # useful refusal: shadowing it would have silently broken the
              # driver's own logging for the rest of the script.
              journal = machine.succeed("cat /tmp/omoya.log 2>/dev/null || true")
              print(journal)

              # ★ THE ASSERTION: the process is alive AND holding the display.
              # Not an exit code — a compositor launched from a console leaves
              # no unit to interrogate, and "the shell returned" says nothing.
              machine.succeed("pgrep -f 'omoya --backend drm'")
              assert "holding the display" in journal, (
                  "omoya did not take the display: see the log above"
              )

              # And it must have taken the session through OUR code, not
              # libseat: the logind arm logs nothing on success, so the
              # negative is what proves it — no "no logind session" refusal.
              assert "no logind session" not in journal, (
                  "omoya fell back to look-only: the logind session was refused"
              )

              # ★ AND IT MUST ACTUALLY PRESENT. This assertion exists because
              # its absence hid a real bug for a whole session: `page_flip`
              # does not modeset, so every flip against the never-committed
              # CRTC was rejected with EINVAL — once per frame, ~57 times a
              # second — while this test passed. "Alive and holding the
              # display" is satisfied by a compositor that shows nothing.
              #
              # A frame error is a hard failure, not a warning: one is the same
              # bug as a thousand, and tolerating "a few" is how the thousand
              # got through.
              assert "frame failed" not in journal, (
                  "omoya took the display but could not present to it — "
                  "scanout is failing per-frame; see the log above"
              )

              # ★ AND IT MUST BE ABLE TO SHOW WHAT IT DREW.
              #
              # `capture()` was fully implemented, with `ExportMem` behind it,
              # and its call site logged "capture requested" and called
              # nothing — for long enough that a blank screen on plo had to be
              # diagnosed by inference from counters. Then the first fix wired
              # it to an env var, which cannot be set on a RUNNING process, so
              # it still could not serve the moment it exists for.
              #
              # This asserts the whole path: a request over kanshou, a frame
              # taken by the render loop, and a file with real pixels in it.
              # Nothing short of the file proves it — "the call compiles" and
              # "the log says captured" were both true while it was broken.
              # Ask for a capture over kanshou and wait for the render loop to
              # take it. The client is a packaged script (`kanshou-capture`)
              # rather than inline python: this text already sits inside a Nix
              # indented-string literal inside a python test script, and a third
              # level of quoting is how the first attempt became a syntax error.
              # (Including, on the second attempt, a pair of single quotes in
              # this very comment, which ended the Nix string.)
              out = machine.succeed("kanshou-capture /tmp/seat.ppm")
              print(out)

              # The file must exist and be big enough to be a real frame. A
              # 1024x768 dump is ~2.3MB; anything tiny means a header was
              # written and the pixels were not.
              size = int(machine.succeed("stat -c %s /tmp/seat.ppm").strip())
              print(f"capture size: {size} bytes")
              assert size > 100_000, (
                  f"capture produced only {size} bytes — that is not a frame"
              )

              # ★ AND THE POINTER MUST BE IN IT.
              #
              # The cursor is drawn at pointer_location, which starts at (0,0),
              # as a CURSOR_SIZE square in Nord snow_storm[2] (#ECEFF4). So the
              # top-left pixels must be BRIGHT. Nothing else on this screen is:
              # the background is polar_night[0] (#2E3440) and there is no
              # client.
              #
              # This asserts two things at once, and the second is the subtle
              # one. That a pointer is drawn at all — it was not, and keyboard
              # focus was only reachable by clicking it. And that the colour was
              # written in the right ENCODING: linear values in a non-sRGB
              # framebuffer come out far too dark, so a wrong `format_is_srgb`
              # would leave a pointer that is technically present and visually
              # absent. Reading the pixel is the only way to tell those apart.
              px = machine.succeed(
                  "ppm-probe /tmp/seat.ppm 4 4"
              ).strip()
              print(f"pixel(4,4) = {px}")
              r, g, b = (int(v) for v in px.split())
              # ★ EXACT, NOT "bright enough". A >180 threshold passed while the
              # encoding was WRONG: the cursor came out rgb(214,220,231), which
              # is snow_storm[2] (#ECEFF4 = 236,239,244) written LINEAR into a
              # non-sRGB framebuffer. It was bright, so the loose check passed,
              # and the same mistake on the BACKGROUND rendered Nord0 as
              # rgb(7,9,13) — a screen the operator reasonably called black.
              #
              # The scanout buffer is DRM_FORMAT_ARGB8888, which applies no
              # conversion, so the bytes written must already be sRGB. Asserting
              # the exact colour is what makes the encoding checkable at all; a
              # range cannot tell "right colour" from "right hue, wrong gamma",
              # and wrong gamma is the entire failure mode.
              assert (r, g, b) == (236, 239, 244), (
                  f"pointer at (4,4) is rgb({r},{g},{b}), expected "
                  "rgb(236,239,244) = Nord snow_storm[2]. "
                  "rgb(214,220,231) specifically means the LINEAR encoding was "
                  "written into a non-sRGB framebuffer — check "
                  "theme::cursor_for_surface's format_is_srgb flag."
              )

              # And the BACKGROUND, which is what the operator actually looks
              # at. Nord0 = #2E3440. rgb(7,9,13) is its linear value and is the
              # exact shade that got reported as "a blank black screen".
              bg = machine.succeed("ppm-probe /tmp/seat.ppm 900 700").strip()
              print(f"background(900,700) = {bg}")
              br, bgc, bb = (int(v) for v in bg.split())
              assert (br, bgc, bb) == (46, 52, 64), (
                  f"background is rgb({br},{bgc},{bb}), expected rgb(46,52,64) "
                  "= Nord0. rgb(7,9,13) means linear-into-non-sRGB again."
              )

              # ★ AND A CLIENT SURFACE MUST ACTUALLY COMPOSITE.
              #
              # This is the assertion whose absence hid the worst defect in the
              # compositor. NuriRenderer::context_id() returned a FRESH
              # ContextId per call; smithay stores an imported texture under
              # that id and looks it up under it, so every lookup missed, the
              # `?` returned None, and every client surface was silently
              # dropped as "not mapped". omoya composited ZERO clients — shm
              # included — while reporting windows: 1 and a healthy frame rate.
              #
              # Nothing short of counting COLOURS catches that. `windows` was
              # right, `frames` was right, no error was logged, and the screen
              # held exactly two colours: background and cursor.
              colours = int(machine.succeed(
                  "ppm-colours /tmp/seat.ppm"
              ).strip())
              print(f"distinct colours on screen: {colours}")
              assert colours > 2, (
                  f"only {colours} distinct colours — the background and the "
                  "cursor. A client is connected (windows > 0) but its surface "
                  "is not reaching the framebuffer. Check "
                  "NuriRenderer::context_id stability and the Xrgb alpha "
                  "normalisation."
              )

              # ── ★ DO TWO WINDOWS TILE, OR JUST STACK? ───────────────────
              #
              # Every assertion above is satisfied by a compositor that maps
              # every window at (0, 0) — which is exactly what omoya did until
              # the layout landed. One client looks identical either way, so a
              # single-window gate can never tell "tiled" from "stacked" and
              # the whole feature could regress invisibly.
              #
              # A second client is spawned directly with WAYLAND_DISPLAY
              # rather than through the Logo+Return chord: driving a real
              # keystroke into a VT from the test driver is a different and
              # much more fragile thing to build, and what is under test here
              # is the LAYOUT, not the keymap. The keymap has its own unit
              # tests.
              # ★ setsid + nohup, AND KEEP ITS STDERR. `su seat -c 'cmd &'`
              # backgrounds the client inside a shell that then exits, and the
              # child can take the SIGHUP with it — so a client that never
              # drew and a client that was killed look identical from outside.
              # Its log is printed below, because the first version of this
              # discarded stderr and left "the second window is not drawing"
              # with no way to ask why.
              machine.succeed(
                  "su seat -c 'WAYLAND_DISPLAY=wayland-1 "
                  "XDG_RUNTIME_DIR=/run/user/1000 setsid nohup "
                  "${pkgs.weston}/bin/weston-presentation-shm "
                  ">/tmp/client2.log 2>&1 &'"
              )
              machine.sleep(4)
              print("client2:", machine.succeed("cat /tmp/client2.log || true"))
              print("client2 alive:", machine.succeed(
                  "pgrep -c -f 'weston-presentation-sh[m]' || true"
              ).strip())
              machine.succeed("kanshou-capture /tmp/two.ppm")
              windows = int(machine.succeed("kanshou-get windows").strip())
              print(f"windows after the second client: {windows}")
              assert windows >= 2, (
                  f"only {windows} window(s) — the second client never mapped, "
                  "so the tiling assertion below would measure nothing."
              )

              # ★ COUNT CONTENT IN EACH HALF, DO NOT SAMPLE A POINT.
              #
              # The first version of this sampled (256,384) and (768,384) and
              # failed with both halves reading as background — while `windows`
              # said 2 and (4,4) held client pixels. The layout was fine; the
              # ASSERTION was wrong. A client decides its own size and is free
              # to ignore the size in an xdg configure, and weston's demos do
              # exactly that: they are fixed-size. So a point at the centre of
              # a half lands on background whenever the window is small, and
              # the failure reads as "stacked" when the truth is "small".
              #
              # What actually distinguishes tiled from stacked is whether there
              # is ANY client content in the right half — under stacking both
              # windows sit at the same origin and that half is empty.
              nord0 = "46 52 64"
              lc, lt = (
                  int(v) for v in
                  machine.succeed(
                      f"ppm-region /tmp/two.ppm 0 0 512 768 {nord0}"
                  ).split()
              )
              rc, rt = (
                  int(v) for v in
                  machine.succeed(
                      f"ppm-region /tmp/two.ppm 512 0 512 768 {nord0}"
                  ).split()
              )
              print(f"content: left {lc}/{lt} px, right {rc}/{rt} px")
              # ★ ASK THE TREE, DO NOT INFER IT FROM PIXELS. The rectangles
              # `apply_layout` assigned, printed beside the pixel counts, so a
              # disagreement between them is readable at a glance: matching
              # rects with an empty half means placement failed, while
              # identical rects mean the SPLIT failed. Inferring either from
              # a screenshot alone is guesswork.
              print("layout:", machine.succeed("kanshou-get layout").strip())
              # ★ THE DISCRIMINATOR. `windows` counts what Space holds, which
              # includes a toplevel that has never attached a buffer;
              # `elements` counts what the last frame actually drew. A gap
              # means a client mapped and never drew — a client-side problem —
              # while equal counts with a missing window mean the compositor
              # dropped it. Different files, identical screenshot.
              elements = int(machine.succeed("kanshou-get elements").strip())
              print(f"render elements (excluding the cursor): {elements}")
              print("geometry:", machine.succeed("kanshou-get geometry").strip())

              # ── ★ CAN THE SEAT BE DRIVEN, NOT JUST READ? ────────────────
              #
              # The operator's ask was "leverage mcp to ... go to desktop and
              # DO THINGS". Every other assertion here reads. This one writes,
              # and then checks the write LANDED rather than that the call
              # returned — a queued deed that nothing drains answers exactly
              # the same as one that ran.
              #
              # focus-right is the verb to test with, because its effect is
              # observable through an independent leaf: `layout` is derived
              # from the tree, and a focus move changes which window a
              # subsequent split would target. Closing or spawning would也
              # work but are harder to undo inside one gate.
              print("verbs:", machine.succeed("kanshou-get verbs").strip())
              print("do:", machine.succeed("kanshou-get do/focus-right").strip())

              # An unknown verb must be REFUSED, by name — not defaulted to
              # something adjacent, and not silently accepted. `kanshou-get`
              # exits non-zero when a leaf does not answer Ok, so `fail` is
              # the assertion.
              refused = machine.fail("kanshou-get do/rm-rf-slash 2>&1")
              print("refused:", refused.strip()[:120])
              assert "not a verb" in refused, (
                  "an unknown verb was not refused by name — the legality "
                  f"gate is not gating. got: {refused!r}"
              )
              # The totals are the denominator: 0 non-background out of 0
              # scanned would mean the rectangle fell off the image, which is
              # a different bug from an empty half and must not read as one.
              assert lt > 0 and rt > 0, (
                  f"scanned {lt} and {rt} pixels — the sample rectangles are "
                  "off the image, so the counts below measure nothing."
              )
              assert lc > 0 and rc > 0, (
                  f"left half has {lc} non-background pixels, right half {rc}. "
                  "An empty half means both windows are at the same origin — "
                  "stacked, not tiled. Check Tiling::map and apply_layout."
              )

              # ── ★ DOES PARTIAL REPAINT ACTUALLY SKIP ANYTHING? ──────────
              #
              # Every assertion above passes just as happily when the
              # compositor repaints the entire screen sixty times a second,
              # because they check WHAT is on the display and not what it
              # cost. Damage tracking is precisely the kind of change that can
              # be wired up, look right in a screenshot, and do nothing — one
              # unstable element `Id` or one mis-derived buffer age turns
              # every frame back into a full repaint with every pixel still
              # correct. Without this the commit message would be a claim and
              # the gate would be its rubber stamp.
              #
              # The client is PAUSED first so the seat is genuinely idle: with
              # weston-presentation-shm running there IS new content every
              # frame and presenting each one is the right answer, so a
              # measurement taken then would prove nothing either way.
              #
              # ★ SIGSTOP, NOT SIGTERM, AND BY PID FROM OMOYA'S OWN LOG.
              #
              # Two traps, both hit on the first attempt. `pkill -f
              # weston-presentation-shm` matches THE SHELL RUNNING PKILL —
              # its own command line contains the pattern — so the test
              # killed itself and reported exit 143, which reads as the
              # client refusing to die. And killing the client for real
              # ends the login session (`pam_unix(login:session): session
              # closed`), taking the compositor with it, so the measurement
              # would have had nothing to query.
              #
              # A name match is not the fix either: Linux truncates `comm`
              # to 15 characters, so this process is `weston-presenta` and
              # `pkill -x weston-presentation-shm` matches nothing at all —
              # silently, exit 1, no output. omoya logs the pid it spawned,
              # so the pid is read from there rather than searched for.
              # The driver type-checks this script, so the None arm has to be
              # handled — and handling it is right anyway: a missing pid means
              # the client never spawned, which is a different failure from
              # the one this block is measuring and should say so.
              # ★ STRIP ANSI FIRST. tracing-subscriber's fmt layer colours
              # FIELD NAMES, and it does so even when its output is a file —
              # the ansi feature is on, and it does not check for a tty. So
              # the bytes on disk are `seat \x1b[3mpid\x1b[0m=1324`, and a
              # literal "seat pid=" never matches. The earlier
              # `"holding the display" in journal` check works only because
              # that substring lies wholly inside one message with no field
              # boundary to colour, which is exactly why this failed in a way
              # that looked like the client had never spawned.
              plain = re.sub(
                  r"\x1b\[[0-9;]*m", "", machine.succeed("cat /tmp/omoya.log")
              )
              m = re.search(r"spawned into the seat.*?pid=(\d+)", plain)
              assert m, "omoya never logged a spawned pid — no client to pause"
              spawned = int(m.group(1))
              machine.succeed(f"kill -STOP {spawned}")
              # ★ BOTH clients, or the seat is not idle — but match on the
              # process NAME, never on the full command line.
              #
              # `pkill -f weston-presentation-shm` stops THE COMPOSITOR. omoya
              # is launched as `omoya --backend drm --session logind --
              # .../weston-presentation-shm`, so the client's path is part of
              # omoya's own argv and `-f` matches it. The symptom is not a
              # dead compositor either: the kernel still accepts connections
              # on the kanshou socket while the process is stopped, so the
              # next query hangs and times out — reading as "introspection is
              # broken" rather than "you froze the thing you were asking".
              # The bracket trick guards against a shell matching ITSELF and
              # does nothing about this.
              #
              # `-x` against `comm` is the fix, and `comm` is TRUNCATED TO 15
              # CHARACTERS by the kernel — hence `weston-presenta`, not the
              # full name, which matches nothing at all (silently, exit 1).
              machine.succeed("pkill -STOP -x weston-presenta || true")
              machine.sleep(2)
              f0, p0 = [
                  int(x) for x in
                  machine.succeed("kanshou-get frames presented").split()
              ]
              machine.sleep(3)
              f1, p1 = [
                  int(x) for x in
                  machine.succeed("kanshou-get frames presented").split()
              ]
              ticks, flips = f1 - f0, p1 - p0
              print(f"idle: {ticks} render ticks, {flips} presentations")
              # The loop must still be ALIVE — otherwise zero presentations is
              # a dead compositor rather than a quiet one, and the two look
              # identical from the flip counter alone. This is the denominator
              # that keeps the assertion below from passing vacuously.
              assert ticks > 10, (
                  f"only {ticks} render ticks in 3s — the render loop is not "
                  "running, so the presentation count below measures nothing."
              )
              assert flips * 4 < ticks, (
                  f"{flips} presentations for {ticks} idle render ticks. "
                  "Nothing on screen changed, so damage tracking should have "
                  "skipped nearly all of them. Suspect an unstable element "
                  "Id (a fresh Id::new() per frame re-damages everything) or "
                  "DirectScanout::back_buffer_age returning 0 forever."
              )
            '';
          };
        }
      );
    in
    nixpkgs.lib.recursiveUpdate (nixpkgs.lib.recursiveUpdate base devOutputs) guiOutputs;
}
