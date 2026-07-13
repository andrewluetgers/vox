//! Integration tests for claude/md2speech.sh — the markdown-to-speakable
//! filter every speech path runs through. Rule under test: never speak
//! syntax; structure becomes pauses.

use std::io::Write;
use std::process::{Command, Stdio};

fn md2speech(input: &str) -> String {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/claude/md2speech.sh");
    let mut child = Command::new("bash")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn md2speech.sh");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn strips_formatting_and_converts_structure() {
    let out = md2speech(
        "## What changed\n\n\
         This has **bold** and `inline code`.\n\
         - item one\n\
         - item two\n\
         1. step one\n\n\
         ```rust\nfn secret() {}\n```\n\
         See the [readme](https://example.com/readme).\n",
    );
    assert!(out.contains("What changed."), "header becomes text + pause: {out}");
    assert!(out.contains("This has bold and inline code."), "markers vanish: {out}");
    assert!(out.contains("item one.") && out.contains("item two."), "bullets end as sentences: {out}");
    assert!(out.contains("1. step one."), "numbered items keep numbers: {out}");
    assert!(out.contains("Code block omitted."), "code collapses: {out}");
    assert!(!out.contains("secret"), "code contents never spoken: {out}");
    assert!(out.contains("readme") && !out.contains("example.com"), "links read text not url: {out}");
    for token in ["**", "##", "`", "](", "- item"] {
        assert!(!out.contains(token), "syntax leaked: {token} in {out}");
    }
}

#[test]
fn italic_underscores_stripped_but_snake_case_survives() {
    let out = md2speech("speed applies to the _next_ utterance, keep snake_case_names and VOX_AUDIO_DIR\n");
    assert!(out.contains("the next utterance"), "{out}");
    assert!(out.contains("snake_case_names"), "{out}");
    assert!(out.contains("VOX_AUDIO_DIR"), "{out}");
}

#[test]
fn tables_read_as_phrases() {
    let out = md2speech("| Stage | Time |\n|-------|------|\n| Load | 0.4 s |\n");
    assert!(out.contains("Stage, Time."), "{out}");
    assert!(out.contains("Load, 0.4 s."), "{out}");
    assert!(!out.contains('|'), "{out}");
}
