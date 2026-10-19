//! J1~J3 서술형 (써서 답하기) · G3 학습 중 품질 신호.
//!
//! 서술형 카드 = 앞면 템플릿에 {{type:필드}} 가 있는 카드 (C3 "서술형" 유형 · 가져온 type-in 노트), 또는 카드 편집에서 스위치를 켠 카드.
//! 핵심어 판정은 1단계 규칙: 필수 키워드 + 동의어와 글자 일치 (띄어쓰기 · 대소문자 무시, 키워드 끝 조사 무시).
//! 모델 · 외부 API 는 쓰지 않는다. 평가 추천은 추천일 뿐이고, 복습 일정은 사용자가 고른 평가로만 바뀐다

use crate::commands::{self, AppState};
use crate::db::now_ms;
use crate::{quality, render};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

type Res<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// 앱이 만드는 서술형 노트 유형 (db.rs NATIVE_TYPES). 필드: 질문 · 모범 답안 · 채점 메모
pub const NATIVE: &str = "서술형";

/// J3 필수 키워드 하나. source: user(직접 추가) | suggested(모범 답안에서 뽑은 제안을 눌러 확정)
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Keyword {
    pub word: String,
    #[serde(default)]
    pub synonyms: Vec<String>,
    #[serde(default)]
    pub source: String,
}

// ------------------------------------------------------------------ 핵심어 판정 (순수 함수)

/// 판정에서 지우는 글자. 띄어쓰기 말고도 "k-fold" · "평균·중앙값" 처럼 붙여 쓰거나 띄어 쓰는 기호
const IGNORED: [char; 3] = ['-', '·', '_'];
/// 키워드 끝에 붙여 쓴 조사 (와 "-거나" 같은 이음 어미). 긴 것부터 맞춘다
const PARTICLES: [&str; 25] = [
    "에서는", "으로는", "에서", "으로", "에게", "까지", "부터", "처럼", "보다", "이나", "이라", "와", "과", "을", "를", "이", "가", "은", "는", "의", "에",
    "로", "도", "만", "나",
];
/// 여러 낱말 키워드에서 낱말 사이에 끼어도 되는 글자 수 ("회귀계수가 불안정" 의 "가", "변수를 제거" 의 "를")
const PARTICLE_GAP: usize = 3;

/// 판정용 글자 (소문자, 띄어쓰기 · 기호 없음)와 글자마다 원문 위치 (UTF-16 [시작, 끝). 화면이 JS 문자열 위치로 밑줄을 긋는다)
fn fold(text: &str) -> (Vec<char>, Vec<(usize, usize)>) {
    let (mut chars, mut pos, mut at) = (vec![], vec![], 0);
    for c in text.chars() {
        let w = c.len_utf16();
        if !c.is_whitespace() && !IGNORED.contains(&c) {
            for l in c.to_lowercase() {
                chars.push(l);
                pos.push((at, at + w));
            }
        }
        at += w;
    }
    (chars, pos)
}

/// 조사를 뗀 말. 남는 글자가 2자 이상일 때만 뗀다 ("결과" 의 "과" 는 조사가 아니다)
fn strip_particle(word: &str) -> Option<&str> {
    PARTICLES.iter().find_map(|p| word.strip_suffix(p).filter(|rest| rest.chars().count() >= 2))
}

/// 키워드 꼴 하나 = 낱말들 (판정용 글자). 낱말 사이에는 조사가 끼어도 된다
type Form = Vec<Vec<char>>;

/// 키워드와 동의어의 판정용 꼴. 끝 조사를 뗀 꼴도 넣는다. 긴 꼴부터 (밑줄이 가능한 한 길게)
fn forms(k: &Keyword) -> Vec<Form> {
    let mut out: Vec<Form> = vec![];
    for w in std::iter::once(&k.word).chain(&k.synonyms) {
        let mut parts: Form = w.split_whitespace().map(|p| fold(p).0).filter(|p| !p.is_empty()).collect();
        let Some(last) = parts.last() else { continue };
        let s: String = last.iter().collect();
        if let Some(stem) = strip_particle(&s) {
            let mut stemmed = parts.clone();
            *stemmed.last_mut().unwrap() = stem.chars().collect();
            out.push(stemmed);
        }
        parts.dedup();
        out.push(parts);
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.iter().map(Vec::len).sum::<usize>()));
    out.dedup();
    out
}

/// hay[i..] 에서 꼴이 시작하면 끝 위치. 낱말 사이에 PARTICLE_GAP 글자까지 끼어도 된다
fn match_at(hay: &[char], i: usize, form: &Form) -> Option<usize> {
    let starts = |at: usize, p: &[char]| hay.get(at..at + p.len()) == Some(p);
    let (first, rest) = form.split_first()?;
    if !starts(i, first) {
        return None;
    }
    let mut end = i + first.len();
    for p in rest {
        end = (end..=end + PARTICLE_GAP).find(|&at| starts(at, p))? + p.len();
    }
    Some(end)
}

/// 글에서 키워드가 나온 자리들 (원문 UTF-16 위치). 겹치는 자리는 한 번만
fn locate(folded: &(Vec<char>, Vec<(usize, usize)>), k: &Keyword) -> Vec<(usize, usize)> {
    let (hay, pos) = folded;
    let mut taken: Vec<(usize, usize)> = vec![]; // 판정용 글자 위치
    for f in forms(k) {
        let mut i = 0;
        while i < hay.len() {
            match match_at(hay, i, &f).filter(|&e| !taken.iter().any(|&(s, t)| i < t && s < e)) {
                Some(e) => {
                    taken.push((i, e));
                    i = e;
                }
                None => i += 1,
            }
        }
    }
    taken.sort();
    taken.into_iter().map(|(s, e)| (pos[s].0, pos[e - 1].1)).collect()
}

/// J2 평가 추천: 핵심어 비율 40% 미만 Again, 80% 미만 Hard, 그 이상 Good. Easy 는 추천하지 않는다 (속도는 모른다)
pub(crate) fn suggest(hits: usize, total: usize) -> Option<u8> {
    if total == 0 {
        return None;
    }
    let pct = hits * 100 / total;
    Some(if pct < 40 {
        1
    } else if pct < 80 {
        2
    } else {
        3
    })
}

#[derive(Serialize, Debug)]
pub struct KeywordHit {
    word: String,
    hit: bool,
}

/// J2 비교 결과. 위치는 UTF-16 [시작, 끝)
#[derive(Serialize, Debug)]
pub struct Compare {
    /// 정답 글자 (줄 단위)
    correct: String,
    keywords: Vec<KeywordHit>,
    hits: usize,
    /// 1 Again · 2 Hard · 3 Good. 키워드가 없으면 없다
    pub(crate) suggest: Option<u8>,
    /// 내 답에서 맞은 핵심어 (초록 밑줄)
    mine_hits: Vec<(usize, usize)>,
    /// 정답에서 맞은 핵심어 (초록 밑줄) · 빠진 핵심어 (빨간 굵은 글씨)
    correct_hits: Vec<(usize, usize)>,
    correct_misses: Vec<(usize, usize)>,
}

pub(crate) fn compare(mine: &str, correct: &str, keywords: &[Keyword]) -> Compare {
    let (m, c) = (fold(mine), fold(correct));
    let mut out = Compare {
        correct: correct.to_string(),
        keywords: vec![],
        hits: 0,
        suggest: None,
        mine_hits: vec![],
        correct_hits: vec![],
        correct_misses: vec![],
    };
    for k in keywords.iter().filter(|k| !k.word.trim().is_empty()) {
        let found = locate(&m, k);
        let hit = !found.is_empty();
        out.mine_hits.extend(found);
        if hit {
            out.hits += 1;
            out.correct_hits.extend(locate(&c, k));
        } else {
            out.correct_misses.extend(locate(&c, k));
        }
        out.keywords.push(KeywordHit { word: k.word.clone(), hit });
    }
    out.suggest = suggest(out.hits, out.keywords.len());
    out
}

// ------------------------------------------------------------------ J3 제안 칩 (순수 함수)

/// 키워드로 쓰기에는 흔한 말
const STOP: [&str; 29] = [
    "방법", "경우", "등", "때", "및", "또는", "그리고", "여러", "가지", "모든", "위해", "통해", "대해", "따라", "이상", "이하", "사용", "결과", "다음", "정도",
    "부분", "관련", "대한", "위한", "통한", "있음", "없음", "하나", "현상",
];
/// 용언 어미 (풀이말은 키워드가 아니다). 짧은 어미는 3자 이상 어절에서만 본다 ("제한" · "결함" 은 명사, "채움" 은 아니다)
const VERB_ENDS: [&str; 15] = ["다", "움", "아", "하고", "되고", "하며", "되며", "하면", "되면", "해서", "하여", "하는", "되는", "있는", "없는"];
const VERB_ENDS_LONG: [&str; 10] = ["한", "된", "할", "될", "던", "는", "함", "됨", "음", "게"];
/// 조사를 뗀 뒤에 남은 용언 줄기 ("불안정해지는" → "불안정해지")
const VERB_STEMS: [&str; 7] = ["하", "되", "있", "없", "않", "같", "지"];
/// 1자 말 뒤의 확실한 조사 ("행을" → 버린다)
const SURE_PARTICLES: [char; 10] = ['을', '를', '은', '는', '이', '가', '의', '에', '도', '만'];
/// 낱말 묶음을 끊는 문장 부호
const PUNCT: &str = ",.;:()[]{}\"'·/—–~!?<>=+「」『』…";

/// 어절이 키워드 후보인가 (조사를 뗀 말 word 로 본다)
fn noun_like(raw: &str, word: &str) -> bool {
    let len = word.chars().count();
    if len < 2 || (word == raw && len == 2 && raw.ends_with(SURE_PARTICLES)) {
        return false;
    }
    let verb = VERB_ENDS.iter().any(|e| word.ends_with(e) || raw.ends_with(e))
        || (len >= 3 && VERB_ENDS_LONG.iter().any(|e| word.ends_with(e)))
        || (word != raw && VERB_STEMS.iter().any(|e| word.ends_with(e)));
    !verb && !STOP.contains(&word) && !word.chars().all(|c| c.is_ascii_digit())
}

/// 모범 답안에서 키워드 제안을 뽑는다. 어절에서 조사를 떼고 풀이말 · 흔한 말 · 1자 말 · 숫자를 뺀 말과,
/// 조사 없이 이어진 두 말의 묶음("주성분 회귀")을 센다. 묶음 · 3자 이상 영문 약어(VIF)는 한 번 더 쳐 주고, 묶음에 든 낱말은 따로 내지 않는다.
/// 같은 점수면 묶음 먼저, 그다음 먼저 나온 순.
/// ponytail: 형태소 분석 없이 어절 규칙만이다. 소형 언어 모델(B-3)을 받으면 그쪽 제안이 낫다
pub(crate) fn suggest_from(text: &str, skip: &[String], limit: usize) -> Vec<String> {
    // 이미 정한 말, 그리고 정한 묶음에 든 낱말 ("독립변수 간 상관" 을 정했으면 "상관" 은 내지 않는다)
    let covered: Vec<Vec<char>> = skip.iter().filter(|s| s.contains(' ')).flat_map(|s| s.split_whitespace()).map(|w| fold(w).0).collect();
    let skip: Vec<Vec<char>> = skip.iter().map(|s| fold(s).0).collect();
    let mut seen: Vec<(String, usize, bool)> = vec![]; // (말, 점수, 묶음인가)
    let mut add = |w: String, phrase: bool| {
        let f = fold(&w).0;
        if skip.contains(&f) || (!phrase && covered.contains(&f)) {
            return;
        }
        match seen.iter_mut().find(|(s, _, _)| fold(s).0 == f) {
            Some(e) => e.1 += 1,
            None => {
                let acronym = w.len() >= 3 && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && w.chars().any(|c| c.is_ascii_uppercase());
                seen.push((w, 1 + (phrase || acronym) as usize, phrase));
            }
        }
    };
    for chunk in text.split(|c: char| c == '\n' || PUNCT.contains(c)) {
        let mut prev: Option<&str> = None; // 조사 없이 끝난 앞 말 (묶음 후보)
        for raw in chunk.split_whitespace() {
            let raw = raw.trim_matches(|c: char| !c.is_alphanumeric());
            let (word, particle) = strip_particle(raw).map_or((raw, false), |s| (s, true));
            if !noun_like(raw, word) {
                prev = None;
                continue;
            }
            add(word.to_string(), false);
            if let Some(p) = prev {
                add(format!("{p} {word}"), true);
            }
            prev = (!particle).then_some(word);
        }
    }
    // 안정 정렬이라 같은 점수는 묶음 먼저, 그다음 먼저 나온 순서를 지킨다
    seen.sort_by_key(|(_, n, phrase)| (std::cmp::Reverse(*n), !*phrase));
    let mut picked: Vec<(String, bool)> = vec![];
    for (w, _, phrase) in seen {
        let inside = |p: &str, single: &str| p.split(' ').any(|x| x == single);
        let dup = if phrase {
            picked.iter().any(|(s, ph)| !ph && inside(&w, s))
        } else {
            picked.iter().any(|(p, ph)| *ph && inside(p, &w))
        };
        if !dup {
            picked.push((w, phrase));
        }
        if picked.len() == limit {
            break;
        }
    }
    picked.into_iter().map(|(w, _)| w).collect()
}

// ------------------------------------------------------------------ 저장

/// 노트의 서술형 설정. 행이 없으면 기본값 (템플릿대로 · 키워드 없음)
#[derive(Default)]
struct Setting {
    write: Option<bool>,
    keywords: Vec<Keyword>,
    dismissed: Vec<String>,
}

fn load_setting(con: &Connection, note_id: i64) -> Res<Setting> {
    Ok(con
        .query_row("SELECT write_mode, keywords, dismissed FROM note_written WHERE note_id = ?1", [note_id], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })
        .optional()
        .map_err(err)?
        .map(|(w, k, d)| Setting {
            write: w.map(|w| w == 1),
            keywords: serde_json::from_str(&k).unwrap_or_default(),
            dismissed: serde_json::from_str(&d).unwrap_or_default(),
        })
        .unwrap_or_default())
}

fn save_setting(con: &Connection, note_id: i64, write: bool, keywords: &[Keyword], dismissed: &[String]) -> Res<()> {
    con.execute(
        "INSERT INTO note_written (note_id, write_mode, keywords, dismissed, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(note_id) DO UPDATE SET write_mode = ?2, keywords = ?3, dismissed = ?4, updated_at = ?5",
        params![note_id, write as i64, serde_json::to_string(keywords).map_err(err)?, serde_json::to_string(dismissed).map_err(err)?, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// 앞면 템플릿의 {{type:필드}} (type:cloze:필드 도). 첫 번째 것
fn type_field(qfmt: &str) -> Option<String> {
    let rest = &qfmt[qfmt.find("{{type:")? + 7..];
    Some(rest[..rest.find("}}")?].rsplit(':').next()?.trim().to_string())
}

/// 카드의 서술형 여부 · 정답 글자 (줄 단위) · 노트 설정 · 질문 글자 (8.3 개념 관계 확인이 쓴다)
struct CardInfo {
    written: bool,
    answer: String,
    setting: Setting,
    question: String,
}

fn card_info(con: &Connection, card_id: i64) -> Res<CardInfo> {
    let names = commands::note_field_names(con)?;
    let (note_id, fields, nt, q, a, ord): (i64, String, i64, Option<String>, Option<String>, u32) = con
        .query_row(
            &format!(
                "SELECT c.note_id, n.fields, n.note_type_id, t.qfmt, t.afmt, c.template_ord
                 FROM card c JOIN note n ON n.id = c.note_id
                 LEFT JOIN card_template t ON t.note_type_id = n.note_type_id AND {}
                 WHERE c.id = ?1",
                crate::notetype::TEMPLATE_OF_CARD
            ),
            [card_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .map_err(err)?;
    let setting = load_setting(con, note_id)?;
    let rendered = commands::render_fields(&names, nt, &fields, q.as_deref(), a.as_deref(), ord);
    let typed = q.as_deref().and_then(type_field);
    // type-in 이면 그 필드가 정답이다. 아니면 뒷면에서 답 부분 (앞면을 뺀 것)
    let html = match &typed {
        Some(f) => {
            let values: Vec<String> = serde_json::from_str(&fields).unwrap_or_default();
            // AU6 소리 표기는 정답 글자가 아니다
            names.get(&nt).and_then(|(_, n)| n.iter().position(|x| x == f)).and_then(|i| values.get(i)).map(|v| render::strip_sound(v)).unwrap_or_default()
        }
        None => commands::answer_part(&rendered.back).to_string(),
    };
    // ---- C4 덱 기본값 (덱 담당): 노트 스위치 > 덱 "써서 답하기"(가장 가까운 위 덱까지) > {{type:}} 템플릿
    let deck_writes = || -> Res<bool> {
        let deck: i64 = con.query_row("SELECT deck_id FROM card WHERE id = ?1", [card_id], |r| r.get(0)).map_err(err)?;
        Ok(commands::write_mode_source(con, deck)?.is_some())
    };
    // Q12 목록 · 선택형은 써서 답하기 덱에서도 자기 방식으로 묻는다
    let special = names.get(&nt).is_some_and(|(name, n)| crate::recall::is_special(name, n));
    let written = !special
        && match setting.write {
            Some(w) => w,
            None => deck_writes()? || typed.is_some(),
        };
    // ---- 여기까지
    Ok(CardInfo { written, answer: quality::html_lines(&html).join("\n"), setting, question: render::strip_tags(&rendered.front) })
}

/// R2 채점 보조가 보내는 모범 답안 (J2 비교 화면 오른쪽 칸과 같은 글자)
pub(crate) fn model_answer(con: &Connection, card_id: i64) -> Res<String> {
    Ok(card_info(con, card_id)?.answer)
}

// ------------------------------------------------------------------ 명령: 학습 화면 (J1 · J2)

#[derive(Serialize)]
pub struct WrittenCard {
    /// 써서 답하는 카드인가 (J1 입력 칸)
    written: bool,
    /// 정답 글자 (줄 단위). 비교 화면 오른쪽 칸
    answer: String,
    keywords: Vec<Keyword>,
    /// J1 "지난번에 쓴 답"
    last: Option<String>,
    last_at: Option<i64>,
}

#[tauri::command]
pub fn written_card(state: State<AppState>, card_id: i64) -> Res<WrittenCard> {
    let con = state.db.lock().map_err(err)?;
    let info = card_info(&con, card_id)?;
    let last: Option<(String, i64)> = con
        .query_row("SELECT answer, written_at FROM written_answer WHERE card_id = ?1", [card_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(err)?;
    Ok(WrittenCard {
        written: info.written,
        answer: info.answer,
        keywords: info.setting.keywords,
        last_at: last.as_ref().map(|l| l.1),
        last: last.map(|l| l.0),
    })
}

fn save_answer(con: &Connection, card_id: i64, answer: &str) -> Res<()> {
    // 빈 글은 남기지 않는다. 지난번에 쓴 답이 지워지지 않게
    if answer.trim().is_empty() {
        return Ok(());
    }
    con.execute(
        "INSERT INTO written_answer (card_id, answer, written_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(card_id) DO UPDATE SET answer = ?2, written_at = ?3",
        params![card_id, answer, now_ms()],
    )
    .map_err(err)?;
    Ok(())
}

/// J1 자동 저장
#[tauri::command]
pub fn save_written_answer(state: State<AppState>, card_id: i64, answer: String) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    save_answer(&con, card_id, &answer)
}

/// J2 [답과 비교하기]. 쓴 답도 저장한다
#[tauri::command]
pub fn compare_written(state: State<AppState>, card_id: i64, answer: String) -> Res<Compare> {
    let con = state.db.lock().map_err(err)?;
    save_answer(&con, card_id, &answer)?;
    let info = card_info(&con, card_id)?;
    Ok(compare(&answer, &info.answer, &info.setting.keywords))
}

// ------------------------------------------------------------------ 8.3 개념 관계 확인 (로컬 모델 · 보조 실행 파일, 선택)
// 결과는 비교 화면에 참고로만 보인다. 평가 추천은 핵심어 검사만 쓴다 (디자인 J4, DECISIONS J6)

#[derive(Serialize)]
pub struct Concept {
    relations: Vec<crate::llm::Relation>,
    verdict: crate::llm::Verdict,
    ms: u64,
}

/// 비교 화면이 뜬 뒤 백그라운드로. 관계를 뽑지 못하면 없다. 이 PC 에서 쓸 수 없거나 엔진이 멈추면 오류 (화면이 알리고 핵심어 채점 그대로).
/// 새 확인 · llm_stop · 평가하면 앞의 확인은 멈춘다 (오류 "확인을 멈췄어요.")
#[tauri::command]
pub async fn concept_check(app: tauri::AppHandle, card_id: i64, answer: String) -> Res<Option<Concept>> {
    use tauri::Manager;
    let job = crate::llm::begin();
    let started = std::time::Instant::now();
    let st = app.state::<AppState>();
    let (question, model, keywords) = {
        let con = st.db.lock().map_err(err)?;
        let info = card_info(&con, card_id)?;
        (info.question, info.answer, info.setting.keywords)
    };
    let (dir, bin, mine) = (st.data_dir.clone(), crate::llm::engine_bin(&app), answer.clone());
    let check = tauri::async_runtime::spawn_blocking(move || crate::llm::run(bin.as_deref(), &dir, job, &question, &model, &answer)).await.map_err(err)??;
    Ok(check.map(|mut relations| {
        guard(&mut relations, &keywords, &mine);
        Concept { verdict: crate::llm::verdict(&relations), relations, ms: started.elapsed().as_millis() as u64 }
    }))
}

/// 8.3 가드 (디자인 J4 노트, DECISIONS J16): 핵심어 검사에서 빠진(✗) 핵심어가 든 관계는 모델 판정과 상관없이 "missing"(안 보여요).
/// 낱말 찾기는 핵심어 검사와 같은 규칙 (띄어쓰기 · 대소문자 · 조사 무시, 동의어 포함)
pub(crate) fn guard(relations: &mut [crate::llm::Relation], keywords: &[Keyword], mine: &str) {
    let m = fold(mine);
    let missed: Vec<&Keyword> = keywords.iter().filter(|k| !k.word.trim().is_empty() && locate(&m, k).is_empty()).collect();
    for r in relations.iter_mut() {
        let rel = fold(&r.rel);
        if missed.iter().any(|k| !locate(&rel, k).is_empty()) {
            r.verdict = "missing".into();
        }
    }
}

/// 서술형 카드가 나오면 (답을 쓰는 동안): 엔진을 띄워 모델을 읽고 관계 뽑기 · 판정 앞부분을 미리 한다. 쓸 수 없으면 아무것도 하지 않는다
#[tauri::command]
pub async fn concept_prepare(app: tauri::AppHandle, card_id: i64) -> Res<()> {
    use tauri::Manager;
    let st = app.state::<AppState>();
    let bin = crate::llm::engine_bin(&app);
    if !crate::llm::ready(&st.data_dir) || bin.is_none() || !crate::llm::cpu_ok() {
        return Ok(());
    }
    let (question, model) = {
        let con = st.db.lock().map_err(err)?;
        let info = card_info(&con, card_id)?;
        (info.question, info.answer)
    };
    let dir = st.data_dir.clone();
    // 미리 하기의 실패는 알리지 않는다 (비교할 때 run 이 다시 띄우고 알린다)
    let _ = tauri::async_runtime::spawn_blocking(move || crate::llm::prepare(bin.as_deref(), &dir, &question, &model)).await;
    Ok(())
}

// ------------------------------------------------------------------ 명령: 카드 만들기 · 편집 (J3)

#[derive(Serialize)]
pub struct WrittenNote {
    /// 서술형으로 쓸 수 있는 노트인가 (앱의 서술형 유형 또는 type-in 템플릿). 아니면 편집 창에 J3 칸이 없다
    capable: bool,
    /// 앱의 서술형 유형 (필드 이름을 질문 · 모범 답안 · 채점 메모로 보여준다)
    native: bool,
    /// 공부할 때 직접 써서 답하기
    write: bool,
    /// 모범 답안 필드 번호 (제안 칩을 뽑는 곳)
    answer_field: Option<usize>,
    keywords: Vec<Keyword>,
    dismissed: Vec<String>,
}

#[tauri::command]
pub fn written_note(state: State<AppState>, note_id: i64) -> Res<WrittenNote> {
    let con = state.db.lock().map_err(err)?;
    let ntid: i64 = con.query_row("SELECT note_type_id FROM note WHERE id = ?1", [note_id], |r| r.get(0)).map_err(err)?;
    let t = crate::notetype::load(&con, ntid)?;
    let qfmt = t.templates.iter().map(|t| t.qfmt.as_str()).collect::<Vec<_>>().join(" ");
    let nt = t.name.clone();
    // note_edit 의 values 와 같은 보이는 순서
    let names: Vec<String> = t.visible().into_iter().map(|(_, n)| n).collect();
    let field = type_field(&qfmt);
    let s = load_setting(&con, note_id)?;
    // ---- C4 덱 기본값 (덱 담당): 노트 스위치가 없으면 덱 "써서 답하기", 그다음 템플릿
    let deck_writes = match con.query_row("SELECT deck_id FROM card WHERE note_id = ?1 LIMIT 1", [note_id], |r| r.get::<_, i64>(0)).optional().map_err(err)? {
        Some(deck) => commands::write_mode_source(&con, deck)?.is_some(),
        None => false,
    };
    Ok(WrittenNote {
        capable: field.is_some(),
        native: nt == NATIVE,
        write: s.write.unwrap_or(deck_writes || field.is_some()),
        answer_field: field.and_then(|f| names.iter().position(|n| *n == f)),
        keywords: s.keywords,
        dismissed: s.dismissed,
    })
}

#[tauri::command]
pub fn save_written_note(state: State<AppState>, note_id: i64, write: bool, keywords: Vec<Keyword>, dismissed: Vec<String>) -> Res<()> {
    let con = state.db.lock().map_err(err)?;
    save_setting(&con, note_id, write, &keywords, &dismissed)
}

/// J3 제안 칩. 모범 답안(HTML)에서 뽑고, 이미 정한 키워드와 뺀 제안은 넘긴다
#[tauri::command]
pub fn suggest_keywords(answer: String, exclude: Vec<String>) -> Vec<String> {
    suggest_from(&quality::html_lines(&answer).join("\n"), &exclude, 6)
}

#[derive(Deserialize)]
pub struct WrittenNew {
    deck_id: i64,
    question: String,
    answer: String,
    memo: String,
    importance: Option<i64>,
    write: bool,
    keywords: Vec<Keyword>,
}

/// C3 "서술형" 카드 추가. 노트 id 를 돌려준다
#[tauri::command]
pub fn add_written_card(state: State<AppState>, card: WrittenNew) -> Res<i64> {
    if render::is_blank(&card.question) {
        return Err("질문을 입력해 주세요.".into());
    }
    if render::is_blank(&card.answer) {
        return Err("모범 답안을 입력해 주세요.".into());
    }
    let mut con = state.db.lock().map_err(err)?;
    let tx = con.transaction().map_err(err)?;
    let now = now_ms();
    let ntid = crate::db::native_type_id(&tx, "written").map_err(err)?;
    tx.execute(
        "INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![ntid, serde_json::to_string(&[&card.question, &card.answer, &card.memo]).map_err(err)?, now],
    )
    .map_err(err)?;
    let nid = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO card (note_id, deck_id, template_ord, importance, importance_source, created_at, updated_at)
         VALUES (?1, ?2, 0, ?3, CASE WHEN ?3 IS NULL THEN NULL ELSE 'user' END, ?4, ?4)",
        params![nid, card.deck_id, card.importance, now],
    )
    .map_err(err)?;
    save_setting(&tx, nid, card.write, &card.keywords, &[])?;
    tx.commit().map_err(err)?;
    Ok(nid)
}

// ------------------------------------------------------------------ G3 학습 중 신호

/// 두 번째 Again 으로 접힐 때 토스트 둘째 줄에 붙일 이유 ("답이 10항목이라"). 카드 모양 문제(G1 규칙)가 없으면 없다.
/// 하루 한 번만 보이는 것은 화면이 정한다 (이 기기의 신호라 DB 에 남기지 않는다)
#[tauri::command]
pub fn split_signal(state: State<AppState>, card_id: i64) -> Res<Option<String>> {
    let con = state.db.lock().map_err(err)?;
    Ok(quality::card_flags(&con, card_id)?.into_iter().find(|f| f.is_shape()).map(|f| match f.kind {
        "items" => format!("답이 {}항목이라", f.n),
        "lines" => format!("답이 {}줄이라", f.n),
        _ => format!("한 카드에 물음이 {}개라", f.n),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kw(word: &str, synonyms: &[&str]) -> Keyword {
        Keyword { word: word.into(), synonyms: synonyms.iter().map(|s| s.to_string()).collect(), source: "user".into() }
    }

    fn hits(mine: &str, k: &[Keyword]) -> Vec<bool> {
        compare(mine, "", k).keywords.iter().map(|h| h.hit).collect()
    }

    #[test]
    fn spacing_case_and_particles_are_ignored() {
        let k = [kw("과대적합", &[]), kw("정규화를", &[]), kw("VIF", &[]), kw("k-fold", &[])];
        assert_eq!(hits("과대 적합을 막으려면 L2 정규화 적용, vif 확인, K fold 교차검증", &k), [true, true, true, true]);
        assert_eq!(hits("드롭아웃", &k), [false, false, false, false]);
    }

    #[test]
    fn synonyms_count_as_the_keyword() {
        let k = [kw("다중공선성", &["VIF", "분산팽창계수"])];
        assert_eq!(hits("분산 팽창 계수가 크면 변수를 뺀다", &k), [true]);
        assert_eq!(hits("vif 가 10 이상", &k), [true]);
        assert_eq!(hits("공선성", &k), [false], "일부만 쓴 말은 아니다");
    }

    #[test]
    fn particles_between_words_of_a_phrase() {
        let k = [kw("회귀계수 불안정", &[]), kw("해결 방법", &["변수 제거"])];
        assert_eq!(hits("회귀계수가 불안정해진다. 변수를 제거한다", &k), [true, true]);
        assert!(!hits("회귀계수와 모형이 불안정", &k)[0], "사이에 다른 말이 길게 끼면 아니다");
        let c = compare("회귀계수가 불안정", "", &k);
        assert_eq!(c.mine_hits, [(0, 9)], "끼어든 조사까지 한 덩어리로 밑줄");
    }

    #[test]
    fn ranges_are_utf16_positions_in_the_original() {
        let c = compare("a 과대 적합", "😀과대적합 · 드롭아웃", &[kw("과대적합", &[]), kw("드롭아웃", &[])]);
        assert_eq!(c.mine_hits, [(2, 7)], "띄어 쓴 칸까지 한 줄로");
        assert_eq!(c.correct_hits, [(2, 6)], "이모지는 2칸");
        assert_eq!(c.correct_misses, [(9, 13)]);
        // 긴 꼴(원래 꼴)이 먼저 잡히고 조사를 뗀 꼴과 겹치지 않는다
        let c = compare("정규화를 하고 또 정규화", "", &[kw("정규화를", &[])]);
        assert_eq!(c.mine_hits, [(0, 4), (10, 13)]);
    }

    #[test]
    fn suggestion_thresholds() {
        assert_eq!([(0, 5), (1, 5), (2, 5), (3, 5), (4, 5), (5, 5)].map(|(h, t)| suggest(h, t)), [1, 1, 2, 2, 3, 3].map(Some));
        assert_eq!(suggest(0, 0), None, "키워드가 없으면 추천하지 않는다");
        let c = compare("과대적합", "", &[kw("과대적합", &[]), kw("정규화", &[]), kw("드롭아웃", &[])]);
        assert_eq!((c.hits, c.suggest), (1, Some(1)), "33% 는 Again");
    }

    #[test]
    fn keyword_suggestions_from_the_model_answer() {
        let answer = "1. 단순 삭제 — 결측이 있는 행을 제거\n2. 평균 대치 — 평균·중앙값으로 채움\n3. 다중 대치 — 여러 번 대치한 결과를 결합";
        let s = suggest_from(answer, &[], 10);
        assert_eq!(&s[..3], ["단순 삭제", "평균 대치", "다중 대치"], "이어진 말 묶음이 먼저: {s:?}");
        for bad in ["있는", "행을", "여러", "대치한", "결과", "1", "채움", "평균", "대치"] {
            assert!(!s.iter().any(|w| w == bad), "{bad} 는 키워드가 아니다: {s:?}");
        }
        assert!(s.contains(&"결측".to_string()) && s.contains(&"중앙값".to_string()), "{s:?}");
        // 이미 정한 키워드는 다시 제안하지 않는다 (띄어쓰기 · 대소문자 무시)
        assert!(!suggest_from("VIF 와 분산 팽창 계수", &["vif".into()], 6).contains(&"VIF".to_string()));
        // 형용사 · 용언 줄기는 빼고, 영문 약어는 한 번 더 쳐 준다
        let s = suggest_from("독립변수 간 상관이 높아 회귀계수 추정이 불안정해지는 현상. VIF 10 이상 또는 공차한계 0.1 이하로 판단하고, 변수 제거나 주성분 회귀로 해결한다.", &[], 6);
        assert_eq!(s, ["회귀계수 추정", "변수 제거", "주성분 회귀", "VIF", "독립변수", "상관"]);
        // 정한 묶음에 든 낱말은 따로 내지 않는다
        let text = "독립변수 간 상관이 높아 회귀계수 추정이 불안정해지는 현상";
        assert_eq!(suggest_from(text, &["회귀계수 추정".into(), "독립변수 간 상관".into()], 6), Vec::<String>::new());
    }

    /// Q12: 써서 답하기 덱에서도 목록 · 선택형은 자기 방식으로 묻는다
    #[test]
    fn list_cards_are_not_written_in_write_decks() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('d', 0, 0)", []).unwrap();
        let deck = con.last_insert_rowid();
        con.execute("INSERT INTO deck_config (deck_id, write_mode, updated_at) VALUES (?1, 1, 0)", [deck]).unwrap();
        let basic = crate::commands::NewCard { deck_id: deck, kind: "basic".into(), front: "q".into(), back: "a".into(), importance: None };
        crate::commands::insert_note(&con, &basic, "", None).unwrap();
        let fields = ["q", "a<br>b", "순서대로", ""].map(String::from);
        crate::recall::insert(&con, crate::recall::LIST, deck, &fields, None, "", None).unwrap();
        let ids: Vec<i64> = con.prepare("SELECT id FROM card ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert!(card_info(&con, ids[0]).unwrap().written, "일반 카드는 덱을 따른다");
        assert!(!card_info(&con, ids[1]).unwrap().written);
    }

    /// 8.3 가드: 핵심어 검사에서 빠진 말이 든 관계는 "안 보여요" (d9: 맞아 보여요 1 · 안 보여요 2)
    #[test]
    fn concept_guard_marks_relations_with_missed_keywords() {
        use crate::llm::Relation;
        let ok = |rel: &str| Relation { rel: rel.into(), verdict: "ok".into() };
        let mut rels = vec![ok("클라이언트의 SYN 요청"), ok("서버의 SYN-ACK 응답"), ok("클라이언트의 ACK 연결 성립")];
        let k = [kw("SYN", &[]), kw("SYN-ACK", &[]), kw("ACK", &[])];
        guard(&mut rels, &k, "클라이언트가 SYN을 보내고 서버가 응답한다.");
        assert_eq!(rels.iter().map(|r| r.verdict.as_str()).collect::<Vec<_>>(), ["ok", "missing", "missing"]);
        // 동의어로 쓴 핵심어는 빠진 것이 아니다 · 핵심어가 없으면 그대로
        let mut rels = vec![Relation { rel: "다중공선성 → 회귀계수 불안정".into(), verdict: "reversed".into() }];
        guard(&mut rels, &[kw("다중공선성", &["VIF"])], "vif 가 높으면 계수가 흔들린다");
        assert_eq!(rels[0].verdict, "reversed");
        guard(&mut rels, &[], "");
        assert_eq!(rels[0].verdict, "reversed");
    }

    /// 8.3: 개념 관계 확인에 넘기는 질문은 앞면 글자 ({{type:}} 칸 · 태그 없이)
    #[test]
    fn concept_question_is_the_front_text() {
        let con = crate::db::open_in_memory().unwrap();
        con.execute("INSERT INTO deck (name, created_at, updated_at) VALUES ('d', 0, 0)", []).unwrap();
        let deck = con.last_insert_rowid();
        let ntid = crate::db::native_type_id(&con, "written").unwrap();
        con.execute(
            "INSERT INTO note (note_type_id, fields, created_at, updated_at) VALUES (?1, ?2, 0, 0)",
            params![ntid, serde_json::to_string(&["수요가 <b>늘면</b>?", "가격이 오른다", ""]).unwrap()],
        )
        .unwrap();
        con.execute("INSERT INTO card (note_id, deck_id, template_ord, created_at, updated_at) VALUES (?1, ?2, 0, 0, 0)", params![con.last_insert_rowid(), deck]).unwrap();
        let info = card_info(&con, con.last_insert_rowid()).unwrap();
        assert_eq!(info.answer, "가격이 오른다");
        assert!(info.question.contains("수요가 늘면?") && !info.question.contains('<') && !info.question.contains("{{"), "{}", info.question);
    }

    #[test]
    fn type_field_from_template() {
        assert_eq!(type_field("{{Front}}<br>{{type:Back}}").as_deref(), Some("Back"));
        assert_eq!(type_field("{{type:cloze:Text}}").as_deref(), Some("Text"));
        assert_eq!(type_field("{{Front}}"), None);
    }
}
