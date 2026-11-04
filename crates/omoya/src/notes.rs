use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use smithay::reexports::calloop::channel::Sender;

use crate::world::SeatMessage;

pub const WORKER: &str = "notes-worker";
const BUS_NAME: &str = "org.freedesktop.Notifications";
const BUS_PATH: &str = "/org/freedesktop/Notifications";
const SUMMARY_MAX: usize = 80;
const BODY_MAX: usize = 200;
const RESPAWN_FIRST: Duration = Duration::from_secs(2);
const RESPAWN_LAST: Duration = Duration::from_secs(60);
pub const RULE_FUEL: usize = 2_000_000;

pub const CLOSED_EXPIRED: u32 = 1;
pub const CLOSED_DISMISSED: u32 = 2;
pub const CLOSED_BY_CALL: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub urgency: u8,
    pub ms: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerLine {
    Note(Note),
    Close(u32),
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closed {
    pub id: u32,
    pub reason: u32,
}

fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[must_use]
pub fn clean(text: &str, max: usize) -> String {
    let flat = strip_markup(text);
    let trimmed = flat.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_owned();
    }
    let mut shortened: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

#[must_use]
pub fn note_of(
    id: u32,
    app: &str,
    summary: &str,
    body: &str,
    urgency: u8,
    expire_timeout: i32,
) -> Note {
    Note {
        id,
        app: clean(app, SUMMARY_MAX),
        summary: clean(summary, SUMMARY_MAX),
        body: clean(body, BODY_MAX),
        urgency: urgency.min(2),
        ms: expire_timeout.max(-1),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownNote {
    pub note: Note,
    pub until: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct NoteShelf {
    notes: Vec<ShownNote>,
    pub generation: u64,
}

impl NoteShelf {
    #[must_use]
    pub fn shown(&self, max: usize) -> Vec<&ShownNote> {
        self.notes.iter().rev().take(max).collect()
    }

    pub fn add(&mut self, note: Note, now: Instant, default_ms: u32) {
        let never = note.urgency >= 2 || note.ms == 0;
        let ms = if note.ms < 0 {
            u64::from(default_ms)
        } else {
            u64::from(note.ms.unsigned_abs())
        };
        let until = if never {
            None
        } else {
            Some(now + Duration::from_millis(ms))
        };
        self.notes.retain(|n| n.note.id != note.id);
        self.notes.push(ShownNote { note, until });
        self.generation += 1;
    }

    pub fn close(&mut self, id: u32) -> bool {
        let before = self.notes.len();
        self.notes.retain(|n| n.note.id != id);
        let changed = self.notes.len() != before;
        if changed {
            self.generation += 1;
        }
        changed
    }

    pub fn expire(&mut self, now: Instant) -> Vec<u32> {
        let gone: Vec<u32> = self
            .notes
            .iter()
            .filter(|n| n.until.is_some_and(|u| u <= now))
            .map(|n| n.note.id)
            .collect();
        if !gone.is_empty() {
            self.notes.retain(|n| !gone.contains(&n.note.id));
            self.generation += 1;
        }
        gone
    }

    pub fn dismiss_newest(&mut self) -> Option<u32> {
        let note = self.notes.pop()?;
        self.generation += 1;
        Some(note.note.id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
}

struct Server {
    next: Mutex<u32>,
    tx: Mutex<mpsc::Sender<WorkerLine>>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
            *next = next.wrapping_add(1).max(1);
            *next
        };
        let urgency = hints
            .get("urgency")
            .and_then(|v| u8::try_from(&**v).ok())
            .unwrap_or(1);
        let note = note_of(id, &app_name, &summary, &body, urgency, expire_timeout);
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(WorkerLine::Note(note));
        }
        id
    }

    fn close_notification(&self, id: u32) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(WorkerLine::Close(id));
        }
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".to_owned()]
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "omoya".to_owned(),
            "pleme-io".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
            "1.2".to_owned(),
        )
    }
}

fn say(line: &WorkerLine) -> bool {
    let Ok(text) = serde_json::to_string(line) else {
        return false;
    };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "{text}").and_then(|()| out.flush()).is_ok()
}

fn apply_rule(rule: &mut blue_lang_worker::Hosted<()>, note: Note) -> Result<Option<Note>, String> {
    let value = serde_json::to_value(&note).map_err(|e| e.to_string())?;
    let request = blue_lang_worker::Request {
        id: u64::from(note.id),
        call: "note".to_owned(),
        args: vec![value],
    };
    match blue_lang_worker::child::answer(rule, &request, RULE_FUEL) {
        blue_lang_worker::Reply::Answer { value, .. } => {
            if value.is_null() {
                return Ok(None);
            }
            let shaped: RuleNote = serde_json::from_value(value).map_err(|e| e.to_string())?;
            Ok(Some(Note {
                id: note.id,
                app: note.app,
                summary: clean(&shaped.summary, SUMMARY_MAX),
                body: clean(&shaped.body.unwrap_or_default(), BODY_MAX),
                urgency: shaped.urgency.unwrap_or(note.urgency).min(2),
                ms: shaped.ms.unwrap_or(note.ms).max(-1),
            }))
        }
        blue_lang_worker::Reply::Refused { refused, .. } => Err(refused.to_string()),
        blue_lang_worker::Reply::LoadRefused { load_refused } => Err(load_refused.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct RuleNote {
    summary: String,
    body: Option<String>,
    urgency: Option<u8>,
    ms: Option<i32>,
}

#[must_use]
pub fn run_worker(rule_file: Option<&Path>) -> i32 {
    let (tx, rx) = mpsc::channel::<WorkerLine>();
    let server = Server {
        next: Mutex::new(0),
        tx: Mutex::new(tx),
    };
    let built = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(BUS_PATH, server))
        .and_then(zbus::blocking::connection::Builder::build);
    let conn = match built {
        Ok(conn) => conn,
        Err(e) => {
            say(&WorkerLine::Refused(format!("no session bus: {e}")));
            return 0;
        }
    };
    match conn.request_name_with_flags(BUS_NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner,
        ) => {}
        Ok(_) => {
            say(&WorkerLine::Refused(format!(
                "another daemon owns {BUS_NAME} on the session bus"
            )));
            return 0;
        }
        Err(e) => {
            say(&WorkerLine::Refused(format!("cannot take {BUS_NAME}: {e}")));
            return 0;
        }
    }
    let closed_conn = conn.clone();
    let spawned = std::thread::Builder::new()
        .name("omoya-notes-stdin".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if let Ok(closed) = serde_json::from_str::<Closed>(&line) {
                    let _ = closed_conn.emit_signal(
                        Option::<&str>::None,
                        BUS_PATH,
                        BUS_NAME,
                        "NotificationClosed",
                        &(closed.id, closed.reason),
                    );
                }
            }
            std::process::exit(0);
        });
    if spawned.is_err() {
        return 1;
    }
    let mut rule = match rule_file.filter(|f| f.exists()) {
        None => None,
        Some(file) => match blue_lang_worker::child::load(file, &[]) {
            Ok(hosted) => Some(hosted),
            Err(refused) => {
                say(&WorkerLine::Refused(format!("notes.b: {refused}")));
                None
            }
        },
    };
    while let Ok(line) = rx.recv() {
        let line = match (line, rule.as_mut()) {
            (WorkerLine::Note(note), Some(hosted)) => match apply_rule(hosted, note.clone()) {
                Ok(Some(shaped)) => WorkerLine::Note(shaped),
                Ok(None) => continue,
                Err(why) => {
                    say(&WorkerLine::Refused(format!("notes.b: {why}")));
                    WorkerLine::Note(note)
                }
            },
            (line, _) => line,
        };
        if !say(&line) {
            break;
        }
    }
    0
}

#[derive(Debug)]
enum Control {
    Closed(Closed),
    Restart,
}

fn worker_loop(
    exe: &Path,
    rule: &Path,
    sender: &Sender<SeatMessage>,
    rx: &mpsc::Receiver<Control>,
) {
    let mut wait = RESPAWN_FIRST;
    loop {
        let mut command = crate::spawn::worker_command(exe);
        command
            .arg(WORKER)
            .arg(rule)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                let _ = sender.send(SeatMessage::PowerRefused(format!("notes: {e}")));
                match rx.recv_timeout(wait) {
                    Err(RecvTimeoutError::Disconnected) => return,
                    _ => {
                        wait = (wait * 2).min(RESPAWN_LAST);
                        continue;
                    }
                }
            }
        };
        let stdout = child.stdout.take();
        let mut stdin: Option<ChildStdin> = child.stdin.take();
        let (dead_tx, dead_rx) = mpsc::channel::<()>();
        let line_sender = sender.clone();
        let reader = stdout.map(|stdout| {
            std::thread::Builder::new()
                .name("omoya-notes-read".to_owned())
                .spawn(move || {
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        let Ok(parsed) = serde_json::from_str::<WorkerLine>(&line) else {
                            continue;
                        };
                        if line_sender.send(SeatMessage::NoteLine(parsed)).is_err() {
                            break;
                        }
                    }
                    let _ = dead_tx.send(());
                })
        });
        if !matches!(reader, Some(Ok(_))) {
            let _ = child.kill();
            return;
        }
        let started = Instant::now();
        let restart = loop {
            match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Control::Closed(closed)) => {
                    if let (Some(pipe), Ok(line)) = (stdin.as_mut(), serde_json::to_string(&closed))
                    {
                        let _ = writeln!(pipe, "{line}").and_then(|()| pipe.flush());
                    }
                }
                Ok(Control::Restart) => break true,
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = child.kill();
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if dead_rx.try_recv().is_ok() {
                break false;
            }
        };
        let _ = child.kill();
        drop(stdin.take());
        if restart || started.elapsed() > RESPAWN_LAST {
            wait = RESPAWN_FIRST;
            continue;
        }
        match rx.recv_timeout(wait) {
            Err(RecvTimeoutError::Disconnected) => return,
            Ok(Control::Restart) => wait = RESPAWN_FIRST,
            _ => wait = (wait * 2).min(RESPAWN_LAST),
        }
    }
}

#[derive(Debug)]
pub struct Notes {
    tx: mpsc::Sender<Control>,
}

impl Notes {
    #[must_use]
    pub fn start(exe: PathBuf, rule: PathBuf, sender: Sender<SeatMessage>) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("omoya-notes".to_owned())
            .spawn(move || worker_loop(&exe, &rule, &sender, &rx));
        match spawned {
            Ok(_) => Some(Self { tx }),
            Err(e) => {
                tracing::error!(error = %e, "no notes thread; notifications go nowhere");
                None
            }
        }
    }

    pub fn closed(&self, id: u32, reason: u32) {
        let _ = self.tx.send(Control::Closed(Closed { id, reason }));
    }

    pub fn restart(&self) {
        let _ = self.tx.send(Control::Restart);
    }
}

impl crate::state::Omoya {
    pub fn note_line(&mut self, line: WorkerLine) {
        match line {
            WorkerLine::Note(note) => {
                self.shelf.add(note, Instant::now(), self.config.notes.ms);
                self.introspect
                    .notes_shown
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.owed.mark(crate::owed::Owed::Chrome);
            }
            WorkerLine::Close(id) => {
                if self.shelf.close(id) {
                    self.owed.mark(crate::owed::Owed::Chrome);
                }
                if let Some(notes) = &self.notes {
                    notes.closed(id, CLOSED_BY_CALL);
                }
            }
            WorkerLine::Refused(why) => {
                tracing::warn!(why, "the notes worker refused");
                self.introspect.push_world_refusal(why);
            }
        }
    }

    pub fn expire_notes(&mut self, now: Instant) {
        let gone = self.shelf.expire(now);
        if gone.is_empty() {
            return;
        }
        self.owed.mark(crate::owed::Owed::Chrome);
        if let Some(notes) = &self.notes {
            for id in gone {
                notes.closed(id, CLOSED_EXPIRED);
            }
        }
    }

    pub fn dismiss_note(&mut self) -> bool {
        let Some(id) = self.shelf.dismiss_newest() else {
            return false;
        };
        self.owed.mark(crate::owed::Owed::Chrome);
        if let Some(notes) = &self.notes {
            notes.closed(id, CLOSED_DISMISSED);
        }
        true
    }

    pub fn adopt_notes(&mut self, exe: &Path, world: &Path, sender: &Sender<SeatMessage>) {
        if !self.config.notes.enabled {
            self.notes = None;
            return;
        }
        if let Some(notes) = &self.notes {
            notes.restart();
            return;
        }
        let rule = world
            .parent()
            .map_or_else(|| PathBuf::from("notes.b"), |d| d.join("notes.b"));
        self.notes = Notes::start(exe.to_path_buf(), rule, sender.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u32, urgency: u8, ms: i32) -> Note {
        note_of(id, "app", "s", "b", urgency, ms)
    }

    #[test]
    fn text_is_one_clean_short_line() {
        assert_eq!(clean(" a\nb\t<b>c</b> ", 80), "a b c");
        assert_eq!(clean("\u{1b}[31mred", 80), "[31mred");
        let long = "x".repeat(300);
        assert_eq!(clean(&long, 10).chars().count(), 10);
        assert!(clean(&long, 10).ends_with('…'));
        assert_eq!(
            clean("1 < 2", 80),
            "1",
            "an unclosed tag ends the line, safe over pretty"
        );
    }

    #[test]
    fn a_note_keeps_its_fields_inside_their_bounds() {
        let n = note_of(9, "app", "s", "b", 7, -5);
        assert_eq!((n.id, n.urgency, n.ms), (9, 2, -1));
    }

    #[test]
    fn default_time_comes_from_the_world_and_a_stated_time_wins() {
        let mut shelf = NoteShelf::default();
        let now = Instant::now();
        shelf.add(note(1, 1, -1), now, 6000);
        shelf.add(note(2, 1, 1000), now, 6000);
        assert_eq!(
            shelf.expire(now + Duration::from_millis(999)),
            Vec::<u32>::new()
        );
        assert_eq!(shelf.expire(now + Duration::from_secs(1)), vec![2]);
        assert_eq!(shelf.expire(now + Duration::from_secs(6)), vec![1]);
        assert!(shelf.is_empty());
    }

    #[test]
    fn critical_and_zero_never_expire() {
        let mut shelf = NoteShelf::default();
        let now = Instant::now();
        shelf.add(note(1, 2, 1000), now, 6000);
        shelf.add(note(2, 1, 0), now, 6000);
        assert_eq!(
            shelf.expire(now + Duration::from_secs(86400)),
            Vec::<u32>::new()
        );
        assert_eq!(shelf.shown(3).len(), 2);
    }

    #[test]
    fn a_replacing_note_takes_the_place_of_the_old_one() {
        let mut shelf = NoteShelf::default();
        let now = Instant::now();
        shelf.add(note(1, 1, -1), now, 6000);
        shelf.add(note_of(1, "app", "new", "b", 1, -1), now, 6000);
        assert_eq!(shelf.shown(3).len(), 1);
        assert_eq!(shelf.shown(3)[0].note.summary, "new");
    }

    #[test]
    fn the_newest_show_first_up_to_the_limit() {
        let mut shelf = NoteShelf::default();
        let now = Instant::now();
        for id in 1..=5 {
            shelf.add(note(id, 1, -1), now, 6000);
        }
        let ids: Vec<u32> = shelf.shown(3).iter().map(|n| n.note.id).collect();
        assert_eq!(ids, vec![5, 4, 3]);
    }

    #[test]
    fn dismiss_takes_the_newest_and_only_a_real_change_moves_the_generation() {
        let mut shelf = NoteShelf::default();
        let now = Instant::now();
        assert_eq!(shelf.dismiss_newest(), None);
        shelf.add(note(1, 1, -1), now, 6000);
        shelf.add(note(2, 1, -1), now, 6000);
        let at = shelf.generation;
        assert!(!shelf.close(99));
        assert_eq!(shelf.generation, at);
        assert_eq!(shelf.dismiss_newest(), Some(2));
        assert!(shelf.generation > at);
        assert!(shelf.close(1));
        assert!(shelf.is_empty());
    }

    #[test]
    fn a_line_round_trips() {
        for line in [
            WorkerLine::Note(note(1, 1, -1)),
            WorkerLine::Close(3),
            WorkerLine::Refused("no".to_owned()),
        ] {
            let text = serde_json::to_string(&line).unwrap();
            assert_eq!(serde_json::from_str::<WorkerLine>(&text).unwrap(), line);
        }
    }
}
