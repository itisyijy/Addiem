"""Anki 패키지 -> Addiem 스키마 참조 적재기.

목적은 두 가지다.
  1. db/schema.sql 이 실제 컬렉션을 담을 수 있는지 검증한다
  2. Rust 구현이 그대로 옮겨 쓸 매핑 규칙을 확정한다

이건 스파이크 산출물이지 제품 코드가 아니다. 실제 앱은 ADR 0001에 따라 Rust로 만든다.
여기서 확정한 매핑과 함정 처리는 docs/engineering/spike-01-apkg-parsing.md 에 정리되어 있다.

사용법:
    python tools/import_reference.py <파일.colpkg|apkg|json> [출력.sqlite]
    python tools/import_reference.py --self-test
"""

import hashlib
import json
import os
import re
import sqlite3
import sys
import time
import zipfile

SCHEMA = os.path.join(os.path.dirname(__file__), "..", "db", "schema.sql")
SEP = "\x1f"  # Anki가 덱 계층과 노트 필드를 나누는 구분자

# 중요도: 0=보충 1=일반 2=핵심
BOOST, NORMAL, CORE = 0, 1, 2

# 태그 -> 중요도. 순위를 보존한다 (실측 컬렉션의 중요도:: 3단계와 우리 3단계가 같다)
TAG_RULES = [
    (re.compile(r"^중요도::(최우선|매우높음)$"), CORE),
    (re.compile(r"^중요도::높음$"), NORMAL),
    (re.compile(r"^중요도::(보통|낮음)$"), BOOST),
    (re.compile(r"^star[45]$"), CORE),
    (re.compile(r"^star3$"), CORE),
    (re.compile(r"^star2$"), NORMAL),
    (re.compile(r"^star1$"), BOOST),
    (re.compile(r"^(highlight|중요|핵심|important)$", re.I), CORE),
]

# 덱 이름 -> 중요도 (MVP 6.5.1 3순위)
DECK_CORE = re.compile(r"(요약|핵심|정리|기출|필수|summary|core|essential)", re.I)
DECK_BOOST = re.compile(r"(부록|참고|심화|보충|appendix|extra|advanced)", re.I)

# 지원하지 않는 템플릿 기능 (MVP 6.4.2)
UNSUPPORTED = [
    (re.compile(r"<script", re.I), "JavaScript"),
    (re.compile(r"\{\{type:"), "type-in 입력"),
    (re.compile(r"\{\{tts"), "TTS"),
]
CLOZE_RE = re.compile(r"\{\{cloze:")


# ------------------------------------------------------------------ protobuf

def pb_fields(buf):
    """protobuf 최상위 필드를 훑는다 -> [(번호, 와이어타입, 값)].

    Anki의 config 블롭에서 필요한 건 문자열 몇 개뿐이라 전체 스키마가 필요 없다.
    """
    out, i, n = [], 0, len(buf)
    while i < n:
        key, shift = 0, 0
        while i < n:
            b = buf[i]; i += 1
            key |= (b & 0x7F) << shift; shift += 7
            if not b & 0x80:
                break
        num, wt = key >> 3, key & 7
        if wt == 0:
            v, shift = 0, 0
            while i < n:
                b = buf[i]; i += 1
                v |= (b & 0x7F) << shift; shift += 7
                if not b & 0x80:
                    break
            out.append((num, wt, v))
        elif wt == 2:
            ln, shift = 0, 0
            while i < n:
                b = buf[i]; i += 1
                ln |= (b & 0x7F) << shift; shift += 7
                if not b & 0x80:
                    break
            out.append((num, wt, buf[i:i + ln])); i += ln
        elif wt == 5:
            out.append((num, wt, buf[i:i + 4])); i += 4
        elif wt == 1:
            out.append((num, wt, buf[i:i + 8])); i += 8
        else:
            break  # 알 수 없는 와이어 타입. 더 읽으면 쓰레기가 나온다
    return out


def pb_str(buf, field_no, default=""):
    for num, wt, v in pb_fields(buf or b""):
        if num == field_no and wt == 2:
            return v.decode("utf-8", "replace")
    return default


# ------------------------------------------------------------------ 패키지 열기

def open_collection(pkg_path, work_dir):
    """패키지에서 collection SQLite를 꺼낸다.

    함정 1: zip 자체는 무압축이고 각 항목이 zstd다. zip만 풀면 SQLite로 안 열린다
    함정 2: collection.anki2 는 구버전 경고용 스텁일 수 있다. anki21b 가 우선이다
    """
    import zstandard

    z = zipfile.ZipFile(pkg_path)
    names = set(z.namelist())
    for entry in ("collection.anki21b", "collection.anki21", "collection.anki2"):
        if entry in names:
            break
    else:
        raise SystemExit(f"collection을 찾을 수 없다: {sorted(names)[:5]}")

    raw = z.read(entry)
    if raw[:4] == b"\x28\xb5\x2f\xfd":  # zstd 매직
        raw = zstandard.ZstdDecompressor().decompress(raw, max_output_size=1 << 30)
    if raw[:15] != b"SQLite format 3":
        raise SystemExit(f"{entry}: SQLite가 아니다")

    out = os.path.join(work_dir, "_collection.sqlite")
    with open(out, "wb") as f:
        f.write(raw)

    src = sqlite3.connect(out)
    # 함정 3: Anki가 런타임에 등록하는 콜레이션이라 DB 파일에 정의가 없다.
    # 읽기 전용이므로 대소문자 무시 비교면 충분하다
    src.create_collation("unicase", lambda a, b: (a.lower() > b.lower()) - (a.lower() < b.lower()))
    return src, z, entry


# ------------------------------------------------------------------ 중요도

def importance_from_tags(tags):
    """가장 높은 중요도가 이긴다. 여러 태그가 붙을 수 있다."""
    best, reason = None, None
    for tag in tags.split():
        for rx, level in TAG_RULES:
            if rx.match(tag):
                if best is None or level > best:
                    best, reason = level, f"태그 '{tag}'"
                break
    return best, reason


def importance_from_deck_name(name):
    if DECK_CORE.search(name):
        return CORE, f"덱 이름에 핵심 키워드 포함"
    if DECK_BOOST.search(name):
        return BOOST, f"덱 이름에 보충 키워드 포함"
    return None, None


# ------------------------------------------------------------------ 적재

def build_target(path):
    if os.path.exists(path):
        os.remove(path)
    con = sqlite3.connect(path)
    with open(SCHEMA, encoding="utf-8") as f:
        con.executescript(f.read())
    return con


def run_import(pkg_path, out_path, work_dir, verbose=True):
    src, zf, entry = open_collection(pkg_path, work_dir)
    dst = build_target(out_path)
    now = int(time.time() * 1000)
    stats = {"issues": []}

    kind = "colpkg" if pkg_path.lower().endswith(".colpkg") else "apkg"
    batch = dst.execute(
        "INSERT INTO import_batch (source_kind, source_name, started_at) VALUES (?,?,?)",
        (kind, os.path.basename(pkg_path), now),
    ).lastrowid

    # ---- 덱. 이름이 \x1f 로 계층이다. 부모부터 만든다
    deck_map = {}          # anki deck id -> our id
    path_map = {}          # 전체 경로 튜플 -> our id
    rows = sorted(src.execute("SELECT id, name FROM decks"), key=lambda r: r[1].count(SEP))
    for adid, aname in rows:
        parts = aname.split(SEP)
        parent = None
        for depth in range(len(parts)):
            key = tuple(parts[: depth + 1])
            if key in path_map:
                parent = path_map[key]
                continue
            leaf = parts[depth]
            imp, reason = importance_from_deck_name(leaf)
            did = dst.execute(
                """INSERT INTO deck (parent_id, name, created_at, updated_at,
                                     importance, importance_source, importance_reason, anki_deck_id)
                   VALUES (?,?,?,?,?,?,?,?)""",
                (parent, leaf, now, now,
                 imp if imp is not None else NORMAL,
                 "rule" if imp is not None else "default",
                 reason, adid if depth == len(parts) - 1 else None),
            ).lastrowid
            path_map[key] = did
            parent = did
        deck_map[adid] = parent

    # ---- 노트 타입. 이름이 아니라 템플릿이 쓰는 기능으로 지원 여부를 판정한다
    nt_map, tmpl_ok = {}, {}
    for antid, name, cfg in src.execute("SELECT id, name, config FROM notetypes"):
        flds = [r[0] for r in src.execute(
            "SELECT name FROM fields WHERE ntid=? ORDER BY ord", (antid,))]
        tmpls = list(src.execute(
            "SELECT ord, name, config FROM templates WHERE ntid=? ORDER BY ord", (antid,)))

        bad = []
        for _, _, tcfg in tmpls:
            body = pb_str(tcfg, 1) + "\n" + pb_str(tcfg, 2)
            for rx, label in UNSUPPORTED:
                if rx.search(body) and label not in bad:
                    bad.append(label)
            if CLOZE_RE.search(body) and "Cloze" not in bad:
                bad.append("Cloze")

        ntid = dst.execute(
            """INSERT INTO note_type (name, fields, css, created_at, updated_at,
                                      is_supported, unsupported_note, anki_model_id)
               VALUES (?,?,?,?,?,?,?,?)""",
            (name, json.dumps(flds, ensure_ascii=False), pb_str(cfg, 3),
             now, now, 0 if bad else 1, ", ".join(bad) or None, antid),
        ).lastrowid
        nt_map[antid] = ntid
        tmpl_ok[antid] = not bad
        if bad:
            stats["issues"].append(("skipped", "unsupported_template", f"{name}: {', '.join(bad)}"))

        for ord_, tname, tcfg in tmpls:
            dst.execute(
                """INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt)
                   VALUES (?,?,?,?,?)""",
                (ntid, ord_, tname, pb_str(tcfg, 1), pb_str(tcfg, 2)),
            )

    # ---- 노트
    note_map, note_tags = {}, {}
    skipped_notes = 0
    for anid, guid, mid, mod, tags, flds in src.execute(
            "SELECT id, guid, mid, mod, tags, flds FROM notes"):
        if not tmpl_ok.get(mid, False):
            skipped_notes += 1
            continue
        nid = dst.execute(
            """INSERT INTO note (note_type_id, fields, tags, created_at, updated_at,
                                 anki_note_id, anki_guid, import_batch_id)
               VALUES (?,?,?,?,?,?,?,?)""",
            (nt_map[mid], json.dumps(flds.split(SEP), ensure_ascii=False),
             tags.strip(), anid, mod * 1000, anid, guid, batch),
        ).lastrowid
        note_map[anid] = nid
        note_tags[anid] = tags.strip()

    # ---- 카드
    # 함정 4: 필터 덱에 올라간 카드는 did가 임시 덱이다. 원래 덱은 odid에 있다
    ANKI_STATE = {0: "new", 1: "learning", 2: "review", 3: "relearning"}
    filtered = 0
    for (acid, nid_a, did, ord_, mod, ctype, queue, due, ivl,
         factor, reps, lapses, odid) in src.execute(
            """SELECT id, nid, did, ord, mod, type, queue, due, ivl,
                      factor, reps, lapses, odid FROM cards"""):
        if nid_a not in note_map:
            continue
        real_did = odid if odid else did
        if odid:
            filtered += 1
        our_deck = deck_map.get(real_did) or deck_map.get(did)
        if our_deck is None:
            stats["issues"].append(("warning", "missing_deck", f"카드 {acid}: 덱 {real_did} 없음"))
            continue

        state = "suspended" if queue == -1 else ANKI_STATE.get(ctype, "new")
        imp, reason = importance_from_tags(note_tags.get(nid_a, ""))
        dst.execute(
            """INSERT INTO card (note_id, deck_id, template_ord, state, interval_days,
                                 reps, lapses, importance, importance_source, importance_reason,
                                 created_at, updated_at, anki_card_id)
               VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)""",
            (note_map[nid_a], our_deck, ord_, state, float(ivl or 0),
             reps or 0, lapses or 0,
             imp, "tag" if imp is not None else None, reason,
             acid, mod * 1000, acid),
        )

    # ---- 복습 이력
    # 카드가 없는 기록(Anki에서 삭제된 카드)도 card_id=NULL 로 보존한다.
    # 학습 히트맵과 "그날 얼마나 공부했나"는 카드가 사라져도 사실로 남아야 한다.
    # 실측 컬렉션에서 7,745건 중 1,310건(17%)이 여기 해당했다
    card_by_anki = dict(dst.execute("SELECT anki_card_id, id FROM card"))
    RATING_OK = {1, 2, 3, 4}
    revlog_n = orphan_n = 0
    for rid, cid, ease, ivl, last_ivl, taken, rtype in src.execute(
            "SELECT id, cid, ease, ivl, lastIvl, time, type FROM revlog"):
        if ease not in RATING_OK:
            continue  # 수동 재조정 등. 평가가 아니다
        our = card_by_anki.get(cid)
        if our is None:
            orphan_n += 1
        dst.execute(
            """INSERT INTO review_log (card_id, reviewed_at, rating, elapsed_ms,
                                       prev_interval_days, next_interval_days, anki_revlog_id)
               VALUES (?,?,?,?,?,?,?)""",
            (our, rid, ease, taken, float(last_ivl or 0), float(ivl or 0), rid),
        )
        revlog_n += 1

    # ---- 미디어
    #
    # `media` 항목은 zstd -> protobuf 이고 구조가 단순하다:
    #     repeated field 1 { 1: 파일명(string), 2: 원본 크기(varint), 3: sha1(bytes) }
    # N번째 엔트리가 zip 안의 "N" 항목과 대응한다.
    #
    # 실제 파일명이 있어야 카드 HTML의 <img src="foo.png"> 가 해석된다.
    # 번호로만 저장하면 미디어가 전부 깨진다
    media_n, size_mismatch = 0, 0
    try:
        import zstandard
        d = zstandard.ZstdDecompressor()
        entries = []
        if "media" in zf.namelist():
            mraw = zf.read("media")
            if mraw[:4] == b"\x28\xb5\x2f\xfd":
                mraw = d.decompress(mraw, max_output_size=1 << 26)
            for num, wt, v in pb_fields(mraw):
                if num == 1 and wt == 2:
                    inner = {n: x for n, w, x in pb_fields(v)}
                    entries.append((
                        inner.get(1, b"").decode("utf-8", "replace") if isinstance(inner.get(1), bytes) else "",
                        inner.get(2, 0),
                    ))

        for idx, key in enumerate(sorted((n for n in zf.namelist() if n.isdigit()), key=int)):
            blob = zf.read(key)
            if blob[:4] == b"\x28\xb5\x2f\xfd":
                blob = d.decompress(blob, max_output_size=1 << 28)
            if idx < len(entries) and entries[idx][0]:
                fname, expect = entries[idx]
                if expect and expect != len(blob):
                    size_mismatch += 1
            else:
                fname = f"unnamed-{key}"
                stats["issues"].append(
                    ("warning", "media_name", f"미디어 {key}: 이름을 찾지 못해 번호로 저장했다"))
            dst.execute(
                """INSERT OR IGNORE INTO media (filename, rel_path, sha256, byte_size, created_at, anki_media_key)
                   VALUES (?,?,?,?,?,?)""",
                (fname, f"media/{fname}", hashlib.sha256(blob).hexdigest(), len(blob), now, key),
            )
            media_n += 1
        if size_mismatch:
            stats["issues"].append(
                ("warning", "media_size",
                 f"미디어 {size_mismatch}건의 크기가 매핑과 다르다. 순서 대응이 틀렸을 수 있다"))
    except Exception as e:  # 미디어 실패가 카드 적재를 막으면 안 된다
        stats["issues"].append(("warning", "media", f"미디어 처리 실패: {e}"))

    for sev, kind_, detail in stats["issues"]:
        dst.execute(
            "INSERT INTO import_issue (batch_id, severity, kind, detail) VALUES (?,?,?,?)",
            (batch, sev, kind_, detail))

    counts = {t: dst.execute(f"SELECT count(*) FROM {t}").fetchone()[0]
              for t in ("deck", "note_type", "card_template", "note", "card", "review_log", "media")}
    dst.execute(
        """UPDATE import_batch SET finished_at=?, status=?, deck_count=?, note_count=?,
                                   card_count=?, media_count=?, skipped_count=? WHERE id=?""",
        (int(time.time() * 1000), "partial" if stats["issues"] else "ok",
         counts["deck"], counts["note"], counts["card"], counts["media"], skipped_notes, batch))
    dst.commit()

    counts.update(skipped_notes=skipped_notes, filtered_cards=filtered,
                  issues=len(stats["issues"]), revlog_in=revlog_n, orphan_revlog=orphan_n)
    if verbose:
        report(src, dst, counts, pkg_path, entry)
    return dst, counts


def report(src, dst, c, pkg, entry):
    def n(q, db=src):
        return db.execute(q).fetchone()[0]
    print(f"\n{'='*64}\n  {os.path.basename(pkg)}  ({entry})\n{'='*64}")
    print(f"{'':>22}{'원본':>10}{'적재':>10}")
    for label, q, key in (("덱", "SELECT count(*) FROM decks", "deck"),
                          ("노트타입", "SELECT count(*) FROM notetypes", "note_type"),
                          ("노트", "SELECT count(*) FROM notes", "note"),
                          ("카드", "SELECT count(*) FROM cards", "card"),
                          ("복습기록", "SELECT count(*) FROM revlog", "review_log")):
        o = n(q)
        mark = "" if o == c[key] else "   <-- 차이"
        print(f"  {label:<20}{o:>10,}{c[key]:>10,}{mark}")
    print(f"  {'미디어':<20}{'':>10}{c['media']:>10,}")
    print(f"\n  필터 덱에서 원래 덱으로 되돌린 카드: {c['filtered_cards']:,}")
    print(f"  미지원 노트타입으로 건너뛴 노트:     {c['skipped_notes']:,}")

    print("\n  중요도 분포 (카드 기준, 덱 상속 포함)")
    for imp, cnt, srcs in dst.execute(
            """SELECT eff_importance, count(*), group_concat(DISTINCT eff_importance_source)
               FROM card_effective GROUP BY eff_importance ORDER BY eff_importance DESC"""):
        print(f"    {['보충','일반','핵심'][imp]:<6}{cnt:>8,}   출처: {srcs}")
    got = dst.execute("SELECT count(*) FROM card WHERE importance IS NOT NULL").fetchone()[0]
    tot = dst.execute("SELECT count(*) FROM card").fetchone()[0]
    print(f"    태그로 카드 단위 지정된 카드: {got:,} / {tot:,} ({got*100//max(tot,1)}%)")

    iss = list(dst.execute("SELECT severity, kind, detail FROM import_issue"))
    print(f"\n  보고할 항목 {len(iss)}건")
    for s, k, d in iss[:10]:
        print(f"    [{s}] {d}")


# ------------------------------------------------------------------ JSON 가져오기

IMPORTANCE_WORDS = {"핵심": CORE, "core": CORE, "일반": NORMAL, "normal": NORMAL,
                    "보충": BOOST, "extra": BOOST}


def parse_json_cards(doc):
    """JSON 문서 -> (카드 목록, 기본 덱, 노트타입, 문제 목록).

    docs/engineering/json-import-format.md 가 이 함수의 명세다.
    항목 하나가 깨져도 전체를 버리지 않는다.
    """
    issues = []
    if isinstance(doc, list):
        raw, default_deck, note_type = doc, None, "Basic"
    elif isinstance(doc, dict):
        raw = doc.get("cards")
        default_deck = doc.get("deck")
        note_type = doc.get("note_type") or "Basic"
        if not isinstance(raw, list):
            return [], None, note_type, [("skipped", "bad_root", "cards 가 배열이 아니다")]
    else:
        return [], None, "Basic", [("skipped", "bad_root", "최상위가 배열도 객체도 아니다")]

    out, seen_front = [], {}
    for i, item in enumerate(raw, 1):
        if not isinstance(item, dict):
            issues.append(("skipped", "bad_item", f"#{i} 항목이 객체가 아니다"))
            continue
        front, back = item.get("front"), item.get("back")
        for key, val in (("front", front), ("back", back)):
            if val is None:
                issues.append(("skipped", "missing_field", f"#{i} {key} 키가 없다"))
                break
            if not isinstance(val, str):
                issues.append(("skipped", "bad_field", f"#{i} {key} 이 문자열이 아니다"))
                break
            if not val.strip():
                issues.append(("skipped", "empty_field", f"#{i} {key} 이 비어 있다"))
                break
        else:
            tags = item.get("tags") or []
            if isinstance(tags, str):
                tags = tags.split()
            tags = " ".join(str(t) for t in tags)

            imp = None
            if "importance" in item:
                imp = IMPORTANCE_WORDS.get(str(item["importance"]).strip().lower())
                if imp is None:
                    imp = IMPORTANCE_WORDS.get(str(item["importance"]).strip())
                if imp is None:
                    issues.append(("warning", "bad_importance",
                                   f"#{i} importance 값 '{item['importance']}' 를 알 수 없다. "
                                   f"태그로 추론했다"))

            if front in seen_front:
                issues.append(("warning", "duplicate",
                               f"#{i} front 가 #{seen_front[front]} 과 같다"))
            else:
                seen_front[front] = i

            out.append({"front": front, "back": back,
                        "deck": item.get("deck") or default_deck or "가져온 카드",
                        "tags": tags, "importance": imp, "index": i})
    return out, default_deck, note_type, issues


def run_json_import(json_path, out_path, verbose=True):
    dst = build_target(out_path)
    now = int(time.time() * 1000)
    with open(json_path, encoding="utf-8") as f:
        doc = json.load(f)
    cards, _, note_type_name, issues = parse_json_cards(doc)

    batch = dst.execute(
        "INSERT INTO import_batch (source_kind, source_name, started_at) VALUES ('json',?,?)",
        (os.path.basename(json_path), now)).lastrowid

    ntid = dst.execute(
        """INSERT INTO note_type (name, fields, created_at, updated_at)
           VALUES (?,'["Front","Back"]',?,?)""", (note_type_name, now, now)).lastrowid
    dst.execute("""INSERT INTO card_template (note_type_id, ord, name, qfmt, afmt)
                   VALUES (?,0,'Card 1','{{Front}}','{{FrontSide}}<hr>{{Back}}')""", (ntid,))

    path_map = {}

    def ensure_deck(path):
        parts = [p for p in path.split("::") if p.strip()] or ["가져온 카드"]
        parent = None
        for depth in range(len(parts)):
            key = tuple(parts[: depth + 1])
            if key not in path_map:
                imp, reason = importance_from_deck_name(parts[depth])
                path_map[key] = dst.execute(
                    """INSERT INTO deck (parent_id, name, created_at, updated_at,
                                         importance, importance_source, importance_reason)
                       VALUES (?,?,?,?,?,?,?)""",
                    (parent, parts[depth], now, now,
                     imp if imp is not None else NORMAL,
                     "rule" if imp is not None else "default", reason)).lastrowid
            parent = path_map[key]
        return parent

    for c in cards:
        did = ensure_deck(c["deck"])
        nid = dst.execute(
            """INSERT INTO note (note_type_id, fields, tags, created_at, updated_at, import_batch_id)
               VALUES (?,?,?,?,?,?)""",
            (ntid, json.dumps([c["front"], c["back"]], ensure_ascii=False),
             c["tags"], now, now, batch)).lastrowid

        imp, reason, src = c["importance"], "JSON의 importance 값", "user"
        if imp is None:
            imp, reason = importance_from_tags(c["tags"])
            src = "tag" if imp is not None else None
        dst.execute(
            """INSERT INTO card (note_id, deck_id, template_ord, importance,
                                 importance_source, importance_reason, created_at, updated_at)
               VALUES (?,?,0,?,?,?,?,?)""",
            (nid, did, imp, src if imp is not None else None,
             reason if imp is not None else None, now, now))

    for sev, kind, detail in issues:
        dst.execute("INSERT INTO import_issue (batch_id, severity, kind, detail) VALUES (?,?,?,?)",
                    (batch, sev, kind, detail))
    skipped = sum(1 for s, _, _ in issues if s == "skipped")
    dst.execute("""UPDATE import_batch SET finished_at=?, status=?, deck_count=?, note_count=?,
                                           card_count=?, skipped_count=? WHERE id=?""",
                (int(time.time() * 1000), "partial" if skipped else "ok",
                 len(path_map), len(cards), len(cards), skipped, batch))
    dst.commit()

    if verbose:
        bar = "=" * 50
        print(f"\n{bar}\n  {os.path.basename(json_path)}\n{bar}")
        print(f"  가져옴   {len(cards):>6,}장")
        print(f"  건너뜀   {skipped:>6,}장")
        print(f"  경고     {len(issues)-skipped:>6,}건")
        print(f"  덱       {len(path_map):>6,}개")
        for sev, _, detail in issues[:10]:
            print(f"    [{sev}] {detail}")
    return dst, cards, issues


# ------------------------------------------------------------------ 자체 검사

def self_test():
    """스키마와 매핑 규칙의 핵심 불변식을 검사한다. 패키지 파일 없이 돈다."""
    import tempfile
    ok = 0

    # 중요도 태그 매핑
    assert importance_from_tags("중요도::최우선 과목::1")[0] == CORE
    assert importance_from_tags("중요도::높음")[0] == NORMAL
    assert importance_from_tags("중요도::보통")[0] == BOOST
    assert importance_from_tags("과목::1 unit_question")[0] is None
    # 여러 태그가 붙으면 가장 높은 것이 이긴다
    assert importance_from_tags("star1 중요도::최우선")[0] == CORE
    ok += 5

    # 덱 이름 규칙
    assert importance_from_deck_name("3장 요약")[0] == CORE
    assert importance_from_deck_name("부록")[0] == BOOST
    assert importance_from_deck_name("4단원")[0] is None
    ok += 3

    # JSON 가져오기 형식 (docs/engineering/json-import-format.md)
    cs, _, nt, iss = parse_json_cards([{"front": "a", "back": "b"}])
    assert len(cs) == 1 and not iss and nt == "Basic", "최소 형태가 동작해야 한다"
    cs, _, _, iss = parse_json_cards({"deck": "D", "cards": [{"front": "a", "back": "b"}]})
    assert cs[0]["deck"] == "D"
    # 항목 하나가 깨져도 나머지는 들어온다
    cs, _, _, iss = parse_json_cards([{"front": "a", "back": "b"}, {"front": "x"},
                                      {"back": "y"}, {"front": "z", "back": "  "}, "문자열"])
    assert len(cs) == 1, "성공한 것만"
    assert len(iss) == 4, "실패한 것은 전부 보고"
    assert all("#" in d for _, _, d in iss), "몇 번째인지 알려야 한다"
    # 중복 front는 버리지 않고 경고만
    cs, _, _, iss = parse_json_cards([{"front": "a", "back": "1"}, {"front": "a", "back": "2"}])
    assert len(cs) == 2 and any(k == "duplicate" for _, k, _ in iss)
    # importance 명시값과 오타
    cs, _, _, _ = parse_json_cards([{"front": "a", "back": "b", "importance": "핵심"}])
    assert cs[0]["importance"] == CORE
    cs, _, _, iss = parse_json_cards([{"front": "a", "back": "b", "importance": "몰라"}])
    assert cs[0]["importance"] is None and any(k == "bad_importance" for _, k, _ in iss)
    # tags는 배열도 문자열도 받는다
    assert parse_json_cards([{"front": "a", "back": "b", "tags": ["x", "y"]}])[0][0]["tags"] == "x y"
    assert parse_json_cards([{"front": "a", "back": "b", "tags": "x y"}])[0][0]["tags"] == "x y"
    # 최상위가 엉뚱하면 전체 실패
    assert parse_json_cards("문자열")[0] == []
    assert parse_json_cards({"cards": "배열아님"})[0] == []
    ok += 13

    # protobuf 리더
    # field1="hi"(0x0a 0x02 h i), field8=varint 1 (0x40 0x01)
    assert pb_str(b"\x0a\x02hi\x40\x01", 1) == "hi"
    assert pb_str(b"\x0a\x02hi", 2, "none") == "none"
    assert pb_fields(b"") == []
    ok += 3

    # 스키마가 적재되고 제약이 살아 있는가
    # Windows는 열린 SQLite 파일을 지우지 못한다. 아래에서 con.close() 한다
    with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as td:
        p = os.path.join(td, "t.sqlite")
        con = build_target(p)
        now = 1
        d = con.execute("INSERT INTO deck (name,created_at,updated_at) VALUES ('d',?,?)",
                        (now, now)).lastrowid
        nt = con.execute("""INSERT INTO note_type (name,fields,created_at,updated_at)
                            VALUES ('Basic','["F","B"]',?,?)""", (now, now)).lastrowid
        nid = con.execute("""INSERT INTO note (note_type_id,fields,created_at,updated_at)
                             VALUES (?,'["a","b"]',?,?)""", (nt, now, now)).lastrowid
        cid = con.execute("""INSERT INTO card (note_id,deck_id,template_ord,created_at,updated_at)
                             VALUES (?,?,0,?,?)""", (nid, d, now, now)).lastrowid

        # 카드 중요도가 NULL이면 덱에서 상속한다
        con.execute("UPDATE deck SET importance=2 WHERE id=?", (d,))
        assert con.execute("SELECT eff_importance FROM card_effective").fetchone()[0] == 2
        # 개별 지정이 상속을 덮는다
        con.execute("UPDATE card SET importance=0 WHERE id=?", (cid,))
        assert con.execute("SELECT eff_importance FROM card_effective").fetchone()[0] == 0
        ok += 2

        # 잘못된 값은 거부된다
        for bad_sql, args in (
            ("UPDATE card SET importance=5 WHERE id=?", (cid,)),
            ("UPDATE card SET state='wat' WHERE id=?", (cid,)),
            ("INSERT INTO review_log (card_id,reviewed_at,rating) VALUES (?,1,9)", (cid,)),
        ):
            try:
                con.execute(bad_sql, args)
                raise AssertionError(f"제약이 동작하지 않는다: {bad_sql}")
            except sqlite3.IntegrityError:
                ok += 1

        # 카드를 지워도 복습 기록은 남는다 (통계가 소급해서 바뀌면 안 된다)
        con.execute("PRAGMA foreign_keys=ON")
        con.execute("INSERT INTO review_log (card_id,reviewed_at,rating) VALUES (?,1,3)", (cid,))
        con.execute("DELETE FROM card WHERE id=?", (cid,))
        assert con.execute("SELECT count(*) FROM review_log").fetchone()[0] == 1
        assert con.execute("SELECT card_id FROM review_log").fetchone()[0] is None
        ok += 2

        # 노트를 지우면 카드도 함께 사라진다
        con.execute("""INSERT INTO card (note_id,deck_id,template_ord,created_at,updated_at)
                       VALUES (?,?,1,?,?)""", (nid, d, now, now))
        con.execute("DELETE FROM note WHERE id=?", (nid,))
        assert con.execute("SELECT count(*) FROM card").fetchone()[0] == 0
        ok += 1
        con.close()

    print(f"자체 검사 통과: {ok}건")


def main():
    if "--self-test" in sys.argv:
        return self_test()
    if len(sys.argv) < 2:
        return print(__doc__)
    pkg = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else "addiem-imported.sqlite"
    if pkg.lower().endswith(".json"):
        run_json_import(pkg, out)
    else:
        run_import(pkg, out, os.path.dirname(os.path.abspath(out)) or ".")
    print(f"\n  -> {out}")


if __name__ == "__main__":
    main()
