use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

fn exe() -> PathBuf {
    std::env::var_os("OMOYA_BIN")
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_omoya")), PathBuf::from)
}

struct Bus {
    daemon: Child,
    address: String,
    dir: PathBuf,
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

fn bus(tag: &str) -> Bus {
    let dir = std::env::temp_dir().join(format!("omoya-notes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("bus");
    let config = dir.join("bus.conf");
    std::fs::write(
        &config,
        format!(
            "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \
             \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\
             <busconfig><type>session</type><listen>unix:path={}</listen>\
             <policy context=\"default\"><allow send_destination=\"*\" eavesdrop=\"true\"/>\
             <allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>",
            socket.display()
        ),
    )
    .unwrap();
    let daemon = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .arg("--nofork")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("these tests need dbus-daemon on PATH");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "no bus socket");
        std::thread::sleep(Duration::from_millis(10));
    }
    Bus {
        daemon,
        address: format!("unix:path={}", socket.display()),
        dir,
    }
}

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_lines(out: ChildStdout) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn worker(bus: &Bus, rule: &Path) -> Worker {
    let mut child = Command::new(exe())
        .arg("notes-worker")
        .arg(rule)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("RUST_LOG", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let lines = read_lines(child.stdout.take().unwrap());
    Worker {
        child,
        stdin: Some(stdin),
        lines,
    }
}

fn client(bus: &Bus) -> zbus::blocking::Connection {
    zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap()
}

fn notify(conn: &zbus::blocking::Connection, app: &str, summary: &str, body: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let hints: std::collections::HashMap<String, zvariant::Value<'_>> =
            std::collections::HashMap::new();
        let reply = conn.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                app,
                0u32,
                "",
                summary,
                body,
                Vec::<String>::new(),
                hints,
                -1i32,
            ),
        );
        match reply {
            Ok(m) => return m.body().deserialize().unwrap(),
            Err(e) => {
                assert!(
                    Instant::now() < deadline,
                    "the worker never took the name: {e}"
                );
                std::thread::sleep(Duration::from_millis(30));
            }
        }
    }
}

fn next(w: &Worker) -> serde_json::Value {
    let line = w
        .lines
        .recv_timeout(Duration::from_secs(20))
        .expect("no line in 20 s");
    serde_json::from_str(&line).unwrap()
}

#[test]
fn notify_send_reaches_the_seat_as_one_clean_note() {
    let b = bus("plain");
    let w = worker(&b, &b.dir.join("no-rule.b"));
    let conn = client(&b);
    let id = notify(&conn, "app", "hello", "line one\nline <b>two</b>");
    assert!(id > 0);
    let v = next(&w);
    assert_eq!(v["note"]["id"], id);
    assert_eq!(v["note"]["summary"], "hello");
    assert_eq!(v["note"]["body"], "line one line two");
    assert_eq!(v["note"]["urgency"], 1);
    assert_eq!(v["note"]["ms"], -1);
}

#[test]
fn a_rule_can_change_a_note_or_drop_it_and_a_raise_lets_it_through_and_says_so() {
    let b = bus("rule");
    let rule = b.dir.join("notes.b");
    std::fs::write(
        &rule,
        "def note(n)\n  if n[:app] == \"drop\"\n    nil\n  elsif n[:app] == \"raise\"\n    raise(\"boom\")\n  else\n    {summary: \"shaped: #{n[:summary]}\", body: n[:body], urgency: n[:urgency], ms: 900}\n  end\nend\n",
    )
    .unwrap();
    let w = worker(&b, &rule);
    let conn = client(&b);
    notify(&conn, "drop", "gone", "");
    notify(&conn, "keep", "kept", "b");
    let v = next(&w);
    assert_eq!(v["note"]["summary"], "shaped: kept", "{v}");
    assert_eq!(v["note"]["ms"], 900);
    notify(&conn, "raise", "raw", "");
    let refused = next(&w);
    assert!(
        refused["refused"].as_str().unwrap().contains("notes.b"),
        "{refused}"
    );
    assert_eq!(next(&w)["note"]["summary"], "raw");
}

#[test]
fn a_rule_that_does_not_load_is_refused_alone_and_notes_still_pass() {
    let b = bus("badrule");
    let rule = b.dir.join("notes.b");
    std::fs::write(&rule, "def (((\n").unwrap();
    let w = worker(&b, &rule);
    let conn = client(&b);
    assert!(next(&w)["refused"].as_str().unwrap().contains("notes.b"));
    notify(&conn, "app", "still here", "");
    assert_eq!(next(&w)["note"]["summary"], "still here");
}

#[test]
fn a_second_worker_cannot_take_the_name_and_says_why() {
    let b = bus("name");
    let first = worker(&b, &b.dir.join("no-rule.b"));
    let conn = client(&b);
    notify(&conn, "app", "a", "");
    let _ = next(&first);
    let second = worker(&b, &b.dir.join("no-rule.b"));
    let v = next(&second);
    assert!(
        v["refused"]
            .as_str()
            .unwrap()
            .contains("org.freedesktop.Notifications"),
        "{v}"
    );
}

#[test]
fn a_closed_note_reaches_its_listener_and_end_of_stdin_ends_the_worker() {
    let b = bus("closed");
    let mut w = worker(&b, &b.dir.join("no-rule.b"));
    let conn = client(&b);
    let id = notify(&conn, "app", "a", "");
    let _ = next(&w);
    let rule =
        "type='signal',interface='org.freedesktop.Notifications',member='NotificationClosed'";
    let iter = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, Some(8)).unwrap();
    let stdin = w.stdin.as_mut().unwrap();
    writeln!(stdin, "{{\"id\":{id},\"reason\":2}}").unwrap();
    stdin.flush().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for m in iter.flatten() {
            let body = m.body();
            if let Ok(pair) = body.deserialize::<(u32, u32)>() {
                let _ = tx.send(pair);
                return;
            }
        }
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(20)).unwrap(), (id, 2));
    drop(w.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(10);
    while w.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the worker outlived its seat");
        std::thread::sleep(Duration::from_millis(20));
    }
}
