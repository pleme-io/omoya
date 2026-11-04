use std::path::{Path, PathBuf};
use std::process::Command;

fn exe() -> PathBuf {
    std::env::var_os("OMOYA_BIN")
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_omoya")), PathBuf::from)
}

fn bench(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("omoya-world-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bar")).unwrap();
    dir
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn check(dir: &Path, file: &Path) -> serde_json::Value {
    let out = Command::new(exe())
        .arg("world-check")
        .arg(file)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_RUNTIME_DIR", dir)
        .env_remove("OMOYA_CONFIG")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .rfind(|l| l.starts_with('{'))
        .unwrap_or_else(|| panic!("no json in {text}"));
    serde_json::from_str(line).unwrap()
}

const WORLD: &str = r#"
use("kurashi", %i[backlight bar_module bind command look volume])

def omoya()
  [
    look({bar_height: 31, accent: :frost3}),
    command(:louder, volume(5)),
    command(:brighter, backlight(5)),
    command(:term, %w[mado]),
    command(:bad, {kind: "nope"}),
    bind("volume_up", :louder, {locked: true}),
    bind("super+a", :term),
    bind("ctrl+alt+f2", :term),
    bind("super+b", :explode),
    bar_module("answers", {every_s: 60}),
    bar_module("raises", {every_s: 60}),
    bar_module("spins", {every_s: 60, fuel: 5000}),
    bar_module("exits", {every_s: 60, allow: [:process]}),
    bar_module("silent", {every_s: 60}),
    bar_module("absent", {every_s: 60})
  ]
end
"#;

#[test]
fn a_world_stands_entry_by_entry_and_each_module_fails_alone() {
    let dir = bench("whole");
    write(&dir, "bar/answers.b", "def render()\n  \"3h\"\nend\n");
    write(
        &dir,
        "bar/raises.b",
        "def render()\n  raise(\"boom\")\nend\n",
    );
    write(
        &dir,
        "bar/spins.b",
        "def spin(n)\n  spin(n + 1)\nend\n\ndef render()\n  spin(0)\nend\n",
    );
    write(&dir, "bar/exits.b", "def render()\n  exit_with(7)\nend\n");
    write(&dir, "bar/silent.b", "def render()\n  nil\nend\n");
    let file = write(&dir, "omoya.b", WORLD);
    let v = check(&dir, &file);
    assert_eq!(v["state"], "loaded", "{v}");
    assert_eq!(v["config"]["bar"]["height"], 31);
    assert_eq!(v["config"]["look"]["accent"], "frost3");
    assert_eq!(
        v["commands"],
        serde_json::json!(["brighter", "louder", "term"])
    );
    let refusals = v["refusals"].as_array().unwrap();
    let text = serde_json::to_string(refusals).unwrap();
    assert_eq!(refusals.len(), 3, "{text}");
    assert!(text.contains("command bad"), "{text}");
    assert!(
        text.contains("bind ctrl+alt+f2") && text.contains("reserved"),
        "{text}"
    );
    assert!(
        text.contains("bind super+b") && text.contains("explode"),
        "{text}"
    );
    let cells = &v["cells"];
    assert_eq!(cells["answers"], "3h", "{cells}");
    assert_eq!(cells["silent"], serde_json::Value::Null, "{cells}");
    assert_eq!(cells["raises"]["refused"], "raised", "{cells}");
    assert_eq!(cells["spins"]["refused"], "exhausted", "{cells}");
    assert_eq!(cells["exits"]["refused"], "died", "{cells}");
    assert_eq!(cells["absent"]["refused"], "load", "{cells}");
}

fn blue_block(nth: usize) -> &'static str {
    include_str!("../../../docs/DAILY-SEAT.md")
        .split("```blue\n")
        .nth(nth)
        .and_then(|rest| rest.split("```").next())
        .unwrap()
}

#[test]
fn the_world_the_plan_shows_stands_with_no_refusal() {
    let dir = bench("plan");
    write(&dir, "bar/uptime.b", blue_block(2));
    let file = write(&dir, "omoya.b", blue_block(1));
    let v = check(&dir, &file);
    assert_eq!(v["state"], "loaded", "{v}");
    assert_eq!(v["refusals"], serde_json::json!([]), "{v}");
    assert_eq!(v["config"]["bar"]["height"], 26, "{v}");
    assert_eq!(v["config"]["idle"]["blank_s"], 300, "{v}");
    assert_eq!(v["config"]["notes"]["enabled"], true, "{v}");
    assert_eq!(v["config"]["layout"]["snap_threshold"], 16, "{v}");
    assert_eq!(v["config"]["lock"]["client"][0], "swaylock", "{v}");
    assert!(
        v["cells"]["uptime"]
            .as_str()
            .is_some_and(|t| t.ends_with('h')),
        "{v}"
    );
    assert!(v["cells"].get("spare").is_none(), "{v}");
}

#[test]
fn a_world_that_does_not_parse_is_refused_whole_and_says_why() {
    let dir = bench("parse");
    let file = write(&dir, "omoya.b", "def (((\n");
    let v = check(&dir, &file);
    assert_eq!(v["state"], "refused");
    assert_eq!(
        v["config"]["bar"]["height"],
        check(&dir, &dir.join("nothing.b"))["config"]["bar"]["height"]
    );
    assert!(
        v["refusals"][0].as_str().unwrap().starts_with("load:"),
        "{v}"
    );
}

#[test]
fn a_world_with_no_entry_function_is_refused_and_a_missing_file_is_absent() {
    let dir = bench("entry");
    let file = write(&dir, "omoya.b", "def other()\n  []\nend\n");
    assert_eq!(check(&dir, &file)["state"], "refused");
    assert_eq!(check(&dir, &dir.join("nothing.b"))["state"], "absent");
}

#[test]
fn a_world_that_hangs_cannot_hold_the_check_past_its_limit() {
    let dir = bench("hang");
    let file = write(
        &dir,
        "omoya.b",
        "def spin(n)\n  spin(n + 1)\nend\n\ndef omoya()\n  spin(0)\nend\n",
    );
    let started = std::time::Instant::now();
    let v = check(&dir, &file);
    assert_eq!(v["state"], "refused", "{v}");
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn a_private_bidama_beside_the_world_is_found() {
    let dir = bench("bidama");
    std::fs::create_dir_all(dir.join("bidamas/mine")).unwrap();
    write(
        &dir,
        "bidamas/mine/Bluefile",
        "package(\"mine\", \"0.1.0\")\n",
    );
    write(&dir, "bidamas/mine/mine.b", "def text()\n  \"mine\"\nend\n");
    write(
        &dir,
        "bar/uses.b",
        "use(\"mine\", [:text])\n\ndef render()\n  text\nend\n",
    );
    let file = write(
        &dir,
        "omoya.b",
        "use(\"kurashi\", [:bar_module])\n\ndef omoya()\n  [bar_module(\"uses\", {})]\nend\n",
    );
    let v = check(&dir, &file);
    assert_eq!(v["cells"]["uses"], "mine", "{v}");
}
