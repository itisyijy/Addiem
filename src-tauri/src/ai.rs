//! G2 "성격별로 묶기" · ChatGPT 요금제 사용 (Sign in with ChatGPT, 오픈소스 · 로컬 앱의 direct 흐름).
//! developers.openai.com/siwc/token-sharing-open-source. 클라이언트 비밀 없음 · PKCE(S256) · state · nonce ·
//! 127.0.0.1 루프백 /auth/callback (sync::auth 의 루프백 · 키체인 도우미를 같이 쓴다).
//!
//! - 처음 연결: client_id=dynamic_agent_client + 이 설치의 ext_agent_host_id(urn:uuid). 돌아온 client_id(oaiapp_…)를 저장해 다음부터 쓴다
//! - 새로 고칠 토큰만 OS 키체인에. 접근 토큰은 메모리에만. client_id · host id · 이메일 · 모델 · 이번 달 보낸 카드 수는 schema_meta (동기화하지 않는다)
//! - 보내는 것: 사용자가 누른 카드 한 장의 질문 글자와 답 항목 글자뿐 (Q8 오답 보기 제안은 편집 중인 질문 · 정답 · 보기 글자, recall.rs). 덱 이름 · 복습 기록 · 자료 · 그림은 보내지 않는다
//! - 결과는 quality::SplitProposal 과 같은 모양의 제안이다. 카드를 바꾸는 것은 여전히 사용자가 승인한 split_apply 뿐
//!
//! - 8.2 [AI로 확인](ai_check_card): 그 카드의 질문 · 답 글자(+ 확인 목록에서 남긴 연결 자료 구간)만 보내고, 판단 · 근거 · 고친 안을 받는다. 바꾸는 것은 승인한 quality::rewrite_apply 뿐
//!
//! Claude · Gemini 구독 로그인은 넣지 않는다: 제3자 앱이 소비자 구독으로 로그인하는 것을 허용하지 않는다 (DECISIONS I73)

use crate::commands::AppState;
use crate::quality::{self, SplitPart, SplitProposal};
use crate::rag;
use crate::sync::{auth, crypto, store};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Value as J, json};
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};
use ureq::http::{Method, Request};

const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
/// https://auth.openai.com/.well-known/openid-configuration 의 revocation_endpoint · jwks_uri (RS256)
const REVOKE: &str = "https://auth.openai.com/api/accounts/oauth/revoke";
const JWKS: &str = "https://auth.openai.com/.well-known/jwks.json";
const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const RESPONSES: &str = "https://api.openai.com/v1/responses";
const MODELS: &str = "https://api.openai.com/v1/models";
pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// 이 권한이 없으면 요금제로 추론할 수 없다 ("사용량 권한")
const USE_SCOPE: &str = "chatgpt.tokens.use.direct";
const DYNAMIC: &str = "dynamic_agent_client";
const CALLBACK: &str = "/auth/callback";
/// 문서 예시와 같은 포트. 쓰고 있으면 브라우저가 막지 않는 아무 포트 (auth::loopback. 포트만 바뀌어도 된다)
const PORT: u16 = 1455;

/// 키체인 이름 (sync::auth 와 같은 서비스)
const REFRESH: &str = "chatgpt-refresh-token";
const META_HOST: &str = "ai_host_id";
const META_CLIENT: &str = "ai_client_id";
/// 그 등록을 어떤 이름(agent_name_hint)으로 했는지. 없으면 Neo-Anki 시절 등록이다 (QA QB-09)
const META_CLIENT_NAME: &str = "ai_client_name";
/// ChatGPT 연결 화면 · 연결된 앱 목록에 보이는 이름
const AGENT_NAME: &str = "Addiem";
/// 있으면 연결됨 (화면 표시용 이메일). 계정 식별은 검사한 ID 토큰의 sub
const META_EMAIL: &str = "ai_email";
const META_SUB: &str = "ai_sub";
const META_MODEL: &str = "ai_model";
/// "YYYY-MM N": 이번 달 보낸 카드와 마지막으로 보낸 시각 (ms). 이 기기에서만 센다
const META_SENT: &str = "ai_sent";
const META_SENT_AT: &str = "ai_sent_at";
/// 첫 연결 환영(L3)을 보였다
const META_WELCOMED: &str = "ai_welcomed";

/// 묶은 카드 수 (L4 디자인 노트: 2–8장)
const MIN_CARDS: usize = 2;
const MAX_CARDS: usize = 8;
const MIN_ITEMS: usize = 3;
const MAX_ITEMS: usize = 60;
const MAX_INPUT_CHARS: usize = 6000;
const MAX_FRONT_CHARS: usize = 200;

type Res<T> = Result<T, String>;

// ---------------------------------------------------------------- 오류 (화면이 kind 로 고른다)

#[derive(Debug, Clone, PartialEq)]
pub enum AiError {
    NotConnected,
    /// 새로 고치기가 끝내 거절됐다 → 다시 로그인
    Expired,
    /// chatgpt.tokens.use.direct 를 허락하지 않았다
    Scope,
    NotEligible,
    /// 잠시 쓸 수 없다 (503). 다시 해 볼 만하다
    Unavailable,
    /// 사용량 한도. [사용량 관리]가 첫 버튼
    Limit,
    Offline(String),
    /// 모델 출력이 규칙에 맞지 않는다 → 규칙 나누기를 보인다 (깨진 응답은 한 조각도 쓰지 않는다)
    BadOutput(String),
    /// [취소] · 연결 끊기
    Cancelled,
    Other(String),
}

impl AiError {
    pub fn kind(&self) -> &'static str {
        match self {
            AiError::NotConnected => "not_connected",
            AiError::Expired => "expired",
            AiError::Scope => "scope",
            AiError::NotEligible => "not_eligible",
            AiError::Unavailable => "unavailable",
            AiError::Limit => "limit",
            AiError::Offline(_) => "offline",
            AiError::BadOutput(_) => "bad_output",
            AiError::Cancelled => "cancelled",
            AiError::Other(_) => "error",
        }
    }
}

impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AiError::NotConnected => write!(f, "ChatGPT 연결이 필요해요."),
            AiError::Expired => write!(f, "ChatGPT 로그인이 만료됐어요. 다시 로그인해 주세요."),
            AiError::Scope => write!(f, "사용량 권한을 허락하지 않았어요."),
            AiError::NotEligible => write!(f, "이 계정의 ChatGPT 요금제로는 쓸 수 없어요."),
            AiError::Unavailable => write!(f, "지금은 ChatGPT 요금제를 쓸 수 없어요. 잠시 뒤 다시 해 보세요."),
            AiError::Limit => write!(f, "사용량 한도에 도달했어요."),
            AiError::Offline(_) => write!(f, "인터넷에 연결되지 않았어요."),
            AiError::BadOutput(_) => write!(f, "ChatGPT 제안이 카드 모양에 맞지 않아 규칙으로 나눈 제안을 보여 드려요."),
            AiError::Cancelled => write!(f, "요청을 멈췄어요."),
            AiError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// 화면에는 { kind, message } 로 간다
impl Serialize for AiError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        json!({ "kind": self.kind(), "message": self.to_string() }).serialize(s)
    }
}

fn net(e: store::SyncError) -> AiError {
    match e {
        store::SyncError::Offline(m) => AiError::Offline(m),
        e => AiError::Other(e.to_string()),
    }
}

/// Responses 오류 코드 (요청 거절 본문의 error.code 이든 스트림 중의 response.failed 이든) → 화면 오류.
/// 사용량 · 자격 오류는 다른 결제로 넘기지 않고 멈춘다 (errors-and-recovery)
fn from_code(code: &str, message: &str) -> AiError {
    match code {
        "subscription_sharing_usage_limit_exceeded" => AiError::Limit,
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => AiError::Unavailable,
        "subscription_sharing_user_not_eligible" => AiError::NotEligible,
        "chatpass_v2_scope_not_authorized" => AiError::Scope,
        _ => AiError::Other(format!("ChatGPT가 요청을 처리하지 못했어요 ({}).", if message.is_empty() { code } else { message })),
    }
}

/// 스트림을 열기 전에 거절된 요청
#[derive(Debug, PartialEq)]
enum Reject {
    /// 401: 토큰을 한 번 새로 받아 다시 보낸다
    Unauthorized,
    /// text.format(json_schema)을 받지 않는다: 형식 없이 다시 (같은 본문을 다시 보내지 않는다)
    DropFormat,
    /// 이 모델을 쓸 수 없다: 목록의 다음 모델로
    NextModel,
    Fail(AiError),
}

fn classify(status: u16, body: &[u8]) -> Reject {
    let v: J = serde_json::from_slice(body).unwrap_or(J::Null);
    let e = &v["error"];
    let code = e["code"].as_str().unwrap_or("");
    let param = e["param"].as_str().unwrap_or("");
    let detail = e["message"].as_str().or(v["detail"].as_str()).unwrap_or("");
    match (status, code) {
        (401, _) => Reject::Unauthorized,
        (_, "subscription_sharing_unsupported_capability") if param.starts_with("text") => Reject::DropFormat,
        (_, "subscription_sharing_unsupported_capability") if param == "model" => Reject::NextModel,
        (404, _) | (_, "model_not_found") => Reject::NextModel,
        (_, c) if !c.is_empty() => Reject::Fail(from_code(c, detail)),
        (429, _) | (500..=599, _) => Reject::Fail(AiError::Unavailable),
        (s, _) => Reject::Fail(AiError::Other(format!("ChatGPT가 요청을 받지 않았어요 ({s}{}).", if detail.is_empty() { String::new() } else { format!(": {detail}") }))),
    }
}

// ---------------------------------------------------------------- 요청 (순수 함수, 테스트한다)

/// 처음 연결이면 client_id 가 없다 (dynamic_agent_client 로 등록). reconsent: 사용량 권한을 빼고 허락했을 때 동의 화면을 다시
pub fn authorize_url(client_id: Option<&str>, host_id: &str, redirect: &str, p: &auth::Pkce, nonce: &str, reconsent: bool) -> String {
    let mut q = vec![
        ("client_id", client_id.unwrap_or(DYNAMIC)),
        ("ext_agent_host_id", host_id),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        ("scope", SCOPE),
        ("resource", RESOURCE),
        ("state", &p.state),
        ("nonce", nonce),
        ("code_challenge", &p.challenge),
        ("code_challenge_method", "S256"),
    ];
    if client_id.is_none() {
        q.push(("agent_name_hint", AGENT_NAME));
    }
    if reconsent {
        q.push(("prompt", "consent"));
    }
    url::Url::parse_with_params(AUTHORIZE, &q).map(String::from).unwrap_or_default()
}

/// 클라이언트 비밀은 없다. 같은 redirect_uri · resource
fn token_req(client_id: &str, code: &str, verifier: &str, redirect: &str) -> store::Req {
    auth::form(
        TOKEN,
        &[("grant_type", "authorization_code"), ("client_id", client_id), ("code", code), ("code_verifier", verifier), ("redirect_uri", redirect), ("resource", RESOURCE)],
    )
}

/// scope 를 보내지 않으면 받은 권한을 그대로 유지한다
fn refresh_req(client_id: &str, refresh: &str) -> store::Req {
    auth::form(TOKEN, &[("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh), ("resource", RESOURCE)])
}

fn revoke_req(client_id: &str, refresh: &str) -> store::Req {
    auth::form(REVOKE, &[("token", refresh), ("token_type_hint", "refresh_token"), ("client_id", client_id)])
}

fn get_req(uri: &str, token: Option<&str>) -> store::Req {
    let b = Request::builder().method(Method::GET).uri(uri);
    let b = match token {
        Some(t) => b.header("Authorization", format!("Bearer {t}")),
        None => b,
    };
    b.body(vec![]).unwrap()
}

fn responses_req(token: &str, body: &J) -> store::Req {
    Request::builder()
        .method(Method::POST)
        .uri(RESPONSES)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .body(body.to_string().into_bytes())
        .unwrap()
}

/// RFC 9562 UUIDv4 (난수 16바이트에 버전 · variant 비트). 설치마다 한 번 만들어 둔다
fn new_host_id() -> String {
    let mut b = crypto::random_bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = crypto::hex(&b);
    format!("urn:uuid:{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// 콜백의 client_id. 처음 등록이면 새로 받은 id(oaiapp_…), 다시 로그인이면 저장한 id 와 같아야 한다
fn issued_client(pending: Option<&str>, got: Option<&str>) -> Result<String, String> {
    match (pending, got) {
        (Some(p), Some(g)) if p != g => Err("로그인 응답이 이 앱의 등록과 맞지 않아요. 다시 시도해 주세요.".into()),
        (Some(p), _) => Ok(p.to_string()),
        (None, Some(g)) if !g.is_empty() && g != DYNAMIC => Ok(g.to_string()),
        (None, _) => Err("ChatGPT가 앱 등록 정보를 주지 않았어요. 다시 시도해 주세요.".into()),
    }
}

fn has_use_scope(scope: &str) -> bool {
    scope.split_whitespace().any(|s| s == USE_SCOPE)
}

struct Tokens {
    access: String,
    /// 만료 시각 (ms). 1분 앞당긴다
    expires: i64,
    refresh: Option<String>,
    id_token: Option<String>,
    /// 받은 권한. 실제 응답(교환 · 새로 고치기)에는 늘 있어서, 없으면 아무 권한도 받지 않은 것으로 본다
    scope: String,
}

fn parse_tokens(body: &[u8]) -> Result<Tokens, AiError> {
    let v: J = serde_json::from_slice(body).map_err(|_| AiError::Other("토큰 응답을 읽지 못했어요.".into()))?;
    let access = v["access_token"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| AiError::Other("토큰을 받지 못했어요.".into()))?.to_string();
    let scope = v["scope"].as_str().unwrap_or_default().to_string();
    Ok(Tokens {
        expires: crate::db::now_ms() + (v["expires_in"].as_i64().unwrap_or(3600) - 60) * 1000,
        refresh: v["refresh_token"].as_str().map(String::from),
        id_token: v["id_token"].as_str().map(String::from),
        access,
        scope,
    })
}

/// 새로 고치기 거절. 쓸 수 없는 토큰이면 Expired (토큰을 지우고 다시 로그인), 네트워크 · 서버 오류면 토큰을 지우지 않는다
fn refresh_error(status: u16, body: &[u8]) -> AiError {
    let v: J = serde_json::from_slice(body).unwrap_or(J::Null);
    let code = v["error"].as_str().or(v["error"]["code"].as_str()).unwrap_or("");
    match code {
        "invalid_grant" | "invalid_refresh_token" | "token_expired" | "refresh_token_expired" | "refresh_token_invalidated" | "refresh_token_reused" => AiError::Expired,
        "invalid_client" => AiError::Other("ChatGPT 앱 등록이 맞지 않아요. 연결을 끊고 다시 연결해 주세요.".into()),
        _ if status >= 500 || status == 429 => AiError::Unavailable,
        _ if status == 401 => AiError::Expired,
        _ => AiError::Other(format!("ChatGPT 로그인을 새로 받지 못했어요 ({status}).")),
    }
}

// ---------------------------------------------------------------- ID 토큰

fn b64(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|_| "ID 토큰을 읽지 못했어요".to_string())
}

/// (머리, 내용, 서명한 부분, 서명)
fn split_jwt(token: &str) -> Result<(J, J, &str, Vec<u8>), String> {
    let mut it = token.split('.');
    let (Some(h), Some(c), Some(s), None) = (it.next(), it.next(), it.next(), it.next()) else { return Err("ID 토큰 모양이 아니에요".into()) };
    let json = |part: &str| -> Result<J, String> { serde_json::from_slice(&b64(part)?).map_err(|_| "ID 토큰을 읽지 못했어요".to_string()) };
    Ok((json(h)?, json(c)?, &token[..h.len() + 1 + c.len()], b64(s)?))
}

/// JWKS 의 RSA 키로 RS256 서명 확인 (ring 은 rustls 가 이미 쓴다)
fn verify_rs256(jwks: &J, kid: Option<&str>, signed: &[u8], sig: &[u8]) -> Result<(), String> {
    use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
    for k in jwks["keys"].as_array().into_iter().flatten() {
        if k["kty"] != "RSA" || kid.is_some_and(|id| k["kid"].as_str() != Some(id)) {
            continue;
        }
        let (Some(n), Some(e)) = (k["n"].as_str(), k["e"].as_str()) else { continue };
        let key = RsaPublicKeyComponents { n: b64(n)?, e: b64(e)? };
        if key.verify(&RSA_PKCS1_2048_8192_SHA256, signed, sig).is_ok() {
            return Ok(());
        }
    }
    Err("ID 토큰 서명이 맞지 않아요".into())
}

#[derive(Debug, PartialEq)]
struct Identity {
    sub: String,
    email: String,
}

/// iss · aud(발급받은 client_id) · exp · nonce. 화면에 보일 계정은 검사한 뒤의 이메일
fn check_claims(c: &J, client_id: &str, nonce: &str, now_s: i64) -> Result<Identity, String> {
    if c["iss"] != ISSUER {
        return Err("ID 토큰 발급자가 OpenAI 가 아니에요".into());
    }
    let aud_ok = match &c["aud"] {
        J::String(a) => a == client_id,
        J::Array(a) => a.iter().any(|x| x == client_id),
        _ => false,
    };
    if !aud_ok {
        return Err("ID 토큰이 이 앱에 발급된 것이 아니에요".into());
    }
    // 시계가 조금 틀려도 1분은 봐 준다
    if c["exp"].as_i64().is_none_or(|exp| exp + 60 < now_s) {
        return Err("ID 토큰이 만료됐어요".into());
    }
    if c["nonce"].as_str() != Some(nonce) {
        return Err("로그인 응답이 이 요청과 맞지 않아요".into());
    }
    let sub = c["sub"].as_str().filter(|s| !s.is_empty()).ok_or("ID 토큰에 계정이 없어요")?.to_string();
    Ok(Identity { sub, email: c["email"].as_str().unwrap_or("ChatGPT 계정").to_string() })
}

fn verify_id_token(agent: &ureq::Agent, token: &str, client_id: &str, nonce: &str) -> Result<Identity, AiError> {
    let other = |e: String| AiError::Other(format!("{e}. 다시 시도해 주세요."));
    let (head, claims, signed, sig) = split_jwt(token).map_err(other)?;
    if head["alg"] != "RS256" {
        return Err(other("ID 토큰 서명 방식이 맞지 않아요".into()));
    }
    let r = store::send(agent, get_req(JWKS, None)).map_err(net)?;
    let jwks: J = serde_json::from_slice(&r.body).map_err(|_| other("OpenAI 서명 키를 받지 못했어요".into()))?;
    verify_rs256(&jwks, head["kid"].as_str(), signed.as_bytes(), &sig).map_err(other)?;
    check_claims(&claims, client_id, nonce, crate::db::now_ms() / 1000).map_err(other)
}

// ---------------------------------------------------------------- 모델

/// visibility == "list" 만, 서버 순서대로. 이 일은 작은 모델로 충분해 이름에 mini 가 든 것을 앞으로
/// ponytail: 이름으로 고르는 어림. 모델을 고르는 화면이 생기면 그 값을 쓴다
fn pick_models(body: &J) -> Vec<String> {
    let list = body["models"].as_array().or(body["data"].as_array());
    let mut out: Vec<String> = list
        .into_iter()
        .flatten()
        .filter(|m| m["visibility"] == "list")
        .filter_map(|m| m["slug"].as_str().or(m["id"].as_str()).map(String::from))
        .collect();
    out.sort_by_key(|s| !s.contains("mini"));
    out
}

fn list_models(agent: &ureq::Agent, token: &str) -> Result<Vec<String>, Reject> {
    let r = store::send(agent, get_req(MODELS, Some(token))).map_err(|e| Reject::Fail(net(e)))?;
    if r.status != 200 {
        return Err(classify(r.status, &r.body));
    }
    Ok(pick_models(&serde_json::from_slice(&r.body).unwrap_or(J::Null)))
}

// ---------------------------------------------------------------- 스트림 (SSE)

#[derive(Debug, PartialEq)]
enum Outcome {
    /// response.completed 까지 받은 글자
    Done(String),
    /// response.failed · error 이벤트 (사용량 한도는 스트림 중간에도 온다)
    Failed(AiError),
    /// response.incomplete (이유)
    Incomplete(String),
    /// 끝 이벤트 없이 끊겼다
    Truncated,
}

/// data: 줄을 모아 빈 줄에서 이벤트 하나로 본다. 종류는 JSON 의 type 으로 가른다
#[derive(Default)]
struct Sse {
    data: String,
    text: String,
    end: Option<Outcome>,
}

impl Sse {
    fn line(&mut self, line: &str) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            self.dispatch();
        } else if let Some(d) = line.strip_prefix("data:") {
            if !self.data.is_empty() {
                self.data.push('\n');
            }
            self.data.push_str(d.strip_prefix(' ').unwrap_or(d));
        }
    }

    fn dispatch(&mut self) {
        let data = std::mem::take(&mut self.data);
        if self.end.is_some() || data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(v) = serde_json::from_str::<J>(&data) else { return };
        let failed = |e: &J| Outcome::Failed(from_code(e["code"].as_str().unwrap_or(""), e["message"].as_str().unwrap_or("")));
        match v["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => self.text.push_str(v["delta"].as_str().unwrap_or("")),
            "response.completed" => {
                let text = if self.text.is_empty() { output_text(&v["response"]) } else { std::mem::take(&mut self.text) };
                self.end = Some(Outcome::Done(text));
            }
            "response.failed" => self.end = Some(failed(&v["response"]["error"])),
            "response.incomplete" => self.end = Some(Outcome::Incomplete(v["response"]["incomplete_details"]["reason"].as_str().unwrap_or("").into())),
            "error" => self.end = Some(failed(if v["error"].is_object() { &v["error"] } else { &v })),
            _ => {}
        }
    }

    fn finish(mut self) -> Outcome {
        self.dispatch();
        self.end.unwrap_or(Outcome::Truncated)
    }
}

/// 조각(delta)을 못 받았을 때 completed 의 output 에서 글자를 모은다
fn output_text(response: &J) -> String {
    let mut out = String::new();
    for item in response["output"].as_array().into_iter().flatten() {
        for c in item["content"].as_array().into_iter().flatten() {
            if c["type"] == "output_text" {
                out.push_str(c["text"].as_str().unwrap_or(""));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- 성격별로 묶기 (프롬프트 · 검사)

const INSTRUCTIONS: &str = "암기 카드 한 장을 더 작은 카드로 나누는 일을 도와요. 사용자가 보낸 질문과 번호 붙은 답 항목을 보고, \
성격(종류)이 같은 항목끼리 2~8장의 카드로 묶으세요.
- 모든 항목 번호를 정확히 한 번씩, 어느 한 카드에 넣으세요. 항목 글자는 고치지 말고 번호만 쓰세요.
- 카드마다 front 에 그 묶음만 묻는 짧은 질문을 쓰세요. 원래 질문의 주제와 묶음의 성격이 드러나게, 원래 질문과 같은 언어로.
- 답은 {\"cards\": [{\"front\": 질문, \"items\": [번호, …]}]} 모양의 JSON 하나만.";

/// R3: 카드에 연결된 자료 구간을 함께 보낼 때 덧붙인다
const EVIDENCE: &str = "- 번호 붙은 자료 구간도 보내요. 묶음과 질문을 정할 때 참고하고, 카드마다 참고한 구간 번호를 \"segments\": [번호, …] 에 쓰세요 (없으면 빈 배열). \
구간 내용을 답에 더하지는 마세요.";

/// 답의 항목 (규칙 분할과 같은 기준: 항목 표시가 있으면 그것, 없으면 줄). 앞 표시(①, 1.)는 뗀다
fn regroup_items(answer: &[String]) -> Vec<String> {
    let items = quality::answer_items(answer);
    let items = if items.len() >= 2 { items } else { answer.to_vec() };
    items.iter().map(|i| quality::strip_marker(i)).filter(|i| !i.is_empty()).collect()
}

/// 보내는 글자 전부: 질문과 번호 붙은 답 항목
fn regroup_input(question: &str, items: &[String]) -> String {
    let mut s = format!("질문: {question}\n답 항목:\n");
    for (i, it) in items.iter().enumerate() {
        s.push_str(&format!("{}. {it}\n", i + 1));
    }
    s
}

/// 요청 본문. schema 가 있으면 text.format(json_schema, strict)으로 (받지 않으면 complete 가 형식 없이 다시 보낸다)
pub(crate) fn request_body(model: &str, instructions: &str, input: &str, schema: Option<(&str, &J)>) -> J {
    let mut b = json!({
        "model": model,
        "instructions": instructions,
        "input": [{ "role": "user", "content": input }],
        "store": false,
        "stream": true,
    });
    if let Some((name, schema)) = schema {
        b["text"] = json!({ "format": { "type": "json_schema", "name": name, "strict": true, "schema": schema } });
    }
    b
}

/// strict json_schema 의 객체 (모든 속성 필수, 다른 속성 없음)
pub(crate) fn object(props: J) -> J {
    let required: Vec<String> = props.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    json!({ "type": "object", "additionalProperties": false, "required": required, "properties": props })
}

fn regroup_schema(segments: bool) -> J {
    // 길이 제한(minItems 등)은 strict 스키마가 받지 않을 수 있어 parse_regroup 이 검사한다
    let ints = json!({ "type": "array", "items": { "type": "integer" } });
    let mut card = json!({ "front": { "type": "string" }, "items": ints });
    if segments {
        card["segments"] = ints;
    }
    object(json!({ "cards": { "type": "array", "items": object(card) } }))
}

/// 모델 출력의 JSON. 코드 블록으로 감싸도 읽는다
pub(crate) fn parse_json(text: &str) -> Result<J, AiError> {
    let t = text.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).map_or(t, |x| x.trim_end().trim_end_matches("```")).trim();
    serde_json::from_str(t).map_err(|_| AiError::BadOutput("JSON 이 아니에요".into()))
}

/// 모델 출력 → 카드. 2–8장, 질문이 비지 않고, 모든 항목이 정확히 한 번씩. 답은 원래 항목 글자 그대로다.
/// 하나라도 어긋나면 BadOutput (화면은 규칙 나누기를 보인다)
fn parse_regroup(text: &str, items: &[String]) -> Result<Vec<SplitPart>, AiError> {
    let bad = |m: &str| AiError::BadOutput(m.to_string());
    let v = parse_json(text)?;
    let cards = v["cards"].as_array().ok_or_else(|| bad("cards 가 없어요"))?;
    if !(MIN_CARDS..=MAX_CARDS).contains(&cards.len()) {
        return Err(bad("카드 수가 2–8장이 아니에요"));
    }
    let mut used = vec![false; items.len()];
    let mut parts = vec![];
    for c in cards {
        let front = c["front"].as_str().unwrap_or("").trim();
        if front.is_empty() || front.chars().count() > MAX_FRONT_CHARS {
            return Err(bad("질문이 비었거나 너무 길어요"));
        }
        let idx = c["items"].as_array().filter(|a| !a.is_empty()).ok_or_else(|| bad("답이 빈 카드가 있어요"))?;
        let mut back = vec![];
        for i in idx {
            let i = i.as_u64().filter(|&i| i >= 1 && i as usize <= items.len()).ok_or_else(|| bad("없는 항목 번호예요"))? as usize - 1;
            if std::mem::replace(&mut used[i], true) {
                return Err(bad("같은 항목이 두 번 들어갔어요"));
            }
            back.push(items[i].clone());
        }
        parts.push(SplitPart { front: front.to_string(), back: back.join("\n") });
    }
    if used.contains(&false) {
        return Err(bad("빠진 항목이 있어요"));
    }
    Ok(parts)
}

// ---------------------------------------------------------------- 개발용 가짜 응답

/// debug 빌드에서만: ADDIEM_AI_FAKE=ok|slow|usage_limit(limit)|expired|offline|unavailable|malformed.
/// 실제 계정 없이 L4 · R1 · R2 · R3 의 보내는 중 · 오류 · 결과를 그리려는 것. 네트워크 · 키체인을 쓰지 않는다
pub(crate) fn fake_mode() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("ADDIEM_AI_FAKE").ok().filter(|v| !v.is_empty())
}

/// 가짜 모드에서 [연결 끊기]를 누른 뒤 (앱을 다시 켤 때까지). 끊긴 화면(L1)과 "끊은 뒤에는 보내지 않음"을 가짜 모드로도 보게 (QA T5-06)
static FAKE_OFF: AtomicBool = AtomicBool::new(false);
fn fake_off() -> bool {
    fake_mode().is_some() && FAKE_OFF.load(Ordering::Relaxed)
}

/// 가짜 응답: 기다린 뒤 오류 모드면 그 오류, malformed 면 깨진 JSON, 아니면 ok() 의 글자.
/// 실제 응답과 같은 검증을 거친다. slow 는 30초 넘김 안내를 보려고 35초 기다린다
pub(crate) fn fake_text(st: &AiState, run: u64, mode: &str, ok: impl FnOnce() -> String) -> Result<String, AiError> {
    let wait_ms = if cfg!(test) { 0 } else if mode == "slow" { 35_000 } else { 1_500 };
    for _ in 0..wait_ms / 100 {
        if st.run.load(Ordering::Relaxed) != run {
            return Err(AiError::Cancelled);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    match mode {
        "limit" | "usage_limit" => Err(AiError::Limit),
        "expired" => Err(AiError::Expired),
        "offline" => Err(AiError::Offline("가짜 오프라인".into())),
        "unavailable" => Err(AiError::Unavailable),
        "malformed" => Ok("{\"cards\": [".into()),
        _ => Ok(ok()),
    }
}

/// 성격별로 묶기 가짜 결과: 항목을 앞에서부터 3묶음 (사람·집단 / 통제·위험 / 구조·성과). 구간이 있으면 첫 구간을 근거로
fn fake_regroup(items: usize, segments: usize) -> String {
    let names = ["사람·집단", "통제·위험", "구조·성과"];
    let cards: Vec<J> = (0..3)
        .map(|g| {
            let idx: Vec<usize> = (0..items).filter(|i| i * 3 / items == g).map(|i| i + 1).collect();
            json!({ "front": format!("{}에 관한 것은?", names[g]), "items": idx, "segments": if segments > 0 { vec![1] } else { vec![] } })
        })
        .collect();
    json!({ "cards": cards }).to_string()
}

/// 보내고 모델 출력 글자를 받는다 (모든 기능이 같이 쓴다). 연결 확인 · 가짜 응답 · 이번 달 보낸 수 세기까지.
/// body(모델, 형식 씀)가 요청 본문을 만든다. fake: 가짜 모드의 ok 출력
pub(crate) fn generate(app: &AppHandle, body: &dyn Fn(&str, bool) -> J, fake: impl FnOnce() -> String) -> Result<String, AiError> {
    let st = app.state::<AiState>();
    let run = st.run.load(Ordering::Relaxed);
    if let Some(mode) = fake_mode() {
        if fake_off() {
            return Err(AiError::NotConnected);
        }
        let text = fake_text(&st, run, &mode, fake)?;
        let _ = with_db(app, count_sent);
        return Ok(text);
    }
    if with_db(app, |c| Ok(get_meta(c, META_EMAIL)))?.is_none() {
        return Err(AiError::NotConnected);
    }
    complete(app, &st, run, body)
}

/// 모델 고르기 · 401 이면 토큰을 한 번 새로 받아 다시 · text.format 을 안 받으면 형식 없이 · 쓸 수 없는 모델이면 목록의 다음.
/// response.completed 의 글자를 돌려준다. 처음 200 을 받으면 이번 달 보낸 수를 센다
fn complete(app: &AppHandle, st: &AiState, run: u64, body: &dyn Fn(&str, bool) -> J) -> Result<String, AiError> {
    let mut token = access_token(app, st, false)?;
    let mut model = with_db(app, |c| Ok(get_meta(c, META_MODEL)))?;
    let mut tried: Vec<String> = vec![];
    let mut models: Option<Vec<String>> = None;
    let (mut format, mut reauthed, mut counted) = (true, false, false);
    loop {
        let m = match model.take() {
            Some(m) => m,
            None => {
                if models.is_none() {
                    models = Some(list_models(&st.agent, &token).map_err(|r| match r {
                        Reject::Fail(e) => e,
                        _ => AiError::Other("쓸 수 있는 모델 목록을 받지 못했어요.".into()),
                    })?);
                }
                let next = models.iter().flatten().find(|m| !tried.contains(*m)).cloned();
                let next = next.ok_or_else(|| AiError::Other("이 계정으로 쓸 수 있는 모델이 없어요.".into()))?;
                let _ = with_db(app, |c| set_meta(c, META_MODEL, Some(&next)));
                next
            }
        };
        tried.push(m.clone());
        let res = agent_stream(st, run, &token, &body(&m, format), &mut || {
            if !counted {
                counted = true;
                let _ = with_db(app, count_sent);
            }
        });
        match res {
            Ok(Outcome::Done(text)) => return Ok(text),
            Ok(Outcome::Failed(e)) => return Err(e),
            Ok(Outcome::Incomplete(reason)) => return Err(AiError::BadOutput(format!("응답이 끝나지 않았어요 ({reason})"))),
            Ok(Outcome::Truncated) if st.run.load(Ordering::Relaxed) != run => return Err(AiError::Cancelled),
            Ok(Outcome::Truncated) => return Err(AiError::Offline("응답이 중간에 끊겼어요".into())),
            Err(Reject::Unauthorized) if !reauthed => {
                reauthed = true;
                token = access_token(app, st, true)?;
                model = Some(m);
                tried.pop();
            }
            Err(Reject::Unauthorized) => return Err(AiError::Other("ChatGPT가 이 계정의 요청을 받지 않았어요 (401).".into())),
            Err(Reject::DropFormat) if format => {
                format = false;
                model = Some(m);
                tried.pop();
            }
            Err(Reject::NextModel) => {}
            Err(Reject::DropFormat) => return Err(AiError::Other("ChatGPT가 이 요청 형식을 받지 않았어요.".into())),
            Err(Reject::Fail(e)) => return Err(e),
        }
        if st.run.load(Ordering::Relaxed) != run {
            return Err(AiError::Cancelled);
        }
    }
}

// ---------------------------------------------------------------- 상태 · 명령

pub struct AiState {
    agent: ureq::Agent,
    /// 접근 토큰과 만료 시각 (ms). 잠금이 새로 고치기도 한 번에 하나로 묶는다 (토큰이 회전한다)
    token: Mutex<Option<(String, i64)>>,
    auth_cancel: Arc<AtomicBool>,
    /// 묶기 [취소] · 연결 끊기마다 커진다. 진행 중인 묶기는 바뀐 것을 보고 멈춘다
    run: AtomicU64,
}

impl AiState {
    pub fn new() -> Self {
        Self { agent: store::agent(), token: Mutex::new(None), auth_cancel: Arc::new(AtomicBool::new(false)), run: AtomicU64::new(0) }
    }
}

fn get_meta(con: &Connection, key: &str) -> Option<String> {
    con.query_row("SELECT value FROM schema_meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
}

fn set_meta(con: &Connection, key: &str, value: Option<&str>) -> Result<(), AiError> {
    match value {
        Some(v) => con.execute("INSERT OR REPLACE INTO schema_meta (key, value) VALUES (?1, ?2)", [key, v]),
        None => con.execute("DELETE FROM schema_meta WHERE key = ?1", [key]),
    }
    .map(|_| ())
    .map_err(|e| AiError::Other(e.to_string()))
}

fn with_db<T>(app: &AppHandle, f: impl FnOnce(&Connection) -> Result<T, AiError>) -> Result<T, AiError> {
    let st = app.state::<AppState>();
    let con = st.db.lock().map_err(|_| AiError::Other("데이터베이스를 잠그지 못했어요".into()))?;
    f(&con)
}

/// 이번 달 보낸 카드 수 ("2026-10 3")
fn sent_this_month(con: &Connection) -> i64 {
    let month = chrono::Local::now().format("%Y-%m").to_string();
    get_meta(con, META_SENT).and_then(|v| v.strip_prefix(&format!("{month} ")).and_then(|n| n.parse().ok())).unwrap_or(0)
}

fn count_sent(con: &Connection) -> Result<(), AiError> {
    let month = chrono::Local::now().format("%Y-%m");
    set_meta(con, META_SENT, Some(&format!("{month} {}", sent_this_month(con) + 1)))?;
    set_meta(con, META_SENT_AT, Some(&crate::db::now_ms().to_string()))
}

/// 다시 로그인해야 하면 토큰과 "연결됨"을 지운다 (client_id · host id 는 남긴다)
fn forget(app: &AppHandle, st: &AiState) {
    auth::delete_secret(REFRESH);
    if let Ok(mut t) = st.token.lock() {
        *t = None;
    }
    let _ = with_db(app, |c| set_meta(c, META_EMAIL, None));
}

fn access_token(app: &AppHandle, st: &AiState, force: bool) -> Result<String, AiError> {
    let mut cached = st.token.lock().map_err(|_| AiError::Other("토큰을 잠그지 못했어요".into()))?;
    if let Some((t, exp)) = cached.as_ref()
        && !force
        && *exp > crate::db::now_ms()
    {
        return Ok(t.clone());
    }
    let client = with_db(app, |c| Ok(get_meta(c, META_CLIENT)))?;
    let (Some(client), Some(refresh)) = (client, auth::load_secret(REFRESH)) else {
        // 키체인에 토큰이 없다 (다른 기기에서 복원한 데이터 등): "연결됨" 표시도 지운다
        drop(cached);
        forget(app, st);
        return Err(AiError::NotConnected);
    };
    let r = store::send(&st.agent, refresh_req(&client, &refresh)).map_err(net)?;
    if r.status != 200 {
        let e = refresh_error(r.status, &r.body);
        if e == AiError::Expired {
            drop(cached);
            forget(app, st);
        }
        return Err(e);
    }
    let t = parse_tokens(&r.body)?;
    if let Some(new) = &t.refresh {
        auth::save_secret(REFRESH, new).map_err(AiError::Other)?;
    }
    *cached = Some((t.access.clone(), t.expires));
    Ok(t.access)
}

#[derive(Serialize)]
pub struct AiStatus {
    connected: bool,
    email: Option<String>,
    model: Option<String>,
    /// 이번 달 보낸 카드와 마지막으로 보낸 시각 (이 기기에서 센 수. 사용량 추정은 하지 않는다)
    sent_month: i64,
    sent_at: Option<i64>,
}

#[tauri::command(async)]
pub fn ai_status(app: AppHandle) -> Res<AiStatus> {
    with_db(&app, |c| {
        let email = get_meta(c, META_EMAIL);
        let sent_at = get_meta(c, META_SENT_AT).and_then(|v| v.parse().ok());
        let email = if fake_off() { None } else { email.or_else(|| fake_mode().map(|m| format!("개발용 가짜 연결 ({m})"))) };
        Ok(AiStatus { connected: email.is_some(), email, model: get_meta(c, META_MODEL), sent_month: sent_this_month(c), sent_at })
    })
    .map_err(|e| e.to_string())
}

/// 로그인 결과 ("ai-auth" 이벤트)
#[derive(Serialize, Clone)]
struct AuthEvent {
    ok: bool,
    /// timeout | cancelled | denied | scope | not_eligible | offline | error
    reason: Option<&'static str>,
    message: Option<String>,
    email: Option<String>,
    /// 이 설치에서 처음 연결했다 (L3 환영을 한 번만)
    first: bool,
}

/// L2 [Continue with ChatGPT]. 루프백을 먼저 열고 브라우저를 연다. 로그인 주소를 돌려준다 ([링크 복사]).
/// reconsent: 사용량 권한을 다시 묻는다. new_account: 저장한 등록 대신 새로 등록한다 (L2-x [다른 계정으로])
#[tauri::command]
pub fn ai_connect(app: AppHandle, state: State<AiState>, reconsent: bool, new_account: bool) -> Res<String> {
    // 가짜 응답으로 화면을 그리는 동안 [다시 로그인] 등을 눌러도 실제 로그인 창을 열지 않는다
    if fake_mode().is_some() {
        return Err("개발용 가짜 응답 모드(ADDIEM_AI_FAKE)에서는 ChatGPT에 로그인하지 않아요.".into());
    }
    let (host, client) = with_db(&app, |c| {
        let host = match get_meta(c, META_HOST) {
            Some(h) => h,
            None => {
                let h = new_host_id();
                set_meta(c, META_HOST, Some(&h))?;
                h
            }
        };
        Ok((host, reusable_client(get_meta(c, META_CLIENT), get_meta(c, META_CLIENT_NAME).as_deref(), new_account)))
    })
    .map_err(|e| e.to_string())?;
    let listener = std::net::TcpListener::bind(("127.0.0.1", PORT)).or_else(|_| auth::loopback()).map_err(|e| e.to_string())?;
    let redirect = format!("http://127.0.0.1:{}{CALLBACK}", listener.local_addr().map_err(|e| e.to_string())?.port());
    let p = auth::pkce();
    let nonce = crypto::random_hex(16);
    let url = authorize_url(client.as_deref(), &host, &redirect, &p, &nonce, reconsent);
    state.auth_cancel.store(false, Ordering::Relaxed);
    crate::sync::open_url(&url);
    std::thread::spawn(move || {
        let ev = match connect_blocking(&app, listener, client, &redirect, &p, &nonce) {
            Ok((email, first)) => AuthEvent { ok: true, reason: None, message: None, email: Some(email), first },
            Err((reason, message)) => AuthEvent { ok: false, reason: Some(reason), message: Some(message), email: None, first: false },
        };
        let _ = app.emit("ai-auth", ev);
    });
    Ok(url)
}

/// 다음 로그인에 저장한 등록(client_id)을 다시 쓸지. 다른 이름으로 한 등록(Neo-Anki 시절)이면 새로 등록해
/// ChatGPT 에 Addiem 으로 보이게 한다. 지금 연결은 그대로고, 다시 연결할 때만 바뀐다 (QA QB-09)
fn reusable_client(client: Option<String>, name: Option<&str>, new_account: bool) -> Option<String> {
    client.filter(|_| !new_account && name == Some(AGENT_NAME))
}

fn connect_blocking(
    app: &AppHandle,
    listener: std::net::TcpListener,
    pending: Option<String>,
    redirect: &str,
    p: &auth::Pkce,
    nonce: &str,
) -> Result<(String, bool), (&'static str, String)> {
    let st = app.state::<AiState>();
    let q = auth::wait_callback(listener, CALLBACK, &p.state, "ChatGPT 연결을 마쳤어요", &st.auth_cancel).map_err(|e| match e {
        auth::WaitError::Timeout => ("timeout", "5분 안에 로그인을 마치지 않아 기다리기를 멈췄어요.".to_string()),
        auth::WaitError::Cancelled => ("cancelled", String::new()),
        auth::WaitError::Denied => ("denied", "로그인을 취소했어요.".to_string()),
        auth::WaitError::Other(m) => ("error", m),
    })?;
    let client = issued_client(pending.as_deref(), q.get("client_id").map(String::as_str)).map_err(|m| ("error", m))?;
    let r = store::send(&st.agent, token_req(&client, q.get("code").map_or("", String::as_str), &p.verifier, redirect)).map_err(|e| fail(net(e)))?;
    if r.status != 200 {
        // invalid_grant: 코드를 버리고 처음부터 (다시 시도)
        return Err(("error", format!("ChatGPT가 로그인을 받아 주지 않았어요 ({}). 다시 시도해 주세요.", r.status)));
    }
    let t = parse_tokens(&r.body).map_err(fail)?;
    let res = save_connection(app, &st, &client, &t, nonce);
    // 저장하지 못했으면 방금 받은 토큰을 폐기한다 (최선 노력)
    if res.is_err()
        && let Some(rt) = &t.refresh
    {
        let _ = store::send(&st.agent, revoke_req(&client, rt));
    }
    res
}

fn fail(e: AiError) -> (&'static str, String) {
    let reason = match e {
        AiError::Scope => "scope",
        AiError::NotEligible => "not_eligible",
        AiError::Offline(_) => "offline",
        _ => "error",
    };
    (reason, e.to_string())
}

/// ID 토큰 · 권한 · 요금제를 확인하고 저장한다. 실패하면 부른 쪽이 새 토큰을 폐기한다
fn save_connection(app: &AppHandle, st: &AiState, client: &str, t: &Tokens, nonce: &str) -> Result<(String, bool), (&'static str, String)> {
    let id = t.id_token.as_deref().ok_or_else(|| ("error", "ID 토큰을 받지 못했어요. 다시 시도해 주세요.".to_string()))?;
    let who = verify_id_token(&st.agent, id, client, nonce).map_err(fail)?;
    // 이 기기에 있던 연결 (아래에서 새 토큰으로 바꾼다)
    let (old_client, old_sub) = with_db(app, |c| Ok((get_meta(c, META_CLIENT), get_meta(c, META_SUB)))).map_err(fail)?;
    let old_refresh = auth::load_secret(REFRESH);
    // 등록은 검증한 ID 토큰의 aud 와 같을 때만 저장한다. 사용량 권한이 빠져도 저장: [다시 연결]은 같은 client_id 로 (errors-and-recovery).
    // 다음 로그인은 이 client_id 로 (dynamic_agent_client 는 다시 쓰지 않는다)
    with_db(app, |c| {
        set_meta(c, META_CLIENT, Some(client))?;
        set_meta(c, META_CLIENT_NAME, Some(AGENT_NAME))
    })
    .map_err(fail)?;
    // 저장하기 전에 권한부터. 사용량 권한이 없으면 요금제로 쓸 수 없다
    if !has_use_scope(&t.scope) {
        return Err(fail(AiError::Scope));
    }
    let refresh = t.refresh.as_deref().ok_or_else(|| ("error", "로그인을 유지할 토큰을 받지 못했어요. 다시 시도해 주세요.".to_string()))?;
    // 모델 목록: 쓸 수 없는 요금제면 여기서 알 수 있다 (연결하지 않는다). 네트워크 오류면 처음 묶을 때 다시 받는다
    let model = match list_models(&st.agent, &t.access) {
        Err(Reject::Fail(AiError::NotEligible)) => return Err(fail(AiError::NotEligible)),
        Ok(m) => m.into_iter().next(),
        Err(_) => None,
    };
    auth::save_secret(REFRESH, refresh).map_err(|m| ("error", m))?;
    // 덮어쓴 이전 토큰: 등록이나 계정이 다르면 폐기한다 ([다른 계정으로]).
    // 같은 등록 · 같은 계정(권한 다시 허락)이면 두는데, 폐기가 그 권한 부여 전체에 걸리면 새 토큰도 끊기기 때문이다
    if let (Some(rt), Some(oc)) = (old_refresh, old_client)
        && rt != refresh
        && (oc != client || old_sub.as_deref() != Some(who.sub.as_str()))
    {
        let _ = store::send(&st.agent, revoke_req(&oc, &rt));
    }
    *st.token.lock().map_err(|_| ("error", "토큰을 잠그지 못했어요".to_string()))? = Some((t.access.clone(), t.expires));
    with_db(app, |c| {
        set_meta(c, META_SUB, Some(&who.sub))?;
        set_meta(c, META_EMAIL, Some(&who.email))?;
        set_meta(c, META_MODEL, model.as_deref())?;
        let first = get_meta(c, META_WELCOMED).is_none();
        set_meta(c, META_WELCOMED, Some("1"))?;
        Ok(first)
    })
    .map(|first| (who.email.clone(), first))
    .map_err(fail)
}

/// L2-w [취소] · Esc: 루프백 서버를 닫는다
#[tauri::command]
pub fn ai_connect_cancel(state: State<AiState>) {
    state.auth_cancel.store(true, Ordering::Relaxed);
}

/// [연결 끊기]: 진행 중인 묶기를 멈추고, 새로 고칠 토큰을 폐기한 뒤 키체인에서 지운다.
/// 등록(client_id)과 host id 는 남긴다 (다시 연결할 때 쓴다). 폐기를 확인하지 못했으면 false (ChatGPT 설정에서 끊을 수 있다)
#[tauri::command]
pub async fn ai_disconnect(app: AppHandle, state: State<'_, AiState>) -> Res<bool> {
    state.run.fetch_add(1, Ordering::Relaxed);
    // 가짜 모드는 키체인 · 네트워크를 쓰지 않는다. 끊긴 상태로만 바꾼다
    if fake_mode().is_some() {
        FAKE_OFF.store(true, Ordering::Relaxed);
        return Ok(true);
    }
    state.auth_cancel.store(true, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AiState>();
        let client = with_db(&app, |c| Ok(get_meta(c, META_CLIENT))).ok().flatten();
        let revoked = match (client, auth::load_secret(REFRESH)) {
            (Some(c), Some(rt)) => store::send(&st.agent, revoke_req(&c, &rt)).is_ok_and(|r| r.status == 200),
            _ => true,
        };
        forget(&app, &st);
        let _ = with_db(&app, |c| set_meta(c, META_MODEL, None));
        revoked
    })
    .await
    .map_err(|e| e.to_string())
}

/// [사용량 관리]: ChatGPT 설정 > 사용량 (시스템 브라우저)
#[tauri::command]
pub fn ai_open_usage() {
    crate::sync::open_url(USAGE_URL);
}

/// [취소] · 결과 전에 평가 (L4 · R1 · R2 · R3). 진행 중인 요청을 멈춘다
#[tauri::command]
pub fn ai_cancel(state: State<AiState>) {
    state.run.fetch_add(1, Ordering::Relaxed);
}

/// G2 [성격별로 묶기]: 카드 한 장을 ChatGPT 로 보내 같은 모양의 제안을 받는다. 아무것도 바꾸지 않는다
#[tauri::command]
/// segment_ids: 첫 확인 목록에서 남긴 구간 (R3). 이 카드에 연결된 구간만 받는다. 비면 카드 한 장만 (L4 그대로)
pub async fn ai_regroup(app: AppHandle, card_id: i64, segment_ids: Vec<i64>) -> Result<SplitProposal, AiError> {
    tauri::async_runtime::spawn_blocking(move || regroup_blocking(&app, card_id, &segment_ids)).await.map_err(|e| AiError::Other(e.to_string()))?
}

fn regroup_blocking(app: &AppHandle, card_id: i64, segment_ids: &[i64]) -> Result<SplitProposal, AiError> {
    let (rule, sent) = with_db(app, |c| {
        let rule = quality::suggest(c, card_id, None).map_err(AiError::Other)?;
        let picks: Vec<(i64, &str)> = rag::linked_segments(c, card_id).map_err(AiError::Other)?.into_iter().filter(|l| segment_ids.contains(&l.0)).collect();
        Ok((rule, rag::sent_of(c, &picks).map_err(AiError::Other)?))
    })?;
    let items = regroup_items(&rule.answer);
    if items.len() < MIN_ITEMS {
        return Err(AiError::Other("답 항목이 3개 이상일 때 나눌 수 있어요.".into()));
    }
    let mut input = regroup_input(&rule.question, &items);
    if items.len() > MAX_ITEMS || input.chars().count() > MAX_INPUT_CHARS {
        return Err(AiError::Other("카드가 너무 길어 보내지 않았어요. 규칙으로 나눠 주세요.".into()));
    }
    let instructions = if sent.is_empty() {
        INSTRUCTIONS.to_string()
    } else {
        input.push_str(&format!("자료 구간:\n{}", rag::segments_input(&sent)));
        format!("{INSTRUCTIONS}\n{EVIDENCE}")
    };
    let schema = regroup_schema(!sent.is_empty());
    let text = generate(app, &|m, f| request_body(m, &instructions, &input, f.then_some(("regroup", &schema))), || fake_regroup(items.len(), sent.len()))?;
    let parts = parse_regroup(&text, &items)?;
    let evidence = parse_cited(&text, &sent)?;
    // L4 "왜?": 걸린 규칙 + ChatGPT 가 묶은 장수 (R3: 함께 보낸 구간 수)
    let seen = if sent.is_empty() { String::new() } else { format!("연결된 자료 {}구간을 보고 ", sent.len()) };
    let why = format!("{}. ChatGPT가 {seen}비슷한 내용끼리 {}장으로 나눴어요.", quality::reason_of(&rule.flags), parts.len());
    Ok(SplitProposal { why, parts, kind: "nature", evidence, sent, ..rule })
}

/// R3 근거 칩: 카드들이 실제로 인용한 구간 (처음 나온 순서, 겹치지 않게). 보내지 않은 번호면 BadOutput (일부도 쓰지 않는다)
fn parse_cited(text: &str, sent: &[rag::Sent]) -> Result<Vec<rag::Sent>, AiError> {
    let v = parse_json(text)?;
    let mut out: Vec<rag::Sent> = vec![];
    for c in v["cards"].as_array().into_iter().flatten() {
        for n in c["segments"].as_array().into_iter().flatten() {
            let id = rag::segment_at(sent, n).ok_or_else(|| AiError::BadOutput("보내지 않은 구간 번호예요".into()))?;
            if !out.iter().any(|s| s.id == id) {
                out.extend(sent.iter().find(|s| s.id == id).cloned());
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- 8.2 AI로 확인 (질문과 답이 맞는지 · 고친 안 제안)

/// 보내는 것: 그 카드의 질문 글자와 답 글자 (quality::rewrite_of 의 필드 글자. HTML · 그림 · [sound:] 를 뺐다) +
/// 이 카드에 연결된 자료 구간 중 확인 목록에서 남긴 것 (R3 와 같은 rag::linked_segments, 비슷한 구간은 찾지 않는다). 없으면 카드 글자만 (I74 범위).
/// 받는 것: 판단 · 근거 한 줄 · 고친 안(근거가 부족하면 없음) · 고친 안이 기댄 구간 번호. 고친 안은 제안일 뿐이고 카드를 바꾸는 것은
/// 사용자가 [이대로 고치기]로 승인한 quality::rewrite_apply 뿐이다. 모델이 글을 쓰는 AI 고쳐 쓰기 길은 이 명령 하나다 (CQ, 사용자 결정 2026-10-19)
const CHECK_INSTRUCTIONS: &str = "암기 카드 한 장의 질문과 답이 서로 맞는지 확인해요. 사용자가 보낸 질문과 답을 보고:
- matches: 답이 질문이 묻는 것(개수 · 때 · 차이 · 이유 · 순서 · 대상)에 맞게 답하면 true, 어긋나면 false.
- reason: 그렇게 본 근거 한 문장. 질문과 같은 언어로, 한국어면 해요체로 (\"…예요\", \"…없어요\"). 같은 말을 되풀이하지 마세요. fix 가 null 이면 화면이 \"근거가 부족해요\"를 이미 보여 주니 무엇이 없는지만 쓰세요 (예: \"자료에 절편 설명이 없어요\").
- fix: 어긋나면 서로 맞게 고친 {\"question\": 질문, \"answer\": 답}. 원래 글자를 되도록 그대로 두고 꼭 필요한 곳만 고치세요. 줄은 줄바꿈으로 나누고 HTML 이나 서식 기호는 쓰지 마세요.
- 카드와 자료 구간에 없는 새 사실을 넣지 마세요. 고칠 근거가 부족하면 fix 를 null 로 두고 reason 에 무엇이 부족한지 쓰세요. 맞으면 fix 는 null.
- checks: 어긋나는데 fix 가 null 이면 사용자가 직접 확인할 것을 해요체 물음으로 2~3개. 첫 물음은 질문을 되풀이하지 말고 어긋나 보이는 이유를 짚으세요 (예: \"질문은 '몇'(개수)을 묻는데 답에 수가 없어요. 답에 몇 계층인지 넣어야 할까요?\"). 아니면 빈 배열.
- segments: 고친 안이 기댄 자료 구간 번호. 없으면 빈 배열.
- 답은 {\"matches\": true 또는 false, \"reason\": 근거, \"fix\": {\"question\": 질문, \"answer\": 답} 또는 null, \"checks\": [물음, …], \"segments\": [번호, …]} 모양의 JSON 하나만.";

const MAX_REASON_CHARS: usize = 300;
const MAX_ANSWER_CHARS: usize = 2000;
/// 근거가 부족할 때 "확인해 주세요" 물음 (CQ3-n)
const MAX_CHECKS: usize = 3;

/// 고친 안 (줄바꿈으로 줄을 나눈다)
#[derive(Serialize, Debug, PartialEq)]
pub struct CardFix {
    question: String,
    answer: String,
}

/// [AI로 확인] 결과. fix 가 없고 matches 가 false 면 "수정안을 만들지 않았어요 · 근거가 부족해요" + checks (CQ3-n)
#[derive(Serialize, Debug)]
pub struct CardCheck {
    matches: bool,
    reason: String,
    fix: Option<CardFix>,
    /// 고친 안이 그림 · 소리 · 서식이 있는 칸(locked)을 바꿔서 그 칸은 버렸다 ("서식이 있는 칸은 바꾸지 않았어요")
    kept_locked: bool,
    /// 사용자가 직접 확인할 물음 (고친 안이 없을 때만)
    checks: Vec<String>,
    /// 함께 보낸 자료 구간 · 고친 안이 기댄 구간 (근거 칩)
    sent: Vec<rag::Sent>,
    evidence: Vec<rag::Sent>,
}

/// 보내는 글자 전부
fn check_input(question: &str, answer: &str, sent: &[rag::Sent]) -> String {
    let mut s = format!("질문: {question}\n답:\n{answer}\n");
    if !sent.is_empty() {
        s.push_str(&format!("자료 구간:\n{}", rag::segments_input(sent)));
    }
    s
}

fn check_schema() -> J {
    let fix = object(json!({ "question": { "type": "string" }, "answer": { "type": "string" } }));
    object(json!({
        "matches": { "type": "boolean" },
        "reason": { "type": "string" },
        "fix": { "anyOf": [fix, { "type": "null" }] },
        "checks": { "type": "array", "items": { "type": "string" } },
        "segments": { "type": "array", "items": { "type": "integer" } },
    }))
}

/// 카드 글자로 받지 않는 표시: HTML 태그 · 빈칸 문법({{c1::…}}) · 굵게 표시(**)
fn markup(t: &str) -> bool {
    t.contains("{{") || t.contains("**") || t.char_indices().any(|(i, c)| c == '<' && t[i + 1..].starts_with(|n: char| n.is_ascii_alphabetic() || n == '/' || n == '!'))
}

/// 모델 출력 → 확인 결과. 근거가 비거나 길면, 고친 안이 비거나 길거나 표시(markup)가 있으면, 보내지 않은 구간 번호면 BadOutput (일부도 쓰지 않는다).
/// 잠긴 칸(locked: 그림 · 소리 · 서식, 글자로 덮을 수 없다)을 바꾼 것은 버리고 보낸 글 그대로 둔다 (QA M2: 승인해도 거부되는 막다른 길).
/// 맞다고 하거나 보낸 글과 같으면 고친 안은 없다
fn parse_check(text: &str, question: &str, answer: &str, sent: &[rag::Sent], locked: [bool; 2]) -> Result<CardCheck, AiError> {
    let bad = |m: &str| AiError::BadOutput(m.to_string());
    let v = parse_json(text)?;
    let matches = v["matches"].as_bool().ok_or_else(|| bad("matches 가 없어요"))?;
    let reason = v["reason"].as_str().unwrap_or("").trim().to_string();
    if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
        return Err(bad("근거가 비었거나 너무 길어요"));
    }
    let fixed = |key: &str, max: usize| {
        let t = quality::tidy(v["fix"][key].as_str().unwrap_or(""));
        if t.is_empty() || t.chars().count() > max {
            return Err(bad("고친 질문이나 답이 비었거나 너무 길어요"));
        }
        if markup(&t) {
            return Err(bad("고친 글에 HTML 이나 서식 기호가 있어요"));
        }
        Ok(t)
    };
    // 맞다고 하면 판단 · 근거만 받는다: 함께 온 fix · checks · segments 는 보지도 검사하지도 않는다 (QA: 맞는 카드가 드물게 형식 오류로 떨어졌다)
    if matches {
        return Ok(CardCheck { matches, reason, fix: None, kept_locked: false, checks: vec![], sent: sent.to_vec(), evidence: vec![] });
    }
    let fix = if v["fix"].is_null() {
        None
    } else {
        Some(CardFix { question: fixed("question", MAX_FRONT_CHARS)?, answer: fixed("answer", MAX_ANSWER_CHARS)? })
    };
    let (q0, a0) = (quality::tidy(question), quality::tidy(answer));
    let mut kept_locked = false;
    let fix = fix.map(|mut f| {
        for (on, new, old) in [(locked[0], &mut f.question, &q0), (locked[1], &mut f.answer, &a0)] {
            if on && new != old {
                *new = old.clone();
                kept_locked = true;
            }
        }
        f
    });
    let fix = fix.filter(|f| f.question != q0 || f.answer != a0);
    // 확인할 물음은 고친 안이 없을 때만. 비었거나 길거나 표시가 있는 물음은 그 물음만 버린다 (덧붙이는 것이라 판단을 버리지 않는다)
    let checks: Vec<String> = if fix.is_some() {
        vec![]
    } else {
        let ok = |c: &&str| !c.is_empty() && c.chars().count() <= MAX_FRONT_CHARS && !markup(c);
        v["checks"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::trim).filter(ok)).take(MAX_CHECKS).map(String::from).collect()
    };
    // 근거 구간은 고친 안이 있을 때만 본다. 보내지 않은 번호를 인용하면 고친 안을 쓰지 않는다 (근거 칩이 틀리면 안 된다)
    let mut evidence: Vec<rag::Sent> = vec![];
    for n in v["segments"].as_array().into_iter().flatten().filter(|_| fix.is_some()) {
        let id = rag::segment_at(sent, n).ok_or_else(|| bad("보내지 않은 구간 번호예요"))?;
        if !evidence.iter().any(|s| s.id == id) {
            evidence.extend(sent.iter().find(|s| s.id == id).cloned());
        }
    }
    Ok(CardCheck { matches, reason, fix, kept_locked, checks, sent: sent.to_vec(), evidence })
}

/// 개발용 가짜 결과 (CQ3 의 세 상태): 규칙에 걸린 카드에 연결 구간이 있으면 첫 구간을 근거로 답 끝에 한 줄을 더한 고친 안,
/// 구간이 없으면 근거가 부족해 고친 안 없음 + 확인할 물음 (앞뒤가 바뀐 카드만은 카드 글자만으로 앞뒤를 바꾼 고친 안), 규칙에 걸리지 않은 카드는 맞다고
fn fake_check(question: &str, answer: &str, rule: Option<&str>, segments: usize) -> String {
    match rule {
        Some(why) if segments > 0 => json!({
            "matches": false,
            "reason": format!("{why} (개발용 가짜 응답)"),
            "fix": { "question": question, "answer": format!("{answer}\n가짜 응답이 자료 구간 1을 보고 더한 줄") },
            "checks": [],
            "segments": [1],
        }),
        Some(why) if why.contains("바뀌었을") => json!({
            "matches": false,
            "reason": format!("{why} (개발용 가짜 응답)"),
            "fix": { "question": answer, "answer": question },
            "checks": [],
            "segments": [],
        }),
        Some(why) => json!({
            "matches": false,
            "reason": format!("{why}. 연결된 자료가 없어요 (개발용 가짜 응답)"),
            "fix": null,
            "checks": [format!("{why}. 답에 질문이 묻는 내용을 넣어야 할까요?"), "답이 다른 카드의 답과 섞였는지 봐 주세요."],
            "segments": [],
        }),
        None => json!({ "matches": true, "reason": "질문이 묻는 것에 맞게 답했어요 (개발용 가짜 응답)", "fix": null, "checks": [], "segments": [] }),
    }
    .to_string()
}

/// 잠긴 칸은 보낸 그대로 두라는 지시 (그래도 바꾸면 parse_check 가 버린다). 자료 구간을 보내지 않았으면 근거에 "자료"를 말하지 않게 한다
/// (QA R-06: 자료 없이 보낸 카드의 근거가 "자료에 … 설명이 없어요"라 자료를 본 것처럼 읽혔다)
fn check_instructions(locked: [bool; 2], sources: bool) -> String {
    let mut s = CHECK_INSTRUCTIONS.to_string();
    if !sources {
        s.push_str("\n- 이번에는 자료 구간을 보내지 않았어요. 카드 글자만 보고 판단하고, reason 과 checks 에 '자료'를 말하지 마세요 (예: \"답에 절편 설명이 없어요\").");
    }
    let side = match locked {
        [true, true] => "질문과 답",
        [true, false] => "질문",
        [false, true] => "답",
        _ => return s,
    };
    format!("{s}\n- {side}에는 그림 · 소리 · 서식이 있어 여기서 바꿀 수 없어요. fix 의 그 칸은 보낸 글 그대로 두세요.")
}

/// 원래 질문과 답 · 함께 보낸 구간 · 보내는 글 · 지시 (DB 를 읽기만 한다)
fn check_prepare(con: &Connection, card_id: i64, segment_ids: &[i64]) -> Result<(quality::Rewrite, Vec<rag::Sent>, String, String), AiError> {
    let r = quality::rewrite_of(con, card_id).map_err(AiError::Other)?;
    let picks: Vec<(i64, &str)> = rag::linked_segments(con, card_id).map_err(AiError::Other)?.into_iter().filter(|l| segment_ids.contains(&l.0)).collect();
    let sent = rag::sent_of(con, &picks).map_err(AiError::Other)?;
    let input = check_input(&r.question, &r.answer, &sent);
    if input.chars().count() > MAX_INPUT_CHARS {
        return Err(AiError::Other("카드가 너무 길어 보내지 않았어요. 직접 고쳐 주세요.".into()));
    }
    let instructions = check_instructions(r.locked, !sent.is_empty());
    Ok((r, sent, input, instructions))
}

/// 8.2 [AI로 확인]: 카드 한 장(+ 남긴 연결 구간)을 ChatGPT 로 보내 질문과 답이 맞는지 보고, 어긋나면 근거와 고친 안을 받는다.
/// 아무것도 바꾸지 않는다 (승인은 quality::rewrite_apply, how = "ai")
#[tauri::command]
pub async fn ai_check_card(app: AppHandle, card_id: i64, segment_ids: Vec<i64>) -> Result<CardCheck, AiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let (r, sent, input, instructions) = with_db(&app, |c| check_prepare(c, card_id, &segment_ids))?;
        let schema = check_schema();
        let rule = Some(r.why.as_str()).filter(|w| !w.is_empty());
        let text = generate(&app, &|m, f| request_body(m, &instructions, &input, f.then_some(("card_check", &schema))), || {
            fake_check(&r.question, &r.answer, rule, sent.len())
        });
        let res = text.and_then(|t| parse_check(&t, &r.question, &r.answer, &sent, r.locked));
        if let Err(e) = &res {
            log_failure(&app, "ai_check_card", e);
        }
        res
    })
    .await
    .map_err(|e| AiError::Other(e.to_string()))?
}

/// 실패 까닭을 이 기기의 `ai-errors.log`(데이터 폴더)와 stderr 에 한 줄로 남긴다. 카드 글 · 응답 글은 남기지 않는다 (까닭 글만).
/// 취소 · 연결 안 됨은 남기지 않는다. ponytail: 크기 제한 없이 덧붙인다 (드문 일), 커지면 돌려쓰기
fn log_failure(app: &AppHandle, what: &str, e: &AiError) {
    if matches!(e, AiError::Cancelled | AiError::NotConnected) {
        return;
    }
    let line = format!("{} {what} {}: {e:?}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), e.kind());
    eprint!("{line}");
    let path = app.state::<AppState>().data_dir.join("ai-errors.log");
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// 보내고 스트림을 끝까지 읽는다. 200 이면 sent() (이번 달 보낸 카드). 취소되면 읽기를 멈춘다 (Truncated)
/// ponytail: 첫 바이트가 오기 전의 취소는 응답이 올 때까지(최대 2분) 스레드가 기다린다. 화면은 바로 놓아 준다
fn agent_stream(st: &AiState, run: u64, token: &str, body: &J, sent: &mut dyn FnMut()) -> Result<Outcome, Reject> {
    let mut resp = st.agent.run(responses_req(token, body)).map_err(|e| Reject::Fail(AiError::Offline(e.to_string())))?;
    let status = resp.status().as_u16();
    if status != 200 {
        let body = resp.body_mut().with_config().limit(1024 * 1024).read_to_vec().unwrap_or_default();
        return Err(classify(status, &body));
    }
    sent();
    let mut sse = Sse::default();
    for line in std::io::BufReader::new(resp.body_mut().as_reader()).lines() {
        if st.run.load(Ordering::Relaxed) != run {
            return Ok(Outcome::Truncated);
        }
        match line {
            Ok(l) => sse.line(&l),
            Err(_) => break,
        }
    }
    Ok(sse.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn fake_ok_groups_every_item_once() {
        let st = AiState::new();
        for n in [3, 4, 5, 6, 7, 11] {
            let items: Vec<String> = (1..=n).map(|i| format!("항목 {i}")).collect();
            let text = fake_text(&st, 0, "ok", || fake_regroup(n, 0)).unwrap();
            let parts = parse_regroup(&text, &items).unwrap();
            assert_eq!(parts.iter().map(|p| p.front.as_str()).collect::<Vec<_>>(), ["사람·집단에 관한 것은?", "통제·위험에 관한 것은?", "구조·성과에 관한 것은?"]);
            assert_eq!(parts.iter().map(|p| p.back.lines().count()).sum::<usize>(), n);
        }
        let text = fake_text(&st, 0, "malformed", || unreachable!()).unwrap();
        assert!(matches!(parse_regroup(&text, &["a".into(), "b".into(), "c".into()]), Err(AiError::BadOutput(_))));
        assert_eq!(fake_text(&st, 0, "usage_limit", String::new), Err(AiError::Limit));
        assert_eq!(fake_text(&st, 0, "limit", String::new), Err(AiError::Limit));
    }

    fn query(u: &str) -> HashMap<String, String> {
        url::Url::parse(u).unwrap().query_pairs().into_owned().collect()
    }

    fn form_of(r: &store::Req) -> HashMap<String, String> {
        url::form_urlencoded::parse(r.body()).into_owned().collect()
    }

    fn pkce() -> auth::Pkce {
        auth::Pkce { verifier: "ver".into(), challenge: "chal".into(), state: "st".into() }
    }

    #[test]
    fn old_registration_is_replaced_on_next_connect() {
        let c = || Some("oaiapp_1".to_string());
        assert_eq!(reusable_client(c(), Some(AGENT_NAME), false).as_deref(), Some("oaiapp_1"), "Addiem 으로 한 등록은 다시 쓴다");
        assert_eq!(reusable_client(c(), None, false), None, "이름 기록이 없으면 Neo-Anki 시절 등록: 새로 등록");
        assert_eq!(reusable_client(c(), Some("Neo-Anki"), false), None);
        assert_eq!(reusable_client(c(), Some(AGENT_NAME), true), None, "[다른 계정으로]");
        assert_eq!(reusable_client(None, Some(AGENT_NAME), false), None);
    }

    #[test]
    fn authorize_url_first_and_returning() {
        let redirect = "http://127.0.0.1:1455/auth/callback";
        let u = authorize_url(None, "urn:uuid:h", redirect, &pkce(), "n0", false);
        assert!(u.starts_with("https://auth.openai.com/api/accounts/authorize?"));
        let q = query(&u);
        assert_eq!(q["client_id"], "dynamic_agent_client", "처음에는 동적 등록");
        assert_eq!(q["agent_name_hint"], "Addiem");
        assert_eq!(q["ext_agent_host_id"], "urn:uuid:h");
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["redirect_uri"], redirect);
        assert_eq!(q["scope"], "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct");
        assert_eq!(q["resource"], "https://api.openai.com/v1");
        assert_eq!((q["state"].as_str(), q["nonce"].as_str()), ("st", "n0"));
        assert_eq!((q["code_challenge"].as_str(), q["code_challenge_method"].as_str()), ("chal", "S256"));
        assert!(!q.contains_key("prompt"), "동의를 매번 강제하지 않는다");
        // 다시 로그인: 저장한 client_id, 앱 이름은 빼고 host id 는 그대로
        let q = query(&authorize_url(Some("oaiapp_1"), "urn:uuid:h", redirect, &pkce(), "n0", true));
        assert_eq!(q["client_id"], "oaiapp_1");
        assert!(!q.contains_key("agent_name_hint"));
        assert_eq!(q["ext_agent_host_id"], "urn:uuid:h");
        assert_eq!(q["prompt"], "consent", "사용량 권한을 다시 물을 때만");
    }

    #[test]
    fn token_requests_are_forms_without_secret() {
        let r = token_req("oaiapp_1", "c0de", "ver", "http://127.0.0.1:1455/auth/callback");
        assert_eq!((r.method(), r.uri().to_string()), (&Method::POST, "https://auth.openai.com/api/accounts/oauth/token".to_string()));
        assert_eq!(r.headers()["Content-Type"], "application/x-www-form-urlencoded");
        let f = form_of(&r);
        assert_eq!(f["grant_type"], "authorization_code");
        assert_eq!(f["client_id"], "oaiapp_1", "교환은 발급받은 id 로 (dynamic_agent_client 아님)");
        assert_eq!((f["code"].as_str(), f["code_verifier"].as_str()), ("c0de", "ver"));
        assert_eq!(f["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
        assert_eq!(f["resource"], "https://api.openai.com/v1");
        assert!(!f.contains_key("client_secret"));
        let f = form_of(&refresh_req("oaiapp_1", "rt"));
        assert_eq!((f["grant_type"].as_str(), f["refresh_token"].as_str(), f["client_id"].as_str()), ("refresh_token", "rt", "oaiapp_1"));
        assert_eq!(f["resource"], "https://api.openai.com/v1");
        assert!(!f.contains_key("scope"), "scope 를 빼면 받은 권한이 유지된다");
        let r = revoke_req("oaiapp_1", "rt");
        assert_eq!(r.uri().to_string(), "https://auth.openai.com/api/accounts/oauth/revoke");
        let f = form_of(&r);
        assert_eq!((f["token"].as_str(), f["token_type_hint"].as_str(), f["client_id"].as_str()), ("rt", "refresh_token", "oaiapp_1"));
        let r = responses_req("tok", &json!({}));
        assert_eq!(r.headers()["Authorization"], "Bearer tok");
        assert_eq!(r.uri().to_string(), "https://api.openai.com/v1/responses");
    }

    #[test]
    fn callback_on_auth_path_with_issued_client() {
        let line = "GET /auth/callback?code=c0de&state=st&scope=openid&client_id=oaiapp_9 HTTP/1.1";
        let Ok(auth::Callback::Code(q)) = auth::parse_callback(line, CALLBACK, "st") else { panic!() };
        assert_eq!((q["code"].as_str(), q["client_id"].as_str()), ("c0de", "oaiapp_9"));
        assert_eq!(auth::parse_callback("GET /?code=c&state=st HTTP/1.1", CALLBACK, "st"), Ok(auth::Callback::Ignore), "다른 경로는 무시");
        assert_eq!(auth::parse_callback(line, CALLBACK, "other"), Ok(auth::Callback::Ignore), "state 가 다르면 받지 않고 계속 기다린다");
        assert_eq!(auth::parse_callback("GET /auth/callback?state=st&error=access_denied HTTP/1.1", CALLBACK, "st"), Ok(auth::Callback::Denied));
        // 처음 등록: 새 id 를 받는다. 다시 로그인: 저장한 id 와 달라지면 받지 않는다
        assert_eq!(issued_client(None, Some("oaiapp_9")), Ok("oaiapp_9".into()));
        assert!(issued_client(None, None).is_err() && issued_client(None, Some("dynamic_agent_client")).is_err());
        assert_eq!(issued_client(Some("oaiapp_1"), None), Ok("oaiapp_1".into()));
        assert_eq!(issued_client(Some("oaiapp_1"), Some("oaiapp_1")), Ok("oaiapp_1".into()));
        assert!(issued_client(Some("oaiapp_1"), Some("oaiapp_2")).is_err());
    }

    #[test]
    fn loopback_returns_issued_client() {
        use std::io::{Read, Write};
        let l = auth::loopback().unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let get = |target: &str| {
                let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(s, "GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).unwrap();
                out
            };
            // state 가 다른 요청(남이 보낸 취소 · 코드)에 끝나지 않고 진짜 리디렉트를 기다린다
            get("/auth/callback?state=evil&error=access_denied");
            get("/auth/callback?state=evil&code=bad&client_id=oaiapp_666");
            get("/auth/callback?state=st&code=c0de&client_id=oaiapp_9")
        });
        let q = auth::wait_callback(l, CALLBACK, "st", "ChatGPT 연결을 마쳤어요", &AtomicBool::new(false)).unwrap();
        assert_eq!((q["client_id"].as_str(), q["code"].as_str()), ("oaiapp_9", "c0de"));
        assert!(t.join().unwrap().contains("ChatGPT 연결을 마쳤어요"));
    }

    #[test]
    fn host_id_is_uuid_v4_urn() {
        let h = new_host_id();
        let u = h.strip_prefix("urn:uuid:").unwrap();
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert!(parts[2].starts_with('4') && "89ab".contains(&parts[3][..1]));
        assert_ne!(h, new_host_id());
    }

    // 테스트 키로 서명한 ID 토큰 (openssl genrsa 2048 · dgst -sha256 -sign). 내용:
    // {"iss":"https://auth.openai.com","aud":"oaiapp_test","sub":"user-1","email":"me@example.com","nonce":"n0","exp":4102444800}
    const TEST_N: &str = "rOWsWYuEuT1L6wFg3kMUWnWGSMqUbKYS-V-IfRV1AOFh23K2QDvDusNqQBQIXgl7_I_x3JiXEc1St4tPNBM85wACa_XLVXlUNKMHx_wIRAzMlJwB4l0MEH6Q_khxN5zNE5GYVczLeA_u6P7DziK4g3zZiNVvG439XG2AJlMKE648_aFEwMz2-Fs_DaN45546z3tIoyueRd9mGoUDyGOiYJL3Zjxp1TKuXithqjyX77maJeKo3AigV5BZn5lbAY3kBF6AEqySfmiyW5Uu7Qd9Ed7ojxgC-GZkg8Qu-pOlJU3AxdBTgDsyJ7pixQ2DQBZ2JRCEODKdVAuHy44DiEJsgw";
    const TEST_JWT: &str = "eyJhbGciOiJSUzI1NiIsImtpZCI6ImsxIiwidHlwIjoiSldUIn0.eyJpc3MiOiJodHRwczovL2F1dGgub3BlbmFpLmNvbSIsImF1ZCI6Im9haWFwcF90ZXN0Iiwic3ViIjoidXNlci0xIiwiZW1haWwiOiJtZUBleGFtcGxlLmNvbSIsIm5vbmNlIjoibjAiLCJleHAiOjQxMDI0NDQ4MDB9.hTOscGEreTxI6Mtai7R2fuiU_5tN6hQe7b_qLV07SU_RT9h4Gohh2yBUG3tRFeSvfrzBRm4bschlYvgHWAirHPc8vcHAa1FSxzLoAt1x9aXsFW8_genZSkqxmbUb9vf3uFo4tWrXBHIuUYKNrdLpGkCBaib0qbq4ct4INpkH3IgDZrysEfdfNG9NL28VdOSqliz0OIBVNx9YhMR-OQt82Ssl87_7u2mvKH031_ywLie1LA8IpdKa4aAdiidHAUT8usjYljP1MXj7FOorvXnOtWSJr4qQK80MP8U4a5Qp-Zay4ML0RgflSvNkOjtBR9-KLlVnV27hiK2VMg_8Yh7lrg";

    #[test]
    fn id_token_signature_and_claims() {
        let jwks = json!({ "keys": [{ "kty": "RSA", "kid": "other", "n": "AQAB", "e": "AQAB" }, { "kty": "RSA", "kid": "k1", "n": TEST_N, "e": "AQAB" }] });
        let (head, claims, signed, sig) = split_jwt(TEST_JWT).unwrap();
        assert_eq!((head["alg"].as_str(), head["kid"].as_str()), (Some("RS256"), Some("k1")));
        assert_eq!(verify_rs256(&jwks, Some("k1"), signed.as_bytes(), &sig), Ok(()));
        assert!(verify_rs256(&jwks, Some("other"), signed.as_bytes(), &sig).is_err(), "다른 kid 키로는 맞지 않는다");
        let mut forged = signed.as_bytes().to_vec();
        *forged.last_mut().unwrap() ^= 1;
        assert!(verify_rs256(&jwks, Some("k1"), &forged, &sig).is_err(), "내용을 바꾸면 서명이 맞지 않는다");
        assert!(split_jwt("a.b").is_err());

        let now = 1_790_000_000;
        assert_eq!(check_claims(&claims, "oaiapp_test", "n0", now), Ok(Identity { sub: "user-1".into(), email: "me@example.com".into() }));
        assert!(check_claims(&claims, "oaiapp_test", "other", now).is_err(), "nonce");
        assert!(check_claims(&claims, "oaiapp_x", "n0", now).is_err(), "aud");
        assert!(check_claims(&claims, "oaiapp_test", "n0", 4_102_444_800 + 120).is_err(), "exp");
        let mut c = claims.clone();
        c["iss"] = json!("https://evil.example");
        assert!(check_claims(&c, "oaiapp_test", "n0", now).is_err(), "iss");
        c["iss"] = json!(ISSUER);
        c["aud"] = json!(["x", "oaiapp_test"]);
        assert!(check_claims(&c, "oaiapp_test", "n0", now).is_ok(), "aud 배열");
    }

    #[test]
    fn scope_and_token_parsing() {
        assert!(has_use_scope("openid email offline_access resource.invoke chatgpt.tokens.use.direct"));
        assert!(!has_use_scope("openid profile email offline_access resource.invoke"), "사용량 권한을 빼고 허락");
        let t = parse_tokens(br#"{"access_token":"a","refresh_token":"r","id_token":"i","expires_in":3600,"scope":"openid email"}"#).unwrap();
        assert_eq!((t.refresh.as_deref(), t.id_token.as_deref(), t.scope.as_str()), (Some("r"), Some("i"), "openid email"));
        assert!(t.expires > crate::db::now_ms() + 3_000_000);
        let t = parse_tokens(br#"{"access_token":"a","refresh_token":"r"}"#).unwrap();
        assert!(!has_use_scope(&t.scope), "scope 가 없으면 사용량 권한도 없다");
        assert!(parse_tokens(br#"{"error":"invalid_grant"}"#).is_err());
        assert_eq!(refresh_error(400, br#"{"error":"refresh_token_reused"}"#), AiError::Expired);
        assert_eq!(refresh_error(400, br#"{"error":{"code":"invalid_grant"}}"#), AiError::Expired);
        assert_eq!(refresh_error(503, b""), AiError::Unavailable, "서버 오류로는 토큰을 지우지 않는다");
        assert!(matches!(refresh_error(400, br#"{"error":"invalid_client"}"#), AiError::Other(_)));
    }

    fn sse(text: &str) -> Outcome {
        let mut s = Sse::default();
        text.lines().for_each(|l| s.line(l));
        s.finish()
    }

    #[test]
    fn sse_outcomes() {
        let ok = "event: response.created\ndata: {\"type\":\"response.created\"}\n\n\
                  data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"a\\\"\"}\n\n\
                  data: {\"type\":\"response.output_text.delta\",\"delta\":\":1}\"}\r\n\r\n\
                  data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
        assert_eq!(sse(ok), Outcome::Done("{\"a\":1}".into()));
        // 조각 없이 끝나면 completed 의 output 에서
        let whole = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}]}}\n\n";
        assert_eq!(sse(whole), Outcome::Done("hi".into()));
        // 사용량 한도는 글자를 받는 중에도 온다
        let limit = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\"}\n\n\
                     data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"message\":\"x\"}}}\n\n";
        assert_eq!(sse(limit), Outcome::Failed(AiError::Limit));
        let unavailable = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_unavailable\"}}}\n\n";
        assert_eq!(sse(unavailable), Outcome::Failed(AiError::Unavailable));
        let inc = "data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n";
        assert_eq!(sse(inc), Outcome::Incomplete("max_output_tokens".into()));
        let err = "event: error\ndata: {\"type\":\"error\",\"code\":\"subscription_sharing_user_not_eligible\",\"message\":\"no\"}\n\n";
        assert_eq!(sse(err), Outcome::Failed(AiError::NotEligible));
        // 끝 이벤트 없이 끊김 (마지막 JSON 도 반쪽)
        let cut = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\"}\n\ndata: {\"type\":\"response.compl";
        assert_eq!(sse(cut), Outcome::Truncated);
        assert_eq!(sse(""), Outcome::Truncated);
        // 끝 뒤의 이벤트는 보지 않는다
        assert_eq!(sse(&format!("{ok}{limit}")), Outcome::Done("{\"a\":1}".into()));
    }

    #[test]
    fn http_rejections() {
        assert_eq!(classify(401, b""), Reject::Unauthorized);
        assert_eq!(classify(429, br#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#), Reject::Fail(AiError::Limit));
        assert_eq!(classify(403, br#"{"error":{"code":"subscription_sharing_user_not_eligible"}}"#), Reject::Fail(AiError::NotEligible));
        assert_eq!(classify(503, br#"{"detail":"direct routing unavailable"}"#), Reject::Fail(AiError::Unavailable));
        assert_eq!(classify(400, br#"{"error":{"code":"subscription_sharing_unsupported_capability","param":"text.format"}}"#), Reject::DropFormat);
        assert_eq!(classify(400, br#"{"error":{"code":"subscription_sharing_unsupported_capability","param":"model"}}"#), Reject::NextModel);
        assert_eq!(classify(403, br#"{"error":{"code":"chatpass_v2_scope_not_authorized"}}"#), Reject::Fail(AiError::Scope));
        assert!(matches!(classify(403, br#"{"detail":"region"}"#), Reject::Fail(AiError::Other(m)) if m.contains("region")));
    }

    #[test]
    fn model_list_keeps_listed_and_prefers_small() {
        let body = json!({ "models": [
            { "slug": "gpt-6.1-sol", "display_name": "GPT-6.1", "visibility": "list" },
            { "slug": "gpt-internal", "visibility": "hide" },
            { "slug": "gpt-6.1-mini", "visibility": "list" },
            { "slug": "gpt-6", "visibility": "list" },
        ]});
        assert_eq!(pick_models(&body), ["gpt-6.1-mini", "gpt-6.1-sol", "gpt-6"]);
        assert!(pick_models(&json!({})).is_empty());
    }

    fn items(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("항목{i}")).collect()
    }

    #[test]
    fn regroup_output_is_validated() {
        let six = items(6);
        let good = r#"{"cards":[{"front":"사람 관련은?","items":[1,4]},{"front":"제도 관련은?","items":[2,3,5]},{"front":"문화는?","items":[6]}]}"#;
        let parts = parse_regroup(good, &six).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!((parts[0].front.as_str(), parts[0].back.as_str()), ("사람 관련은?", "항목1\n항목4"), "답은 원래 항목 글자 그대로");
        assert!(parse_regroup(&format!("```json\n{good}\n```"), &six).is_ok(), "코드 블록으로 감싸도");
        let bad = |s: &str| matches!(parse_regroup(s, &six), Err(AiError::BadOutput(_)));
        assert!(bad(r#"{"cards":[{"front":"전부","items":[1,2,3,4,5,6]}]}"#), "1장은 너무 적다");
        let nine: Vec<String> = (1..=9).map(|i| format!(r#"{{"front":"q{i}","items":[{i}]}}"#)).collect();
        assert!(matches!(parse_regroup(&format!(r#"{{"cards":[{}]}}"#, nine.join(",")), &items(9)), Err(AiError::BadOutput(_))), "9장은 너무 많다");
        let eight: Vec<String> = (1..=8).map(|i| format!(r#"{{"front":"q{i}","items":[{i}]}}"#)).collect();
        assert!(parse_regroup(&format!(r#"{{"cards":[{}]}}"#, eight.join(",")), &items(8)).is_ok(), "8장까지");
        assert!(bad(r#"{"cards":[{"front":" ","items":[1,2,3]},{"front":"b","items":[4,5,6]}]}"#), "빈 질문");
        assert!(bad(r#"{"cards":[{"front":"a","items":[]},{"front":"b","items":[1,2,3,4,5,6]}]}"#), "빈 답");
        assert!(bad(r#"{"cards":[{"front":"a","items":[1,2,3]},{"front":"b","items":[3,4,5,6]}]}"#), "같은 항목 두 번");
        assert!(bad(r#"{"cards":[{"front":"a","items":[1,2,3]},{"front":"b","items":[4,5]}]}"#), "빠진 항목");
        assert!(bad(r#"{"cards":[{"front":"a","items":[0,1,2]},{"front":"b","items":[3,4,5,6]}]}"#), "없는 번호");
        assert!(bad(r#"{"cards":[{"front":"a","items":[1,2,3]},{"front":"b","items":[4,5,6,7]}]}"#), "없는 번호");
        assert!(bad("묶으면 이렇게 돼요: {\"cards\""), "JSON 이 아니다");
        assert!(bad(r#"{"cards":"x"}"#));
    }

    #[test]
    fn cited_segments_become_evidence() {
        let seg = |id: i64| rag::Sent {
            id,
            material_id: 1,
            material: "ITPM.pdf".into(),
            kind: "pdf".into(),
            page: id,
            signals: vec![],
            text: format!("구간 {id}"),
            how: "manual".into(),
        };
        let sent = vec![seg(31), seg(32), seg(33)];
        let text = r#"{"cards":[{"front":"a","items":[1,2],"segments":[2]},{"front":"b","items":[3],"segments":[2,1]}]}"#;
        let ev = parse_cited(text, &sent).unwrap();
        assert_eq!(ev.iter().map(|s| s.id).collect::<Vec<_>>(), [32, 31], "인용한 구간만, 처음 나온 순서, 한 번씩 (33 은 칩이 없다)");
        assert!(matches!(parse_cited(r#"{"cards":[{"front":"a","items":[1],"segments":[4]}]}"#, &sent), Err(AiError::BadOutput(_))));
        assert!(parse_cited(r#"{"cards":[{"front":"a","items":[1]}]}"#, &sent).unwrap().is_empty(), "segments 가 없으면 근거 없음");
        let schema = regroup_schema(true);
        assert_eq!(schema["properties"]["cards"]["items"]["required"], json!(["front", "items", "segments"]));
        let parts = parse_regroup(&fake_regroup(3, 2), &items(3)).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parse_cited(&fake_regroup(3, 2), &sent[..2]).unwrap().len(), 1);
    }

    #[test]
    fn only_plain_card_text_is_sent() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('비밀 덱 이름', 0, 0)", []).unwrap();
        let deck = con.last_insert_rowid();
        let card = crate::commands::NewCard {
            deck_id: deck,
            kind: "basic".into(),
            front: "<b>조직 문화</b>의 특성은? <img src=\"secret.png\">".into(),
            back: "① 공유된 <i>가치</i><br>② 규범 [sound:voice.mp3]<br>③ 상징&nbsp;&amp; 의례<br><img src=\"chart.png\">".into(),
            importance: None,
        };
        crate::commands::insert_note(&con, &card, "", None).unwrap();
        let id: i64 = con.query_row("SELECT max(id) FROM card", [], |r| r.get(0)).unwrap();
        con.execute("INSERT INTO review_log (card_id, reviewed_at, rating) VALUES (?1, 1, 1)", [id]).unwrap();
        let p = quality::suggest(&con, id, None).unwrap();
        let items = regroup_items(&p.answer);
        assert_eq!(items, ["공유된 가치", "규범", "상징 & 의례"]);
        let input = regroup_input(&p.question, &items);
        assert_eq!(input, "질문: 조직 문화의 특성은?\n답 항목:\n1. 공유된 가치\n2. 규범\n3. 상징 & 의례\n");
        for leak in ["<", "secret.png", "chart.png", "sound", "비밀 덱"] {
            assert!(!input.contains(leak), "{leak}");
        }
        let schema = regroup_schema(false);
        let body = request_body("m", INSTRUCTIONS, &input, Some(("regroup", &schema)));
        assert_eq!((body["store"].as_bool(), body["stream"].as_bool()), (Some(false), Some(true)));
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["schema"]["properties"]["cards"]["items"]["required"], json!(["front", "items"]));
        assert!(request_body("m", INSTRUCTIONS, &input, None).get("text").is_none());
        // 이번 달 보낸 카드: 이 기기에서만 센다
        assert_eq!(sent_this_month(&con), 0);
        count_sent(&con).unwrap();
        count_sent(&con).unwrap();
        assert_eq!(sent_this_month(&con), 2);
        set_meta(&con, META_SENT, Some("2000-01 9")).unwrap();
        assert_eq!(sent_this_month(&con), 0, "지난달 수는 세지 않는다");
    }

    fn seg(id: i64) -> rag::Sent {
        rag::Sent { id, material_id: 1, material: "4장 슬라이드.pptx".into(), kind: "pptx".into(), page: 12, signals: vec![], text: format!("구간 {id}"), how: "manual".into() }
    }

    #[test]
    fn card_check_output_is_validated() {
        let sent = vec![seg(31), seg(32)];
        let (q, a) = ("TCP와 UDP의 차이를 설명하시오.", "80");
        let check = |t: &str| parse_check(t, q, a, &sent, [false, false]);
        let good = r#"{"matches":false,"reason":"차이를 묻는데 포트 번호예요","fix":{"question":"TCP와 UDP의 차이를 설명하시오.","answer":"TCP 연결형\nUDP 비연결형"},"checks":["x"],"segments":[2,2]}"#;
        let c = check(good).unwrap();
        assert_eq!(c.fix, Some(CardFix { question: q.into(), answer: "TCP 연결형\nUDP 비연결형".into() }));
        assert_eq!(c.evidence.iter().map(|s| s.id).collect::<Vec<_>>(), [32], "인용한 구간만 한 번씩");
        assert!(c.checks.is_empty(), "고친 안이 있으면 확인할 물음은 없다");
        assert_eq!(c.sent.len(), 2);
        assert!(check(&format!("```json\n{good}\n```")).is_ok(), "코드 블록으로 감싸도");
        // 맞다고 하면 고친 안 · 물음 · 근거는 버린다
        let c = check(r#"{"matches":true,"reason":"맞아요","fix":{"question":"x","answer":"y"},"checks":["?"],"segments":[1]}"#).unwrap();
        assert!(c.matches && c.fix.is_none() && c.checks.is_empty() && c.evidence.is_empty());
        // 맞다고 하면 함께 온 것이 이상해도(빈 물음 · 긴 물음 · 서식 · 보내지 않은 구간 · 깨진 fix) 판단은 받는다 (QA 4)
        let long = "가".repeat(MAX_FRONT_CHARS + 1);
        for extra in [
            r#""checks":[""],"segments":[]"#.to_string(),
            format!(r#""checks":["{long}"],"segments":[]"#),
            r#""checks":["<b>x</b>"],"segments":[9]"#.to_string(),
            r#""checks":"x","segments":"y""#.to_string(),
        ] {
            let t = format!(r#"{{"matches":true,"reason":"맞아요","fix":{{"question":"","answer":"<b>"}},{extra}}}"#);
            let c = check(&t).unwrap_or_else(|e| panic!("{extra}: {e:?}"));
            assert!(c.matches && c.fix.is_none() && c.checks.is_empty() && c.evidence.is_empty(), "{extra}");
        }
        // 고친 안이 있으면 물음은 보지 않는다. 고친 안이 없을 때 이상한 물음은 그 물음만 버린다
        assert!(check(r#"{"matches":false,"reason":"r","fix":{"question":"q","answer":"a"},"checks":[""],"segments":[]}"#).unwrap().fix.is_some());
        let c = check(r#"{"matches":false,"reason":"r","fix":null,"checks":["","좋은 물음인가요?","<b>x</b>"],"segments":[7]}"#).unwrap();
        assert_eq!(c.checks, ["좋은 물음인가요?"], "고친 안이 없으면 구간 번호도 보지 않는다");
        // 근거가 부족하면 고친 안 없이 확인할 물음만 (억지로 만들지 않는다)
        let c = check(r#"{"matches":false,"reason":"자료에 답이 없어요","fix":null,"checks":["묻는 것이 맞나요?","다른 카드와 섞였나요?"],"segments":[]}"#).unwrap();
        assert!(!c.matches && c.fix.is_none());
        assert_eq!(c.checks, ["묻는 것이 맞나요?", "다른 카드와 섞였나요?"]);
        // 보낸 글과 같은 고친 안은 고친 안이 아니다
        assert!(check(r#"{"matches":false,"reason":"r","fix":{"question":"TCP와 UDP의 차이를 설명하시오.","answer":" 80 "},"checks":[],"segments":[]}"#).unwrap().fix.is_none());
        let bad = |t: &str| matches!(check(t), Err(AiError::BadOutput(_)));
        assert!(bad(r#"{"matches":false,"reason":" ","fix":null,"checks":[],"segments":[]}"#), "빈 근거");
        assert!(bad(&format!(r#"{{"matches":false,"reason":"{}","fix":null,"checks":[],"segments":[]}}"#, "가".repeat(MAX_REASON_CHARS + 1))), "긴 근거");
        assert!(bad(r#"{"reason":"r","fix":null,"checks":[],"segments":[]}"#), "matches 가 없다");
        for fix in [r#"{"question":"","answer":"a"}"#, r#"{"question":"q","answer":"<b>a</b>"}"#, r#"{"question":"{{c1::q}}","answer":"a"}"#, r#"{"question":"q","answer":"**a**"}"#] {
            assert!(bad(&format!(r#"{{"matches":false,"reason":"r","fix":{fix},"checks":[],"segments":[]}}"#)), "{fix}");
        }
        assert!(bad(&format!(r#"{{"matches":false,"reason":"r","fix":{{"question":"q","answer":"{}"}},"checks":[],"segments":[]}}"#, "가".repeat(MAX_ANSWER_CHARS + 1))), "긴 답");
        assert!(bad(r#"{"matches":false,"reason":"r","fix":{"question":"q","answer":"a"},"checks":[],"segments":[3]}"#), "보내지 않은 구간");
        assert!(!bad(r#"{"matches":false,"reason":"r","fix":null,"checks":[""],"segments":[]}"#), "빈 물음은 그 물음만 버린다");
        assert!(bad("고치면 이래요: {"), "JSON 이 아니다");
        assert!(!markup("a < b 그리고 3<5"), "부등호는 글자다");
        // 잠긴 칸(서식 · 그림)을 바꾼 고친 안: 그 칸은 버리고 알린다 (QA M2). 남는 것이 없으면 고친 안이 없다
        let both = r#"{"matches":false,"reason":"r","fix":{"question":"고친 질문","answer":"고친 답"},"checks":[],"segments":[]}"#;
        let c = parse_check(both, q, a, &sent, [false, true]).unwrap();
        assert_eq!(c.fix, Some(CardFix { question: "고친 질문".into(), answer: a.into() }));
        assert!(c.kept_locked);
        let c = parse_check(both, q, a, &sent, [true, true]).unwrap();
        assert!(c.fix.is_none() && c.kept_locked);
        assert!(!check(both).unwrap().kept_locked);
        let schema = check_schema();
        let required: Vec<&str> = schema["required"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        for k in ["matches", "reason", "fix", "checks", "segments"] {
            assert!(required.contains(&k), "{k}");
        }
        assert_eq!(schema["properties"]["fix"]["anyOf"][1], json!({ "type": "null" }), "고친 안 없음(null)을 받는다");
    }

    #[test]
    fn fake_check_gives_the_three_states() {
        let st = AiState::new();
        let text = fake_text(&st, 0, "ok", || fake_check("Q?", "80", Some("근거"), 1)).unwrap();
        let c = parse_check(&text, "Q?", "80", &[seg(5)], [false, false]).unwrap();
        assert!(c.fix.is_some() && c.evidence.len() == 1, "연결 구간이 있으면 그 구간을 근거로 고친 안");
        let c = parse_check(&fake_check("Q?", "80", Some("근거"), 0), "Q?", "80", &[], [false, false]).unwrap();
        assert!(c.fix.is_none() && c.checks.len() == 2, "구간이 없으면 근거 부족");
        let c = parse_check(&fake_check("광합성", "빛으로 무엇을 만들까?", Some("답이 물음으로 끝나요. 앞면과 뒷면이 바뀌었을 수 있어요"), 0), "광합성", "빛으로 무엇을 만들까?", &[], [false, false]).unwrap();
        assert_eq!(c.fix, Some(CardFix { question: "빛으로 무엇을 만들까?".into(), answer: "광합성".into() }), "앞뒤가 바뀐 카드는 자료 없이 고친 안");
        assert!(c.evidence.is_empty() && c.sent.is_empty());
        assert!(parse_check(&fake_check("Q?", "80", None, 0), "Q?", "80", &[], [false, false]).unwrap().matches);
        let text = fake_text(&st, 0, "malformed", || unreachable!()).unwrap();
        assert!(matches!(parse_check(&text, "Q?", "80", &[], [false, false]), Err(AiError::BadOutput(_))));
    }

    #[test]
    fn card_check_sends_plain_text_and_changes_nothing() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (id, name, created_at, updated_at) VALUES (1, '비밀 덱 이름', 0, 0)", []).unwrap();
        let card = crate::commands::NewCard {
            deck_id: 1,
            kind: "basic".into(),
            front: "<b>TCP</b>와 UDP의 차이를 설명하시오. <img src=\"secret.png\">".into(),
            back: "<div>80 [sound:port.mp3]</div>".into(),
            importance: None,
        };
        crate::commands::insert_note(&con, &card, "", None).unwrap();
        let id: i64 = con.query_row("SELECT max(id) FROM card", [], |r| r.get(0)).unwrap();
        con.execute_batch(&format!(
            "INSERT INTO review_log (card_id, reviewed_at, rating) VALUES ({id}, 1, 1);
             INSERT INTO source_material (id, deck_id, file_name, rel_path, kind, status, created_at, updated_at) VALUES (1, 1, '4장.pptx', 'sources/1.pptx', 'pptx', 'ready', 0, 0);
             INSERT INTO source_segment (id, material_id, page, ord, text, signals, weight) VALUES (11, 1, 12, 0, 'TCP: 연결 지향 / UDP: 비연결', 'bold', 1), (12, 1, 13, 0, '연결 안 된 구간', 'bold', 1);
             INSERT INTO source_link (segment_id, card_id, method, created_at) VALUES (11, {id}, 'manual', 0);"
        ))
        .unwrap();
        let snapshot = |con: &Connection| {
            con.query_row("SELECT n.fields, n.updated_at, (SELECT count(*) FROM card_change) FROM note n JOIN card c ON c.note_id = n.id WHERE c.id = ?1", [id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
            })
            .unwrap()
        };
        let before = snapshot(&con);
        // 구간을 모두 뺐으면 카드 글자만
        let (_, sent, input, instructions) = check_prepare(&con, id, &[]).unwrap();
        assert!(instructions.contains("질문과 답에는 그림 · 소리 · 서식이 있어"), "잠긴 칸(그림 · 굵게 · 소리)은 바꾸지 말라고 한다");
        assert!(sent.is_empty());
        assert_eq!(input, "질문: TCP와 UDP의 차이를 설명하시오.\n답:\n80\n");
        for leak in ["<", "secret.png", "sound", "port.mp3", "비밀 덱"] {
            assert!(!input.contains(leak), "{leak}");
        }
        // 확인 목록에서 남긴 연결 구간만 (연결되지 않은 12 는 보내지 않는다)
        let (r, sent, input, _) = check_prepare(&con, id, &[11, 12]).unwrap();
        assert_eq!(sent.iter().map(|s| s.id).collect::<Vec<_>>(), [11]);
        assert!(input.ends_with("자료 구간:\n[1] TCP: 연결 지향 / UDP: 비연결\n"), "{input}");
        assert!(!input.contains("연결 안 된 구간"));
        let schema = check_schema();
        let body = request_body("m", CHECK_INSTRUCTIONS, &input, Some(("card_check", &schema)));
        assert_eq!((body["store"].as_bool(), body["stream"].as_bool()), (Some(false), Some(true)));
        // 고친 안을 받아도 카드는 그대로다: 바꾸는 길은 사용자가 승인한 quality::rewrite_apply 뿐
        let c = parse_check(&fake_check(&r.question, &r.answer, Some("x"), sent.len()), &r.question, &r.answer, &sent, r.locked).unwrap();
        assert!(c.fix.is_none() && c.kept_locked, "잠긴 답을 바꾼 고친 안은 버린다");
        assert_eq!(snapshot(&con), before, "필드 · 수정 시각 · 변경 기록 그대로");
    }

    /// QA R-06: 자료 구간을 보내지 않으면 근거 · 물음에 '자료'를 말하지 말라고 한다. 보냈으면 그 줄이 없다. 잠긴 칸 줄은 그대로 붙는다
    #[test]
    fn check_without_sources_does_not_talk_about_sources() {
        assert!(check_instructions([false, false], false).contains("자료 구간을 보내지 않았어요"));
        assert_eq!(check_instructions([false, false], true), CHECK_INSTRUCTIONS);
        let locked = check_instructions([true, false], false);
        assert!(locked.contains("자료 구간을 보내지 않았어요") && locked.ends_with("fix 의 그 칸은 보낸 글 그대로 두세요."));
    }
}
