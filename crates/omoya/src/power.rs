use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use blue_lang_worker::{OnceRun, run_once};
use smithay::reexports::calloop::channel::Sender;

use crate::logind::LogindPower;
use crate::world::SeatMessage;

const TOOL_LIMIT: Duration = Duration::from_secs(5);
const BACKLIGHT_SUBSYSTEM: &str = "backlight";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub name: String,
    pub max: u32,
    pub dir: PathBuf,
}

fn read_number(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn kind_rank(kind: &str) -> u8 {
    match kind.trim() {
        "firmware" => 0,
        "platform" => 1,
        _ => 2,
    }
}

#[must_use]
pub fn find_panel(class_dir: &Path) -> Option<Panel> {
    let mut panels: Vec<(u8, Panel)> = std::fs::read_dir(class_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let max = read_number(&dir.join("max_brightness")).filter(|m| *m > 0)?;
            let kind = std::fs::read_to_string(dir.join("type")).unwrap_or_default();
            let name = entry.file_name().to_string_lossy().into_owned();
            Some((kind_rank(&kind), Panel { name, max, dir }))
        })
        .collect();
    panels.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    panels.into_iter().next().map(|(_, panel)| panel)
}

#[must_use]
pub fn stepped(current: u32, max: u32, step_pct: i8) -> u32 {
    let floor = (max / 100).max(1);
    let step = (u64::from(max) * u64::from(step_pct.unsigned_abs()) / 100).max(1);
    let step = u32::try_from(step).unwrap_or(max);
    let next = if step_pct >= 0 {
        current.saturating_add(step)
    } else {
        current.saturating_sub(step)
    };
    next.clamp(floor, max)
}

#[must_use]
pub fn percent(current: u32, max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    let value = u64::from(current) * 100 / u64::from(max);
    u32::try_from(value).unwrap_or(100)
}

#[must_use]
pub fn volume_args(step_pct: i8) -> Vec<String> {
    let sign = if step_pct >= 0 { '+' } else { '-' };
    vec![
        "wpctl".to_owned(),
        "set-volume".to_owned(),
        "-l".to_owned(),
        "1.0".to_owned(),
        "@DEFAULT_AUDIO_SINK@".to_owned(),
        format!("{}%{sign}", step_pct.unsigned_abs()),
    ]
}

#[must_use]
pub fn mute_args() -> Vec<String> {
    vec![
        "wpctl".to_owned(),
        "set-mute".to_owned(),
        "@DEFAULT_AUDIO_SINK@".to_owned(),
        "toggle".to_owned(),
    ]
}

#[must_use]
pub fn volume_sign(wpctl_output: &str) -> Option<String> {
    let rest = wpctl_output.trim().strip_prefix("Volume:")?.trim();
    let muted = rest.contains("[MUTED]");
    let level: f64 = rest.split_whitespace().next()?.parse().ok()?;
    if muted {
        return Some("vol muted".to_owned());
    }
    Some(format!("vol {:.0}%", level * 100.0))
}

fn run_tool(argv: &[String]) -> Result<String, String> {
    let Some((program, rest)) = argv.split_first() else {
        return Err("an empty command".to_owned());
    };
    if !crate::spawn::resolves(program) {
        return Err(format!("{program}: no such program"));
    }
    let mut command = crate::spawn::worker_command(Path::new(program));
    command.args(rest);
    match run_once(command, TOOL_LIMIT) {
        OnceRun::Finished { stdout } => Ok(String::from_utf8_lossy(&stdout).into_owned()),
        OnceRun::TimedOut => Err(format!(
            "{program} gave no answer in {} s",
            TOOL_LIMIT.as_secs()
        )),
        OnceRun::Failed(e) => Err(format!("{program}: {e}")),
    }
}

fn get_volume() -> Vec<String> {
    vec![
        "wpctl".to_owned(),
        "get-volume".to_owned(),
        "@DEFAULT_AUDIO_SINK@".to_owned(),
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PowerCommand {
    Backlight(i8),
    Volume(i8),
    Mute,
    Suspend,
    TakeSleepInhibitor,
    ReleaseSleepInhibitor,
    LockedHint(bool),
    IdleHint(bool),
}

fn flash(sender: &Sender<SeatMessage>, text: String) {
    let _ = sender.send(SeatMessage::Flash(text));
}

fn refuse(sender: &Sender<SeatMessage>, what: &str, why: String) {
    tracing::warn!(what, why, "a power command was refused");
    let _ = sender.send(SeatMessage::PowerRefused(format!("{what}: {why}")));
}

fn backlight(
    logind: Option<&LogindPower>,
    class_dir: &Path,
    step: i8,
    sender: &Sender<SeatMessage>,
) {
    let Some(logind) = logind else {
        refuse(
            sender,
            "backlight",
            "this seat has no logind session".to_owned(),
        );
        return;
    };
    let Some(panel) = find_panel(class_dir) else {
        refuse(
            sender,
            "backlight",
            "no panel under /sys/class/backlight".to_owned(),
        );
        return;
    };
    let Some(current) = read_number(&panel.dir.join("brightness")) else {
        refuse(
            sender,
            "backlight",
            format!("{} cannot be read", panel.name),
        );
        return;
    };
    let next = stepped(current, panel.max, step);
    match logind.set_brightness(BACKLIGHT_SUBSYSTEM, &panel.name, next) {
        Ok(()) => flash(sender, format!("light {}%", percent(next, panel.max))),
        Err(e) => refuse(sender, "backlight", e.to_string()),
    }
}

fn sound(argv: &[String], sender: &Sender<SeatMessage>) {
    if let Err(why) = run_tool(argv) {
        refuse(sender, "volume", why);
        return;
    }
    match run_tool(&get_volume()) {
        Ok(text) => {
            if let Some(sign) = volume_sign(&text) {
                flash(sender, sign);
            }
        }
        Err(why) => refuse(sender, "volume", why),
    }
}

fn power_loop(
    logind: Option<LogindPower>,
    class_dir: &Path,
    sender: &Sender<SeatMessage>,
    rx: &mpsc::Receiver<PowerCommand>,
) {
    let mut inhibitor: Option<std::os::fd::OwnedFd> = None;
    while let Ok(command) = rx.recv() {
        match command {
            PowerCommand::Backlight(step) => backlight(logind.as_ref(), class_dir, step, sender),
            PowerCommand::Volume(step) => sound(&volume_args(step), sender),
            PowerCommand::Mute => sound(&mute_args(), sender),
            PowerCommand::TakeSleepInhibitor => {
                if inhibitor.is_none()
                    && let Some(logind) = &logind
                {
                    match logind.inhibit_sleep() {
                        Ok(fd) => inhibitor = Some(fd),
                        Err(e) => refuse(sender, "sleep", e.to_string()),
                    }
                }
            }
            PowerCommand::ReleaseSleepInhibitor => inhibitor = None,
            PowerCommand::Suspend => match &logind {
                Some(logind) => {
                    if let Err(e) = logind.suspend() {
                        refuse(sender, "suspend", e.to_string());
                    }
                }
                None => refuse(
                    sender,
                    "suspend",
                    "this seat has no logind session".to_owned(),
                ),
            },
            PowerCommand::LockedHint(locked) => {
                if let Some(logind) = &logind
                    && let Err(e) = logind.set_locked_hint(locked)
                {
                    tracing::debug!(error = %e, "logind took no locked hint");
                }
            }
            PowerCommand::IdleHint(idle) => {
                if let Some(logind) = &logind
                    && let Err(e) = logind.set_idle_hint(idle)
                {
                    tracing::debug!(error = %e, "logind took no idle hint");
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct Power {
    tx: mpsc::Sender<PowerCommand>,
}

impl Power {
    #[must_use]
    pub fn start(logind: Option<LogindPower>, sender: Sender<SeatMessage>) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("omoya-power".to_owned())
            .spawn(move || {
                power_loop(logind, Path::new("/sys/class/backlight"), &sender, &rx);
            });
        match spawned {
            Ok(_) => Some(Self { tx }),
            Err(e) => {
                tracing::error!(error = %e, "no power thread; brightness and volume keys do nothing");
                None
            }
        }
    }

    pub fn send(&self, command: PowerCommand) -> bool {
        self.tx.send(command).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(tag: &str, panels: &[(&str, &str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("omoya-power-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (name, kind, max) in panels {
            let p = dir.join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("type"), kind).unwrap();
            std::fs::write(p.join("max_brightness"), max).unwrap();
            std::fs::write(p.join("brightness"), "100").unwrap();
        }
        dir
    }

    #[test]
    fn zeks_panel_is_found_and_a_firmware_panel_wins() {
        let zek = class("zek", &[("intel_backlight", "raw\n", "400\n")]);
        let panel = find_panel(&zek).unwrap();
        assert_eq!((panel.name.as_str(), panel.max), ("intel_backlight", 400));
        let both = class(
            "both",
            &[
                ("intel_backlight", "raw\n", "400\n"),
                ("acpi_video0", "firmware\n", "10\n"),
            ],
        );
        assert_eq!(find_panel(&both).unwrap().name, "acpi_video0");
        let none = class("none", &[("dead", "raw\n", "0\n")]);
        assert_eq!(find_panel(&none), None);
        assert_eq!(find_panel(Path::new("/nonexistent/backlight")), None);
    }

    #[test]
    fn a_step_on_zeks_panel_moves_twenty_raw_units() {
        assert_eq!(stepped(200, 400, 5), 220);
        assert_eq!(stepped(200, 400, -5), 180);
        assert_eq!(percent(220, 400), 55);
    }

    #[test]
    fn the_panel_never_steps_to_black_and_never_past_its_max() {
        assert_eq!(stepped(4, 400, -5), 4, "1% of 400 is the floor");
        assert_eq!(stepped(0, 400, -5), 4);
        assert_eq!(stepped(395, 400, 5), 400);
        assert_eq!(stepped(1, 10, -50), 1);
        assert_eq!(stepped(5, 7, 1), 6, "a tiny range still takes one step");
    }

    #[test]
    fn percent_of_a_dead_range_is_zero() {
        assert_eq!(percent(5, 0), 0);
        assert_eq!(percent(400, 400), 100);
    }

    #[test]
    fn a_step_renders_to_wpctl_and_the_limit_holds_at_full() {
        assert_eq!(
            volume_args(5),
            [
                "wpctl",
                "set-volume",
                "-l",
                "1.0",
                "@DEFAULT_AUDIO_SINK@",
                "5%+"
            ]
        );
        assert_eq!(volume_args(-10).last().unwrap(), "10%-");
        assert_eq!(mute_args()[1], "set-mute");
    }

    #[test]
    fn wpctl_answers_read_as_a_small_sign() {
        assert_eq!(volume_sign("Volume: 0.45\n").as_deref(), Some("vol 45%"));
        assert_eq!(
            volume_sign("Volume: 0.45 [MUTED]\n").as_deref(),
            Some("vol muted")
        );
        assert_eq!(volume_sign("Volume: 1.00").as_deref(), Some("vol 100%"));
        assert_eq!(volume_sign("nonsense"), None);
        assert_eq!(volume_sign("Volume: loud"), None);
    }

    #[test]
    fn a_missing_tool_is_a_named_refusal_not_a_panic() {
        let why = run_tool(&["/nonexistent/wpctl".to_owned()]).unwrap_err();
        assert!(why.contains("wpctl"), "{why}");
        assert!(run_tool(&[]).is_err());
    }
}
