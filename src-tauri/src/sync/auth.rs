//! Google 로그인 (drive-sync.md 3장 · K2). 시스템 기본 브라우저 + PKCE + 127.0.0.1 루프백.
//! 토큰은 OS 키체인에만 둔다 (Windows 자격 증명 관리자, macOS 키체인). DB 나 파일에 쓰지 않는다.
//!
//! OAuth 클라이언트는 빌드할 때 넣는다: NEOANKI_GOOGLE_CLIENT_ID, NEOANKI_GOOGLE_CLIENT_SECRET.
//! 데스크톱 클라이언트의 "비밀"은 Google 이 설치형 앱에 주는 값이라 비밀이 아니다 (PKCE 가 보호한다)

use super::crypto;
use super::store::{Req, SRes, SyncError, other};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use ureq::http::{Method, Request};

pub const CLIENT_ID: Option<&str> = option_env!("NEOANKI_GOOGLE_CLIENT_ID");
pub const CLIENT_SECRET: Option<&str> = option_env!("NEOANKI_GOOGLE_CLIENT_SECRET");
/// 두 개만. Drive 전체 권한(drive)은 요청하지 않는다
pub const SCOPES: [&str; 2] = ["https://www.googleapis.com/auth/drive.file", "https://www.googleapis.com/auth/drive.appdata"];
const AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const REVOKE: &str = "https://oauth2.googleapis.com/revoke";
/// 루프백 서버는 5분 기다리고 닫힌다 (K2)
pub const WAIT: Duration = Duration::from_secs(5 * 60);

const SERVICE: &str = "io.github.itisyijy.neoanki";
pub const REFRESH: &str = "google-refresh-token";
pub const KEY: &str = "drive-lock-key";

pub fn client() -> Option<(&'static str, &'static str)> {
    CLIENT_ID.filter(|c| !c.is_empty()).map(|id| (id, CLIENT_SECRET.unwrap_or("")))
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

/// RFC 7636: verifier 는 32바이트 난수(43자), challenge = BASE64URL(SHA256(verifier))
pub fn pkce() -> Pkce {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let verifier = b64.encode(crypto::random_bytes::<32>());
    let challenge = b64.encode(crypto::sha256(verifier.as_bytes()));
    Pkce { verifier, challenge, state: crypto::random_hex(16) }
}

pub fn auth_url(client_id: &str, redirect: &str, p: &Pkce) -> String {
    let scope = SCOPES.join(" ");
    url::Url::parse_with_params(
        AUTH,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect),
            ("response_type", "code"),
            ("scope", &scope),
            ("code_challenge", &p.challenge),
            ("code_challenge_method", "S256"),
            ("state", &p.state),
            // 새로 고칠 토큰을 받는다. 다시 연결할 때도 동의 화면을 보여 줘 권한 두 개를 다시 고르게 한다
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map(String::from)
    .unwrap_or_default()
}

pub(crate) fn form(uri: &str, fields: &[(&str, &str)]) -> Req {
    let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields).finish();
    Request::builder().method(Method::POST).uri(uri).header("Content-Type", "application/x-www-form-urlencoded").body(body.into_bytes()).unwrap()
}

pub fn token_req(client: (&str, &str), code: &str, verifier: &str, redirect: &str) -> Req {
    form(
        TOKEN,
        &[("client_id", client.0), ("client_secret", client.1), ("code", code), ("code_verifier", verifier), ("redirect_uri", redirect), ("grant_type", "authorization_code")],
    )
}

pub fn refresh_req(client: (&str, &str), refresh: &str) -> Req {
    form(TOKEN, &[("client_id", client.0), ("client_secret", client.1), ("refresh_token", refresh), ("grant_type", "refresh_token")])
}

pub fn revoke_req(token: &str) -> Req {
    form(REVOKE, &[("token", token)])
}

/// 사용자가 동의 화면에서 하나를 빼면 동작하지 않는다 ("권한 2개가 모두 필요해요")
pub fn granted_all(scope: &str) -> bool {
    let got: Vec<&str> = scope.split_whitespace().collect();
    SCOPES.iter().all(|s| got.contains(s))
}

/// 루프백으로 온 쿼리 (code · state · scope, ChatGPT 는 처음 등록 때 client_id 도)
pub type Query = std::collections::HashMap<String, String>;

#[derive(Debug, PartialEq)]
pub enum Callback {
    Code(Query),
    /// 사용자가 동의 화면에서 취소했다
    Denied,
    /// 다른 요청 (favicon 등). 계속 기다린다
    Ignore,
}

/// 브라우저가 루프백으로 보낸 요청 줄: "GET /?state=..&code=..&scope=.. HTTP/1.1".
/// path 는 리디렉트 주소의 경로 (Google "/" · ChatGPT "/auth/callback"). 다른 경로는 무시한다
pub fn parse_callback(request_line: &str, path: &str, state: &str) -> Result<Callback, String> {
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else { return Ok(Callback::Ignore) };
    let q: Query = url.query_pairs().into_owned().collect();
    if q.is_empty() || url.path() != path {
        return Ok(Callback::Ignore);
    }
    if q.get("state").map(String::as_str) != Some(state) {
        return Err("로그인 응답이 이 요청과 맞지 않아요. 다시 시도해 주세요.".into());
    }
    if q.contains_key("error") {
        return Ok(Callback::Denied);
    }
    if q.contains_key("code") { Ok(Callback::Code(q)) } else { Ok(Callback::Ignore) }
}

fn done_page(title: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>Neo-Anki</title>\
<body style=\"font-family:system-ui,sans-serif;display:grid;place-items:center;height:90vh;color:#16161d\">\
<div style=text-align:center><h2>{title}</h2><p>이 탭을 닫고 Neo-Anki 로 돌아가세요.</p></div>"
    )
}

#[derive(Debug, PartialEq)]
pub enum WaitError {
    Timeout,
    Cancelled,
    Denied,
    Other(String),
}

/// 루프백 서버: 코드가 올 때까지 (최대 5분, 취소하면 바로) 기다린다 (Google)
pub fn wait_code(listener: TcpListener, state: &str, cancel: &AtomicBool) -> Result<String, WaitError> {
    wait_callback(listener, "/", state, "Google 로그인을 마쳤어요", cancel).map(|mut q| q.remove("code").unwrap_or_default())
}

/// wait_code 의 일반형: path 로 온 쿼리 전부를 돌려준다. done: 브라우저 탭에 보일 제목 (ai.rs 도 쓴다)
pub fn wait_callback(listener: TcpListener, path: &str, state: &str, done: &str, cancel: &AtomicBool) -> Result<Query, WaitError> {
    listener.set_nonblocking(true).map_err(|e| WaitError::Other(e.to_string()))?;
    let until = Instant::now() + WAIT;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(WaitError::Cancelled);
        }
        if Instant::now() > until {
            return Err(WaitError::Timeout);
        }
        match listener.accept() {
            Ok((mut s, _)) => {
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 8192];
                let n = s.read(&mut buf).unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..n]);
                let line = text.lines().next().unwrap_or("");
                let res = parse_callback(line, path, state);
                let page = match &res {
                    Ok(Callback::Code(_)) => done_page(done),
                    Ok(Callback::Denied) => done_page("로그인을 취소했어요"),
                    _ => String::new(),
                };
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len());
                match res {
                    Ok(Callback::Code(c)) => return Ok(c),
                    Ok(Callback::Denied) => return Err(WaitError::Denied),
                    Ok(Callback::Ignore) => {}
                    Err(e) => return Err(WaitError::Other(e)),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(150)),
            Err(e) => return Err(WaitError::Other(e.to_string())),
        }
    }
}

pub struct Tokens {
    pub access: String,
    /// 만료 시각 (ms)
    pub expires: i64,
    pub refresh: Option<String>,
    pub scope: String,
}

pub fn parse_tokens(body: &[u8]) -> SRes<Tokens> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(other)?;
    let access = v["access_token"].as_str().ok_or_else(|| other("토큰을 받지 못했어요"))?.to_string();
    Ok(Tokens {
        access,
        // 만료 1분 전에 새로 받는다
        expires: crate::db::now_ms() + (v["expires_in"].as_i64().unwrap_or(3600) - 60) * 1000,
        refresh: v["refresh_token"].as_str().map(String::from),
        scope: v["scope"].as_str().unwrap_or_default().to_string(),
    })
}

/// 코드 -> 토큰
pub fn exchange(agent: &ureq::Agent, client: (&str, &str), code: &str, verifier: &str, redirect: &str) -> SRes<Tokens> {
    let r = super::store::send(agent, token_req(client, code, verifier, redirect))?;
    if r.status != 200 {
        return Err(other(format!("Google 이 로그인을 받아 주지 않았어요 ({})", r.status)));
    }
    parse_tokens(&r.body)
}

/// 새로 고치기. 거절(취소 · 만료)이면 Auth = "다시 로그인"
pub fn refresh(agent: &ureq::Agent, client: (&str, &str), refresh: &str) -> SRes<Tokens> {
    let r = super::store::send(agent, refresh_req(client, refresh))?;
    match r.status {
        200 => parse_tokens(&r.body),
        400 | 401 => Err(SyncError::Auth),
        s => Err(other(format!("토큰을 새로 받지 못했어요 ({s})"))),
    }
}

pub fn revoke(agent: &ureq::Agent, token: &str) {
    let _ = super::store::send(agent, revoke_req(token));
}

// ---------------------------------------------------------------- OS 키체인

/// 검증용 데이터 폴더(NEOANKI_DATA_DIR)로 띄운 앱은 폴더마다 다른 항목 이름을 쓴다.
/// 실제 앱의 로그인 토큰을 읽거나 덮거나 지우지 않게 하려는 것
fn secret_name(name: &str) -> String {
    use sha2::Digest;
    match std::env::var_os("NEOANKI_DATA_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => {
            let h = sha2::Sha256::digest(dir.to_string_lossy().as_bytes());
            format!("{name}@{}", h[..4].iter().map(|b| format!("{b:02x}")).collect::<String>())
        }
        None => name.to_string(),
    }
}

pub fn save_secret(name: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, &secret_name(name)).and_then(|e| e.set_password(value)).map_err(|e| format!("키체인에 저장하지 못했어요: {e}"))
}

pub fn load_secret(name: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, &secret_name(name)).and_then(|e| e.get_password()).ok()
}

pub fn delete_secret(name: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, &secret_name(name)) {
        let _ = e.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_and_auth_url() {
        let p = pkce();
        assert_eq!(p.verifier.len(), 43, "RFC 7636: 43~128자");
        assert!(p.verifier.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        use base64::Engine;
        assert_eq!(p.challenge, base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crypto::sha256(p.verifier.as_bytes())));
        let u = url::Url::parse(&auth_url("cid", "http://127.0.0.1:5123", &p)).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(u.host_str(), Some("accounts.google.com"));
        assert_eq!(q["scope"], "https://www.googleapis.com/auth/drive.file https://www.googleapis.com/auth/drive.appdata");
        assert_eq!((q["code_challenge_method"].as_str(), q["redirect_uri"].as_str()), ("S256", "http://127.0.0.1:5123"));
        assert_eq!((q["access_type"].as_str(), q["state"].as_str()), ("offline", p.state.as_str()));
    }

    #[test]
    fn token_requests_are_forms() {
        let r = token_req(("cid", "sec"), "4/0Ab", "ver", "http://127.0.0.1:5123");
        assert_eq!((r.method(), r.uri().to_string()), (&Method::POST, "https://oauth2.googleapis.com/token".to_string()));
        assert_eq!(r.headers()["Content-Type"], "application/x-www-form-urlencoded");
        let body: std::collections::HashMap<_, _> = url::form_urlencoded::parse(r.body()).into_owned().collect();
        assert_eq!(body["code"], "4/0Ab");
        assert_eq!(body["code_verifier"], "ver");
        assert_eq!(body["grant_type"], "authorization_code");
        let r = refresh_req(("cid", "sec"), "rt");
        let body: std::collections::HashMap<_, _> = url::form_urlencoded::parse(r.body()).into_owned().collect();
        assert_eq!((body["grant_type"].as_str(), body["refresh_token"].as_str()), ("refresh_token", "rt"));
        assert_eq!(revoke_req("t").uri().to_string(), "https://oauth2.googleapis.com/revoke");
    }

    #[test]
    fn callback_parsing_and_scopes() {
        let line = "GET /?state=abc&code=4%2F0Ab&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file HTTP/1.1";
        assert!(matches!(parse_callback(line, "/", "abc"), Ok(Callback::Code(q)) if q["code"] == "4/0Ab"));
        assert!(parse_callback(line, "/", "zzz").is_err(), "state 가 다르면 받지 않는다 (CSRF)");
        assert_eq!(parse_callback("GET /?state=abc&error=access_denied HTTP/1.1", "/", "abc"), Ok(Callback::Denied));
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1", "/", "abc"), Ok(Callback::Ignore));
        assert!(granted_all("https://www.googleapis.com/auth/drive.appdata https://www.googleapis.com/auth/drive.file"));
        assert!(!granted_all("https://www.googleapis.com/auth/drive.file"), "하나만 허용하면 다시 연결");
    }

    #[test]
    fn loopback_returns_code() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let cancel = AtomicBool::new(false);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET /?state=st&code=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out
        });
        assert_eq!(wait_code(l, "st", &cancel), Ok("xyz".into()));
        assert!(t.join().unwrap().contains("Google 로그인을 마쳤어요"));
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(wait_code(l, "st", &cancel), Err(WaitError::Cancelled));
    }
}
