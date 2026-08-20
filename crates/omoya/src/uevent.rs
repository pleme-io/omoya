//! The udev monitor's netlink wire, spoken directly — no libudev, no `.so`.
//!
//! ── ★ WHY THIS IS THE ONLY CORRECTLY-TIMED HOTPLUG SOURCE ─────────────────
//! A hotplugged `/dev/input/event*` must be opened through the session
//! (`logind TakeDevice`), and logind REFUSES a device udev has not finished
//! processing. Measured in systemd v257:
//!
//!   * `logind-session-device.c:333-346` — `TakeDevice` looks the device up in
//!     the manager's seat-device map and, on a miss, calls
//!     `manager_process_seat_device()`.
//!   * `logind-core.c:263-264` — that function treats any device for which
//!     `sd_device_has_current_tag(d, "seat") <= 0` as a REMOVAL and returns.
//!   * `sd-device.c:2219-2229` — `sd_device_has_current_tag` reads the udev
//!     DATABASE (`device_read_db`).
//!
//! So `TakeDevice` on a device with no udev db entry returns `-ENODEV`. And
//! the node exists long before that entry does — Linux v6.12
//! `drivers/base/core.c:3646` calls `devtmpfs_create_node()` and only then
//! `:3653` `kobject_uevent(KOBJ_ADD)`. inotify sees the node at step one
//! (`fs/namei.c:4135-4137`, `vfs_mknod` → `fsnotify_create`), and the raw
//! kernel netlink group sees the uevent at step two; udev's own group sees it
//! at step three, AFTER the database is written. Only step three is in time.
//!
//! ── ★ WHY SPEAKING udev's WIRE IS NOT "REINTRODUCING libudev" ─────────────
//! Same posture `crate::logind` takes with libseat: a protocol is a WIRE, so
//! speak it and own the executor. This costs zero shared objects — the socket,
//! the bind and the recvmsg are `smithay::reexports::rustix`, which is already
//! in the closure because smithay itself depends on it.
//!
//! ── ★ THE PERMISSION FACT THAT MAKES THIS SAFE TO READ UNPRIVILEGED ───────
//! Linux v6.12 `lib/kobject_uevent.c:770-777`: the uevent netlink socket is
//! created with `NL_CFG_F_NONROOT_RECV` and WITHOUT `NL_CFG_F_NONROOT_SEND`.
//! An unprivileged compositor may subscribe; an unprivileged process may not
//! broadcast. `af_netlink.c:2015-2016` rounds the group count up to 32, so
//! binding group 2 is legal even though the kernel declared `.groups = 1`.
//!
//! The remaining spoofer is a user-namespace root, which is why the sender's
//! `SCM_CREDENTIALS` uid must be 0 as seen from OUR namespace — the same check
//! systemd makes at `device-monitor.c:628-635`, in its strict form.

use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

use smithay::reexports::rustix;

/// `MONITOR_GROUP_UDEV` — systemd `device-monitor-private.h:13`. Group 1 is
/// the raw kernel stream and is deliberately NOT subscribed: see the header.
const MONITOR_GROUP_UDEV: u32 = 2;
/// `UDEV_MONITOR_MAGIC` — systemd `device-monitor.c:70`. Stored big-endian.
const UDEV_MONITOR_MAGIC: u32 = 0xfeed_cafe;
/// systemd `device-monitor.c:73` — the prefix separating a udev message from a
/// raw kernel one.
const LIBUDEV_PREFIX: &[u8] = b"libudev\0";
/// `monitor_netlink_header` — prefix[8] + 8 × u32 (`device-monitor.c:72-89`).
const HEADER_LEN: usize = 40;

/// A device appearing or disappearing, already narrowed to evdev nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hotplug {
    /// udev finished processing a new `/dev/input/event*`. Safe to `TakeDevice`.
    Added(PathBuf),
    /// The node is gone. logind has already revoked our fd — see
    /// `logind-session-device.c:415-434`.
    Removed(PathBuf),
}

/// The subscription. One datagram socket; nothing else.
pub struct UeventMonitor {
    sock: OwnedFd,
}

impl std::fmt::Debug for UeventMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UeventMonitor").finish_non_exhaustive()
    }
}

impl AsFd for UeventMonitor {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.sock.as_fd()
    }
}

impl UeventMonitor {
    /// Subscribe to udev's monitor group.
    ///
    /// # Errors
    /// If the socket cannot be created or bound. On a system with no udev this
    /// binds fine and simply never delivers — a seat with static devices, not
    /// a failure.
    pub fn new() -> rustix::io::Result<Self> {
        use rustix::net::{AddressFamily, SocketFlags, SocketType, bind, netlink, socket_with};

        let sock = socket_with(
            AddressFamily::NETLINK,
            SocketType::DGRAM,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            Some(netlink::KOBJECT_UEVENT),
        )?;

        // ★ Required, or `recvmsg` carries no `SCM_CREDENTIALS` and the sender
        // check below can never pass — a monitor that silently drops every
        // message. systemd sets the same option (`device-monitor.c`).
        rustix::net::sockopt::set_socket_passcred(&sock, true)?;

        // A hub plug-in bursts dozens of uevents. Best-effort: an undersized
        // buffer costs dropped events, not a broken seat, so a refusal here is
        // not worth failing the whole backend for.
        let _ = rustix::net::sockopt::set_socket_recv_buffer_size(&sock, 2 * 1024 * 1024);

        // pid = 0 lets the kernel assign the port id. Binding a fixed pid is
        // how two monitors in one process collide with EADDRINUSE.
        bind(&sock, &netlink::SocketAddrNetlink::new(0, MONITOR_GROUP_UDEV))?;

        Ok(Self { sock })
    }

    /// Read every pending datagram. Never blocks — the socket is `NONBLOCK`.
    pub fn drain<F: FnMut(Hotplug)>(&self, mut sink: F) {
        use rustix::io::{Errno, IoSliceMut};
        use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, recvmsg};

        let mut buf = [0u8; 8192];
        loop {
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmCredentials(1))];
            let mut control = RecvAncillaryBuffer::from(&mut space[..]);

            let received = {
                let mut iov = [IoSliceMut::new(&mut buf)];
                recvmsg(&self.sock, &mut iov, &mut control, RecvFlags::empty())
            };

            let msg = match received {
                Ok(m) => m,
                // The normal end of a drain.
                Err(Errno::AGAIN) => return,
                Err(Errno::INTR) => continue,
                Err(e) => {
                    tracing::warn!(error = ?e, "uevent recvmsg failed — hotplug is now blind");
                    return;
                }
            };

            // ★ THE SENDER CHECK. Only root may have sent this. A
            // user-namespace root maps to a NON-zero uid as seen from here, so
            // the strict form is also the correct one.
            let mut from_root = false;
            for anc in control.drain() {
                if let RecvAncillaryMessage::ScmCredentials(cred) = anc {
                    from_root = cred.uid.is_root();
                }
            }
            if !from_root {
                continue;
            }

            // ★ THE GROUP CHECK. A unicast datagram (`nl_groups == 0`) is an
            // impersonation attempt, not a broadcast — systemd rejects it the
            // same way at `device-monitor.c:615-621`.
            let Some(addr) = msg.address else { continue };
            let Ok(nl) = rustix::net::netlink::SocketAddrNetlink::try_from(addr) else {
                continue;
            };
            if nl.groups() != MONITOR_GROUP_UDEV {
                continue;
            }

            if let Some(event) = parse(&buf[..msg.bytes.min(buf.len())]) {
                sink(event);
            }
        }
    }
}

/// Decode one udev monitor datagram into a hotplug event.
///
/// Pure: no syscalls, no fds — so this is unit-testable on any host, which is
/// the half of the wire most likely to be wrong.
///
/// ★ `properties_len` IS DELIBERATELY NOT USED. systemd validates only
/// `properties_off` and then reads to the END of the datagram
/// (`device-monitor.c:645-668`, `device_new_from_nulstr(..., n - offset)`).
/// Trusting the length field instead would cut the property list short on any
/// sender that pads.
fn parse(msg: &[u8]) -> Option<Hotplug> {
    if msg.len() < HEADER_LEN || !msg.starts_with(LIBUDEV_PREFIX) {
        return None;
    }
    // ★ NETWORK order for the magic, HOST order for the offsets. The struct
    // comment says so field by field; making it uniform either way silently
    // rejects every message on one endianness.
    if u32::from_be_bytes(msg[8..12].try_into().ok()?) != UDEV_MONITOR_MAGIC {
        return None;
    }
    let off = u32::from_ne_bytes(msg[16..20].try_into().ok()?) as usize;
    if off < HEADER_LEN || off >= msg.len() {
        return None;
    }

    let mut action: Option<&[u8]> = None;
    let mut subsystem: Option<&[u8]> = None;
    let mut devname: Option<&[u8]> = None;
    for prop in msg[off..].split(|b| *b == 0) {
        let Some(eq) = prop.iter().position(|b| *b == b'=') else {
            continue;
        };
        match &prop[..eq] {
            b"ACTION" => action = Some(&prop[eq + 1..]),
            b"SUBSYSTEM" => subsystem = Some(&prop[eq + 1..]),
            b"DEVNAME" => devname = Some(&prop[eq + 1..]),
            _ => {}
        }
    }

    if subsystem? != b"input" {
        return None;
    }
    // udev normalises DEVNAME to an absolute path; the kernel's own form is
    // relative. Both are accepted rather than assumed.
    let name = std::str::from_utf8(devname?).ok()?;
    let path = if name.starts_with('/') {
        PathBuf::from(name)
    } else {
        Path::new("/dev").join(name)
    };
    if !path.file_name()?.to_str()?.starts_with("event") {
        return None;
    }

    match action? {
        b"add" => Some(Hotplug::Added(path)),
        b"remove" => Some(Hotplug::Removed(path)),
        // `bind`/`unbind`/`change` are real actions that are NOT appearances or
        // disappearances of the node. Acting on them would re-open a device we
        // already hold.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a datagram in the shape `device-monitor.c:709-724` sends.
    fn datagram(props: &[&str]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(LIBUDEV_PREFIX);
        v.extend_from_slice(&UDEV_MONITOR_MAGIC.to_be_bytes()); // magic, BE
        let hdr = u32::try_from(HEADER_LEN).expect("a 40-byte header fits in u32");
        v.extend_from_slice(&hdr.to_ne_bytes()); // header_size
        v.extend_from_slice(&hdr.to_ne_bytes()); // properties_off
        let mut body = Vec::new();
        for p in props {
            body.extend_from_slice(p.as_bytes());
            body.push(0);
        }
        let blen = u32::try_from(body.len()).expect("a test fixture fits in u32");
        v.extend_from_slice(&blen.to_ne_bytes()); // properties_len
        v.extend_from_slice(&[0u8; 16]); // the four hash fields
        assert_eq!(v.len(), HEADER_LEN);
        v.extend_from_slice(&body);
        v
    }

    #[test]
    fn an_add_for_an_evdev_node_is_recognised() {
        let d = datagram(&["ACTION=add", "SUBSYSTEM=input", "DEVNAME=/dev/input/event7"]);
        assert_eq!(
            parse(&d),
            Some(Hotplug::Added(PathBuf::from("/dev/input/event7")))
        );
    }

    #[test]
    fn a_relative_devname_is_rooted_at_dev() {
        let d = datagram(&["ACTION=remove", "SUBSYSTEM=input", "DEVNAME=input/event3"]);
        assert_eq!(
            parse(&d),
            Some(Hotplug::Removed(PathBuf::from("/dev/input/event3")))
        );
    }

    #[test]
    fn the_js_node_of_the_same_device_is_not_an_evdev_node() {
        // ★ A joystick raises BOTH `event*` and `js*` under subsystem=input.
        // Opening the js node would take a device whose event stream this
        // backend cannot decode.
        let d = datagram(&["ACTION=add", "SUBSYSTEM=input", "DEVNAME=/dev/input/js0"]);
        assert_eq!(parse(&d), None);
    }

    #[test]
    fn a_drm_hotplug_is_not_an_input_hotplug() {
        let d = datagram(&["ACTION=add", "SUBSYSTEM=drm", "DEVNAME=/dev/dri/card1"]);
        assert_eq!(parse(&d), None);
    }

    #[test]
    fn a_raw_kernel_message_is_refused_for_lacking_the_prefix() {
        // ★ What group 1 delivers: `add@/devices/...` then the properties. It
        // has no header, so `properties_off` would be read out of the property
        // text. Refusing on the prefix is what stops that.
        let mut raw = b"add@/devices/virtual/input/input9\0".to_vec();
        raw.extend_from_slice(b"ACTION=add\0SUBSYSTEM=input\0DEVNAME=/dev/input/event9\0");
        assert_eq!(parse(&raw), None);
    }

    #[test]
    fn a_wrong_magic_is_refused() {
        let mut d = datagram(&["ACTION=add", "SUBSYSTEM=input", "DEVNAME=/dev/input/event1"]);
        d[8] ^= 0xff;
        assert_eq!(parse(&d), None);
    }

    #[test]
    fn a_properties_offset_past_the_end_cannot_panic() {
        let mut d = datagram(&["ACTION=add", "SUBSYSTEM=input", "DEVNAME=/dev/input/event1"]);
        d[16..20].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(parse(&d), None);
    }
}
