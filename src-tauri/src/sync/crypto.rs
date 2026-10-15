//! 해시 · 난수 · 암호로 잠그기 (drive-sync.md 7장).
//!
//! 잠그면 비밀번호에서 Argon2id 로 키(32바이트)를 만들고, Drive 에 올리는 파일마다 XChaCha20-Poly1305 로 암호화한다.
//! 비밀번호와 키는 Drive 에 올리지 않는다. Drive 에는 crypto.json (salt, KDF 매개변수, 키 확인값)만 둔다

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};

/// 암호화한 파일의 머리. 이것이 없으면 평문이다
const MAGIC: &[u8; 4] = b"NAE1";

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("OS 난수를 읽지 못했어요");
    b
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_hex(n: usize) -> String {
    hex(&random_bytes::<32>()[..n.min(32)])
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// HMAC-SHA256 (RFC 2104). 잠근 Drive 의 그림 이름: 평문 해시는 "같은 그림이 있다"는 사실을 드러낸다
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| k.iter().map(|x| x ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(msg).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

/// KDF 매개변수. Drive 의 crypto.json 에 같이 둔다 (기기마다 같은 키가 나오게)
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KdfParams {
    pub salt: String,
    /// KiB
    pub m: u32,
    pub t: u32,
    pub p: u32,
    /// 키 확인값: 키로 암호화한 고정 문장. 비밀번호가 맞는지 Drive 데이터를 받기 전에 안다
    pub check: String,
}

const CHECK: &[u8] = b"neo-anki key check";

pub fn derive_key(password: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<[u8; 32], String> {
    let params = argon2::Params::new(m, t, p, Some(32)).map_err(|e| e.to_string())?;
    let mut out = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// 이 앱이 쓰는 KDF 매개변수 (m KiB, t, p). m = 64MiB, t = 3 (OWASP 권장 범위)
/// Drive 의 crypto.json 은 이것만 받는다: 큰 값을 써 넣으면 비밀번호를 넣을 때마다 메모리를 다 쓰거나 멈춘다
/// ponytail: 값을 바꾸면 옛 앱이 새 crypto.json 을 열지 못한다. 그때는 허용 목록으로
const KDF: (u32, u32, u32) = (64 * 1024, 3, 1);

/// 새 비밀번호: salt 를 만들고 키와 crypto.json 내용을 돌려준다
pub fn new_key(password: &str) -> Result<([u8; 32], KdfParams), String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    let salt = random_bytes::<16>();
    let (m, t, p) = KDF;
    let key = derive_key(password, &salt, m, t, p)?;
    Ok((key, KdfParams { salt: b64.encode(salt), m, t, p, check: b64.encode(seal(&key, CHECK)) }))
}

/// 다른 기기에서 비밀번호를 넣었을 때. 틀리면 None
pub fn open_key(password: &str, kdf: &KdfParams) -> Result<Option<[u8; 32]>, String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;
    if (kdf.m, kdf.t, kdf.p) != KDF {
        return Err("이 앱이 만든 잠금 설정이 아니에요".into());
    }
    let salt = b64.decode(&kdf.salt).map_err(|e| e.to_string())?;
    let key = derive_key(password, &salt, kdf.m, kdf.t, kdf.p)?;
    let check = b64.decode(&kdf.check).map_err(|e| e.to_string())?;
    Ok((unseal(&key, &check).ok().as_deref() == Some(CHECK)).then_some(key))
}

/// 파일 하나 암호화: MAGIC + 24바이트 nonce + 암호문(태그 포함). nonce 는 파일마다 새 난수
pub fn seal(key: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).expect("키 길이 32");
    let nonce = random_bytes::<24>();
    let body = cipher.encrypt(&XNonce::from(nonce), plain).expect("암호화");
    [MAGIC.as_slice(), &nonce, &body].concat()
}

pub fn unseal(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, String> {
    if !is_sealed(data) || data.len() < 4 + 24 + 16 {
        return Err("암호화된 파일이 아니에요".into());
    }
    let cipher = XChaCha20Poly1305::new_from_slice(key).expect("키 길이 32");
    let nonce: [u8; 24] = data[4..28].try_into().expect("24");
    cipher.decrypt(&XNonce::from(nonce), &data[28..]).map_err(|_| "암호가 맞지 않거나 파일이 손상됐어요".to_string())
}

pub fn is_sealed(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_known_vectors() {
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        // RFC 4231 테스트 케이스 2
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // 블록보다 긴 키 (RFC 4231 케이스 6)
        assert_eq!(
            hex(&hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First")),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn seal_round_trip_and_wrong_key() {
        // 테스트는 가벼운 매개변수로 (실제는 64MiB)
        let salt = random_bytes::<16>();
        let key = derive_key("비밀번호", &salt, 8, 1, 1).unwrap();
        assert_eq!(key, derive_key("비밀번호", &salt, 8, 1, 1).unwrap(), "같은 비밀번호 · salt 면 같은 키");
        let sealed = seal(&key, b"hello");
        assert!(is_sealed(&sealed) && sealed != seal(&key, b"hello"), "nonce 가 매번 다르다");
        assert_eq!(unseal(&key, &sealed).unwrap(), b"hello");
        let other = derive_key("틀린 비밀번호", &salt, 8, 1, 1).unwrap();
        assert!(unseal(&other, &sealed).is_err());
        let mut broken = sealed.clone();
        *broken.last_mut().unwrap() ^= 1;
        assert!(unseal(&key, &broken).is_err(), "고친 파일은 열리지 않는다");
    }

    #[test]
    fn open_key_refuses_kdf_params_from_drive() {
        // crypto.json 의 매개변수가 이 앱이 쓰는 값이 아니면 따르지 않는다 (큰 값을 따르면 메모리를 다 쓰거나 멈춘다).
        // 따르는 쪽이면 가벼운 값으로 키를 만들어 "암호가 틀렸다"(Ok(None))가 된다
        for (m, t, p) in [(8, 1, 1), (64 * 1024, 3, 2)] {
            let kdf = KdfParams { salt: "AAAAAAAAAAAAAAAAAAAAAA==".into(), m, t, p, check: String::new() };
            assert!(open_key("비밀번호", &kdf).is_err(), "{m} {t} {p}");
        }
    }
}
