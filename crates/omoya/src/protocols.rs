//! What the seat tells clients it can do — and what makes each claim true.
//!
//! ── ★ THE INVARIANT: ADVERTISED IMPLIES SERVED ──────────────────────────
//!
//! A Wayland global is a PROMISE. Binding one is a client saying "I will build
//! on this", and there is no negotiation afterwards and no error path for
//! "actually we ignore that". A compositor that advertises a protocol it does
//! not honour produces the worst failure shape this codebase keeps meeting:
//! everything succeeds, and the result is quietly wrong.
//!
//! Both directions of that invariant were broken here, which is why this file
//! exists rather than a comment saying to be careful:
//!
//! * **Served, never advertised.** `input.rs` computed a `RelativeMotionEvent`
//!   for every mouse event and called `PointerHandle::relative_motion`, under a
//!   comment explaining why relative motion is not cursor motion. smithay sends
//!   those to `known_relative_pointers` (`relative_pointer.rs:112`), a list
//!   filled only when a client binds `zwp_relative_pointer_manager_v1` — and
//!   omoya never advertised it, so the list was permanently empty. Correct code,
//!   written deliberately, reachable by nobody.
//!
//! * **Advertisable, not served.** smithay 0.7.0 ships forty-four
//!   `delegate_*!` macros and omoya used ten. Most of the remaining thirty-four
//!   are one line to advertise and real work to HONOUR — `wp_viewporter` means
//!   nothing unless the renderer applies the src/dst rects, `alpha_modifier`
//!   means nothing unless the blend uses the alpha. Adding them by macro alone
//!   would have looked like a large feature push and shipped a pile of lies.
//!
//! So the catalog below is not a list of protocols. It is a list of
//! **promises**, each paired with the thing that keeps it, and
//! `every_delegate_declares_what_serves_it` fails the build when a
//! `delegate_*!` line appears in `handlers.rs` with no row here — or a row
//! appears here with no delegate.
//!
//! ★ HONEST TIER: this is CI-caught, not unrepresentable. Nothing stops a row
//! from claiming `Served::By("a function that does nothing")`. What it removes
//! is the SILENT case — advertising without ever writing the sentence "this is
//! what serves it" — and it makes the sentence reviewable in one place.

/// What makes an advertised global's promise true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// A code path in omoya honours it. The string names WHERE, so a reader can
    /// go and check the claim instead of trusting it.
    By(&'static str),
    /// The protocol is a hint the compositor may ignore, by its own spec.
    ///
    /// Deliberately a separate arm rather than `By("nothing")`: "we ignore this
    /// and the spec says that is fine" and "we ignore this and it matters" must
    /// not render the same. The string says why it is optional.
    HintOnly(&'static str),
}

/// One global the seat advertises.
#[derive(Debug, Clone, Copy)]
pub struct Protocol {
    /// The wire interface a client binds, as it appears in `wayland-info`.
    pub interface: &'static str,
    /// The `delegate_*!` macro that wires it, WITHOUT the `delegate_` prefix.
    /// This is the key the source-scan gate matches on.
    pub delegate: &'static str,
    /// What keeps the promise.
    pub served: Served,
}

/// Every global omoya advertises, and what serves it.
///
/// Ordered as the delegates appear in `handlers.rs`, so a reader diffing the
/// two files reads them in the same sequence.
pub const ADVERTISED: &[Protocol] = &[
    Protocol {
        interface: "wl_compositor",
        delegate: "compositor",
        served: Served::By("handlers.rs CompositorHandler::commit — damage, shadows, layout"),
    },
    Protocol {
        interface: "wl_shm",
        delegate: "shm",
        served: Served::By("nuri_renderer / kasane_renderer import shm buffers"),
    },
    Protocol {
        interface: "xdg_wm_base",
        delegate: "xdg_shell",
        served: Served::By("handlers.rs XdgShellHandler + layout.rs tiling"),
    },
    Protocol {
        interface: "wp_presentation",
        delegate: "presentation",
        served: Served::By("drm.rs presentation feedback on flip completion"),
    },
    Protocol {
        interface: "zxdg_decoration_manager_v1",
        delegate: "xdg_decoration",
        served: Served::By("chrome.rs draws server-side decorations"),
    },
    Protocol {
        interface: "zwlr_layer_shell_v1",
        delegate: "layer_shell",
        served: Served::By("handlers.rs WlrLayerShellHandler + layer surfaces in the render pass"),
    },
    Protocol {
        interface: "wl_seat",
        delegate: "seat",
        served: Served::By("input.rs — keyboard, pointer, focus"),
    },
    Protocol {
        interface: "wl_data_device_manager",
        delegate: "data_device",
        served: Served::By("smithay's own selection plumbing (one of three planes; see below)"),
    },
    Protocol {
        interface: "wl_output / xdg_output",
        delegate: "output",
        served: Served::By("drm.rs / winit.rs map a real output with a mode"),
    },
    Protocol {
        interface: "zwp_relative_pointer_manager_v1",
        delegate: "relative_pointer",
        served: Served::By("input.rs pointer.relative_motion on every PointerMotion event"),
    },
    Protocol {
        interface: "zwp_linux_dmabuf_v1",
        delegate: "dmabuf",
        served: Served::By(
            "handlers.rs DmabufHandler::dmabuf_imported + the renderer's import path",
        ),
    },
];

/// What the seat deliberately does NOT advertise, and why.
///
/// ★ THIS IS THE HALF THAT USUALLY GOES UNWRITTEN, and it is the more useful
/// half when someone asks "why is my app broken on omoya". An absent protocol
/// is a decision; without this list it reads as an oversight, and the next
/// person adds the delegate line without the work behind it.
///
/// Not enforced by the gate — a protocol may leave this list by being
/// implemented, and demanding an edit here for that would be friction with no
/// invariant behind it. It is documentation with a reason attached.
pub const WITHHELD: &[(&str, &str)] = &[
    (
        "wp_viewporter",
        "one line to advertise; means nothing until the renderer applies the \
         src/dst rects. Advertising it would make every scaled client render \
         at the wrong size with no error. Prerequisite for fractional scale \
         and for xwayland-satellite.",
    ),
    (
        "wp_fractional_scale_v1",
        "requires SENDING a preferred scale the compositor actually honours; \
         needs wp_viewporter first, and wl_compositor v6 for \
         preferred_buffer_scale.",
    ),
    (
        "wp_alpha_modifier_v1",
        "the client sets an alpha the compositor must APPLY in the blend. \
         Ignoring it renders opaque what the client asked to be transparent.",
    ),
    (
        "wp_single_pixel_buffer_v1",
        "a buffer TYPE the renderer must be able to import. Advertising it \
         before nuri/kasane handle it yields a broken surface, not a solid one.",
    ),
    (
        "zwp_pointer_gestures_v1",
        "the compositor must SEND pinch/swipe events; evdev_backend.rs does not \
         synthesise them. A bound client would wait forever.",
    ),
    (
        "ext_session_lock_v1",
        "★ SECURITY. smithay 0.7.0's implementation has a reachable lock-screen \
         bypass: session_lock/lock.rs:181-189 posts InvalidUnlock and then calls \
         state.unlock() anyway, with no else and no return, while every \
         Request::Lock mints a fresh lock_status = false. Any client can unlock \
         a locked screen at the cost of its own connection. Fixed on smithay \
         master, in no release. See docs/DESKTOP-PLAN.md P4.",
    ),
    (
        "zwp_primary_selection_v1 / wlr_data_control / ext_data_control",
        "the other two selection planes. Real work in hasami, not a delegate \
         line — and a clipboard manager needs data-control specifically.",
    ),
    (
        "zwp_text_input_v3 / zwp_input_method_v2",
        "for INPUT METHODS (fcitx5/ibus, CJK). NOT needed for dead keys: \
         hairetsu emits dead_acute and the client composes it with its own \
         libxkbcommon. Wiring these without an input-method client running \
         buys nothing.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every `delegate_*!(Omoya)` in `handlers.rs`, read from the source.
    ///
    /// Enumerated from source rather than from a second hand-list, for the
    /// reason `introspect.rs`'s leaf gate gives: a hand-list has to be updated
    /// in the same commit as the thing it mirrors, and that is exactly the
    /// discipline that already failed here twice.
    fn delegates_in_source() -> BTreeSet<String> {
        let src = include_str!("handlers.rs");
        let mut out = BTreeSet::new();
        for line in src.lines() {
            let t = line.trim();
            // Skip the `use` block and prose; only a real invocation counts.
            if t.starts_with("//") || !t.contains("!(Omoya);") {
                continue;
            }
            let Some(i) = t.find("delegate_") else {
                continue;
            };
            let rest = &t[i + "delegate_".len()..];
            let Some(bang) = rest.find('!') else { continue };
            out.insert(rest[..bang].to_string());
        }
        out
    }

    #[test]
    fn the_scan_finds_the_delegates_at_all() {
        // Anti-vacuity. If the source shape changes and the scan stops
        // matching, every assertion below passes over an empty set.
        let found = delegates_in_source();
        assert!(
            found.len() >= 10,
            "the delegate scan found only {} macros — it has stopped matching \
             the source shape and is now a vacuous gate: {found:?}",
            found.len()
        );
    }

    #[test]
    fn every_delegate_declares_what_serves_it() {
        // ★ THE GATE. A `delegate_*!` line with no catalog row is a promise
        // made to every client with nothing written down about who keeps it.
        let found = delegates_in_source();
        let declared: BTreeSet<String> =
            ADVERTISED.iter().map(|p| p.delegate.to_string()).collect();

        let undeclared: Vec<&String> = found.difference(&declared).collect();
        assert!(
            undeclared.is_empty(),
            "these protocols are ADVERTISED with no row in protocols::ADVERTISED, \
             so nothing states what serves them: {undeclared:?}"
        );

        let phantom: Vec<&String> = declared.difference(&found).collect();
        assert!(
            phantom.is_empty(),
            "these rows claim a protocol handlers.rs does not advertise — the \
             catalog is describing a seat we do not ship: {phantom:?}"
        );
    }

    #[test]
    fn a_served_claim_names_somewhere_to_look() {
        // A row whose evidence is empty is a row that says nothing. This is the
        // weakest half of the gate and it is labelled as such in the header:
        // it cannot tell a true claim from a plausible one, only a written
        // claim from a missing one.
        for p in ADVERTISED {
            let s = match p.served {
                Served::By(s) | Served::HintOnly(s) => s,
            };
            assert!(
                s.len() > 12,
                "{}: `served` must name where the promise is kept",
                p.interface
            );
        }
    }

    #[test]
    fn interfaces_and_delegates_are_unique() {
        let ifaces: BTreeSet<&str> = ADVERTISED.iter().map(|p| p.interface).collect();
        assert_eq!(ifaces.len(), ADVERTISED.len(), "duplicate interface");
        let dels: BTreeSet<&str> = ADVERTISED.iter().map(|p| p.delegate).collect();
        assert_eq!(dels.len(), ADVERTISED.len(), "duplicate delegate");
    }

    #[test]
    fn withheld_protocols_carry_a_reason_and_do_not_overlap() {
        for (name, why) in WITHHELD {
            assert!(why.len() > 40, "{name}: a withheld protocol needs a REASON");
        }
        // A protocol cannot be both advertised and withheld.
        let ifaces: BTreeSet<&str> = ADVERTISED.iter().map(|p| p.interface).collect();
        for (name, _) in WITHHELD {
            assert!(
                !ifaces.contains(name),
                "{name} is listed as both advertised and withheld"
            );
        }
    }

    #[test]
    fn relative_pointer_is_advertised_now_that_input_sends_it() {
        // ★ The regression this file was written around. `input.rs` sends
        // relative motion unconditionally; if this row ever disappears while
        // that call remains, the events go back to a permanently empty list.
        let src = include_str!("input.rs");
        assert!(
            src.contains("relative_motion"),
            "input.rs no longer sends relative motion — remove the protocol row too"
        );
        assert!(
            ADVERTISED.iter().any(|p| p.delegate == "relative_pointer"),
            "input.rs sends relative motion into a protocol we do not advertise: \
             smithay's known_relative_pointers is empty without the global, so \
             every one of those events goes nowhere"
        );
    }
}
