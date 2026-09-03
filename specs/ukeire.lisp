;;; ukeire (受け入れ) — the seat's intake of physical input.
;;;
;;; ★ THIS IS THE DESTINATION FORM, NOT THE WIRED FORM.
;;;
;;; Nothing loads this file. M0 authors the vocabulary as plain typed Rust in
;;; `crates/omoya/src/ukeire.rs` and configures it through shikumi yaml; the
;;; `(defukeire …)` keyword and its `#[derive(DeriveTataraDomain)]` border are
;;; a named M1 in `docs/UKEIRE.md`. It is committed anyway so the destination
;;; is a reviewable artifact rather than a sentence in a doc — and so the M1
;;; that wires it has a target to be byte-compared against.
;;;
;;; Read `docs/UKEIRE.md` first: it carries the census that justified the
;;; domain and the tier ledger that says which of these fields is a type and
;;; which is a validator.

(defukeire seat
  ;; ── What a keyboard event MEANS ────────────────────────────────────────
  ;;
  ;; Absent = xkb's own defaults, which is byte-for-byte what
  ;; `XkbConfig::default()` gave before this vocabulary existed. That is the
  ;; landing condition: adopting ukeire changes no running seat.
  ;;
  ;; ★ `:layout` is PROJECTED from the node's `services.xserver.xkb.layout`
  ;; by the nix module, not declared here. Three declarations of "what layout
  ;; is this machine" already existed and the Wayland seat read none of them;
  ;; a fourth is the defect, not the fix.
  :keymap (keymap
            :rules   nil
            :model   nil
            :layout  nil
            :variant nil
            ;; NOT the place for `caps:escape`. omoya remaps CapsLock at the
            ;; evdev layer, below xkb, so it survives a mid-session layout
            ;; switch. Setting it here too would be a second answer.
            :options nil)

  ;; ── How fast a held key is TAKEN ───────────────────────────────────────
  ;;
  ;; No `:enable`. `:rate-hz 0` is `wl_keyboard.repeat_info`'s own spelling
  ;; for off, so "disabled with a delay" has no representation and needs no
  ;; cross-field rule.
  ;;
  ;; 200/45, not the 600/25 the seat shipped with: that pair was chosen when
  ;; omoya's only face was the greeter, where a held key repeating into a
  ;; password field is a real hazard. The greeter keeps its own answer at the
  ;; consumer (`awase::KeyRepeatGate`), so the seat need not stay slow for
  ;; everyone.
  :repeat (repeat
            :delay-ms 200      ; 50..2000, clamped at the parse boundary
            :rate-hz  45)      ; 0..100, where 0 is off

  ;; ── How an axis event is TAKEN ─────────────────────────────────────────
  ;;
  ;; ★ Direction is a CLOSED KEYWORD, never a sign on the magnitude. Encoding
  ;; "natural" as a negative factor makes two independent facts share one
  ;; number, so "inverted" and "twice as fast" become the same edit and a
  ;; stray minus is a silent inversion.
  :scroll (scroll
            :direction :traditional   ; :traditional | :natural
            :factor    3.0)           ; 0.25..10.0 lines per detent

  ;; ── How the pointer is PRESENTED ───────────────────────────────────────
  :pointer (pointer
             :cursor-scale 2)         ; 1..6 screen px per mask cell

  ;; ── Which modifier the chord vocabulary hangs off ──────────────────────
  ;;
  ;; ★ TWO VARIANTS, BOTH KNOWN-SAFE. An arbitrary modifier would let someone
  ;; select `ctrl`, at which point every fleet chord collides with
  ;; `Ctrl+Alt+F1..F12` and the machine soft-bricks with no VT escape. The
  ;; dangerous choice has no spelling rather than a runtime check.
  :modifier :super)                   ; :super | :alt
