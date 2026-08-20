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
                    socks = glob.glob("/run/user/*/kanshou/omoya-*.sock")
                    if not socks:
                        print("no omoya kanshou socket found")
                        sys.exit(1)
                    sock = socks[0]
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
            '';
          };
        }
      );
    in
    nixpkgs.lib.recursiveUpdate (nixpkgs.lib.recursiveUpdate base devOutputs) guiOutputs;
}
