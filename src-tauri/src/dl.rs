//! 모델 파일 받기 공통 (embed.rs 분석 모델 · llm.rs 개념 관계 확인 모델).
//! 연결 · 본문 읽기는 따로 도는 스레드가 하고, 부른 쪽은 받은 조각만 쓴다. 그래서 읽기가 막혀 있어도
//! [멈추기]는 곧바로 먹고, 데이터가 IDLE 동안 오지 않으면 "연결이 멈췄다"로 끝낸다.
//! ureq 3.4.2 에는 읽기마다의 시간 제한이 없고 본문 전체 제한(`timeout_recv_body`)만 있어서다.
//! 멈추거나 끝낸 연결은 버린다: 읽던 스레드는 다음 데이터 · 끊김 · 전체 제한(STRAY)에서 스스로 끝난다

use std::io::Read;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// 이만큼 데이터가 오지 않으면 연결이 멈춘 것으로 본다
pub const IDLE: Duration = Duration::from_secs(30);
/// 버린 연결을 읽던 스레드가 늦어도 끝나는 때 (본문 전체 제한). 느린 회선의 1.1GB 도 넉넉히
/// ponytail: 버린 스레드는 이때까지 소켓 하나를 쥘 수 있다. 문제가 되면 연결을 직접 닫는 Transport 로
const STRAY: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, PartialEq)]
pub enum Fail {
    /// 사용자가 멈췄다
    Cancelled,
    /// IDLE 동안 데이터가 오지 않았다
    Stalled,
    /// 연결 · 응답 · 읽기 오류 (원인 글, 화면에는 쓰지 않고 로그로)
    Net(String),
    /// 받은 조각을 쓰다가 (파일 쓰기 등) 난 오류 (그대로 보인다)
    Write(String),
}

enum Msg {
    Data(Vec<u8>),
    End(Result<(), String>),
}

/// url 의 본문을 limit 바이트까지 받아 조각마다 on 을 부른다. cancel() 은 0.2초마다 본다
pub fn stream(url: &str, limit: u64, idle: Duration, cancel: &dyn Fn() -> bool, mut on: impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), Fail> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(STRAY))
        .build()
        .into();
    let url = url.to_string();
    // 조각 16개까지만 쌓는다 (쓰기가 느려도 메모리가 늘지 않게)
    let (tx, rx) = mpsc::sync_channel::<Msg>(16);
    std::thread::spawn(move || {
        let res = (|| -> Result<(), String> {
            let resp = agent.get(&url).call().map_err(|e| e.to_string())?;
            let mut reader = resp.into_body().into_with_config().limit(limit).reader();
            let mut buf = vec![0u8; 1 << 16];
            loop {
                let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    return Ok(());
                }
                if tx.send(Msg::Data(buf[..n].to_vec())).is_err() {
                    return Ok(()); // 부른 쪽이 그만뒀다
                }
            }
        })();
        let _ = tx.send(Msg::End(res));
    });
    let mut last = Instant::now();
    loop {
        if cancel() {
            return Err(Fail::Cancelled);
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Msg::Data(b)) => {
                last = Instant::now();
                on(&b).map_err(Fail::Write)?;
            }
            Ok(Msg::End(Ok(()))) => return Ok(()),
            Ok(Msg::End(Err(e))) => return Err(Fail::Net(e)),
            Err(mpsc::RecvTimeoutError::Timeout) if last.elapsed() > idle => return Err(Fail::Stalled),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(Fail::Net("받던 스레드가 끝났어요".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 로컬 서버: 헤더는 total 바이트라고 하고 send 바이트만 보낸 뒤 hold 동안 연결을 연 채 아무것도 보내지 않는다
    fn server(total: usize, send: usize, hold: Duration) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nContent-Type: application/octet-stream\r\n\r\n");
            let _ = s.write_all(&vec![7u8; send]);
            let _ = s.flush();
            std::thread::sleep(hold);
        });
        format!("http://{addr}/model.gguf")
    }

    #[test]
    fn whole_body_arrives() {
        let url = server(300_000, 300_000, Duration::from_millis(10));
        let mut got = 0;
        assert_eq!(stream(&url, 300_001, IDLE, &|| false, |b| {
            got += b.len();
            Ok(())
        }), Ok(()));
        assert_eq!(got, 300_000);
    }

    /// QA (2026-10-22): 연결이 끊기지 않고 데이터만 멈추면 IDLE 뒤에 끝난다 (전에는 끝나지 않았다)
    #[test]
    fn stalled_connection_ends_after_idle() {
        let url = server(1_000_000, 8_000, Duration::from_secs(20));
        let (t, mut got) = (Instant::now(), 0);
        assert_eq!(stream(&url, 1_000_001, Duration::from_secs(1), &|| false, |b| {
            got += b.len();
            Ok(())
        }), Err(Fail::Stalled));
        assert_eq!(got, 8_000);
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// QA (2026-10-22): 읽기가 막혀 있어도 [멈추기]가 곧바로 먹는다
    #[test]
    fn cancel_works_while_the_read_is_blocked() {
        let url = server(1_000_000, 8_000, Duration::from_secs(20));
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            s2.store(true, Ordering::SeqCst);
        });
        let t = Instant::now();
        assert_eq!(stream(&url, 1_000_001, IDLE, &|| stop.load(Ordering::SeqCst), |_| Ok(())), Err(Fail::Cancelled));
        assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    }

    #[test]
    fn closed_port_is_a_network_error_and_write_errors_pass_through() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port(); // 닫힌 포트
        assert!(matches!(stream(&format!("http://127.0.0.1:{port}/x"), 10, IDLE, &|| false, |_| Ok(())), Err(Fail::Net(_))));
        let url = server(1000, 1000, Duration::from_millis(10));
        assert_eq!(stream(&url, 1001, IDLE, &|| false, |_| Err("디스크가 가득 찼어요".into())), Err(Fail::Write("디스크가 가득 찼어요".into())));
    }
}
