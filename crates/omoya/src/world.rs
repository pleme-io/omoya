use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use blue_lang_worker::child::{self, DEFAULT_FUEL, DEFAULT_MEMORY_MB, Judge, ServeArgs};
use blue_lang_worker::{Event, OnceRun, Pool, Refusal, Reply, WorkerSpec, run_once, within};
use serde_json::{Value, json};
use smithay::reexports::calloop::channel::Sender;

use crate::config::OmoyaConfig;
use crate::deed::SeatCommand;

pub const JUDGE: Judge<'static> = Judge {
    entry: "omoya",
    program: "use(\"kurashi\", [:declare])\n\ndef judged(entries)\n  d = declare(entries)\n  {accepted: d[:accepted], off: d[:off], refusals: map((r) -> {kind: r[0], why: r[1]}, d[:refusals])}\nend\n",
};

pub const WORLD_LIMIT: Duration = Duration::from_secs(10);
pub const FIRST_PATIENCE: Duration = Duration::from_millis(400);
pub const DEFAULT_EVERY_S: u64 = 30;
pub const CELL_MAX_CHARS: usize = 48;
const SETTLE: Duration = Duration::from_millis(120);
const FLASH_FOR: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldBind {
    pub chord: String,
    pub action: String,
    pub locked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarModuleSpec {
    pub name: String,
    pub file: PathBuf,
    pub allow: Vec<String>,
    pub every: Duration,
    pub fuel: usize,
    pub soft_ms: u64,
    pub hard_ms: u64,
}

impl BarModuleSpec {
    #[must_use]
    pub fn new(name: &str, dir: &Path) -> Self {
        Self {
            name: name.to_owned(),
            file: dir.join(format!("{name}.b")),
            allow: Vec::new(),
            every: Duration::from_secs(DEFAULT_EVERY_S),
            fuel: DEFAULT_FUEL,
            soft_ms: blue_lang_worker::pool::DEFAULT_SOFT_MS,
            hard_ms: blue_lang_worker::pool::DEFAULT_HARD_MS,
        }
    }

    fn worker_spec(&self) -> WorkerSpec {
        let mut spec = WorkerSpec::new(
            &self.name,
            child::serve_args(&ServeArgs {
                file: self.file.clone(),
                allow: self.allow.clone(),
                fuel: self.fuel,
                memory_mb: DEFAULT_MEMORY_MB,
            }),
        );
        spec.soft_ms = self.soft_ms;
        spec.hard_ms = self.hard_ms;
        spec
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldState {
    Absent,
    Loaded,
    Refused(Refusal),
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub config: OmoyaConfig,
    pub binds: Vec<WorldBind>,
    pub commands: BTreeMap<String, SeatCommand>,
    pub modules: Vec<BarModuleSpec>,
    pub refusals: Vec<Refusal>,
    pub state: WorldState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorldLoad {
    Absent,
    Judged(Value),
    Refused(Refusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarControl {
    Set(Vec<BarModuleSpec>),
    Pause,
    Resume,
}

#[derive(Debug)]
pub enum SeatMessage {
    World(Box<Resolved>),
    Cell { name: String, text: Option<String> },
    ModuleRefused { name: String, refusal: Refusal },
    Flash(String),
    PowerRefused(String),
    PrepareForSleep(bool),
    LockSignal,
    NoteLine(crate::notes::WorkerLine),
}

#[must_use]
pub fn world_file() -> PathBuf {
    if let Some(explicit) = std::env::var_os("OMOYA_WORLD") {
        return PathBuf::from(explicit);
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config.join("omoya").join("omoya.b")
}

#[must_use]
pub fn read_world(exe: &Path, file: &Path) -> WorldLoad {
    if !file.exists() {
        return WorldLoad::Absent;
    }
    if !crate::spawn::resolves(&exe.to_string_lossy()) {
        return WorldLoad::Refused(Refusal::new(
            "spawn",
            format!("{} is not runnable", exe.display()),
        ));
    }
    let mut command = crate::spawn::worker_command(exe);
    command.args(child::judged_args(file));
    match run_once(command, WORLD_LIMIT) {
        OnceRun::Finished { stdout } => {
            let text = String::from_utf8_lossy(&stdout);
            match serde_json::from_str::<Reply>(text.trim()) {
                Ok(Reply::Answer { value, .. }) => WorldLoad::Judged(value),
                Ok(Reply::LoadRefused { load_refused }) => WorldLoad::Refused(load_refused),
                Ok(Reply::Refused { refused, .. }) => WorldLoad::Refused(refused),
                Err(_) => {
                    WorldLoad::Refused(Refusal::new("died", "the world worker wrote no answer"))
                }
            }
        }
        OnceRun::TimedOut => WorldLoad::Refused(Refusal::new(
            "killed",
            format!("the world gave no answer in {} ms", WORLD_LIMIT.as_millis()),
        )),
        OnceRun::Failed(e) => WorldLoad::Refused(Refusal::new("spawn", e.to_string())),
    }
}

fn trial_merge(config: &OmoyaConfig, path: &[&str], value: &Value) -> Result<OmoyaConfig, String> {
    let mut whole = serde_json::to_value(config).map_err(|e| e.to_string())?;
    let Some((leaf, groups)) = path.split_last() else {
        return Err("an empty path".to_owned());
    };
    let mut at = &mut whole;
    for group in groups {
        let Value::Object(map) = at else {
            return Err(format!("{group} holds no fields"));
        };
        at = map.entry((*group).to_owned()).or_insert_with(|| json!({}));
    }
    let Value::Object(map) = at else {
        return Err(format!("{leaf} holds no fields"));
    };
    map.insert((*leaf).to_owned(), value.clone());
    serde_json::from_value(whole).map_err(|e| e.to_string())
}

fn set_field(resolved: &mut Resolved, path: &[&str], value: &Value) {
    match trial_merge(&resolved.config, path, value) {
        Ok(next) => resolved.config = next,
        Err(why) => resolved.refusals.push(Refusal::new(
            "invalid",
            format!("{}: {why}", path.join(".")),
        )),
    }
}

fn set_look(resolved: &mut Resolved, key: &str, value: &Value) {
    match key {
        "bar_height" => set_field(resolved, &["bar", "height"], value),
        "focus_ring" => set_field(resolved, &["focus_ring"], value),
        "ground_image" => set_field(resolved, &["ground", "image"], value),
        other => set_field(resolved, &["look", other], value),
    }
}

fn set_layouts(resolved: &mut Resolved, value: &Value) {
    let Some(names) = value.as_array() else {
        resolved.refusals.push(Refusal::new(
            "invalid",
            "ukeire.layouts: not a list of layouts",
        ));
        return;
    };
    let mut standing = Vec::new();
    for name in names {
        match name.as_str() {
            Some(text) if crate::ukeire::layout_is_realised(text) => standing.push(json!(text)),
            _ => resolved.refusals.push(Refusal::new(
                "invalid",
                format!("ukeire.layouts: {name} is no layout this seat can realise"),
            )),
        }
    }
    set_field(resolved, &["ukeire", "layouts"], &Value::Array(standing));
}

fn word(value: &Value) -> Option<&str> {
    value.as_str().map(|s| s.trim_start_matches(':'))
}

fn number_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
}

fn module_from(name: &str, dir: &Path, value: &Value) -> BarModuleSpec {
    let mut spec = BarModuleSpec::new(name, dir);
    if let Some(allow) = value.get("allow").and_then(Value::as_array) {
        spec.allow = allow.iter().filter_map(word).map(str::to_owned).collect();
    }
    if let Some(every) = value.get("every_s").and_then(number_u64) {
        spec.every = Duration::from_secs(every.max(1));
    }
    if let Some(fuel) = value.get("fuel").and_then(number_u64) {
        spec.fuel = usize::try_from(fuel).unwrap_or(usize::MAX);
    }
    if let Some(ms) = value.get("deadline_ms").and_then(number_u64) {
        spec.hard_ms = ms.max(1);
        spec.soft_ms = spec.soft_ms.min(spec.hard_ms);
    }
    spec
}

fn apply_entry(entry: &Value, resolved: &mut Resolved, modules_dir: &Path) {
    let kind = entry.get("entry").and_then(word).unwrap_or_default();
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let value = entry.get("value").cloned().unwrap_or_else(|| json!({}));
    match kind {
        "config" => {
            if let Some(fields) = value.as_object() {
                for (key, field) in fields {
                    if name == "look" {
                        set_look(resolved, key, field);
                    } else if name == "ukeire" && key == "layouts" {
                        set_layouts(resolved, field);
                    } else {
                        set_field(resolved, &[name, key], field);
                    }
                }
            }
        }
        "command" => match SeatCommand::from_world(&value) {
            Ok(command) => {
                resolved.commands.insert(name.to_owned(), command);
            }
            Err(why) => resolved
                .refusals
                .push(Refusal::new("invalid", format!("command {name}: {why}"))),
        },
        "bind" => resolved.binds.push(WorldBind {
            chord: name.to_owned(),
            action: value
                .get("action")
                .and_then(word)
                .unwrap_or_default()
                .to_owned(),
            locked: value
                .get("locked")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }),
        "bar_module" => {
            let spec = module_from(name, modules_dir, &value);
            resolved.modules.retain(|m| m.name != spec.name);
            resolved.modules.push(spec);
        }
        other => resolved.refusals.push(Refusal::new(
            "invalid",
            format!("{other} {name}: no such kind"),
        )),
    }
}

#[must_use]
pub fn resolve_with(base: OmoyaConfig, load: &WorldLoad, world: &Path) -> Resolved {
    let mut resolved = Resolved {
        config: base,
        binds: Vec::new(),
        commands: BTreeMap::new(),
        modules: Vec::new(),
        refusals: Vec::new(),
        state: WorldState::Absent,
    };
    let modules_dir = world
        .parent()
        .map_or_else(|| PathBuf::from("bar"), |d| d.join("bar"));
    match load {
        WorldLoad::Absent => {}
        WorldLoad::Refused(refusal) => {
            resolved.refusals.push(refusal.clone());
            resolved.state = WorldState::Refused(refusal.clone());
        }
        WorldLoad::Judged(judged) => {
            resolved.state = WorldState::Loaded;
            let list = |key: &str| {
                judged
                    .get(key)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            };
            for refusal in list("refusals") {
                let kind = refusal
                    .get("kind")
                    .and_then(word)
                    .unwrap_or("invalid")
                    .to_owned();
                let why = refusal
                    .get("why")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                resolved.refusals.push(Refusal { kind, why });
            }
            for entry in list("accepted") {
                apply_entry(&entry, &mut resolved, &modules_dir);
            }
        }
    }
    resolved
}

#[must_use]
pub fn resolve_all(exe: &Path, world: &Path, base: OmoyaConfig) -> Resolved {
    let load = read_world(exe, world);
    resolve_with(base, &load, world)
}

#[must_use]
pub fn first_world(
    exe: PathBuf,
    world: PathBuf,
    base: OmoyaConfig,
    sender: Sender<SeatMessage>,
) -> Resolved {
    let fallback = base.clone();
    let path = world.clone();
    within(
        FIRST_PATIENCE,
        move || resolve_all(&exe, &world, base),
        move |late| {
            let _ = sender.send(SeatMessage::World(Box::new(late)));
        },
    )
    .unwrap_or_else(|| resolve_with(fallback, &WorldLoad::Absent, &path))
}

#[must_use]
pub fn cell_text(value: &Value) -> Result<Option<String>, Refusal> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => {
            let line = text.lines().next().unwrap_or("").trim();
            if line.is_empty() {
                return Ok(None);
            }
            Ok(Some(line.chars().take(CELL_MAX_CHARS).collect()))
        }
        other => Err(Refusal::new(
            "shape",
            format!("render returned {other}, not text or nil"),
        )),
    }
}

#[derive(Debug)]
enum ModuleMessage {
    Control(BarControl),
    Pool(Event),
}

struct Scheduled {
    spec: BarModuleSpec,
    next: Instant,
}

fn module_loop(
    exe: PathBuf,
    sender: Sender<SeatMessage>,
    tx: mpsc::Sender<ModuleMessage>,
    rx: mpsc::Receiver<ModuleMessage>,
) {
    let sink_tx = std::sync::Mutex::new(tx);
    let sink: blue_lang_worker::Sink = Arc::new(move |event| {
        if let Ok(tx) = sink_tx.lock() {
            let _ = tx.send(ModuleMessage::Pool(event));
        }
    });
    let mut pool = Pool::new(exe, sink).with_prepare(crate::spawn::prepare_worker);
    let mut scheduled: Vec<Scheduled> = Vec::new();
    let mut paused = false;
    let mut id = 0u64;
    let refused = |name: String, refusal: Refusal| {
        let _ = sender.send(SeatMessage::ModuleRefused { name, refusal });
    };
    loop {
        let now = Instant::now();
        let next_tick = if paused {
            None
        } else {
            scheduled.iter().map(|s| s.next).min()
        };
        let wake = [pool.next_deadline(), next_tick]
            .into_iter()
            .flatten()
            .min();
        let received = match wake {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(now)),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(ModuleMessage::Control(BarControl::Set(specs))) => {
                pool.restart_all();
                pool.set_specs(specs.iter().map(BarModuleSpec::worker_spec).collect());
                let at = Instant::now();
                scheduled = specs
                    .into_iter()
                    .map(|spec| Scheduled { spec, next: at })
                    .collect();
            }
            Ok(ModuleMessage::Control(BarControl::Pause)) => {
                paused = true;
                pool.cancel_all();
            }
            Ok(ModuleMessage::Control(BarControl::Resume)) => {
                paused = false;
                let at = Instant::now();
                for s in &mut scheduled {
                    s.next = at;
                }
            }
            Ok(ModuleMessage::Pool(Event::Line {
                worker,
                epoch,
                reply,
            })) => match reply {
                Reply::Answer { value, .. } => {
                    if pool.answered(&worker, epoch) {
                        match cell_text(&value) {
                            Ok(text) => {
                                let _ = sender.send(SeatMessage::Cell { name: worker, text });
                            }
                            Err(refusal) => refused(worker, refusal),
                        }
                    }
                }
                Reply::Refused {
                    refused: refusal, ..
                } => {
                    if pool.answered(&worker, epoch) {
                        refused(worker, refusal);
                    }
                }
                Reply::LoadRefused { load_refused } => {
                    if pool.load_refused(&worker, epoch, &load_refused) {
                        refused(worker, load_refused);
                    }
                }
            },
            Ok(ModuleMessage::Pool(Event::Closed { worker, epoch })) => {
                if let Some((_, refusal)) = pool.closed(&worker, epoch) {
                    refused(worker, refusal);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let at = Instant::now();
                for overdue in pool.tick(at) {
                    if overdue.refusal.kind == "killed" {
                        refused(overdue.worker, overdue.refusal);
                    }
                }
                if !paused {
                    for s in &mut scheduled {
                        if s.next <= at {
                            s.next = at + s.spec.every;
                            id = id.wrapping_add(1);
                            if let Err(refusal) = pool.ask(&s.spec.name, id, "render", Vec::new()) {
                                refused(s.spec.name.clone(), refusal);
                            }
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

pub struct BarModules {
    tx: mpsc::Sender<ModuleMessage>,
}

impl BarModules {
    #[must_use]
    pub fn start(exe: PathBuf, sender: Sender<SeatMessage>) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let loop_tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("omoya-bar-modules".to_owned())
            .spawn(move || module_loop(exe, sender, loop_tx, rx));
        match spawned {
            Ok(_) => Some(Self { tx }),
            Err(e) => {
                tracing::error!(error = %e, "no thread for the bar modules; the bar shows none");
                None
            }
        }
    }

    pub fn control(&self, control: BarControl) {
        let _ = self.tx.send(ModuleMessage::Control(control));
    }
}

#[must_use]
pub fn concerns_the_world(paths: &[PathBuf]) -> bool {
    paths
        .iter()
        .any(|p| p.extension().is_some_and(|e| e == "b"))
}

#[must_use]
pub fn changes_a_file(kind: &notify::EventKind) -> bool {
    use notify::EventKind;
    use notify::event::{AccessKind, AccessMode};
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

pub struct WorldWatcher {
    _watcher: notify::RecommendedWatcher,
}

pub fn watch(
    exe: PathBuf,
    world: PathBuf,
    base: OmoyaConfig,
    sender: Sender<SeatMessage>,
) -> Result<WorldWatcher, notify::Error> {
    use notify::{RecursiveMode, Watcher};
    let dir = world
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    std::fs::create_dir_all(&dir).map_err(notify::Error::io)?;
    let (tx, rx) = mpsc::channel::<notify::Event>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && changes_a_file(&event.kind)
        {
            let _ = tx.send(event);
        }
    })?;
    watcher.watch(&dir, RecursiveMode::Recursive)?;
    std::thread::Builder::new()
        .name("omoya-world-watch".to_owned())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                if !concerns_the_world(&first.paths) {
                    continue;
                }
                while rx.recv_timeout(SETTLE).is_ok() {}
                let resolved = resolve_all(&exe, &world, base.clone());
                if sender.send(SeatMessage::World(Box::new(resolved))).is_err() {
                    return;
                }
            }
        })
        .map_err(notify::Error::io)?;
    Ok(WorldWatcher { _watcher: watcher })
}

#[must_use]
pub fn check(world: &Path, wait: Duration) -> Value {
    use smithay::reexports::calloop::{EventLoop, channel};
    let exe = crate::spawn::own_exe();
    let resolved = resolve_all(&exe, world, crate::config::load());
    let state = match &resolved.state {
        WorldState::Absent => "absent",
        WorldState::Loaded => "loaded",
        WorldState::Refused(_) => "refused",
    };
    let mut refusals: Vec<String> = resolved.refusals.iter().map(ToString::to_string).collect();
    let built = crate::deed::world_bindings(
        crate::deed::default_bindings_for(resolved.config.ukeire.modifier.modifiers(), false).0,
        &resolved.binds,
        &resolved.commands,
    );
    refusals.extend(built.refusals.iter().map(ToString::to_string));
    let mut cells = serde_json::Map::new();
    if !resolved.modules.is_empty() {
        let (tx, rx) = channel::channel::<SeatMessage>();
        let Ok(mut event_loop) = EventLoop::<Vec<SeatMessage>>::try_new() else {
            return json!({"state": state, "refusals": refusals, "cells": cells});
        };
        let inserted = event_loop.handle().insert_source(rx, |event, (), got| {
            if let channel::Event::Msg(message) = event {
                got.push(message);
            }
        });
        if inserted.is_ok()
            && let Some(bar) = BarModules::start(exe, tx)
        {
            bar.control(BarControl::Set(resolved.modules.clone()));
            let mut got = Vec::new();
            let deadline = Instant::now() + wait;
            let answered = |got: &[SeatMessage]| {
                got.iter()
                    .filter_map(|m| match m {
                        SeatMessage::Cell { name, .. }
                        | SeatMessage::ModuleRefused { name, .. } => Some(name.as_str()),
                        _ => None,
                    })
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            };
            while Instant::now() < deadline && answered(&got) < resolved.modules.len() {
                let _ = event_loop.dispatch(Duration::from_millis(50), &mut got);
            }
            for message in got {
                match message {
                    SeatMessage::Cell { name, text } => {
                        cells.insert(name, json!(text));
                    }
                    SeatMessage::ModuleRefused { name, refusal } => {
                        cells.insert(name, json!({"refused": refusal.kind, "why": refusal.why}));
                    }
                    _ => {}
                }
            }
        }
    }
    json!({
        "state": state,
        "refusals": refusals,
        "commands": resolved.commands.keys().collect::<Vec<_>>(),
        "binds": resolved.binds.iter().map(|b| b.chord.clone()).collect::<Vec<_>>(),
        "config": resolved.config,
        "cells": cells,
    })
}

#[derive(Debug, Default)]
pub struct SeatWorld {
    pub base: Option<OmoyaConfig>,
    pub cli_terminal: Option<Vec<String>>,
    pub cli_launcher: Option<Vec<String>>,
    pub commands: BTreeMap<String, SeatCommand>,
    pub binds: Vec<WorldBind>,
    pub behind_lock: Vec<String>,
    pub cells: Vec<(String, String)>,
    pub modules: Vec<String>,
    pub module_specs: Vec<BarModuleSpec>,
    pub bar: Option<BarModules>,
    pub generation: u64,
    pub layout_index: usize,
    pub flash: Option<(String, Instant)>,
    pub seat_tx: Option<Sender<SeatMessage>>,
    pub path: Option<PathBuf>,
    pub exe: Option<PathBuf>,
}

impl std::fmt::Debug for BarModules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BarModules")
    }
}

impl SeatWorld {
    #[must_use]
    pub fn cells_line(&self) -> Option<String> {
        let flash = self.flash.as_ref().map(|(text, _)| text.as_str());
        let parts: Vec<&str> = flash
            .into_iter()
            .chain(self.cells.iter().map(|(_, text)| text.as_str()))
            .collect();
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("   "))
    }

    pub fn set_flash(&mut self, text: String, now: Instant) {
        self.flash = Some((text, now + FLASH_FOR));
    }

    pub fn expire_flash(&mut self, now: Instant) -> bool {
        if self.flash.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.flash = None;
            return true;
        }
        false
    }

    pub fn set_cell(&mut self, name: &str, text: Option<String>) -> bool {
        let before = self.cells.clone();
        match text {
            None => self.cells.retain(|(n, _)| n != name),
            Some(text) => {
                if let Some(cell) = self.cells.iter_mut().find(|(n, _)| n == name) {
                    cell.1 = text;
                } else {
                    self.cells.push((name.to_owned(), text));
                    let order: Vec<String> = self.modules.clone();
                    self.cells.sort_by_key(|(n, _)| {
                        order.iter().position(|o| o == n).unwrap_or(usize::MAX)
                    });
                }
            }
        }
        before != self.cells
    }
}

impl crate::state::Omoya {
    pub fn on_seat_message(&mut self, message: SeatMessage) {
        match message {
            SeatMessage::World(resolved) => self.adopt_world(*resolved),
            SeatMessage::Cell { name, text } => {
                if self.world.set_cell(&name, text) {
                    self.owed.mark(crate::owed::Owed::Chrome);
                }
            }
            SeatMessage::Flash(text) => {
                self.world.set_flash(text, Instant::now());
                self.owed.mark(crate::owed::Owed::Chrome);
            }
            SeatMessage::PowerRefused(why) => self.introspect.push_world_refusal(why),
            SeatMessage::NoteLine(line) => self.note_line(line),
            SeatMessage::PrepareForSleep(true) => self.sleep_starts(),
            SeatMessage::PrepareForSleep(false) => self.sleep_ended(),
            SeatMessage::LockSignal => {
                if !self.lock_session() {
                    tracing::warn!("logind asked for a lock, and this seat has no lock client");
                }
            }
            SeatMessage::ModuleRefused { name, refusal } => {
                tracing::warn!(module = %name, refusal = %refusal, "a bar module was refused");
                self.introspect
                    .push_world_refusal(format!("bar {name}: {refusal}"));
                if self.world.set_cell(&name, None) {
                    self.owed.mark(crate::owed::Owed::Chrome);
                }
            }
        }
    }

    pub fn adopt_world(&mut self, resolved: Resolved) {
        let Resolved {
            config,
            binds,
            commands,
            modules,
            refusals,
            state,
        } = resolved;
        for refusal in &refusals {
            tracing::warn!(refusal = %refusal, "a world entry was refused");
        }
        self.world.generation += 1;
        self.introspect
            .publish_world(&state, &refusals, self.world.generation, &commands);
        if let WorldState::Refused(refusal) = &state
            && self.world.generation > 1
        {
            tracing::error!(refusal = %refusal, "the world was refused; the seat stays on what stood");
            return;
        }
        let config = config.with_cli_overrides(
            self.world.cli_terminal.clone(),
            self.world.cli_launcher.clone(),
        );
        self.world.commands = commands;
        self.world.binds = binds;
        self.world.modules = modules.iter().map(|m| m.name.clone()).collect();
        self.world.module_specs.clone_from(&modules);
        let keep: Vec<String> = self.world.modules.clone();
        self.world.cells.retain(|(n, _)| keep.contains(n));
        self.adopt_config(config);
        self.rebuild_bindings();
        if let Some(bar) = &self.world.bar {
            bar.control(BarControl::Set(modules));
        }
        self.refresh_notes();
        self.owed.mark(crate::owed::Owed::Chrome);
    }

    pub fn refresh_notes(&mut self) {
        let (Some(exe), Some(world), Some(sender)) = (
            self.world.exe.clone(),
            self.world.path.clone(),
            self.world.seat_tx.clone(),
        ) else {
            return;
        };
        self.adopt_notes(&exe, &world, &sender);
    }

    pub fn power_hint(&self, command: crate::power::PowerCommand) {
        if let Some(power) = &self.power {
            power.send(command);
        }
    }

    pub fn note_input(&mut self) -> bool {
        self.idle.input(Instant::now());
        if self.panel == omoya_spec::idle::Panel::On {
            return false;
        }
        self.panel_wake_due = true;
        self.panel_dark_due = false;
        self.panel_flip_in_flight = false;
        if let Some(bar) = &self.world.bar {
            bar.control(BarControl::Resume);
        }
        self.power_hint(crate::power::PowerCommand::IdleHint(false));
        true
    }

    pub fn blank_panel(&mut self) {
        if self.panel != omoya_spec::idle::Panel::On {
            return;
        }
        self.panel = omoya_spec::idle::Panel::Darkening;
        self.publish_panel();
        self.owed.mark(crate::owed::Owed::Lock);
        if let Some(bar) = &self.world.bar {
            bar.control(BarControl::Pause);
        }
        self.power_hint(crate::power::PowerCommand::IdleHint(true));
    }

    pub fn publish_panel(&self) {
        let word = match self.panel {
            omoya_spec::idle::Panel::On => "on",
            omoya_spec::idle::Panel::Darkening => "darkening",
            omoya_spec::idle::Panel::Dark => "dark",
        };
        *self
            .introspect
            .panel_state
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = word.to_owned();
    }

    pub fn publish_lock(&self) {
        let machine = &self.session_lock.machine;
        let word = if !machine.hides_desktop() {
            "open"
        } else if machine.needs_a_client() {
            "locked, no client"
        } else if machine.on_glass() {
            "locked"
        } else {
            "locking"
        };
        *self
            .introspect
            .lock_state
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = word.to_owned();
    }

    pub fn seat_tick(&mut self, now: Instant) {
        for step in self.idle.tick(now) {
            match step {
                omoya_spec::idle::IdleStep::Lock => {
                    if !self.lock_session() {
                        tracing::warn!("idle asked for a lock, and this seat has no lock client");
                    }
                }
                omoya_spec::idle::IdleStep::Blank => self.blank_panel(),
            }
        }
        self.supervise_lock(now);
        self.expire_notes(now);
        if self.world.expire_flash(now) {
            self.owed.mark(crate::owed::Owed::Chrome);
        }
        self.publish_lock();
    }

    pub fn sleep_starts(&mut self) {
        self.introspect
            .sleeps
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.config.sleep.lock_before && self.lock_session() {
            self.sleep_wait = true;
            self.owed.mark(crate::owed::Owed::Lock);
            if self.session_lock.machine.on_glass()
                || self.panel.shows_nothing()
                || self.presence.is_away()
            {
                self.finish_sleep_wait();
            }
            return;
        }
        self.power_hint(crate::power::PowerCommand::ReleaseSleepInhibitor);
    }

    pub fn finish_sleep_wait(&mut self) {
        if self.sleep_wait
            && (self.session_lock.machine.on_glass()
                || self.panel.shows_nothing()
                || self.presence.is_away())
        {
            self.sleep_wait = false;
            self.power_hint(crate::power::PowerCommand::ReleaseSleepInhibitor);
        }
    }

    pub fn sleep_ended(&mut self) {
        self.introspect
            .resumes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.sleep_wait = false;
        self.power_hint(crate::power::PowerCommand::TakeSleepInhibitor);
        self.idle.input(Instant::now());
        self.presence.leave();
        self.presence.come_back();
        self.note_input();
        #[allow(clippy::cast_possible_truncation)]
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u32);
        self.release_all_keys(time);
        self.owed.mark(crate::owed::Owed::Resume);
        tracing::info!("the machine came back from sleep");
    }

    pub fn rearm_modules(&self) {
        if let Some(bar) = &self.world.bar {
            bar.control(BarControl::Set(self.world.module_specs.clone()));
        }
    }

    pub fn rebuild_bindings(&mut self) {
        let modifier = self.config.ukeire.modifier.modifiers();
        let floating = self.config.layout.mode == crate::config::LayoutMode::Floating;
        let (map, clashes) = crate::deed::default_bindings_for(modifier, floating);
        if !clashes.is_empty() {
            tracing::error!(
                ?clashes,
                "duplicate key bindings; the later ones were refused"
            );
        }
        let built = crate::deed::world_bindings(map, &self.world.binds, &self.world.commands);
        self.bindings = built.map;
        self.world.behind_lock = built.behind_lock;
        for refusal in &built.refusals {
            tracing::warn!(refusal = %refusal, "a binding was refused");
            self.introspect.push_world_refusal(refusal.to_string());
        }
    }

    pub fn adopt_config(&mut self, config: OmoyaConfig) {
        let keymap_changed = self.config.ukeire.keymap != config.ukeire.keymap
            || self.config.ukeire.layouts != config.ukeire.layouts
            || self.world.generation <= 1;
        if self.world.layout_index >= config.ukeire.layouts.len() {
            self.world.layout_index = 0;
        }
        let repeat = config.ukeire.repeat;
        self.idle.set_policy(config.idle.policy());
        self.remaps = config.remap_pairs();
        self.session_command.clone_from(&config.terminal);
        self.launcher_command.clone_from(&config.launcher);
        self.introspect.td_mode.store(
            crate::truedamage::Mode::resolve(config.damage.authority).to_u64(),
            std::sync::atomic::Ordering::Relaxed,
        );
        self.config = config;
        self.publish_ukeire();
        if let Some(kb) = self.seat.get_keyboard() {
            let (delay, rate) = repeat.smithay_repeat_info();
            kb.change_repeat_info(rate, delay);
            if keymap_changed {
                self.apply_keymap(&kb);
            }
        }
        self.apply_layout();
    }

    fn publish_ukeire(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        let u = &self.config.ukeire;
        let i = &self.introspect;
        let (delay, rate) = u.repeat.smithay_repeat_info();
        i.ukeire_repeat_delay_ms.store(delay as u64, Relaxed);
        i.ukeire_repeat_rate_hz.store(rate as u64, Relaxed);
        i.ukeire_scroll_factor_milli
            .store((u.scroll.factor.get() * 1000.0).round() as u64, Relaxed);
        i.ukeire_scroll_natural.store(
            u64::from(u.scroll.direction == crate::ukeire::ScrollDirection::Natural),
            Relaxed,
        );
        i.ukeire_cursor_scale
            .store(u.pointer.cursor_scale.get() as u64, Relaxed);
        i.ukeire_remaps.store(u.remaps.len() as u64, Relaxed);
        if let Ok(mut g) = i.ukeire_modifier.lock() {
            *g = match u.modifier {
                crate::ukeire::SeatModifier::Super => String::from("super"),
                crate::ukeire::SeatModifier::Alt => String::from("alt"),
            };
        }
    }

    #[must_use]
    pub fn active_keymap(&self) -> crate::ukeire::Keymap {
        let mut km = self.config.ukeire.keymap.clone();
        if let Some(name) = self.config.ukeire.layouts.get(self.world.layout_index) {
            let (layout, variant) = crate::ukeire::layout_pair(name);
            km.layout = Some(layout.to_owned());
            km.variant = Some(variant.to_owned());
        }
        km
    }

    #[must_use]
    pub fn layout_sign(&self) -> Option<String> {
        if self.world.layout_index == 0 {
            return None;
        }
        self.config
            .ukeire
            .layouts
            .get(self.world.layout_index)
            .cloned()
    }

    pub fn next_layout(&mut self) -> bool {
        let count = self.config.ukeire.layouts.len();
        if count < 2 {
            return false;
        }
        self.world.layout_index = (self.world.layout_index + 1) % count;
        if let Some(kb) = self.seat.get_keyboard() {
            self.apply_keymap(&kb);
        }
        self.owed.mark(crate::owed::Owed::Chrome);
        true
    }

    fn apply_keymap(&mut self, kb: &smithay::input::keyboard::KeyboardHandle<Self>) {
        let km = self.active_keymap();
        if km.is_default() {
            if let Ok(mut g) = self.introspect.ukeire_keymap_layout.lock() {
                *g = String::from("<bare>");
            }
            return;
        }
        let layout = km.layout.clone().unwrap_or_default();
        let xkb = smithay::input::keyboard::XkbConfig {
            rules: km.rules.as_deref().unwrap_or(""),
            model: km.model.as_deref().unwrap_or(""),
            layout: &layout,
            variant: km.variant.as_deref().unwrap_or(""),
            options: km.options.clone(),
        };
        let shown = match km.variant.as_deref() {
            _ if layout.is_empty() => "<xkb default>".to_owned(),
            Some(variant) if !variant.is_empty() => format!("{layout}+{variant}"),
            _ => layout.clone(),
        };
        let shown = shown.as_str();
        match kb.set_xkb_config(self, xkb) {
            Ok(()) => {
                tracing::info!(layout = shown, "ukeire: keymap");
                if let Ok(mut g) = self.introspect.ukeire_keymap_layout.lock() {
                    *g = shown.to_string();
                }
            }
            Err(e) => tracing::error!(
                layout = shown,
                error = ?e,
                "ukeire: the keymap did not compile; the seat keeps the one it had"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: &str, name: &str, value: Value) -> Value {
        json!({"entry": format!(":{kind}"), "name": name, "value": value})
    }

    fn judged(accepted: Value) -> WorldLoad {
        WorldLoad::Judged(json!({"accepted": accepted, "off": [], "refusals": []}))
    }

    fn resolve(load: &WorldLoad) -> Resolved {
        resolve_with(
            OmoyaConfig::prescribed(),
            load,
            Path::new("/c/omoya/omoya.b"),
        )
    }

    #[test]
    fn no_world_leaves_the_config_untouched() {
        let r = resolve(&WorldLoad::Absent);
        assert_eq!(r.config, OmoyaConfig::prescribed());
        assert!(r.refusals.is_empty());
        assert_eq!(r.state, WorldState::Absent);
    }

    #[test]
    fn a_refused_world_says_why_and_changes_nothing() {
        let r = resolve(&WorldLoad::Refused(Refusal::new(
            "load",
            "parse error at 3:1",
        )));
        assert_eq!(r.config, OmoyaConfig::prescribed());
        assert_eq!(r.refusals[0].why, "parse error at 3:1");
        assert!(matches!(r.state, WorldState::Refused(_)));
    }

    #[test]
    fn a_bad_field_is_refused_alone_and_its_siblings_apply() {
        let r = resolve(&judged(json!([entry(
            "config",
            "layout",
            json!({"mode": "floating", "snap_threshold": "wide", "brand_new": 1})
        )])));
        assert_eq!(r.config.layout.mode, crate::config::LayoutMode::Floating);
        assert_eq!(r.refusals.len(), 2, "{:?}", r.refusals);
        assert!(
            r.refusals
                .iter()
                .any(|x| x.why.starts_with("layout.snap_threshold:"))
        );
        assert!(
            r.refusals
                .iter()
                .any(|x| x.why.starts_with("layout.brand_new:"))
        );
    }

    #[test]
    fn an_unknown_group_is_refused_and_the_config_stands() {
        let r = resolve(&judged(json!([entry(
            "config",
            "nonsense",
            json!({"a": 1})
        )])));
        assert_eq!(r.refusals.len(), 1, "{:?}", r.refusals);
        assert!(r.refusals[0].why.starts_with("nonsense.a:"));
        assert_eq!(r.config, OmoyaConfig::prescribed());
    }

    #[test]
    fn the_look_words_reach_their_real_fields() {
        let r = resolve(&judged(json!([entry(
            "config",
            "look",
            json!({"bar_height": 31, "focus_ring": "accent", "ground_image": "/g.png"})
        )])));
        assert!(r.refusals.is_empty(), "{:?}", r.refusals);
        assert_eq!(r.config.bar.height, 31);
        assert_eq!(r.config.focus_ring, crate::config::FocusRing::Accent);
        assert_eq!(r.config.ground.image.as_deref(), Some(Path::new("/g.png")));
    }

    #[test]
    fn a_reserved_remap_in_a_world_is_refused_like_in_a_file() {
        let r = resolve(&judged(json!([entry(
            "config",
            "ukeire",
            json!({"remaps": [{"from": 60, "to": 1}]})
        )])));
        assert_eq!(r.refusals.len(), 1, "{:?}", r.refusals);
        assert!(r.refusals[0].why.contains("reserved"), "{:?}", r.refusals);
        assert_eq!(
            r.config.remap_pairs(),
            OmoyaConfig::prescribed().remap_pairs()
        );
    }

    #[test]
    fn a_layout_nobody_can_realise_is_refused_alone_and_the_rest_stand() {
        let r = resolve(&judged(json!([entry(
            "config",
            "ukeire",
            json!({"layouts": ["us", "de", "us+intl", "us+dvorak", 7]})
        )])));
        assert_eq!(
            r.config.ukeire.layouts,
            vec!["us".to_owned(), "us+intl".to_owned()]
        );
        assert_eq!(r.refusals.len(), 3, "{:?}", r.refusals);
        assert!(
            r.refusals
                .iter()
                .all(|x| x.why.starts_with("ukeire.layouts:"))
        );
    }

    #[test]
    fn a_bind_and_a_command_and_a_module_carry_their_fields() {
        let r = resolve(&judged(json!([
            entry(
                "bind",
                "super+a",
                json!({"action": "louder", "locked": true})
            ),
            entry(
                "command",
                "louder",
                json!({"kind": "volume", "step_pct": 5})
            ),
            entry(
                "bar_module",
                "uptime",
                json!({"every_s": 60, "allow": [":filesystem"], "deadline_ms": 40, "fuel": 9})
            ),
        ])));
        assert!(r.refusals.is_empty(), "{:?}", r.refusals);
        assert_eq!(
            r.binds,
            vec![WorldBind {
                chord: "super+a".to_owned(),
                action: "louder".to_owned(),
                locked: true
            }]
        );
        assert_eq!(r.commands["louder"], SeatCommand::Volume(5));
        let m = &r.modules[0];
        assert_eq!(m.file, PathBuf::from("/c/omoya/bar/uptime.b"));
        assert_eq!(m.allow, vec!["filesystem"]);
        assert_eq!(m.every, Duration::from_secs(60));
        assert_eq!((m.fuel, m.hard_ms, m.soft_ms), (9, 40, 40));
    }

    #[test]
    fn a_command_the_seat_cannot_read_is_refused_alone() {
        let r = resolve(&judged(json!([
            entry("command", "bad", json!({"kind": "deed", "verb": "explode"})),
            entry("command", "good", json!({"kind": "run", "argv": ["mado"]})),
        ])));
        assert_eq!(r.refusals.len(), 1, "{:?}", r.refusals);
        assert!(r.refusals[0].why.starts_with("command bad:"));
        assert_eq!(
            r.commands["good"],
            SeatCommand::Run(vec!["mado".to_owned()])
        );
        assert!(!r.commands.contains_key("bad"));
    }

    #[test]
    fn a_judges_refusals_reach_the_seat() {
        let r = resolve(&WorldLoad::Judged(json!({
            "accepted": [],
            "off": [],
            "refusals": [{"kind": ":invalid", "why": "bind x: no"}]
        })));
        assert_eq!(r.refusals, vec![Refusal::new("invalid", "bind x: no")]);
        assert_eq!(r.state, WorldState::Loaded);
    }

    #[test]
    fn a_cell_is_one_short_line_or_nothing() {
        assert_eq!(cell_text(&json!(null)), Ok(None));
        assert_eq!(cell_text(&json!("  ")), Ok(None));
        assert_eq!(cell_text(&json!(" 3h \nmore")), Ok(Some("3h".to_owned())));
        let long = "x".repeat(200);
        assert_eq!(
            cell_text(&json!(long)).unwrap().unwrap().chars().count(),
            CELL_MAX_CHARS
        );
        assert_eq!(cell_text(&json!(7)).unwrap_err().kind, "shape");
    }

    #[test]
    fn cells_keep_the_worlds_order_and_only_a_change_owes_a_frame() {
        let mut w = SeatWorld {
            modules: vec!["a".to_owned(), "b".to_owned()],
            ..SeatWorld::default()
        };
        assert_eq!(w.cells_line(), None);
        assert!(w.set_cell("b", Some("two".to_owned())));
        assert!(w.set_cell("a", Some("one".to_owned())));
        assert_eq!(w.cells_line().as_deref(), Some("one   two"));
        assert!(!w.set_cell("a", Some("one".to_owned())));
        assert!(w.set_cell("a", None));
        assert!(!w.set_cell("a", None));
        assert_eq!(w.cells_line().as_deref(), Some("two"));
    }

    #[test]
    fn a_flash_shows_first_and_leaves_when_its_time_is_up() {
        let mut w = SeatWorld {
            modules: vec!["a".to_owned()],
            ..SeatWorld::default()
        };
        w.set_cell("a", Some("one".to_owned()));
        let now = Instant::now();
        w.set_flash("vol 45%".to_owned(), now);
        assert_eq!(w.cells_line().as_deref(), Some("vol 45%   one"));
        assert!(!w.expire_flash(now));
        assert!(w.expire_flash(now + Duration::from_secs(2)));
        assert_eq!(w.cells_line().as_deref(), Some("one"));
        assert!(!w.expire_flash(now + Duration::from_secs(3)));
    }

    #[test]
    fn a_read_is_no_change_and_a_write_is() {
        use notify::EventKind;
        use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind};
        for read in [
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            EventKind::Access(AccessKind::Read),
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
        ] {
            assert!(!changes_a_file(&read), "{read:?}");
        }
        for change in [
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
        ] {
            assert!(changes_a_file(&change), "{change:?}");
        }
    }

    #[test]
    fn only_a_blue_file_reloads_the_world() {
        assert!(concerns_the_world(&[PathBuf::from("/c/omoya.b")]));
        assert!(concerns_the_world(&[PathBuf::from("/c/bar/uptime.b")]));
        assert!(!concerns_the_world(&[PathBuf::from("/c/omoya.yaml")]));
        assert!(!concerns_the_world(&[]));
    }

    fn world_arrives(
        channel: &smithay::reexports::calloop::channel::Channel<SeatMessage>,
        within: Duration,
    ) -> bool {
        let deadline = std::time::Instant::now() + within;
        while std::time::Instant::now() < deadline {
            if matches!(channel.try_recv(), Ok(SeatMessage::World(_))) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn saving_a_world_or_swapping_its_link_delivers_it_and_a_stranger_does_not() {
        let dir = std::env::temp_dir().join(format!("omoya-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let world = dir.join("omoya.b");
        let (sender, channel) = smithay::reexports::calloop::channel::channel();
        let _watching = watch(
            PathBuf::from("/nonexistent/omoya"),
            world.clone(),
            OmoyaConfig::prescribed(),
            sender,
        )
        .unwrap();

        std::fs::write(dir.join("omoya.yaml"), "x: 1\n").unwrap();
        assert!(
            !world_arrives(&channel, SETTLE * 4),
            "a file that is no blue reloaded the world"
        );

        std::fs::write(&world, "def omoya()\n  []\nend\n").unwrap();
        assert!(
            world_arrives(&channel, Duration::from_secs(10)),
            "a saved world never arrived"
        );
        while world_arrives(&channel, SETTLE * 4) {}

        let next = dir.join("generation-2");
        std::fs::write(&next, "def omoya()\n  []\nend\n").unwrap();
        let link = dir.join("omoya.b.tmp");
        std::os::unix::fs::symlink(&next, &link).unwrap();
        std::fs::rename(&link, &world).unwrap();
        assert!(
            world_arrives(&channel, Duration::from_secs(10)),
            "a swapped link never arrived"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_blue_runs_on_the_render_thread() {
        let code: String = include_str!("drm.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap_or("")
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for word in ["blue_lang", "read_world", "resolve_all", "Program::"] {
            assert!(!code.contains(word), "drm.rs reaches blue through {word}");
        }
        let main: String = include_str!("main.rs")
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !main.contains("blue_lang_host"),
            "main.rs embeds blue in the seat"
        );
    }
}
