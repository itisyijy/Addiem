"""Addiem x 실제 Anki(pylib) 검증 하네스. README.md 참고.

    <venv>\\Scripts\\python.exe -I real_anki.py all --repo <Addiem worktree> --work <빈 작업 폴더>

단계: make (pylib 로 v1 · v2 패키지와 Anki 렌더 기준) → rust phase1 (가져오기 · 덱 업데이트 · 내보내기)
→ anki (Neo 가 내보낸 .apkg 를 Anki 로 가져와 비교, Anki 에서 고치고 공부한 뒤 내보내기) → rust phase2 (다시 가져오기) → report.
판정은 각 단계가 <work>/checks_*.json 에 남긴다. 개인 데이터는 쓰지 않는다 (모두 여기서 만든다).
"""
import argparse, hashlib, html, json, os, re, shutil, struct, subprocess, sys, zipfile, zlib

CHECKS = []


def check(cid, area, name, ok, evidence, info=False):
    """ok: True/False. info: 판정 없이 알림 (설계상 다른 것)"""
    status = "INFO" if info else "PASS" if ok else "FAIL"
    CHECKS.append({"id": cid, "area": area, "name": name, "status": status, "evidence": evidence})


def save_checks(work, phase):
    with open(os.path.join(work, f"checks_{phase}.json"), "w", encoding="utf-8") as f:
        json.dump(CHECKS, f, ensure_ascii=False, indent=1)


def dump(path, obj):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False, indent=1)


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def png(rgb):
    raw = b"".join(b"\x00" + bytes(rgb) * 4 for _ in range(4))
    ch = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + ch(b"IHDR", struct.pack(">IIBBBBB", 4, 4, 8, 2, 0, 0, 0)) + ch(b"IDAT", zlib.compress(raw)) + ch(b"IEND", b"")


SPECIAL_Q = (
    "{{Front}}<div class=qa-meta>"
    "<span class=qa-tags>{{Tags}}</span><span class=qa-deck>{{Deck}}</span><span class=qa-sub>{{Subdeck}}</span>"
    "<span class=qa-card>{{Card}}</span><span class=qa-type>{{Type}}</span></div>"
    "{{#Extra}}<i class=qa-ext>{{Extra}}</i>{{/Extra}}{{^Extra}}<i class=qa-noext>없음</i>{{/Extra}}"
    "<div class=qa-hint>{{hint:Hint}}</div>"
)
SPECIAL_A = "{{FrontSide}}<hr id=answer>{{Back}}<span class=qa-text>{{text:Back}}</span>"
CUSTOM = "QA 특수필드"
ROOT = "QA원본"
PROBES = ["qa-tags", "qa-deck", "qa-sub", "qa-card", "qa-type", "qa-hint", "qa-text"]


# ------------------------------------------------------------------ Anki 쪽 읽기


def anki_mod():
    from anki.collection import Collection, ExportAnkiPackageOptions, DeckIdLimit, ImportAnkiPackageRequest, ImportAnkiPackageOptions
    from anki.scheduler_pb2 import CardAnswer

    return Collection, ExportAnkiPackageOptions, DeckIdLimit, ImportAnkiPackageRequest, ImportAnkiPackageOptions, CardAnswer


def answer(col, cid, rating):
    *_, CardAnswer = anki_mod()
    r = {1: CardAnswer.AGAIN, 2: CardAnswer.HARD, 3: CardAnswer.GOOD, 4: CardAnswer.EASY}[rating]
    c = col.get_card(cid)
    c.start_timer()
    st = col._backend.get_scheduling_states(cid)
    col.sched.answer_card(col.sched.build_answer(card=c, states=st, rating=r))


def card_key(col, c):
    """노트 guid | 카드 종류 이름 | 빈칸이면 ord. Neo 쪽도 같은 키를 쓴다 (qa_real_anki.rs key_of)"""
    m = c.note_type()
    t = m["tmpls"][0 if m["type"] == 1 else c.ord]
    return f"{c.note().guid}|{t['name']}|{c.ord if m['type'] == 1 else ''}"


def snapshot(col, root):
    """루트 덱 아래 노트 · 카드 · 유형 · 덱 · 미디어 · 렌더. Neo 쪽 비교의 기준"""
    cids = col.find_cards(f'"deck:{root}"')
    notes, cards, types, renders = {}, [], {}, {}
    today = col.sched.today
    for cid in cids:
        c = col.get_card(cid)
        n = c.note()
        m = c.note_type()
        notes[n.guid] = {"type": m["name"], "fields": [[k, v] for k, v in n.items()], "tags": sorted(n.tags)}
        types[str(m["id"])] = {
            "name": m["name"],
            "fields": [f["name"] for f in m["flds"]],
            "templates": [[t["name"], t["qfmt"], t["afmt"]] for t in m["tmpls"]],
            "css": m["css"],
            "cloze": m["type"] == 1,
        }
        ms = c.memory_state
        key = card_key(col, c)
        revs = col.db.list("select id from revlog where cid = ? and ease between 1 and 4 order by id", cid)
        cards.append({
            "key": key, "guid": n.guid, "ord": c.ord, "deck": col.decks.name(c.odid or c.did), "type": c.type, "queue": c.queue,
            "due_days": (c.due - today) if c.type == 2 else None, "due_secs": c.due if c.type in (1, 3) and c.queue == 1 else None,
            "ivl": c.ivl, "reps": c.reps, "lapses": c.lapses,
            "s": ms.stability if ms else None, "d": ms.difficulty if ms else None, "revlog": revs, "id": c.id,
        })
        ro = c.render_output(reload=True)
        renders[key] = {"q": ro.question_text, "a": ro.answer_text}
    media = {}
    mdir = col.media.dir()
    for name in sorted(os.listdir(mdir)):
        with open(os.path.join(mdir, name), "rb") as f:
            media[name] = hashlib.sha1(f.read()).hexdigest()
    decks = sorted({c["deck"] for c in cards})
    return {"notes": notes, "cards": cards, "types": types, "renders": renders, "media": media, "decks": decks, "today": today}


# ------------------------------------------------------------------ make: v1 · v2 패키지


def make(work):
    Collection, ExportAnkiPackageOptions, DeckIdLimit, *_ = anki_mod()
    adir = os.path.join(work, "anki")
    shutil.rmtree(adir, ignore_errors=True)
    os.makedirs(adir)
    col = Collection(os.path.join(adir, "src.anki2"))
    col.set_config("fsrs", True)
    col.add_image_occlusion_notetype()
    d = {k: col.decks.id(v) for k, v in {"root": ROOT, "d1": f"{ROOT}::1장", "d11": f"{ROOT}::1장::세부", "d2": f"{ROOT}::2장"}.items()}
    col.media.write_data("qa_red.png", png([255, 0, 0]))
    col.media.write_data("qa_beep.mp3", b"ID3\x03\x00\x00\x00\x00\x00\x0fQA-FAKE-MP3-DATA")
    io_img = os.path.join(work, "qa_io.png")
    with open(io_img, "wb") as f:
        f.write(png([0, 160, 0]))

    # 사용자 정의 유형: 필드 넷, 카드 종류 둘 (B 는 Extra 가 있을 때만), 특수 필드 · 필터 · 조건부
    mm = col.models
    m = mm.new(CUSTOM)
    for f in ["Front", "Back", "Extra", "Hint"]:
        mm.add_field(m, mm.new_field(f))
    t = mm.new_template("카드A")
    t["qfmt"], t["afmt"] = SPECIAL_Q, SPECIAL_A
    mm.add_template(m, t)
    t = mm.new_template("카드B")
    t["qfmt"], t["afmt"] = "{{#Extra}}{{Extra}} <span class=qa-card>{{Card}}</span>{{/Extra}}", "{{FrontSide}}<hr id=answer>{{Front}}"
    t["did"] = d["d2"]  # 카드 종류의 덱 (deck override, NT76): 카드B 는 노트를 어디 넣든 2장으로
    mm.add_template(m, t)
    m["css"] = ".card { font-family: serif; }\n.qa-meta { color: gray; }\n"
    mm.add(m)

    roles = {}

    def add(role, ntype, fields, deck, tags=()):
        n = col.new_note(mm.by_name(ntype))
        for k, v in fields.items():
            n[k] = v
        n.tags = list(tags)
        col.add_note(n, d[deck])
        roles[role] = n.guid
        return n

    notes = {}
    notes["basic1"] = add("basic1", "Basic", {"Front": "사과", "Back": "apple"}, "d1", ["과목::1장", "중요도::높음"])
    notes["basic2"] = add("basic2", "Basic", {"Front": "바나나", "Back": "banana"}, "d1")
    notes["basic3"] = add("basic3", "Basic", {"Front": "소리 <b>굵게</b>", "Back": "[sound:qa_beep.mp3]"}, "d11")
    notes["basic4"] = add("basic4", "Basic", {"Front": "내가 지울 노트", "Back": "x"}, "d2")
    notes["img1"] = add("img1", "Basic", {"Front": 'img <img src="qa_red.png">', "Back": "빨강"}, "d2")
    notes["rev1"] = add("rev1", "Basic (and reversed card)", {"Front": "앞1", "Back": "뒤1"}, "d1")
    notes["rev2"] = add("rev2", "Basic (and reversed card)", {"Front": "앞2", "Back": "뒤2"}, "d2")
    notes["cloze1"] = add("cloze1", "Cloze", {"Text": "{{c1::서울}}은 {{c2::한국}}의 수도, {{c3::한강::강}}", "Back Extra": "추가"}, "d1")
    notes["cloze2"] = add("cloze2", "Cloze", {"Text": "{{c1::A}} {{c1::B}} {{c2::C}}"}, "d11")
    notes["tin1"] = add("tin1", "Basic (type in the answer)", {"Front": "1+1?", "Back": "2"}, "d2")
    notes["cust1"] = add("cust1", CUSTOM, {"Front": "특수1", "Back": "<b>굵은</b> &amp; 뒷면", "Extra": "추가설명", "Hint": "힌트1"}, "d11", ["과목::1장", "특수"])
    notes["cust2"] = add("cust2", CUSTOM, {"Front": "특수2", "Back": "뒷면2"}, "d1")
    io = mm.by_name("Image Occlusion")
    col.add_image_occlusion_note(
        notetype_id=io["id"], image_path=io_img,
        occlusions="{{c1::image-occlusion:rect:left=.1:top=.1:width=.3:height=.2:oi=1}}{{c2::image-occlusion:rect:left=.5:top=.5:width=.3:height=.2:oi=1}}",
        header="그림 헤더", back_extra="그림 추가", tags=["그림"],
    )
    io_nid = max(col.find_notes(f"mid:{io['id']}"))
    io_note = col.get_note(io_nid)
    col.set_deck([c.id for c in io_note.cards()], d["d2"])
    roles["io1"] = io_note.guid

    # FSRS 로 몇 장 공부 (복습 기록 · 기억 상태가 패키지에 실린다)
    cid = lambda role, ord=0: next(c.id for c in col.get_note(notes[role].id).cards() if c.ord == ord)
    answer(col, cid("basic1"), 4)
    answer(col, cid("cloze1", 0), 4)
    answer(col, cid("cloze2", 1), 3)
    answer(col, cid("rev1", 0), 1)
    answer(col, cid("rev1", 0), 3)
    answer(col, cid("cust1", 0), 4)
    answer(col, cid("tin1"), 3)
    col.sched.suspend_cards([cid("rev2", 1)])

    def export(name, legacy):
        p = os.path.join(work, name)
        col.export_anki_package(
            out_path=p,
            options=ExportAnkiPackageOptions(with_scheduling=True, with_deck_configs=True, with_media=True, legacy=legacy),
            limit=DeckIdLimit(deck_id=d["root"]),
        )
        return p

    export("v1_modern.apkg", False)
    leg = export("v1_legacy.apkg", True)
    # 아주 옛 .apkg (collection.anki2 하나, 스키마 11, 미디어 목록 JSON). 최신 Anki 는 만들지 않아 legacy 패키지에서 다시 묶는다
    with zipfile.ZipFile(leg) as src, zipfile.ZipFile(os.path.join(work, "v1_anki2.apkg"), "w", zipfile.ZIP_DEFLATED) as dst:
        for e in src.namelist():
            if e in ("meta", "collection.anki2"):
                continue
            dst.writestr("collection.anki2" if e == "collection.anki21" else e, src.read(e))
    ref = snapshot(col, ROOT)
    ref["roles"] = roles
    ref["fsrs"] = bool(col.get_config("fsrs", False))
    dump(os.path.join(work, "ref_v1.json"), ref)

    # ---- 만든 사람의 v2 (deck-update.md 2장 판정표 · 6 유형 · 3 새/사라진 노트)
    def setf(role, **kv):
        n = col.get_note(notes[role].id)
        for k, v in kv.items():
            n[k.replace("_", " ")] = v
        col.update_note(n)
        return n

    n = setf("basic1", Back="apple (fruit)")  # 만든 사람만 → 새 버전
    n.tags.append("v2")
    col.update_note(n)
    setf("basic2", Front="바나나 🍌")  # 나도 Front 를 고친다 → 충돌
    setf("rev1", Back="뒤1 (v2)")  # 나는 Front → 합침
    col.remove_notes([notes["basic3"].id])  # 새 버전에 없는 노트 (DU11)
    setf("cloze1", Text="{{c2::서울}}은 {{c1::한국}}의 수도, {{c3::한강::강}}")  # 번호만 바꿈 (DU13)
    setf("cloze2", Text="{{c1::A}} {{c1::B}} {{c4::C}}")  # c2 없어짐 · c4 새로
    m = mm.by_name(CUSTOM)
    mm.add_field(m, mm.new_field("Source"))  # 새 필드
    t = mm.new_template("카드C")
    t["qfmt"], t["afmt"] = "{{Front}} (C)", "{{FrontSide}}<hr id=answer>{{Source}}"
    mm.add_template(m, t)  # 새 카드 종류
    mm.update_dict(m)
    setf("cust1", Source="출처값")
    m = mm.by_name("Basic (type in the answer)")
    mm.rename_field(m, next(f for f in m["flds"] if f["name"] == "Back"), "Answer")  # 이름 바뀐 필드
    mm.update_dict(m)
    m = mm.by_name("Cloze")
    m["css"] += "\n.cloze { color: red; }\n"  # 나도 CSS 를 고친다 → 모양 충돌 (DU9)
    mm.update_dict(m)
    m = mm.by_name("Basic")
    m["tmpls"][0]["qfmt"] = "<div class=front>{{Front}}</div>"  # 만든 사람만 → 새 모양
    mm.update_dict(m)
    notes["basic_new"] = add("basic_new", "Basic", {"Front": "새 노트", "Back": "new"}, "d2")
    notes["cloze_new"] = add("cloze_new", "Cloze", {"Text": "{{c1::새}} 빈칸"}, "d1")
    export("v2_modern.apkg", False)
    export("v2_legacy.apkg", True)
    ref2 = snapshot(col, ROOT)
    ref2["roles"] = roles
    dump(os.path.join(work, "ref_v2.json"), ref2)
    col.close()
    sizes = {p: os.path.getsize(os.path.join(work, p)) for p in ["v1_modern.apkg", "v1_legacy.apkg", "v1_anki2.apkg", "v2_modern.apkg"]}
    entries = {p: zipfile.ZipFile(os.path.join(work, p)).namelist() for p in sizes}
    check("P0", "0 준비", "pylib 로 v1 (anki21b · anki21 · anki2) · v2 패키지 만들기", True, json.dumps({"bytes": sizes, "entries": entries}, ensure_ascii=False))
    check("P0-fsrs", "0 준비", "FSRS 켜고 공부한 카드에 기억 상태가 있다", ref["fsrs"] and sum(1 for c in ref["cards"] if c["s"]) >= 4,
          f"fsrs={ref['fsrs']}, 기억 상태 있는 카드 {sum(1 for c in ref['cards'] if c['s'])}장, 복습 기록 {sum(len(c['revlog']) for c in ref['cards'])}건")
    save_checks(work, "make")


# ------------------------------------------------------------------ 렌더 비교


def norm_text(s):
    """화면 글자만: [sound:] · anki:play · Neo 소리 칩 → ♪, 태그 제거, 엔티티 풀기, 빈칸 자리 [...] 로, 공백 접기"""
    s = re.sub(r"\[anki:play:[qa]:\d+\]", "♪", s)
    s = re.sub(r'<span class="snd"[^>]*></span>', "♪", s)
    # Anki IO 템플릿의 [Toggle Masks] 버튼은 Anki 학습 화면 부품이다 (Neo 는 IO 를 직접 그린다, occlusion.rs)
    s = re.sub(r"<style>.*?</style>|<script>.*?</script>|<button[^>]*>.*?</button>", "", s, flags=re.S)
    s = re.sub(r"<br\s*/?>|<hr[^>]*>|</?div[^>]*>", " ", s)
    s = re.sub(r"<[^>]+>", "", s)
    s = html.unescape(s).replace("\xa0", " ")
    s = re.sub(r"\[\s+\]", "[...]", s)
    return " ".join(s.split())


def probe(raw, cls):
    """탐침 칸의 원문. 힌트 칸은 안에 div 가 있어 바깥 </div> 까지 (탐침 뒤에 오는 첫 </div>\\n? 가 아니라 마지막 것)"""
    if cls == "qa-hint":
        m = re.search(r"<div class=qa-hint>(.*)</div>", raw, re.S)
        return None if m is None else m.group(1)
    m = re.search(rf"<span class={cls}>(.*?)</span>", raw, re.S)
    return None if m is None else m.group(1)


def probe_ok(p, anki, neo, hint_value):
    """특수 필드 탐침 판정. 힌트는 Anki 처럼 값이 있고, 처음엔 숨겨져 있고, 눌러 여는 것이 있으면 같다고 본다 (문구 · id 는 다를 수 있다)"""
    if neo is None:
        return False
    if p == "qa-hint":
        if not hint_value:
            return norm_text(neo) == ""
        hidden = re.search(r"display:\s*none|<details|hidden", neo) is not None
        toggle = re.search(r"<a |<summary|<button", neo) is not None
        return hint_value in neo and hidden and toggle
    return html.unescape(anki).strip() == html.unescape(neo).strip()


def type_in(s, fields, side):
    """Anki 의 [[type:필드]] 는 학습 화면이 입력칸(앞) · 정답 비교(뒤)로 바꾼다. 앞은 비우고 뒤는 정답 값으로 본다 (Neo 의 J 서술형과 같은 꼴)"""
    def rep(m):
        name = m.group(1).split(":")[-1]
        return "" if side == "q" else re.sub(r"<[^>]+>", "", dict(fields).get(name, ""))
    return re.sub(r"\[\[type:([^\]]+)\]\]", rep, s)


def compare_renders(cid, area, ref_r, neo_r, label, notes):
    """카드마다 화면 글자 비교. 특수 필드 탐침(qa-*)은 따로 판정한다. Anki 가 기준. notes: guid -> {fields}"""
    keys = sorted(ref_r)
    missing = [k for k in keys if k not in neo_r]
    diffs, probes, empty = [], {}, []
    strip = lambda s: re.sub(r"<span class=qa-(tags|deck|sub|card|type|text)>.*?</span>|<div class=qa-hint>.*</div>", "", s, flags=re.S)
    for k in keys:
        if k not in neo_r:
            continue
        fields = notes.get(k.rsplit("|", 2)[0], {}).get("fields", [])  # guid 에도 | 가 들어갈 수 있다
        for side in "qa":
            a, n = ref_r[k][side], neo_r[k][side]
            for p in PROBES:
                pa = probe(a, p)
                if pa is not None and (side == "q" or p == "qa-text"):
                    probes.setdefault(p, []).append((k, side, pa, probe(n, p), dict(fields).get("Hint", "")))
            na, nn = norm_text(type_in(strip(a), fields, side)), norm_text(strip(n))
            if na != nn:
                (empty if na.startswith("No cloze") else diffs).append({"key": k, "side": side, "anki": na[:160], "neo": nn[:160]})
    check(f"{cid}-cards", area, f"{label}: 렌더할 카드가 모두 있다", not missing, f"Anki 카드 {len(keys)}장, Neo 에 없는 키 {missing[:5]}")
    check(f"{cid}-text", area, f"{label}: 앞 · 뒷면 화면 글자가 Anki 와 같다 (특수 필드 칸 제외)", not diffs,
          f"카드 {len(keys) - len(missing)}장 × 2면 중 다름 {len(diffs)}: " + json.dumps(diffs[:12], ensure_ascii=False))
    if empty:
        check(f"{cid}-empty", area, f"{label}: 빈칸 번호가 없어진 카드 (Neo 는 남겨 학습에서 빼고, Anki 는 'No cloze N found' 카드로 보인다)", False,
              json.dumps(empty[:4], ensure_ascii=False), info=True)
    for p in PROBES:
        rows = probes.get(p, [])
        bad = [r for r in rows if not probe_ok(p, r[2], r[3], r[4])]
        check(f"{cid}-{p}", area, f"{label}: {p} 칸 (Anki 값과 같나)", bool(rows) and not bad,
              f"{len(rows)}면 중 다름 {len(bad)}: " + json.dumps([{"key": r[0], "anki": norm_text(r[2])[:60], "neo": r[3]} for r in bad[:3]], ensure_ascii=False))
    return diffs


# ------------------------------------------------------------------ 상태 비교 (1 가져오기 · 3 내보내기가 같이 쓴다)


def sha1_of(path):
    try:
        with open(path, "rb") as f:
            return hashlib.sha1(f.read()).hexdigest()
    except OSError:
        return "없음"


def compare_snap(cid, area, anki, neo, label, export, plus=False):
    """anki · neo 는 snapshot() · qa_real_anki.rs snapshot() 의 꼴. export 면 Neo 가 원본 (태그는 Anki 쪽이 더 많아도 된다: 기출 태그).
    유형은 Anki 모델 id 로 맞춘다. plus: Anki 가 가져오며 이름이 겹친 유형에 붙인 '+' · '_' 를 떼고 이름을 비교 (Anki 자신의 패키지도 그렇다: E3-control)"""
    an, nn = anki["notes"], neo["notes"]
    miss = sorted(set(an) ^ set(nn))
    fdiff = [(g, nn[g]["fields"], an[g]["fields"]) for g in nn if g in an and nn[g]["fields"] != an[g]["fields"]]
    check(f"{cid}1", area, f"{label}: 노트 수 · guid · 필드 (이름 · 순서 · 값)", not miss and not fdiff,
          f"Anki {len(an)} · Neo {len(nn)} · 한쪽에만 있는 guid {miss[:3]} · 필드 다름 {len(fdiff)}: {json.dumps(fdiff[:2], ensure_ascii=False)}")
    same_tags = (lambda n, a: set(n) <= set(a)) if export else (lambda n, a: set(n) == set(a))
    tdiff = [(g, nn[g]["tags"], an[g]["tags"]) for g in nn if g in an and not same_tags(nn[g]["tags"], an[g]["tags"])]
    check(f"{cid}2", area, f"{label}: 태그", not tdiff, f"노트 {len(nn)}개 중 다름 {len(tdiff)}: {json.dumps(tdiff[:3], ensure_ascii=False)}")
    ty = []
    for mid, t in anki["types"].items():
        x = neo["types"].get(mid)
        if x is None:
            ty.append(f"{t['name']}({mid}): Neo 에 없음")
            continue
        name = t["name"].rstrip("+_") if plus else t["name"]
        for k in ["fields", "templates", "css", "cloze"]:
            # 앱 유형처럼 CSS 가 비어 있으면 내보내기가 Anki 기본 CSS 를 넣는다 (export.rs model())
            if k == "css" and export and not x[k].strip():
                continue
            if t[k] != x[k]:
                ty.append(f"{name}.{k}: anki={json.dumps(t[k], ensure_ascii=False)[:150]} neo={json.dumps(x[k], ensure_ascii=False)[:150]}")
        if name != x["name"]:
            ty.append(f"이름 anki={t['name']!r} neo={x['name']!r}")
    check(f"{cid}3", area, f"{label}: 노트 유형 (모델 id · 이름 · 필드 이름 · 카드 종류 앞뒷면 · CSS · 빈칸 여부)", not ty and set(anki["types"]) == set(neo["types"]),
          "; ".join(ty[:5]) or f"유형 {sorted((t['name'], k) for k, t in anki['types'].items())}")
    nm = {k: sha1_of(v) for k, v in neo["media"].items()}
    md = {k: (anki["media"].get(k), nm.get(k)) for k in set(anki["media"]) | set(nm) if anki["media"].get(k) != nm.get(k)}
    check(f"{cid}4", area, f"{label}: 미디어 파일 (이름 · sha1)", not md, f"Anki {sorted(anki['media'])} · 다름 {json.dumps(md, ensure_ascii=False)}")
    ac = {c["key"]: c for c in anki["cards"]}
    nc = {c["key"]: c for c in neo["cards"]}
    revn = lambda c: len(c["revlog"]) if isinstance(c["revlog"], list) else c["revlog"]
    kind = {"new": 0, "learning": 1, "review": 2, "relearning": 3}
    st_bad, due_bad, fsrs_bad, rev_bad = [], [], [], []
    for k in sorted(set(ac) | set(nc)):
        a, n = ac.get(k), nc.get(k)
        if a is None or n is None:
            st_bad.append((k, "한쪽에만 있음", a is None))
            continue
        ok = a["queue"] == -1 if n["state"] == "suspended" else (kind.get(n["state"]) == a["type"] and a["queue"] != -1)
        if not ok:
            st_bad.append((k, n["state"], a["type"], a["queue"]))
        if a["type"] == 2 and n["state"] in ("review", "suspended") and (a["due_days"], a["ivl"]) != (n["due_days"], max(1, round(n["ivl"]))):
            due_bad.append((k, "anki due/ivl", a["due_days"], a["ivl"], "neo", n["due_days"], n["ivl"]))
        if a["due_secs"] is not None and n["state"] in ("learning", "relearning") and abs(a["due_secs"] - (n["due_secs"] or 0)) > 60:
            due_bad.append((k, "학습 기한 초", a["due_secs"], n["due_secs"]))
        if (a["reps"], a["lapses"]) != (n["reps"], n["lapses"]):
            due_bad.append((k, "reps/lapses", a["reps"], a["lapses"], n["reps"], n["lapses"]))
        sa, sn = (a["s"], a["d"]), (n["s"], n["d"])
        if (sa[0] is None) != (sn[0] is None) or (sa[0] is not None and (abs(sa[0] - sn[0]) > 1e-3 or abs(sa[1] - sn[1]) > 1e-3)):
            fsrs_bad.append((k, "anki", sa, "neo", sn))
        if revn(a) != revn(n):
            rev_bad.append((k, revn(a), revn(n)))
    check(f"{cid}5", area, f"{label}: 카드 상태 (신규 · 학습 · 복습 · 일시 정지)", not st_bad, f"카드 Anki {len(ac)} · Neo {len(nc)}, 다름 {json.dumps(st_bad[:5], ensure_ascii=False)}")
    check(f"{cid}6", area, f"{label}: 기한 (복습 = 오늘부터 일수, 학습 = 초) · 간격 · reps/lapses", not due_bad, f"다름 {json.dumps(due_bad[:5], ensure_ascii=False)}")
    check(f"{cid}7", area, f"{label}: FSRS 기억 상태 (s · d)", not fsrs_bad,
          f"기억 상태 있는 카드 Anki {sum(1 for c in anki['cards'] if c['s'] is not None)} · Neo {sum(1 for c in neo['cards'] if c['s'] is not None)}, 다름 {json.dumps(fsrs_bad[:4], ensure_ascii=False)}")
    ta, tn = sum(revn(c) for c in anki["cards"]), sum(revn(c) for c in neo["cards"])
    check(f"{cid}8", area, f"{label}: 복습 기록 (카드마다 수 · 합계)", not rev_bad and ta == tn, f"Anki {ta}건 · Neo {tn}건, 다름 {rev_bad[:4]}")
    check(f"{cid}9", area, f"{label}: 덱 트리 (카드가 있는 덱)", anki["decks"] == neo["decks"], f"Anki {anki['decks']} · Neo {neo['decks']}")
    compare_renders(f"{cid}R", area, anki["renders"], neo["renders"], f"{label} 렌더 (Anki 기준)", anki["notes"])


# ------------------------------------------------------------------ anki: 1 비교, Neo 내보내기 → Anki, 그다음 Anki 에서 고치고 공부 → 내보내기


def anki_side(work):
    Collection, ExportAnkiPackageOptions, DeckIdLimit, ImportAnkiPackageRequest, ImportAnkiPackageOptions, _ = anki_mod()
    ref1 = load(os.path.join(work, "ref_v1.json"))
    for variant, label in [("modern", "anki21b"), ("legacy", "anki21"), ("anki2", "anki2")]:
        p = os.path.join(work, f"neo_v1_{variant}.json")
        if os.path.exists(p):
            compare_snap(f"I-{variant}-", "1 가져오기", ref1, load(p), f"v1 {label} → Neo", export=False)
    neo = load(os.path.join(work, "neo_state.json"))
    adir = os.path.join(work, "anki")
    p = os.path.join(adir, "fromneo.anki2")
    for x in [p, p.replace(".anki2", ".media")]:
        shutil.rmtree(x, ignore_errors=True) if os.path.isdir(x) else (os.path.exists(x) and os.remove(x))
    col = Collection(p)
    col.set_config("fsrs", True)
    area = "3 내보내기 → Anki"
    try:
        log = col.import_anki_package(ImportAnkiPackageRequest(package_path=os.path.join(work, "neo_export.apkg"),
                                                              options=ImportAnkiPackageOptions(with_scheduling=True, with_deck_configs=True)))
        ok = True
        ev = f"새 노트 {len(log.log.new)} · 갱신 {len(log.log.updated)} · 중복 {len(log.log.duplicate)} · 유형 충돌 {len(log.log.conflicting)} · 첫 필드 일치 {len(log.log.first_field_match)} · 첫 필드 빔 {len(log.log.empty_first_field)}"
    except Exception as e:
        ok, ev = False, repr(e)
    check("E0", area, "Anki 26.9.3 이 Neo 의 .apkg 를 받는다 (일정 포함)", ok, ev)
    if not ok:
        save_checks(work, "anki")
        col.close()
        return
    got = snapshot(col, neo["root"])
    # 대조군: Anki 가 내보낸 v1 을 새 컬렉션에 가져와도 기본 유형 이름에 '+' 가 붙나
    cp = os.path.join(adir, "control.anki2")
    for x in [cp, cp.replace(".anki2", ".media")]:
        shutil.rmtree(x, ignore_errors=True) if os.path.isdir(x) else (os.path.exists(x) and os.remove(x))
    ctl = Collection(cp)
    ctl.import_anki_package(ImportAnkiPackageRequest(package_path=os.path.join(work, "v1_modern.apkg"), options=ImportAnkiPackageOptions(with_scheduling=True)))
    ctl_names = sorted(t["name"] for t in snapshot(ctl, ROOT)["types"].values())
    ctl.close()
    got_names = sorted(t["name"] for t in got["types"].values())
    check("E3-control", area, "Anki 가 기본 유형과 이름이 겹치는 유형에 '+' 를 붙이는 것은 Anki 자신의 패키지에서도 같다 (Neo 탓이 아님)",
          any(t.endswith("+") for t in ctl_names), f"Neo 패키지 → {got_names} · Anki v1 패키지 → {ctl_names}")
    compare_snap("E", area, got, neo, "Neo 내보내기 → Anki", export=True, plus=True)
    # NT76 카드 종류의 덱 (deck override). (1) Neo 패키지 안에서 카드B 의 did = 그 패키지의 2장 덱 id
    with zipfile.ZipFile(os.path.join(work, "neo_export.apkg")) as z:
        raw = z.read("collection.anki2")
    pkg_col = os.path.join(adir, "neo_export_col.anki2")
    with open(pkg_col, "wb") as f:
        f.write(raw)
    import sqlite3
    with sqlite3.connect(pkg_col) as db:
        models_json, decks_json = db.execute("select models, decks from col").fetchone()
    pkg_decks = {v["name"]: int(k) for k, v in json.loads(decks_json).items()}
    pm = next((m for m in json.loads(models_json).values() if m["name"] == CUSTOM), {})
    pdid = next((t.get("did") for t in pm.get("tmpls", []) if t["name"] == "카드B"), None)
    check("E-tdid", area, "카드 종류의 덱(deck override): Neo 가 내보낸 카드B 의 did = 그 패키지의 2장 덱 id (다른 카드 종류는 null)",
          pdid is not None and pdid == pkg_decks.get(f"{neo['root']}::2장") and all(t.get("did") is None for t in pm.get("tmpls", []) if t["name"] != "카드B"),
          f"카드B did={pdid} · 패키지 2장 {pkg_decks.get(neo['root'] + '::2장')} · 템플릿 did {[(t['name'], t.get('did')) for t in pm.get('tmpls', [])]}")
    # (2) 원래 Anki 컬렉션(덱 id 가 같다)으로 돌아가 가져오면 Anki 가 그 덱에 새 카드를 넣는다. 왕복(4)에 섞이지 않게 사본에서
    tp = os.path.join(adir, "tdid.anki2")
    for x in [tp, tp.replace(".anki2", ".media")]:
        shutil.rmtree(x, ignore_errors=True) if os.path.isdir(x) else (os.path.exists(x) and os.remove(x))
    shutil.copyfile(os.path.join(adir, "src.anki2"), tp)
    tc = Collection(tp)
    tc.import_anki_package(ImportAnkiPackageRequest(package_path=os.path.join(work, "neo_export.apkg"), options=ImportAnkiPackageOptions(with_scheduling=True)))
    tm = next((m for m in tc.models.all() if m["name"].rstrip("+") == CUSTOM), None)
    placed = None
    if tm:
        nn = tc.new_note(tm)
        nn["Front"], nn["Back"], nn["Extra"] = "덱 지정 확인", "td", "예문"
        tc.add_note(nn, tc.decks.id_for_name(f"{neo['root']}::1장"))
        placed = sorted((c.ord, tc.decks.name(c.did)) for c in nn.cards())
    check("E-tdid-anki", area, "원래 Anki 컬렉션에 Neo 패키지를 가져온 뒤 1장에 새 노트를 더하면 Anki 도 카드B 를 2장에 넣는다",
          placed is not None and any(o == 1 and dn.endswith("::2장") for o, dn in placed) and any(o == 0 and dn.endswith("::1장") for o, dn in placed),
          f"새 노트의 카드 {placed}")
    tc.close()
    # (3) 참고: 새 컬렉션에 가져오면 Anki 가 덱 id 를 새로 매기고 템플릿 did 는 그대로 둔다 (Anki 자신의 패키지도 같다 → 기본 덱)
    check("E-tdid-fresh", area, "새 컬렉션으로 가져오면 Anki 는 덱 id 만 새로 매기고 카드 종류의 did 는 고치지 않는다 (Anki 패키지도 같다, Neo 와 상관없음)", True,
          "Anki 26.9.3 동작: 패키지 덱은 새 id, notetype 템플릿 did 는 패키지 값 그대로", info=True)

    # ---- 4 왕복: Anki 에서 고치고 공부한 뒤 내보내기
    roles = load(os.path.join(work, "ref_v1.json"))["roles"]
    by_guid = lambda g: col.get_note(col.db.scalar("select id from notes where guid = ?", g))
    n = by_guid(roles["basic1"])
    n["Back"] = "apple (Anki에서 고침)"
    col.update_note(n)
    studied = []
    for role in ["basic1", "basic2"]:
        c = by_guid(roles[role]).cards()[0]
        answer(col, c.id, 3)
        studied.append(card_key(col, c))
    nat = by_guid(neo["native_guid"])  # 앱에서 만든 노트 (Neo 왕복 기준 확인, RT-native)
    nat[list(nat.keys())[1]] = "neo (Anki에서 고침)"
    col.update_note(nat)
    new = col.new_note(col.models.by_name("Basic"))
    new["Front"], new["Back"] = "Anki 새 노트", "rt"
    col.add_note(new, col.decks.id_for_name(f"{neo['root']}::2장"))
    col.export_anki_package(out_path=os.path.join(work, "anki_rt.apkg"),
                            options=ExportAnkiPackageOptions(with_scheduling=True, with_deck_configs=True, with_media=True, legacy=False),
                            limit=DeckIdLimit(deck_id=col.decks.id_for_name(neo["root"])))
    rt = snapshot(col, neo["root"])
    rt.update({"edited_guid": roles["basic1"], "studied": studied, "new_reviews": len(studied), "new_guid": new.guid,
               "native_guid": neo["native_guid"], "native_value": "neo (Anki에서 고침)"})
    dump(os.path.join(work, "rt.json"), rt)
    check("R0", "4 왕복", "Anki 에서 노트 2개 고침 (가져온 것 · 앱에서 만든 것) · 2장 공부 · 새 노트 1개 → anki_rt.apkg (anki21b)", os.path.exists(os.path.join(work, "anki_rt.apkg")),
          f"studied={studied}, new_guid={new.guid}")
    col.close()
    save_checks(work, "anki")


def anki_side2(work):
    """NT76 2차 왕복 (Q6 #1): 1차 왕복한 Anki 컬렉션(tdid.anki2)에 덱만 바꾼 Neo 패키지를 기본 옵션으로 가져오면 카드B 의 did 가 새 덱이고,
    새 노트의 카드B 도 그 덱에 들어간다 (mod 가 그대로면 Anki 는 옛 덱을 쓴다)"""
    Collection, _, _, ImportAnkiPackageRequest, ImportAnkiPackageOptions, _ = anki_mod()
    neo = load(os.path.join(work, "neo_state.json"))
    tp = os.path.join(work, "anki", "tdid.anki2")
    pkg = os.path.join(work, "neo_export2.apkg")
    area = "4 왕복"
    if not (os.path.exists(tp) and os.path.exists(pkg)):
        check("E-tdid-2", area, "덱만 바꾼 뒤 같은 Anki 컬렉션으로 2차 왕복", False, f"tdid.anki2={os.path.exists(tp)} neo_export2={os.path.exists(pkg)}")
        save_checks(work, "anki2")
        return
    tc = Collection(tp)
    tc.import_anki_package(ImportAnkiPackageRequest(package_path=pkg, options=ImportAnkiPackageOptions(with_scheduling=True)))
    tm = next((m for m in tc.models.all() if m["name"].rstrip("+") == CUSTOM), None)
    want = tc.decks.id_for_name(f"{neo['root']}::1장")
    tdid = next((t.get("did") for t in (tm or {}).get("tmpls", []) if t["name"] == "카드B"), None)
    placed = None
    if tm:
        nn = tc.new_note(tm)
        nn["Front"], nn["Back"], nn["Extra"] = "덱 지정 2차", "td2", "예문"
        tc.add_note(nn, tc.decks.id_for_name(f"{neo['root']}::2장"))
        placed = sorted((c.ord, tc.decks.name(c.did)) for c in nn.cards())
    check("E-tdid-2", area, "카드B 의 덱만 2장 → 1장으로 바꾼 Neo 패키지를 1차 왕복한 Anki 컬렉션에 가져오면 카드B did 가 1장이고, 2장에 더한 새 노트의 카드B 도 1장",
          tdid == want and placed is not None and any(o == 1 and dn.endswith("::1장") for o, dn in placed) and any(o == 0 and dn.endswith("::2장") for o, dn in placed),
          f"카드B did={tdid} (1장 {want}), 새 노트의 카드 {placed}")
    tc.close()
    save_checks(work, "anki2")


# ------------------------------------------------------------------ 보고


def report(work):
    rows = []
    for phase in ["make", "rust1", "anki", "rust2", "anki2"]:
        p = os.path.join(work, f"checks_{phase}.json")
        if os.path.exists(p):
            rows += load(p)
        else:
            rows.append({"id": phase, "area": "-", "name": f"{phase} 단계 결과 없음 (앞 단계 실패?)", "status": "FAIL", "evidence": p})
    out = ["| 영역 | id | 확인 | 결과 | 근거 |", "|---|---|---|---|---|"]
    for r in rows:
        ev = r["evidence"].replace("|", "\\|").replace("\n", " ")
        out.append(f"| {r['area']} | {r['id']} | {r['name']} | {r['status']} | {ev[:400]} |")
    counts = {s: sum(1 for r in rows if r["status"] == s) for s in ["PASS", "FAIL", "INFO"]}
    out.append("")
    out.append(f"합계: {counts}")
    text = "\n".join(out)
    with open(os.path.join(work, "report.md"), "w", encoding="utf-8") as f:
        f.write(text + "\n")
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")  # Windows 콘솔(cp949)이 › 같은 글자에서 멈추지 않게
    print(text)
    return counts["FAIL"]


def cargo(repo, test, work, target):
    env = dict(os.environ, NA_QR_WORK=work, CARGO_TARGET_DIR=target, CARGO_BUILD_JOBS=os.environ.get("CARGO_BUILD_JOBS", "4"))
    cmd = ["cargo", "test", "--lib", f"qa_real_anki::{test}", "--", "--ignored", "--exact", "--nocapture", "--test-threads=1"]
    print(">", " ".join(cmd), flush=True)
    r = subprocess.run(cmd, cwd=os.path.join(repo, "src-tauri"), env=env)
    if r.returncode != 0:
        print(f"cargo {test} 실패 (exit {r.returncode}). 판정 파일이 있으면 보고에 남는다", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("step", choices=["make", "anki", "report", "all"])
    ap.add_argument("--work", required=True, help="작업 폴더 (all 은 비우고 시작)")
    ap.add_argument("--repo", help="Addiem worktree (all)")
    ap.add_argument("--target", default=os.path.join(os.environ.get("TEMP", "."), "na-qr-target"), help="CARGO_TARGET_DIR")
    a = ap.parse_args()
    work = os.path.abspath(a.work)
    if a.step == "all":
        if not a.repo:
            sys.exit("--repo 가 필요해요")
        shutil.rmtree(work, ignore_errors=True)
        os.makedirs(work)
        make(work)
        CHECKS.clear()
        cargo(os.path.abspath(a.repo), "phase1", work, a.target)
        if os.path.exists(os.path.join(work, "neo_state.json")):
            anki_side(work)
            CHECKS.clear()
            cargo(os.path.abspath(a.repo), "phase2", work, a.target)
            CHECKS.clear()
            anki_side2(work)
        sys.exit(1 if report(work) else 0)
    os.makedirs(work, exist_ok=True)
    {"make": make, "anki": anki_side, "report": report}[a.step](work)


if __name__ == "__main__":
    main()
