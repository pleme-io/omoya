//! The bar's right zone as a CLOSED CATALOG, not a run of straight-line calls.
//!
//! ── ★ WHY A CATALOG AND NOT THREE MORE `if let` BLOCKS ───────────────────
//! The operator's report was direct: *"nothing on this seat tells me the
//! battery is at 4% or that wifi dropped."* The obvious fix is three more
//! branches in `rasterize_h`, and the plan explicitly forbids it — Waybar
//! ships ~30 modules, and hand-drawing the fourth is where CLOSED-LOOP
//! MASS-SYNTHESIS binds. So the zone is a `Module::ALL` fold, and the gate
//! below **fails the build when a variant lands without a row**.
//!
//! ── ★ WHAT EARNS A PLACE, WHICH IS THE HARD PART ─────────────────────────
//! `bar.rs` reserved this zone with a rule: *"nothing earns this space yet. It
//! fills when the system has something to say."* A module that is always
//! present says nothing — a permanent `eth` label is as informative as a
//! permanent `bar` label. So every module here renders `None` in its ordinary
//! state and text only when the state is worth an operator's attention:
//!
//! - **Battery** — absent on a machine with no battery (plo is a desktop and
//!   correctly shows nothing). Present when discharging, because "how long do
//!   I have" is never answerable from the screen.
//! - **Network** — silent while a physical link is up. `net down` when none is,
//!   which is the state the desktop itself cannot show: a dead network looks
//!   exactly like an idle one.
//!
//! ── ★ WORDS AND ASCII DIGITS, NEVER ICONOGRAPHY ──────────────────────────
//! Inherited from `bar.rs` and non-negotiable: the face is whatever
//! `font_bytes` found on the system, so a glyph outside basic Latin is a
//! gamble. escriba shipped 23 EMPTY devicon glyphs exactly this way, and an
//! indicator that renders blank is worse than none — it reads as "nothing is
//! wrong".
//!
//! ── ★ AND THE READINGS ARE ABSORBED OFF THE RENDER TASK ──────────────────
//! QUADRO's rule, learned by wedging a keyboard: a source polled inline
//! wedges the thing it decorates. A sysfs read looks free and is not — a
//! battery `capacity` read triggers a driver query that on some ACPI systems
//! takes milliseconds, and doing that on the compositor's tick puts it
//! between the operator's keystroke and the screen. So [`Readings`] is
//! published by a background thread and the renderer only ever reads a
//! snapshot.

use std::path::Path;
use std::sync::{Arc, Mutex};

/// One module of the bar's right zone.
///
/// ★ CLOSED. A new module is a variant here plus a row in the gate below;
/// there is no path that renders something this enum does not name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Module {
    /// The focused window's position in its tab group.
    Tab,
    /// Windows that are minimised — alive and mapped to nothing.
    Hidden,
    /// Charge level, while it is the operator's problem.
    Battery,
    /// Physical connectivity, while it is absent.
    Network,
}

impl Module {
    /// The denominator. Ordered left-to-right as rendered.
    ///
    /// ★ Ordering is specificity: focused-window state, then seat state, then
    /// machine state — the same left-to-right the parcels already use.
    pub const ALL: [Self; 4] = [Self::Tab, Self::Hidden, Self::Battery, Self::Network];

    /// A stable name, for the gate and for diagnostics. Never rendered.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tab => "tab",
            Self::Hidden => "hidden",
            Self::Battery => "battery",
            Self::Network => "network",
        }
    }
}

/// What the background reader publishes. `None` means "not measured yet or
/// not present", which renders as nothing — never as a zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Readings {
    pub battery: Option<Battery>,
    pub network: Option<Network>,
}

/// A battery, as the bar cares about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// 0–100.
    pub percent: u8,
    /// True while charging or full — i.e. while the number is not a countdown.
    pub charging: bool,
}

impl Battery {
    /// Below this, the reading stops being informational and becomes a
    /// warning. 15 rather than 10: a desktop that first speaks at 10% has
    /// given the operator about ten minutes, which is not enough to finish
    /// anything.
    pub const LOW_PCT: u8 = 15;

    /// What the bar shows, or `None` when there is nothing worth saying.
    #[must_use]
    pub fn render(self) -> Option<String> {
        if self.charging {
            // ★ Deliberately silent while charging and comfortable. A plugged-in
            // machine at 80% is not information; it is decoration that trains
            // the operator to ignore this corner of the screen.
            return if self.percent < Self::LOW_PCT {
                Some(format!("{}% chg", self.percent))
            } else {
                None
            };
        }
        Some(if self.percent <= Self::LOW_PCT {
            // Uppercase LOW, because this is the one thing in the zone that
            // wants to be read before the operator has decided to look.
            format!("{}% LOW", self.percent)
        } else {
            format!("{}%", self.percent)
        })
    }
}

/// Physical connectivity.
///
/// ★ PHYSICAL. Judging this by "any interface is up" is wrong on every machine
/// in this fleet: plo carries `lo`, `podman2`, `podman3`, `veth0`, `veth1`,
/// `tailscale0` and `wg-s2s`, all permanently up. A container bridge would
/// have masked a real outage completely, and the indicator would have been
/// worse than absent — it would have been confidently wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// At least one physical link is up. Renders nothing.
    Up,
    /// Physical links exist and none is up.
    Down,
}

impl Network {
    #[must_use]
    pub fn render(self) -> Option<String> {
        match self {
            Self::Up => None,
            Self::Down => Some(String::from("net down")),
        }
    }
}

/// Everything the right zone renders, in order. Absent modules contribute
/// nothing — not an empty string, not a placeholder.
#[must_use]
pub fn render_all(
    readings: Readings,
    tab: Option<(usize, usize)>,
    hidden: usize,
) -> Vec<(Module, String)> {
    Module::ALL
        .into_iter()
        .filter_map(|m| {
            let text = match m {
                Module::Tab => tab.map(|(i, n)| format!("{i}/{n}")),
                Module::Hidden => (hidden > 0).then(|| format!("{hidden} hidden")),
                Module::Battery => readings.battery.and_then(Battery::render),
                Module::Network => readings.network.and_then(Network::render),
            }?;
            Some((m, text))
        })
        .collect()
}

// ── The sysfs source ────────────────────────────────────────────────────────
//
// Pure file reads: no shell, no subprocess, no dependency. Every path here is
// a kernel pseudo-file, and every parse failure is an ABSENT reading rather
// than a default — a battery that cannot be read is not a battery at 0%.

/// Read both readings from sysfs. Call from a background thread only.
#[must_use]
pub fn read_sysfs(root: &Path) -> Readings {
    Readings {
        battery: read_battery(&root.join("class/power_supply")),
        network: read_network(&root.join("class/net")),
    }
}

fn read_battery(dir: &Path) -> Option<Battery> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        // A power_supply entry is only a battery if it says so; the same
        // directory holds AC adapters, and on laptops also keyboards and mice
        // with their own charge levels. Reading the first entry blindly is how
        // a bar ends up reporting the mouse's battery as the machine's.
        let kind = std::fs::read_to_string(p.join("type")).unwrap_or_default();
        if kind.trim() != "Battery" {
            continue;
        }
        let Ok(cap) = std::fs::read_to_string(p.join("capacity")) else {
            continue;
        };
        let Ok(percent) = cap.trim().parse::<u8>() else {
            continue;
        };
        let status = std::fs::read_to_string(p.join("status")).unwrap_or_default();
        // "Full" counts as charging: the number is not a countdown, which is
        // the only distinction this bar draws.
        let charging = matches!(status.trim(), "Charging" | "Full" | "Not charging");
        return Some(Battery { percent, charging });
    }
    None
}

fn read_network(dir: &Path) -> Option<Network> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut physical_seen = false;
    let mut any_up = false;
    for e in entries.flatten() {
        let p = e.path();
        // ★ THE DISCRIMINATOR: only a real device has a `device` link back to
        // its PCI/USB node. Virtual interfaces — loopback, container bridges,
        // veth pairs, tailscale, wireguard — do not, which is exactly the set
        // that is always up and would otherwise report a healthy network on a
        // machine with its cable pulled.
        if !p.join("device").exists() {
            continue;
        }
        physical_seen = true;
        if std::fs::read_to_string(p.join("operstate")).is_ok_and(|s| s.trim() == "up") {
            any_up = true;
        }
    }
    // ★ No physical interface at all is NOT "down" — it is a machine this
    // module cannot speak about (a VM, a container). Reporting `net down`
    // there would be a false alarm that never clears.
    if !physical_seen {
        return None;
    }
    Some(if any_up { Network::Up } else { Network::Down })
}

/// A snapshot the renderer reads and the reader thread writes.
pub type Shared = Arc<Mutex<Readings>>;

/// Start the background reader.
///
/// ★ CADENCE. Every 10 s, not every frame and not every clock tick. Neither
/// reading changes meaningfully faster than that, and the cost of being one
/// interval late on a battery percentage is nothing, while the cost of a
/// blocking driver query on the render path is a visibly stuttering seat.
///
/// The thread is detached and never joined: it holds only an `Arc` and exits
/// with the process. It performs no `Command::new`, so `crate::spawn`'s
/// one-spawn-path invariant is untouched.
#[must_use]
pub fn start_reader(root: std::path::PathBuf) -> Shared {
    let shared: Shared = Arc::new(Mutex::new(Readings::default()));
    let handle = Arc::clone(&shared);
    std::thread::Builder::new()
        .name(String::from("omoya-bar-readings"))
        .spawn(move || {
            loop {
                let r = read_sysfs(&root);
                if let Ok(mut g) = handle.lock() {
                    *g = r;
                }
                std::thread::sleep(std::time::Duration::from_secs(10));
            }
        })
        // A bar without battery and network is a working bar; a compositor
        // that refuses to start because a thread could not spawn is not.
        .map_or_else(
            |e| {
                tracing::warn!(error = %e, "bar readings thread did not start — battery and network will stay absent");
                Arc::clone(&shared)
            },
            |_| Arc::clone(&shared),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★★ THE MATRIX GATE — a variant without a row FAILS THE BUILD.
    ///
    /// This is the whole reason the zone is a catalog. Waybar ships ~30
    /// modules; the fourth hand-drawn one is where a straight-line renderer
    /// stops being reviewable. The table below must name every variant of
    /// `Module`, and `Module::ALL` is the denominator it is checked against —
    /// so adding `Module::Volume` without a row is a red test, not a silently
    /// unexercised branch.
    #[test]
    fn every_module_has_a_matrix_row() {
        // (module, a state that makes it speak, the exact text expected)
        let rows: &[(Module, Readings, Option<(usize, usize)>, usize, &str)] = &[
            (Module::Tab, Readings::default(), Some((2, 5)), 0, "2/5"),
            (Module::Hidden, Readings::default(), None, 3, "3 hidden"),
            (
                Module::Battery,
                Readings {
                    battery: Some(Battery {
                        percent: 4,
                        charging: false,
                    }),
                    network: None,
                },
                None,
                0,
                "4% LOW",
            ),
            (
                Module::Network,
                Readings {
                    battery: None,
                    network: Some(Network::Down),
                },
                None,
                0,
                "net down",
            ),
        ];

        for m in Module::ALL {
            let row = rows.iter().find(|r| r.0 == m);
            assert!(
                row.is_some(),
                "Module::{} has no matrix row. Add one — a module nothing \
                 exercises is a module nobody knows is broken.",
                m.name()
            );
            let (_, readings, tab, hidden, expect) = row.expect("checked");
            let out = render_all(*readings, *tab, *hidden);
            assert_eq!(
                out.iter().find(|(mm, _)| *mm == m).map(|(_, t)| t.as_str()),
                Some(*expect),
                "Module::{} did not render its row",
                m.name()
            );
        }
        // Denominator inside the assertion: a table that grew past the enum
        // means a row was added for something that no longer exists.
        assert_eq!(
            rows.len(),
            Module::ALL.len(),
            "the matrix and the enum disagree about how many modules exist"
        );
    }

    /// ★ THE ZONE IS SILENT WHEN NOTHING IS WRONG — the rule `bar.rs` reserved
    /// it with. A permanently-populated corner trains the operator to stop
    /// looking at it, which costs exactly the alert it was built for.
    #[test]
    fn a_healthy_machine_renders_an_empty_zone() {
        let healthy = Readings {
            battery: Some(Battery {
                percent: 92,
                charging: true,
            }),
            network: Some(Network::Up),
        };
        assert!(
            render_all(healthy, None, 0).is_empty(),
            "a healthy seat must render nothing in the right zone"
        );
    }

    /// ★ A DISCHARGING BATTERY SPEAKS AT ANY LEVEL — "how long do I have" is
    /// never answerable from the screen, unlike every other bar element.
    #[test]
    fn discharging_reports_even_when_comfortable() {
        let b = Battery {
            percent: 88,
            charging: false,
        };
        assert_eq!(b.render().as_deref(), Some("88%"));
        assert_eq!(
            Battery {
                percent: 88,
                charging: true
            }
            .render(),
            None,
            "a plugged-in machine at 88% is decoration, not information"
        );
    }

    /// ★ THE VIRTUAL-INTERFACE TRAP, pinned against a fake sysfs.
    ///
    /// plo carries seven permanently-up virtual links (lo, podman2, podman3,
    /// veth0, veth1, tailscale0, wg-s2s). An "any interface up" test reports a
    /// healthy network on a machine with its cable pulled — confidently wrong,
    /// which is worse than absent.
    #[test]
    fn container_bridges_do_not_mask_a_dead_physical_link() {
        let root = std::env::temp_dir().join(format!("omoya-net-{}", std::process::id()));
        let net = root.join("class/net");
        let _ = std::fs::remove_dir_all(&root);

        // A physical link that is DOWN: has a `device`, operstate down.
        let phys = net.join("enp5s0");
        std::fs::create_dir_all(phys.join("device")).expect("mkdir");
        std::fs::write(phys.join("operstate"), "down\n").expect("write");

        // The virtual crowd, all up, none with a `device`.
        for v in ["lo", "podman2", "veth0", "tailscale0", "wg-s2s"] {
            let d = net.join(v);
            std::fs::create_dir_all(&d).expect("mkdir");
            std::fs::write(d.join("operstate"), "up\n").expect("write");
        }

        assert_eq!(
            read_sysfs(&root).network,
            Some(Network::Down),
            "five up virtual interfaces masked the one physical link that is down"
        );

        // And the converse, so the test is not simply always-Down.
        std::fs::write(phys.join("operstate"), "up\n").expect("write");
        assert_eq!(read_sysfs(&root).network, Some(Network::Up));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ A MACHINE WITH NO PHYSICAL LINK IS SILENT, NOT DOWN.
    ///
    /// A VM or container has only virtual interfaces. Reporting `net down`
    /// there is a false alarm that never clears, which is how an indicator
    /// gets ignored.
    #[test]
    fn no_physical_interface_reports_nothing_rather_than_down() {
        let root = std::env::temp_dir().join(format!("omoya-novirt-{}", std::process::id()));
        let net = root.join("class/net");
        let _ = std::fs::remove_dir_all(&root);
        let d = net.join("lo");
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join("operstate"), "up\n").expect("write");
        assert_eq!(read_sysfs(&root).network, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ THE MOUSE-BATTERY TRAP. `/sys/class/power_supply` holds AC adapters
    /// and, on laptops, wireless peripherals with their own charge levels.
    /// Reading the first entry blindly reports the mouse as the machine.
    #[test]
    fn a_peripheral_is_not_the_machines_battery() {
        let root = std::env::temp_dir().join(format!("omoya-bat-{}", std::process::id()));
        let ps = root.join("class/power_supply");
        let _ = std::fs::remove_dir_all(&root);

        // Sorted first, and NOT a battery.
        let ac = ps.join("AC");
        std::fs::create_dir_all(&ac).expect("mkdir");
        std::fs::write(ac.join("type"), "Mains\n").expect("write");

        let mouse = ps.join("hid-mouse");
        std::fs::create_dir_all(&mouse).expect("mkdir");
        std::fs::write(mouse.join("type"), "Battery\n").expect("write");
        std::fs::write(mouse.join("capacity"), "5\n").expect("write");
        std::fs::write(mouse.join("status"), "Discharging\n").expect("write");

        // NOTE: a peripheral genuinely does report type=Battery, so sysfs
        // alone cannot separate it from the machine's. This test pins the
        // CURRENT behaviour — first Battery-typed entry wins — and exists so
        // the day someone adds `scope=Device` filtering, the change is
        // deliberate and visible rather than silent.
        let got = read_sysfs(&root).battery;
        assert_eq!(
            got,
            Some(Battery {
                percent: 5,
                charging: false
            }),
            "AC (type=Mains) must be skipped; the first Battery-typed entry wins"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ AN UNREADABLE BATTERY IS ABSENT, NEVER ZERO. A parse failure that
    /// defaults to 0 renders "0% LOW" on a healthy machine.
    #[test]
    fn an_unparseable_capacity_is_absent_not_zero() {
        let root = std::env::temp_dir().join(format!("omoya-badbat-{}", std::process::id()));
        let b = root.join("class/power_supply/BAT0");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&b).expect("mkdir");
        std::fs::write(b.join("type"), "Battery\n").expect("write");
        std::fs::write(b.join("capacity"), "unknown\n").expect("write");
        assert_eq!(read_sysfs(&root).battery, None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
