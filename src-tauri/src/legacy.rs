//! 예전 이름(Neo-Anki) 시절 데이터를 새 이름(Addiem)으로 옮긴다. 식별자가 바뀌어 데이터 폴더
//! (%APPDATA% · %LOCALAPPDATA%\<식별자>), 키체인 서비스, DB · 백업 파일 이름이 모두 바뀌었다.
//! 옮기지 못하면 예전 것을 그대로 쓰고 다음에 켤 때 다시 한다. 아무것도 지우지 않는다

use std::path::{Path, PathBuf};

/// 예전 식별자. 데이터 폴더 이름이자 키체인 서비스 이름이었다
pub const OLD_ID: &str = "io.github.itisyijy.neoanki";
pub const OLD_DB_FILE: &str = "neoanki.sqlite";
pub const BACKUP_PREFIX: &str = "addiem-";
const OLD_BACKUP_PREFIX: &str = "neoanki-";

/// new 옆의 예전 식별자 폴더를 new 로 옮긴다 (new 가 없거나 빈 폴더일 때만). 쓸 폴더를 돌려준다:
/// 옮기지 못하면(예전 앱이 켜져 있어 파일이 잠겼다 …) 예전 폴더를 그대로 쓴다
pub fn move_dir(new: &Path) -> PathBuf {
    let old = new.with_file_name(OLD_ID);
    let empty = new.read_dir().map_or(true, |mut d| d.next().is_none());
    if !old.is_dir() || !empty {
        return new.to_path_buf();
    }
    let moved = (!new.exists() || std::fs::remove_dir(new).is_ok()) && std::fs::rename(&old, new).is_ok();
    if moved { new.to_path_buf() } else { old }
}

/// 데이터 폴더 안의 예전 이름 파일. DB 는 -wal · -shm 과 한 벌로 옮기고 하나라도 못 옮기면 되돌린다 (본 파일만 옮기면 WAL 의 변경을 잃는다).
/// 백업 이름은 보기에만 쓰므로 못 바꿔도 그대로 둔다 (backup_rest 가 예전 이름도 읽는다)
pub fn rename_files(dir: &Path) {
    let db = crate::db::FILE;
    if !dir.join(db).exists() && dir.join(OLD_DB_FILE).exists() {
        let mut done = vec![];
        for s in ["-wal", "-shm", ""] {
            let (from, to) = (dir.join(format!("{OLD_DB_FILE}{s}")), dir.join(format!("{db}{s}")));
            if !from.exists() {
                continue;
            }
            if std::fs::rename(&from, &to).is_err() {
                for (from, to) in done.iter().rev() {
                    let _ = std::fs::rename(to, from);
                }
                break;
            }
            done.push((from, to));
        }
    }
    for d in [dir.to_path_buf(), dir.join("backups")] {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(rest) = name.strip_prefix(OLD_BACKUP_PREFIX) {
                let to = d.join(format!("{BACKUP_PREFIX}{rest}"));
                if !to.exists() {
                    let _ = std::fs::rename(e.path(), to);
                }
            }
        }
    }
}

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

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("addiem-legacy-{tag}-{}-{}", std::process::id(), crate::db::now_ms()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn moves_the_old_folder_only_into_a_missing_or_empty_one() {
        let root = tmp("dir");
        let (old, new) = (root.join(OLD_ID), root.join("io.github.itisyijy.addiem"));
        std::fs::create_dir_all(old.join("backups")).unwrap();
        std::fs::write(old.join(OLD_DB_FILE), b"db").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        assert_eq!(move_dir(&new), new, "빈 새 폴더는 예전 폴더로 바꾼다");
        assert!(!old.exists() && new.join(OLD_DB_FILE).exists() && new.join("backups").is_dir());
        // 새 폴더에 이미 데이터가 있으면 예전 폴더를 건드리지 않는다
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(OLD_DB_FILE), b"other").unwrap();
        assert_eq!(move_dir(&new), new);
        assert_eq!(std::fs::read(old.join(OLD_DB_FILE)).unwrap(), b"other");
        // 예전 폴더가 없으면 새 폴더 그대로
        let fresh = root.join("x").join("io.github.itisyijy.addiem");
        assert_eq!(move_dir(&fresh), fresh);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn renames_the_database_with_its_wal_and_the_backups() {
        let dir = tmp("files");
        std::fs::create_dir_all(dir.join("backups")).unwrap();
        for f in [OLD_DB_FILE, "neoanki.sqlite-wal", "neoanki.sqlite-shm", "neoanki-before-restore-20261020-101010.sqlite"] {
            std::fs::write(dir.join(f), f).unwrap();
        }
        for f in ["neoanki-20261020-101010-deck.zip", "neoanki-mine.zip", "addiem-mine.zip", "notes.txt"] {
            std::fs::write(dir.join("backups").join(f), f).unwrap();
        }
        rename_files(&dir);
        assert_eq!(names(&dir), ["addiem-before-restore-20261020-101010.sqlite", "addiem.sqlite", "addiem.sqlite-shm", "addiem.sqlite-wal", "backups"]);
        assert_eq!(std::fs::read(dir.join("addiem.sqlite-wal")).unwrap(), b"neoanki.sqlite-wal", "WAL 도 같이 옮긴다");
        assert_eq!(
            names(&dir.join("backups")),
            ["addiem-20261020-101010-deck.zip", "addiem-mine.zip", "neoanki-mine.zip", "notes.txt"],
            "같은 새 이름이 있으면 그대로 둔다"
        );
        // 이미 새 DB 가 있으면 예전 DB 는 그대로 둔다
        std::fs::write(dir.join(OLD_DB_FILE), b"stale").unwrap();
        rename_files(&dir);
        assert_eq!(std::fs::read(dir.join(OLD_DB_FILE)).unwrap(), b"stale");
        assert_eq!(std::fs::read(dir.join("addiem.sqlite")).unwrap(), OLD_DB_FILE.as_bytes());
        std::fs::remove_dir_all(dir).unwrap();
    }

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
