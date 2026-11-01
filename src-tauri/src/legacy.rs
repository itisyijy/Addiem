//! 예전 이름(Neo-Anki) 시절에 만든 백업 파일을 읽는다. 백업 이름 앞(neoanki-)과 zip 안의 DB 이름(neoanki.sqlite)이 달랐다.
//! 데이터 폴더 · DB · 키체인을 옮기던 코드는 v0.1.2 에서 뺐다 (v0.1.0 부터 새 이름이라 옮길 것이 남지 않았다)

pub const OLD_DB_FILE: &str = "neoanki.sqlite";
pub const BACKUP_PREFIX: &str = "addiem-";
const OLD_BACKUP_PREFIX: &str = "neoanki-";

/// 백업 파일 이름에서 앞(addiem- · 예전 neoanki-)을 뗀 나머지
pub fn backup_rest(name: &str) -> Option<&str> {
    name.strip_prefix(BACKUP_PREFIX).or_else(|| name.strip_prefix(OLD_BACKUP_PREFIX))
}

/// 백업 zip 안의 DB. 예전 이름으로 만든 백업도 읽는다
pub fn zip_db<R: std::io::Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Option<zip::read::ZipFile<'_, R>> {
    let name = if zip.index_for_name(crate::db::FILE).is_some() { crate::db::FILE } else { OLD_DB_FILE };
    zip.by_name(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_names_and_zips_from_either_name() {
        assert_eq!(backup_rest("addiem-2026-10-14-0912.zip"), Some("2026-10-14-0912.zip"));
        assert_eq!(backup_rest("neoanki-2026-10-14-0912.zip"), Some("2026-10-14-0912.zip"));
        assert_eq!(backup_rest("mine.zip"), None);
        for inner in [crate::db::FILE, OLD_DB_FILE] {
            let mut out = std::io::Cursor::new(Vec::new());
            let mut w = zip::ZipWriter::new(&mut out);
            w.start_file(inner, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, b"db").unwrap();
            w.finish().unwrap();
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(out.into_inner())).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut zip_db(&mut z).unwrap(), &mut s).unwrap();
            assert_eq!(s, "db", "{inner}");
        }
    }
}
