//! 동기화 저장소 (drive-sync.md 4장). "파일을 새로 쓰고, 목록을 보고, 내려받기"만 쓴다 (9장).
//!
//! 경로 규칙: `backups/<이름>` 은 사용자에게 보이는 "Neo-Anki 백업" 폴더, 나머지는 앱 전용 숨김 공간이다.
//!   - DriveStore: Google Drive v3 REST. 숨김 공간은 appDataFolder (drive.appdata), 보이는 폴더는 drive.file
//!   - FolderStore: 로컬 폴더. 테스트와 개발용 (NEOANKI_SYNC_FOLDER)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use ureq::http::{Method, Request};

pub const BACKUP_FOLDER: &str = "Neo-Anki 백업";
const API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3";

/// 동기화가 멈추는 이유. 화면 표시(K5)가 이 구분을 그대로 쓴다
#[derive(Debug, Clone, PartialEq)]
pub enum SyncError {
    /// 인터넷이 안 된다. 변경은 이 기기에 쌓아 둔다
    Offline(String),
    /// 토큰 만료 · 권한 취소. "다시 로그인"
    Auth,
    /// Drive 공간 부족
    Quota,
    /// Drive 데이터가 이 앱보다 새 스키마
    NewerSchema,
    /// 암호로 잠근 데이터인데 이 기기에 키가 없다
    Locked,
    NotFound(String),
    Other(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Offline(e) => write!(f, "인터넷에 연결되지 않았어요 ({e})"),
            SyncError::Auth => write!(f, "Google 로그인이 만료됐어요. 다시 로그인해 주세요."),
            SyncError::Quota => write!(f, "Drive 공간이 부족해요."),
            SyncError::NewerSchema => write!(f, "Drive 데이터가 더 새 버전의 앱에서 만들어졌어요. 앱을 업데이트해 주세요."),
            SyncError::Locked => write!(f, "Drive 데이터가 암호로 잠겨 있어요. 암호를 입력해 주세요."),
            SyncError::NotFound(p) => write!(f, "Drive 에서 파일을 찾지 못했어요: {p}"),
            SyncError::Other(e) => write!(f, "{e}"),
        }
    }
}

pub type SRes<T> = Result<T, SyncError>;

pub fn other<E: std::fmt::Display>(e: E) -> SyncError {
    SyncError::Other(e.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    /// 만든 시각 (ms). 백업 목록에만 쓴다
    pub at: i64,
}

pub trait Store: Send + Sync {
    /// prefix 로 시작하는 파일 (하위 폴더 포함)
    fn list(&self, prefix: &str) -> SRes<Vec<Entry>>;
    fn get(&self, path: &str) -> SRes<Vec<u8>>;
    /// 없으면 만들고 있으면 바꾼다. 바꾸는 것은 자기 기기 파일(devices/<나>.json)뿐이다
    fn put(&self, path: &str, data: &[u8]) -> SRes<()>;
    fn delete(&self, path: &str) -> SRes<()>;
    /// 계정 (화면 표시)과 Drive 공간 (쓴 양, 한도). 폴더는 공간을 모른다
    fn about(&self) -> SRes<(String, Option<(u64, u64)>)>;
    /// 보이는 백업 폴더를 여는 주소 (브라우저 또는 탐색기)
    fn folder_link(&self) -> SRes<String>;
}

// ---------------------------------------------------------------- 로컬 폴더

pub struct FolderStore {
    pub root: PathBuf,
}

impl FolderStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn file(&self, path: &str) -> SRes<PathBuf> {
        // Windows 는 \ 도 경로 구분자이고 C: 같은 드라이브 경로는 root 밖이다
        if path.contains(['\\', ':']) || path.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
            return Err(other(format!("잘못된 경로: {path}")));
        }
        Ok(match path.strip_prefix("backups/") {
            Some(name) => self.root.join(BACKUP_FOLDER).join(name),
            None => self.root.join("appdata").join(path),
        })
    }
}

fn walk(dir: &std::path::Path, rel: &str, out: &mut Vec<Entry>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            walk(&e.path(), &path, out);
        } else if !name.starts_with(".tmp-") {
            let at = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
            out.push(Entry { path, size: meta.len(), at });
        }
    }
}

impl Store for FolderStore {
    fn list(&self, prefix: &str) -> SRes<Vec<Entry>> {
        let mut out = vec![];
        if let Some(p) = prefix.strip_prefix("backups/") {
            walk(&self.root.join(BACKUP_FOLDER), "", &mut out);
            out.iter_mut().for_each(|e| e.path = format!("backups/{}", e.path));
            out.retain(|e| e.path["backups/".len()..].starts_with(p));
        } else {
            walk(&self.root.join("appdata"), "", &mut out);
            out.retain(|e| e.path.starts_with(prefix));
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn get(&self, path: &str) -> SRes<Vec<u8>> {
        std::fs::read(self.file(path)?).map_err(|_| SyncError::NotFound(path.into()))
    }

    fn put(&self, path: &str, data: &[u8]) -> SRes<()> {
        let file = self.file(path)?;
        let dir = file.parent().ok_or_else(|| other("경로"))?;
        std::fs::create_dir_all(dir).map_err(other)?;
        // 반쯤 쓴 파일을 다른 기기가 읽지 않게: 임시 파일에 쓰고 이름을 바꾼다
        let tmp = dir.join(format!(".tmp-{}", crate::sync::crypto::random_hex(8)));
        std::fs::write(&tmp, data).map_err(other)?;
        std::fs::rename(&tmp, &file).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            other(e)
        })
    }

    fn delete(&self, path: &str) -> SRes<()> {
        match std::fs::remove_file(self.file(path)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(other(e)),
        }
    }

    fn about(&self) -> SRes<(String, Option<(u64, u64)>)> {
        let name = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok((format!("로컬 폴더 · {name}"), None))
    }

    fn folder_link(&self) -> SRes<String> {
        let dir = self.root.join(BACKUP_FOLDER);
        std::fs::create_dir_all(&dir).map_err(other)?;
        Ok(dir.display().to_string())
    }
}

// ---------------------------------------------------------------- Google Drive v3

/// 요청 하나. 만드는 함수와 보내는 함수를 나눠, 만드는 쪽(주소 · 헤더 · 본문)을 계정 없이 테스트한다
pub type Req = Request<Vec<u8>>;

fn url(base: &str, params: &[(&str, &str)]) -> String {
    url::Url::parse_with_params(base, params).map(String::from).unwrap_or_else(|_| base.into())
}

fn req(method: Method, uri: &str, token: &str) -> ureq::http::request::Builder {
    Request::builder().method(method).uri(uri).header("Authorization", format!("Bearer {token}"))
}

/// 숨김 공간 전체 목록 (한 쪽에 1000개). 경로는 파일 이름에 그대로 넣었다 ("changes/<기기>/<번호>.jsonl.gz")
pub fn list_req(token: &str, page: Option<&str>) -> Req {
    let mut p = vec![("spaces", "appDataFolder"), ("fields", "nextPageToken,files(id,name,size,createdTime)"), ("pageSize", "1000")];
    if let Some(page) = page {
        p.push(("pageToken", page));
    }
    req(Method::GET, &url(&format!("{API}/files"), &p), token).body(vec![]).unwrap()
}

/// 보이는 백업 폴더 안 목록
pub fn folder_list_req(token: &str, folder: &str, page: Option<&str>) -> Req {
    let q = format!("'{folder}' in parents and trashed = false");
    let mut p = vec![("q", q.as_str()), ("spaces", "drive"), ("fields", "nextPageToken,files(id,name,size,createdTime)"), ("pageSize", "1000")];
    if let Some(page) = page {
        p.push(("pageToken", page));
    }
    req(Method::GET, &url(&format!("{API}/files"), &p), token).body(vec![]).unwrap()
}

/// drive.file 권한은 앱이 만든 파일만 보인다. 사용자가 같은 이름 폴더를 직접 만들어도 섞이지 않는다
pub fn find_folder_req(token: &str) -> Req {
    let q = format!("name = '{BACKUP_FOLDER}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false");
    req(Method::GET, &url(&format!("{API}/files"), &[("q", &q), ("spaces", "drive"), ("fields", "files(id)")]), token).body(vec![]).unwrap()
}

pub fn create_folder_req(token: &str) -> Req {
    let body = serde_json::json!({ "name": BACKUP_FOLDER, "mimeType": "application/vnd.google-apps.folder" }).to_string();
    req(Method::POST, &url(&format!("{API}/files"), &[("fields", "id")]), token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .body(body.into_bytes())
        .unwrap()
}

pub fn get_req(token: &str, id: &str) -> Req {
    req(Method::GET, &url(&format!("{API}/files/{id}"), &[("alt", "media")]), token).body(vec![]).unwrap()
}

pub fn delete_req(token: &str, id: &str) -> Req {
    req(Method::DELETE, &format!("{API}/files/{id}"), token).body(vec![]).unwrap()
}

pub fn about_req(token: &str) -> Req {
    req(Method::GET, &url(&format!("{API}/about"), &[("fields", "user(emailAddress),storageQuota(limit,usage)")]), token).body(vec![]).unwrap()
}

/// 재개 가능한 업로드 1단계 (5MB 넘는 백업 · 그림도 한 번에). 새 파일이면 이름과 부모, 기존 파일이면 id.
/// 응답의 Location 으로 내용을 PUT 한다
pub fn upload_start_req(token: &str, name: &str, parent: &str, existing: Option<&str>, len: usize) -> Req {
    let (method, uri, meta) = match existing {
        Some(id) => (Method::PATCH, url(&format!("{UPLOAD}/files/{id}"), &[("uploadType", "resumable"), ("fields", "id")]), serde_json::json!({})),
        None => (
            Method::POST,
            url(&format!("{UPLOAD}/files"), &[("uploadType", "resumable"), ("fields", "id")]),
            serde_json::json!({ "name": name, "parents": [parent] }),
        ),
    };
    req(method, &uri, token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .header("X-Upload-Content-Type", "application/octet-stream")
        .header("X-Upload-Content-Length", len.to_string())
        .body(meta.to_string().into_bytes())
        .unwrap()
}

pub fn upload_put_req(session: &str, data: &[u8]) -> Req {
    Request::builder().method(Method::PUT).uri(session).header("Content-Type", "application/octet-stream").body(data.to_vec()).unwrap()
}

pub struct Resp {
    pub status: u16,
    pub location: Option<String>,
    pub body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> SRes<serde_json::Value> {
        serde_json::from_slice(&self.body).map_err(other)
    }
}

/// 상태 코드 -> 멈춤 이유. 403 은 본문의 reason 으로 공간 부족과 속도 제한을 가른다
pub fn classify(status: u16, body: &[u8]) -> Option<SyncError> {
    let text = String::from_utf8_lossy(body);
    match status {
        200..=299 | 308 => None,
        401 => Some(SyncError::Auth),
        403 if text.contains("storageQuotaExceeded") || text.contains("quotaExceeded") && !text.contains("rateLimit") => Some(SyncError::Quota),
        403 if text.contains("insufficientPermissions") || text.contains("ACCESS_TOKEN_SCOPE_INSUFFICIENT") => Some(SyncError::Auth),
        404 => Some(SyncError::NotFound(String::new())),
        _ => Some(SyncError::Other(format!("Drive 응답 {status}: {}", text.chars().take(200).collect::<String>()))),
    }
}

/// 속도 제한 · 서버 오류는 다시 시도할 만하다
fn retryable(status: u16, body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body);
    status == 429 || status >= 500 || (status == 403 && (text.contains("rateLimitExceeded") || text.contains("userRateLimitExceeded")))
}

pub fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(120))).build(),
    )
}

/// 보내기. 속도 제한 · 5xx 는 지수 백오프로 4번까지 (1 · 2 · 4초)
pub fn send(agent: &ureq::Agent, r: Req) -> SRes<Resp> {
    let mut wait = Duration::from_secs(1);
    for attempt in 0..4 {
        let res = if r.body().is_empty() { agent.run(r.clone().map(|_| ())) } else { agent.run(r.clone()) };
        let resp = match res {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(String::from);
                let body = resp.body_mut().with_config().limit(512 * 1024 * 1024).read_to_vec().map_err(|e| SyncError::Offline(e.to_string()))?;
                Resp { status, location, body }
            }
            Err(e) => return Err(SyncError::Offline(e.to_string())),
        };
        if retryable(resp.status, &resp.body) && attempt < 3 {
            std::thread::sleep(wait);
            wait *= 2;
            continue;
        }
        return Ok(resp);
    }
    unreachable!()
}

/// 토큰을 주는 쪽. 만료되면 새로 받아 오고, 못 받으면 Auth 를 돌려준다
pub type TokenFn = Box<dyn Fn(bool) -> SRes<String> + Send + Sync>;

pub struct DriveStore {
    agent: ureq::Agent,
    token: TokenFn,
    /// 경로 -> 파일 id. list 가 채운다
    ids: Mutex<HashMap<String, String>>,
    folder: Mutex<Option<String>>,
}

impl DriveStore {
    pub fn new(token: TokenFn) -> Self {
        Self { agent: agent(), token, ids: Mutex::new(HashMap::new()), folder: Mutex::new(None) }
    }

    /// 401 이면 토큰을 한 번 새로 받아 다시 보낸다
    fn call(&self, make: &dyn Fn(&str) -> Req) -> SRes<Resp> {
        let mut resp = send(&self.agent, make(&(self.token)(false)?))?;
        if resp.status == 401 {
            resp = send(&self.agent, make(&(self.token)(true)?))?;
        }
        match classify(resp.status, &resp.body) {
            None => Ok(resp),
            Some(e) => Err(e),
        }
    }

    fn list_all(&self, make: &dyn Fn(&str, Option<&str>) -> Req) -> SRes<Vec<(String, Entry)>> {
        let mut out = vec![];
        let mut page: Option<String> = None;
        loop {
            let v = self.call(&|t| make(t, page.as_deref()))?.json()?;
            for f in v["files"].as_array().into_iter().flatten() {
                let at = f["createdTime"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.timestamp_millis()).unwrap_or(0);
                let size = f["size"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0);
                out.push((f["id"].as_str().unwrap_or_default().to_string(), Entry { path: f["name"].as_str().unwrap_or_default().to_string(), size, at }));
            }
            match v["nextPageToken"].as_str() {
                Some(p) => page = Some(p.to_string()),
                None => return Ok(out),
            }
        }
    }

    /// 보이는 백업 폴더 id. 사용자가 지웠으면 다음 백업 때 새로 만든다 (8장)
    fn folder(&self, create: bool) -> SRes<Option<String>> {
        if let Some(id) = self.folder.lock().map_err(other)?.clone() {
            return Ok(Some(id));
        }
        let found = self.call(&find_folder_req)?.json()?["files"][0]["id"].as_str().map(String::from);
        let id = match found {
            Some(id) => Some(id),
            None if create => self.call(&create_folder_req)?.json()?["id"].as_str().map(String::from),
            None => None,
        };
        *self.folder.lock().map_err(other)? = id.clone();
        Ok(id)
    }

    fn id_of(&self, path: &str) -> SRes<Option<String>> {
        if let Some(id) = self.ids.lock().map_err(other)?.get(path) {
            return Ok(Some(id.clone()));
        }
        let prefix = if path.starts_with("backups/") { "backups/" } else { "" };
        self.list(prefix)?;
        Ok(self.ids.lock().map_err(other)?.get(path).cloned())
    }
}

impl Store for DriveStore {
    fn list(&self, prefix: &str) -> SRes<Vec<Entry>> {
        let files = if prefix.starts_with("backups/") {
            let Some(folder) = self.folder(false)? else { return Ok(vec![]) };
            let mut files = self.list_all(&|t, p| folder_list_req(t, &folder, p))?;
            files.iter_mut().for_each(|(_, e)| e.path = format!("backups/{}", e.path));
            files
        } else {
            self.list_all(&list_req)?
        };
        let mut ids = self.ids.lock().map_err(other)?;
        let mut out: Vec<Entry> = vec![];
        for (id, e) in files {
            ids.insert(e.path.clone(), id);
            if e.path.starts_with(prefix) {
                out.push(e);
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    fn get(&self, path: &str) -> SRes<Vec<u8>> {
        let id = self.id_of(path)?.ok_or_else(|| SyncError::NotFound(path.into()))?;
        Ok(self.call(&|t| get_req(t, &id))?.body)
    }

    fn put(&self, path: &str, data: &[u8]) -> SRes<()> {
        let (name, parent) = match path.strip_prefix("backups/") {
            Some(name) => (name.to_string(), self.folder(true)?.ok_or_else(|| other("백업 폴더를 만들지 못했어요"))?),
            None => (path.to_string(), "appDataFolder".to_string()),
        };
        // 바꾸는 파일(devices/<나>.json, crypto.json)은 이미 있는지 먼저 본다. Drive 는 같은 이름 파일을 여러 개 허락한다.
        // 캐시는 따로 읽는다: match 조건식에 잠금을 두면 match 가 끝날 때까지 쥐고 있어 id_of 가 같은 잠금을 기다리며 멈췄다
        let cached = self.ids.lock().map_err(other)?.get(path).cloned();
        let existing = match cached {
            Some(id) => Some(id),
            None if path.starts_with("devices/") || path == "crypto.json" => self.id_of(path)?,
            None => None,
        };
        let start = self.call(&|t| upload_start_req(t, &name, &parent, existing.as_deref(), data.len()))?;
        let session = start.location.ok_or_else(|| other("업로드 주소를 받지 못했어요"))?;
        // 세션 주소는 그 자체로 권한이다 (Authorization 없이 PUT)
        let resp = send(&self.agent, upload_put_req(&session, data))?;
        if let Some(e) = classify(resp.status, &resp.body) {
            return Err(e);
        }
        if let Some(id) = resp.json().ok().and_then(|v| v["id"].as_str().map(String::from)) {
            self.ids.lock().map_err(other)?.insert(path.to_string(), id);
        }
        Ok(())
    }

    fn delete(&self, path: &str) -> SRes<()> {
        let Some(id) = self.id_of(path)? else { return Ok(()) };
        match self.call(&|t| delete_req(t, &id)) {
            Ok(_) | Err(SyncError::NotFound(_)) => {
                self.ids.lock().map_err(other)?.remove(path);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn about(&self) -> SRes<(String, Option<(u64, u64)>)> {
        let v = self.call(&about_req)?.json()?;
        let n = |k: &str| v["storageQuota"][k].as_str().and_then(|s| s.parse::<u64>().ok());
        // 한도가 없는 계정(무제한)은 limit 이 빠진다
        let quota = n("usage").zip(n("limit"));
        Ok((v["user"]["emailAddress"].as_str().unwrap_or("Google 계정").to_string(), quota))
    }

    fn folder_link(&self) -> SRes<String> {
        let id = self.folder(true)?.ok_or_else(|| other("백업 폴더를 찾지 못했어요"))?;
        Ok(format!("https://drive.google.com/drive/folders/{id}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header<'a>(r: &'a Req, k: &str) -> &'a str {
        r.headers().get(k).unwrap().to_str().unwrap()
    }

    #[test]
    fn drive_requests_are_well_formed() {
        let r = list_req("tok", None);
        assert_eq!(r.method(), Method::GET);
        assert_eq!(
            r.uri().to_string(),
            "https://www.googleapis.com/drive/v3/files?spaces=appDataFolder&fields=nextPageToken%2Cfiles%28id%2Cname%2Csize%2CcreatedTime%29&pageSize=1000"
        );
        assert_eq!(header(&r, "Authorization"), "Bearer tok");
        assert!(list_req("tok", Some("p2")).uri().to_string().ends_with("&pageToken=p2"));

        let r = get_req("tok", "abc");
        assert_eq!(r.uri().to_string(), "https://www.googleapis.com/drive/v3/files/abc?alt=media");
        assert_eq!(delete_req("tok", "abc").method(), Method::DELETE);

        // 새 파일: 숨김 공간에 이름 그대로, 길이는 헤더로
        let r = upload_start_req("tok", "changes/d1/00000001.jsonl.gz", "appDataFolder", None, 42);
        assert_eq!(r.method(), Method::POST);
        assert_eq!(r.uri().to_string(), "https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable&fields=id");
        assert_eq!(header(&r, "X-Upload-Content-Length"), "42");
        let meta: serde_json::Value = serde_json::from_slice(r.body()).unwrap();
        assert_eq!(meta, serde_json::json!({ "name": "changes/d1/00000001.jsonl.gz", "parents": ["appDataFolder"] }));
        // 기존 파일 (devices/<나>.json) 은 PATCH
        let r = upload_start_req("tok", "devices/d1.json", "appDataFolder", Some("id9"), 3);
        assert_eq!(r.method(), Method::PATCH);
        assert!(r.uri().to_string().starts_with("https://www.googleapis.com/upload/drive/v3/files/id9?uploadType=resumable"));

        let r = upload_put_req("https://upload.example/session", b"abc");
        assert_eq!((r.method(), r.body().as_slice()), (&Method::PUT, &b"abc"[..]));
        assert!(r.headers().get("Authorization").is_none(), "세션 주소에는 토큰을 다시 싣지 않는다");

        let r = find_folder_req("tok");
        let q: HashMap<_, _> = url::Url::parse(&r.uri().to_string()).unwrap().query_pairs().into_owned().collect();
        assert_eq!(q["q"], "name = 'Neo-Anki 백업' and mimeType = 'application/vnd.google-apps.folder' and trashed = false");
        assert_eq!(q["spaces"], "drive");
        let r = folder_list_req("tok", "F1", None);
        let q: HashMap<_, _> = url::Url::parse(&r.uri().to_string()).unwrap().query_pairs().into_owned().collect();
        assert_eq!(q["q"], "'F1' in parents and trashed = false");
        assert!(about_req("tok").uri().to_string().contains("storageQuota"));
    }

    #[test]
    fn drive_errors_are_classified() {
        assert_eq!(classify(200, b""), None);
        assert_eq!(classify(401, b""), Some(SyncError::Auth));
        assert_eq!(classify(403, br#"{"error":{"errors":[{"reason":"storageQuotaExceeded"}]}}"#), Some(SyncError::Quota));
        assert!(matches!(classify(403, br#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#), Some(SyncError::Other(_))));
        assert!(retryable(403, br#"{"reason":"userRateLimitExceeded"}"#));
        assert!(retryable(429, b"") && retryable(503, b"") && !retryable(404, b""));
    }

    #[test]
    fn folder_store_round_trip() {
        let dir = std::env::temp_dir().join(format!("neoanki-store-{}", crate::sync::crypto::random_hex(6)));
        let s = FolderStore::new(&dir);
        s.put("changes/d1/00000001.jsonl.gz", b"a").unwrap();
        s.put("devices/d1.json", b"1").unwrap();
        s.put("devices/d1.json", b"2").unwrap();
        s.put("backups/neoanki-2026-10-14-0912.zip", b"zip").unwrap();
        assert_eq!(s.get("devices/d1.json").unwrap(), b"2");
        assert_eq!(s.list("changes/").unwrap().iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["changes/d1/00000001.jsonl.gz"]);
        assert_eq!(s.list("backups/").unwrap()[0].path, "backups/neoanki-2026-10-14-0912.zip");
        assert!(dir.join(BACKUP_FOLDER).join("neoanki-2026-10-14-0912.zip").exists(), "백업은 보이는 폴더에");
        s.delete("devices/d1.json").unwrap();
        s.delete("devices/d1.json").unwrap();
        assert!(matches!(s.get("devices/d1.json"), Err(SyncError::NotFound(_))));
        assert!(s.put("../x", b"").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
