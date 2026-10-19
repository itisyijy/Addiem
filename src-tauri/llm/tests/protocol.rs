//! 실제 실행 파일로: 한 줄 JSON 요청 · 답, 깨진 줄, stdin 이 닫히면 끝나는지 (본체가 죽어도 고아 프로세스가 남지 않게)
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn answers_line_by_line_and_exits_when_stdin_closes() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_neoanki-llm")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    writeln!(stdin, r#"{{"id":7,"op":"stats"}}"#).unwrap();
    out.read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["id"], 7);
    assert!(v["ok"]["peak_mb"].is_number(), "{line}");
    line.clear();
    writeln!(stdin, "깨진 줄").unwrap();
    out.read_line(&mut line).unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["id"], 0);
    assert!(v["err"].is_string(), "{line}");
    drop(stdin);
    let t = Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(t.elapsed() < Duration::from_secs(5), "stdin 이 닫혔는데 끝나지 않았다");
        std::thread::sleep(Duration::from_millis(50));
    }
}
