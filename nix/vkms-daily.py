import json
import time

NIGHT = "46 52 64"
HIDDEN = f"{NIGHT} 20 24 34"


def leaf(name):
    return json.loads(machine.succeed(f"kanshou-call {name}"))


def call(*segments):
    return machine.succeed("kanshou-call " + " ".join(segments))


def wait_leaf(name, want, seconds=30):
    deadline = time.time() + seconds
    got = None
    while time.time() < deadline:
        got = leaf(name)
        if got == want:
            return
        time.sleep(0.25)
    raise AssertionError(f"{name} is {got!r}, not {want!r}")


def share(path, colours=NIGHT):
    machine.succeed(f"kanshou-capture {path}")
    return float(machine.succeed(f"ppm-share {path} {colours}").strip())


def write_world(text):
    machine.succeed("install -d -o seat /home/seat/.config /home/seat/.config/omoya")
    machine.succeed(f"cat > /home/seat/.config/omoya/omoya.b <<'EOF'\n{text}\nEOF")
    machine.succeed("chown seat /home/seat/.config/omoya/omoya.b")


WORLD = """
use("kurashi", %i[bind command idle keyboard lock_with notes swaylock])

def omoya()
  [
    keyboard(%w[us us+intl]),
    lock_with(swaylock(:night0)),
    notes({enabled: true, ms: 60000, max: 3}),
    idle({blank_s: %BLANK%}),
    command(:spare, %w[true]),
    bind("super+g", :spare)
  ]
end
"""

machine.wait_for_unit("multi-user.target")
machine.succeed("modprobe vkms")
machine.wait_until_succeeds("test -e /dev/dri/card0")
machine.wait_until_succeeds("grep -q connected /sys/class/drm/card0-Virtual-1/status")
write_world(WORLD.replace("%BLANK%", "0"))
machine.wait_until_succeeds("loginctl list-sessions --no-legend | grep -q seat0")
machine.send_chars(
    "exec omoya --backend drm --session logind -- weston-presentation-shm > /tmp/omoya.log 2>&1\n"
)
machine.wait_until_succeeds("kanshou-call world_state | grep -q loaded", timeout=60)
print(machine.succeed("tail -20 /tmp/omoya.log"))

try:
    with subtest("the world stood, entry by entry"):
        assert leaf("world_refusals") == [], leaf("world_refusals")
        assert "spare" in leaf("world_commands")
        assert leaf("ukeire_keymap_layout") == "us"

    with subtest("a layout switch reaches the seat"):
        call("do", "next-layout")
        wait_leaf("ukeire_keymap_layout", "us+intl")
        call("do", "next-layout")
        wait_leaf("ukeire_keymap_layout", "us")

    with subtest("the desktop draws before any lock"):
        machine.sleep(3)
        before = share("/tmp/open.ppm")
        print(f"open: {before}")
        assert before < 0.95, f"the desktop looks locked already: {before}"

    with subtest("a media key reaches its deed through a real keyboard"):
        machine.send_key("volumeup")
        machine.wait_until_succeeds("kanshou-call world_refusals | grep -q wpctl")

    with subtest("a lock hides the desktop and swaylock holds it"):
        sid = machine.succeed(
            "loginctl list-sessions --no-legend | awk '$4 == \"seat0\" {print $1; exit}'"
        ).strip()
        machine.succeed(f"loginctl lock-session {sid}")
        wait_leaf("lock_state", "locked")
        machine.wait_until_succeeds("pgrep -x .swaylock-wrapp", timeout=30)
        locked = share("/tmp/locked.ppm")
        print(f"locked: {locked}")
        assert locked > 0.95, f"the desktop shows through a lock: {locked}"

    with subtest("the crash: killing the client exposes nothing"):
        machine.succeed("pkill -9 -x .swaylock-wrapp")
        wait_leaf("lock_holder_deaths", 1)
        crashed = share("/tmp/crashed.ppm", HIDDEN)
        print(f"crashed: {crashed}")
        assert crashed > 0.95, f"a dead lock client exposed the desktop: {crashed}"
        assert leaf("lock_state").startswith("locked")

    with subtest("a new client takes over by itself"):
        machine.wait_until_succeeds("pgrep -x .swaylock-wrapp", timeout=30)
        wait_leaf("lock_state", "locked")
        assert share("/tmp/respawned.ppm") > 0.95

    with subtest("a shot behind the lock is refused, and so is any other deed"):
        call("do", "screenshot")
        call("do", "close")
        machine.sleep(1)
        assert machine.succeed("ls /home/seat/Pictures 2>/dev/null | wc -l || true").strip() == "0"

    with subtest("the password unlocks through the real keyboard and PAM"):
        machine.sleep(2)
        machine.send_chars("seat\n")
        wait_leaf("lock_state", "open", seconds=60)
        machine.sleep(2)
        opened = share("/tmp/opened.ppm")
        print(f"opened: {opened}")
        assert opened < 0.95, f"the desktop did not come back: {opened}"

    with subtest("a sleep locks first, and the resume is witnessed"):
        machine.succeed("systemctl suspend")
        wait_leaf("sleeps", 1, seconds=30)
        wait_leaf("resumes", 1, seconds=60)
        assert leaf("lock_state").startswith("lock"), leaf("lock_state")
        assert share("/tmp/slept.ppm", HIDDEN) > 0.95
        machine.wait_until_succeeds("pgrep -x .swaylock-wrapp", timeout=30)
        wait_leaf("lock_state", "locked")
        machine.sleep(2)
        machine.send_chars("seat\n")
        wait_leaf("lock_state", "open", seconds=60)

    with subtest("notify-send draws in the seat"):
        machine.succeed(
            "su seat -c 'DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus "
            "notify-send daily-seat works'"
        )
        wait_leaf("notes_shown", 1)

    with subtest("a screenshot chord writes a real PNG"):
        call("do", "screenshot")
        machine.wait_until_succeeds("ls /home/seat/Pictures/omoya-*.png")
        machine.succeed("file /home/seat/Pictures/omoya-*.png | grep -q 'PNG image data, 1024 x 768'")

    with subtest("grim takes the frame through screencopy"):
        socket = leaf("socket")
        machine.succeed(
            "su seat -c 'XDG_RUNTIME_DIR=/run/user/$(id -u) "
            f"WAYLAND_DISPLAY={socket} grim /tmp/grim.png'"
        )
        machine.succeed("file /tmp/grim.png | grep -q 'PNG image data, 1024 x 768'")

    with subtest("saving the world reloads it: the panel blanks and wakes"):
        generation = leaf("world_generation")
        write_world(WORLD.replace("%BLANK%", "3"))
        machine.wait_until_succeeds(
            f'test "$(kanshou-call world_generation)" -gt {generation}', timeout=30
        )
        wait_leaf("panel_state", "dark", seconds=30)
        machine.wait_until_succeeds("grep -qi off /sys/class/drm/card0-Virtual-1/dpms")
        machine.send_key("shift")
        wait_leaf("panel_state", "on", seconds=30)
        machine.wait_until_succeeds("grep -qi on /sys/class/drm/card0-Virtual-1/dpms")

    with subtest("a world that stops parsing leaves what stood"):
        write_world("def ((( broken")
        wait_leaf("world_state", "refused", seconds=30)
        assert "spare" in leaf("world_commands")
finally:
    print(machine.execute("ps -eo pid,comm,args | tail -30; cat /tmp/omoya.log | tail -60")[1])
