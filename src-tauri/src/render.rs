//! Anki 템플릿 -> HTML. MVP 6.4.2 의 지원 범위만 다룬다:
//! {{필드}}, {{FrontSide}}, {{#필드}}..{{/필드}}, {{^필드}}..{{/필드}}, {{cloze:필드}}, {{text:필드}}, {{hint:필드}}, {{필터:필드}}.
//! 특수 필드(Tags · Deck · …)는 부르는 쪽이 `special_fields` 를 필드 맵에 넣을 때만 값이 있다.
//! AU20 {{tts 언어:필드}} 는 읽어주기 자리 표시로 바꾼다 (tts_mark).
//! 스파이크 1: 실측 컬렉션의 템플릿은 전부 이 범위 안이었다. 그림 가리기(Anki IO) 칸은 occlusion.rs 가 그린다.
//!
//! 결과 HTML 은 신뢰하지 않는다. 화면에서 반드시 살균한 뒤 넣는다 (ADR 0001 2장).

use std::collections::{BTreeSet, HashMap};

pub struct Rendered {
    pub front: String,
    pub back: String,
}

/// Anki 특수 필드 {{Tags}} {{Deck}} {{Subdeck}} {{Type}} {{Card}} {{CardFlag}} 의 값. 필드 맵에 넣어(같은 이름 필드보다 이긴다, Anki 와 같다) 그린다.
/// 학습 화면(card_view) · 노트 유형 미리보기만 넣는다: 다른 곳(목록 · 품질 · 서술형)은 지금처럼 빈 값. 깃발은 앱에 없어 늘 빈 값 (Anki 의 깃발 없는 카드).
/// 값은 실제 Anki 와 같다 (pylib 26.9.3 으로 확인, 코디네이터 QA): 태그는 공백으로 잇고 앞뒤 공백 없음, 덱은 전체 경로 "A::B", 하위 덱은 마지막 칸
pub fn special_fields(tags: &str, deck_path: &[String], type_name: &str, card_name: &str) -> [(String, String); 6] {
    [
        ("Tags".into(), crate::occlusion::escape(&tags.split_whitespace().collect::<Vec<_>>().join(" "))),
        ("Deck".into(), crate::occlusion::escape(&deck_path.join("::"))),
        ("Subdeck".into(), crate::occlusion::escape(deck_path.last().map_or("", String::as_str))),
        ("Type".into(), crate::occlusion::escape(type_name)),
        ("Card".into(), crate::occlusion::escape(card_name)),
        ("CardFlag".into(), String::new()),
    ]
}

pub fn render_card(qfmt: &str, afmt: &str, fields: &HashMap<String, String>, ord: u32) -> Rendered {
    let front = sound_marks(&render(qfmt, fields, ord, false, ""));
    let back = sound_marks(&render(afmt, fields, ord, true, &front));
    if crate::occlusion::is_template(qfmt) {
        return crate::occlusion::finish(front, back, qfmt, fields, ord);
    }
    Rendered { front, back }
}

/// 소리 자리 표시의 앞부분. 화면(cardHtml.ts)이 소리 칩으로 바꾼다
pub const SOUND_MARK: &str = "<span class=\"snd\" data-snd=\"";

/// AU1 Anki 의 [sound:파일] -> 빈 자리 표시 <span class="snd" data-snd="파일"></span>. 글자가 없어서 글자만 쓰는 곳
/// (목록 · 품질 검사 · AI · 서술형 비교)은 strip_tags 로 그대로 빠진다 (AU6). 이름이 안전하지 않으면 지우고, 닫히지 않은 표기는 글자 그대로
fn sound_marks(html: &str) -> String {
    sounds(html, |name| match crate::commands::safe_media_name(name) {
        Ok(n) => format!("{SOUND_MARK}{}\"></span>", crate::occlusion::escape(n)),
        Err(_) => String::new(),
    })
}

/// AU20 {{tts en_US speed=1.2:필드}} -> <span class="snd" data-tts="en_US" data-text="글자" data-speed="1.2"></span>. 화면이 읽어주기 칩으로 바꾼다.
/// 읽을 글자는 화면 글자만 (AU25: HTML · [sound:] · 수식을 뺀다). 글자는 속성에만 있어 strip_tags 를 쓰는 곳(목록 · 검색 · AI · 서술형 비교)에 섞이지 않는다.
/// voices= 는 따르지 않는다 (AU24). 읽을 글자가 없으면 칩도 없다
fn tts_mark(opts: &str, html: &str) -> String {
    let mut words = opts.split_whitespace();
    let lang = words.next().unwrap_or_default();
    let text = strip_tags(&strip_math(&strip_sound(html)));
    if lang.is_empty() || !text.chars().any(char::is_alphanumeric) {
        return String::new();
    }
    let speed = words.find_map(|w| w.strip_prefix("speed=")?.parse::<f32>().ok()).filter(|s| s.is_finite() && *s > 0.0);
    // [ 도 바꾼다. 뒤에 도는 sound_marks 가 속성 안의 "[sound:" 를 잡지 않게
    let esc = |s: &str| crate::occlusion::escape(s).replace('[', "&#91;");
    let speed = speed.map(|s| format!(" data-speed=\"{s}\"")).unwrap_or_default();
    format!("<span class=\"snd\" data-tts=\"{}\" data-text=\"{}\"{speed}></span>", esc(lang), esc(&text))
}

/// 수식 구간을 지운다 (구분자는 math.ts 와 같다)
fn strip_math(s: &str) -> String {
    let mut out = s.to_string();
    for (open, close) in [("\\(", "\\)"), ("\\[", "\\]"), ("[$$]", "[/$$]"), ("[$]", "[/$]"), ("[latex]", "[/latex]")] {
        while let Some(i) = out.find(open) {
            let Some(j) = out[i + open.len()..].find(close) else { break };
            out.replace_range(i..i + open.len() + j + close.len(), " ");
        }
    }
    out
}

/// [sound:파일] 을 지운다. 템플릿을 거치지 않는 필드 글자(CSV 내보내기, type-in 정답)용
pub fn strip_sound(html: &str) -> String {
    sounds(html, |_| String::new())
}

fn sounds(html: &str, mark: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("[sound:") {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(j) => {
                out.push_str(&mark(&rest[i + 7..i + j]));
                rest = &rest[i + j + 1..];
            }
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 필드가 쓰는 소리 파일 이름 (닫히지 않은 표기는 뺀다)
pub fn sound_names(html: &str) -> impl Iterator<Item = &str> {
    html.split("[sound:").skip(1).filter_map(|s| s.split_once(']').map(|(n, _)| n))
}

fn render(tpl: &str, fields: &HashMap<String, String>, ord: u32, is_back: bool, front_side: &str) -> String {
    let sectioned = sections(tpl, fields);
    let mut out = String::with_capacity(sectioned.len());
    let mut rest = sectioned.as_str();
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start + 2..].find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let tag = rest[start + 2..start + 2 + len].trim();
        out.push_str(&tag_value(tag, fields, ord, is_back, front_side));
        rest = &rest[start + 2 + len + 2..];
    }
    out.push_str(rest);
    out
}

fn tag_value(tag: &str, fields: &HashMap<String, String>, ord: u32, is_back: bool, front_side: &str) -> String {
    if tag == "FrontSide" {
        // 답 면의 앞면 부분 힌트도 펼친 채 (디자인 NT10: 답을 볼 때는 힌트가 열려 있다)
        return front_side.replace("<details class=\"hint\">", "<details class=\"hint\" open>");
    }
    match tag.rsplit_once(':') {
        Some((filters, field)) => {
            let value = fields.get(field.trim()).cloned().unwrap_or_default();
            // type-in 입력칸은 앞면에서 비운다 (답이 새지 않게. 입력은 학습 화면의 J1 칸이 받는다).
            // 뒷면에서는 Anki 처럼 정답을 보여준다
            if filters.split(':').any(|f| f.trim() == "type") {
                return if is_back { value } else { String::new() };
            }
            let has = |name: &str| filters.split(':').any(|f| f.trim() == name);
            let value = if has("cloze") {
                cloze(&value, ord + 1, is_back)
            } else if has("cloze-only") {
                cloze_only(&value, ord + 1, is_back)
            } else {
                value
            };
            // text: 서식을 뺀 글자, 엔티티는 푼다 (Anki 와 같다: `<b>굵은</b> &amp; 뒷면` → `굵은 & 뒷면`). 결과는 화면이 살균한다
            let value = if has("text") { strip_tags(&value) } else { value };
            // hint: Anki 는 스크립트 링크, 여기는 스크립트 없는 네이티브 펼치기 (cardHtml.ts 살균이 details · summary · open 을 받는다).
            // 이름은 필드 이름 ("힌트 보기 · " 는 CSS, 디자인 NT10), 답 면은 펼친 채, 빈 값이면 없음
            let value = if has("hint") && !value.trim().is_empty() {
                format!("<details class=\"hint\"{}><summary>{}</summary>{value}</details>", if is_back { " open" } else { "" }, crate::occlusion::escape(field.trim()))
            } else if has("hint") {
                String::new()
            } else {
                value
            };
            // AU20 읽어주기: Anki 처럼 빈칸을 먼저 푼 뒤 읽는다 (질문 면은 가린 말을 읽지 않는다, AU22)
            match filters.split(':').find_map(|f| f.trim().strip_prefix("tts ")) {
                Some(opts) => tts_mark(opts, &value),
                None => value,
            }
        }
        None => fields.get(tag).cloned().unwrap_or_default(),
    }
}

/// {{#필드}}..{{/필드}} 와 {{^필드}}..{{/필드}}
fn sections(tpl: &str, fields: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = tpl;
    loop {
        let open = [rest.find("{{#"), rest.find("{{^")].into_iter().flatten().min();
        let Some(start) = open else {
            out.push_str(rest);
            return out;
        };
        let inverted = &rest[start + 2..start + 3] == "^";
        let Some(name_len) = rest[start + 3..].find("}}") else {
            out.push_str(rest);
            return out;
        };
        let name = rest[start + 3..start + 3 + name_len].trim();
        let body_start = start + 3 + name_len + 2;
        let close = format!("{{{{/{name}}}}}");
        let Some(body_len) = rest[body_start..].find(&close) else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        // Anki 와 같이 그림만 있는 필드는 비어 있지 않다
        let has = fields.get(name).is_some_and(|v| !strip_tags(v).is_empty() || v.to_ascii_lowercase().contains("<img"));
        if has != inverted {
            out.push_str(&sections(&rest[body_start..body_start + body_len], fields));
        }
        rest = &rest[body_start + body_len + close.len()..];
    }
}

/// {{c1::답}} 또는 {{c1::답::힌트}}. 이 카드의 번호만 가린다
pub fn cloze(text: &str, n: u32, is_back: bool) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some((start, num, answer, hint, end)) = next_cloze(rest) {
        out.push_str(&rest[..start]);
        if let Some(body) = answer.strip_prefix("image-occlusion:") {
            out.push_str(&crate::occlusion::layer(num, body, num == n, is_back));
        } else if num == n {
            if is_back {
                out.push_str(&format!("<span class=\"cloze\">{answer}</span>"));
            } else {
                // C3-b 디자인: 힌트가 없으면 넓은 빈칸으로 보인다
                let shown = hint.map_or("\u{a0}".repeat(6), |h| h.to_string());
                out.push_str(&format!("<span class=\"cloze\">[{shown}]</span>"));
            }
        } else {
            out.push_str(answer);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// {{cloze-only:필드}} (읽어주기용, rslib reveal_cloze_text_only): 이 카드 빈칸의 답만 ", " 로. 질문 면은 힌트, 없으면 "..."
fn cloze_only(text: &str, n: u32, is_back: bool) -> String {
    let mut out = vec![];
    let mut rest = text;
    while let Some((_, num, answer, hint, end)) = next_cloze(rest) {
        if num == n && !answer.starts_with("image-occlusion:") {
            out.push(if is_back { answer } else { hint.unwrap_or("...") });
        }
        rest = &rest[end..];
    }
    out.join(", ")
}

/// 카드를 만드는 빈칸 번호. c0 은 카드가 없다 (Anki IO 의 글자 칸). 번호 - 1 이 template_ord 라 0 이 들어가면 넘친다
pub fn cloze_numbers(text: &str) -> BTreeSet<u32> {
    let mut nums = BTreeSet::new();
    let mut rest = text;
    while let Some((_, num, _, _, end)) = next_cloze(rest) {
        if num > 0 {
            nums.insert(num);
        }
        rest = &rest[end..];
    }
    nums
}

/// 빈칸마다 (번호, 답). 덱 업데이트가 "빈칸 글은 같고 번호만 바뀐 노트"를 알아볼 때 쓴다
pub fn cloze_parts(text: &str) -> Vec<(u32, &str)> {
    let mut out = vec![];
    let mut rest = text;
    while let Some((_, num, answer, _, end)) = next_cloze(rest) {
        out.push((num, answer));
        rest = &rest[end..];
    }
    out
}

/// (시작, 번호, 답, 힌트, 끝)
fn next_cloze(s: &str) -> Option<(usize, u32, &str, Option<&str>, usize)> {
    let mut from = 0;
    while let Some(i) = s[from..].find("{{c") {
        let start = from + i;
        let after = &s[start + 3..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && after[digits.len()..].starts_with("::") {
            let body_start = start + 3 + digits.len() + 2;
            if let Some(len) = s[body_start..].find("}}") {
                let body = &s[body_start..body_start + len];
                // IO 칸의 값에는 \: 로 이스케이프한 :: 가 들어갈 수 있다. Anki 도 힌트를 나누지 않는다
                let (answer, hint) = match body.split_once("::").filter(|_| !body.starts_with("image-occlusion:")) {
                    Some((a, h)) => (a, Some(h)),
                    None => (body, None),
                };
                return Some((start, digits.parse().ok()?, answer, hint, body_start + len + 2));
            }
        }
        from = start + 3;
    }
    None
}

/// 화면에 아무것도 보이지 않는 HTML 인가 (&nbsp;, <br> 만 있는 칸). 그림만 있으면 비어 있지 않다
pub fn is_blank(html: &str) -> bool {
    strip_tags(html).is_empty() && !html.to_ascii_lowercase().contains("<img")
}

/// 목록에 보일 한 줄 글자. 줄을 나누는 태그(div, br, li 등)는 띄어쓰기로 바꾼다
pub fn strip_tags(html: &str) -> String {
    // 템플릿이 붙여 쓴 span 들("<span>구분</span><span>높음</span>")이 한 단어로 붙지 않게 한다
    let spaced = html.replace("</span><span", "</span> <span");
    let html = spaced.as_str();
    const BLOCK: [&str; 16] = ["br", "div", "p", "li", "tr", "td", "th", "hr", "h1", "h2", "h3", "h4", "h5", "h6", "details", "summary"];
    let mut out = String::with_capacity(html.len());
    let mut tag: Option<String> = None;
    for ch in html.chars() {
        match (&mut tag, ch) {
            (None, '<') => tag = Some(String::new()),
            (Some(t), '>') => {
                let name: String = t.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                if BLOCK.contains(&name.to_ascii_lowercase().as_str()) {
                    out.push(' ');
                }
                tag = None;
            }
            (Some(t), c) => t.push(c),
            (None, c) => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn sound_tags_are_removed() {
        assert_eq!(strip_sound("발음 [sound:a.mp3] 끝"), "발음  끝");
        assert_eq!(strip_sound("[sound:a.mp3][sound:b.mp3]"), "");
        assert_eq!(strip_sound("닫히지 않은 [sound:a"), "닫히지 않은 [sound:a");
        assert_eq!(sound_names("[sound:a.mp3] x [sound:b c.ogg][sound:끝").collect::<Vec<_>>(), ["a.mp3", "b c.ogg"]);
    }

    #[test]
    fn sound_tags_become_marks() {
        let mark = |n: &str| format!("<span class=\"snd\" data-snd=\"{n}\"></span>");
        assert_eq!(sound_marks("발음 [sound:a.mp3] 끝"), format!("발음 {} 끝", mark("a.mp3")));
        assert_eq!(sound_marks("[sound:a.mp3][sound:b.mp3]"), format!("{}{}", mark("a.mp3"), mark("b.mp3")), "여러 개는 나온 순서대로");
        assert_eq!(sound_marks(r#"[sound:a"><img src=x onerror=alert(1)>.mp3]"#), mark("a&quot;&gt;&lt;img src=x onerror=alert(1)&gt;.mp3"), "이름은 이스케이프한다");
        for bad in ["[sound:../a.mp3]", r"[sound:C:\a.mp3]", r"[sound:..\a.mp3]", "[sound:a/b.mp3]", "[sound:.a.mp3]", "[sound:]", "[sound:https://x.com/a.mp3]"] {
            assert_eq!(sound_marks(bad), "", "미디어 폴더 밖을 가리키는 이름은 지운다: {bad}");
        }
        assert_eq!(sound_marks("닫히지 않은 [sound:a"), "닫히지 않은 [sound:a");
        // 앞면 소리는 {{FrontSide}} 로 뒷면에도 있다. 글자만 쓰는 곳에는 남지 않는다
        let r = render_card("{{Front}}[sound:{{Audio}}]", "{{FrontSide}}<hr id=answer>{{Back}}", &f(&[("Front", "Q"), ("Audio", "q.mp3"), ("Back", "A [sound:a.mp3]")]), 0);
        assert_eq!(r.front, format!("Q{}", mark("q.mp3")));
        assert_eq!(r.back, format!("Q{}<hr id=answer>A {}", mark("q.mp3"), mark("a.mp3")));
        assert_eq!((strip_tags(&r.front), strip_tags(&r.back)), ("Q".to_string(), "Q A".to_string()));
        assert!(!is_blank(&r.front));
    }

    #[test]
    fn strip_tags_keeps_line_breaks_as_spaces() {
        assert_eq!(strip_tags("<div>1. 삭제</div><div>2. <b>대치</b></div>"), "1. 삭제 2. 대치");
        assert_eq!(strip_tags("A&nbsp;&amp;&lt;B<br>C"), "A &<B C");
        assert_eq!(strip_tags("<img src=\"a.png\">"), "");
        assert_eq!(strip_tags("<span>구분</span><span>높음</span>"), "구분 높음");
        assert!(is_blank("&nbsp;<br>") && is_blank(" <div></div> "));
        assert!(!is_blank("<img src=\"a.png\">") && !is_blank("x"));
    }

    #[test]
    fn basic_and_front_side() {
        let r = render_card("{{Front}}", "{{FrontSide}}<hr id=answer>{{Back}}", &f(&[("Front", "Q"), ("Back", "A")]), 0);
        assert_eq!(r.front, "Q");
        assert_eq!(r.back, "Q<hr id=answer>A");
    }

    #[test]
    fn conditionals() {
        let t = "{{Front}}{{#Extra}}<p>{{Extra}}</p>{{/Extra}}{{^Extra}}없음{{/Extra}}";
        assert_eq!(render_card(t, "", &f(&[("Front", "Q"), ("Extra", "E")]), 0).front, "Q<p>E</p>");
        assert_eq!(render_card(t, "", &f(&[("Front", "Q"), ("Extra", "<br>")]), 0).front, "Q없음", "태그만 있으면 빈 필드다");
    }

    #[test]
    fn cloze_cards() {
        let text = "{{c1::서울}}은 {{c2::한국::나라}}의 수도";
        assert_eq!(cloze_numbers(text).into_iter().collect::<Vec<_>>(), vec![1, 2]);
        let fields = f(&[("Text", text)]);
        let c1 = render_card("{{cloze:Text}}", "{{cloze:Text}}", &fields, 0);
        assert_eq!(c1.front, format!("<span class=\"cloze\">[{}]</span>은 한국의 수도", "\u{a0}".repeat(6)));
        assert_eq!(c1.back, "<span class=\"cloze\">서울</span>은 한국의 수도");
        let c2 = render_card("{{cloze:Text}}", "{{cloze:Text}}", &fields, 1);
        assert_eq!(c2.front, "서울은 <span class=\"cloze\">[나라]</span>의 수도");
    }

    #[test]
    fn tts_tags_become_marks() {
        let mark = |lang: &str, text: &str| format!("<span class=\"snd\" data-tts=\"{lang}\" data-text=\"{text}\"></span>");
        let fields = f(&[("Front", "abandon"), ("Back", "버리다,&nbsp;<b>포기하다</b> [sound:a.mp3] \\(x^2\\)")]);
        let r = render_card("{{Front}}{{tts en_US:Front}}", "{{FrontSide}}<hr id=answer>{{Back}}{{tts ko_KR voices=Heami speed=1.2:Back}}", &fields, 0);
        assert_eq!(r.front, format!("abandon{}", mark("en_US", "abandon")));
        assert!(r.back.ends_with("<span class=\"snd\" data-tts=\"ko_KR\" data-text=\"버리다, 포기하다\" data-speed=\"1.2\"></span>"), "HTML · 소리 · 수식은 읽지 않는다: {}", r.back);
        // AU25 글자만 쓰는 곳에는 읽어주기 글자가 섞이지 않는다
        assert_eq!(strip_tags(&r.front), "abandon");
        assert_eq!(strip_tags(&r.back).matches("포기하다").count(), 1);
        // 이스케이프. [ 는 sound_marks 가 속성 안을 건드리지 않게 바꾼다
        let evil = f(&[("F", "a\"&lt;img src=x&gt; [sound:x")]);
        assert_eq!(render_card("{{tts en_US:F}}", "", &evil, 0).front, mark("en_US", "a&quot;&lt;img src=x&gt; &#91;sound:x"));
        assert_eq!(render_card("{{tts en\"x:F}}", "", &evil, 0).front.matches('"').count(), 6, "언어도 이스케이프한다");
        // 읽을 글자가 없으면 칩이 없다. 빈칸은 이 면에 보이는 대로 (질문 면은 가린 말을 읽지 않는다)
        assert_eq!(render_card("{{tts en_US:F}}", "", &f(&[("F", "<img src=\"a.png\">")]), 0).front, "");
        let cz = f(&[("Text", "{{c1::Seoul}} is the capital {{c2::Korea::country}}")]);
        let c1 = render_card("{{tts en_US:cloze:Text}}", "{{tts en_US:cloze:Text}}", &cz, 0);
        assert_eq!(c1.front, mark("en_US", "&#91; ] is the capital Korea"));
        assert_eq!(c1.back, mark("en_US", "Seoul is the capital Korea"));
        let only = render_card("{{tts en_US:cloze-only:Text}}", "{{tts en_US:cloze-only:Text}}", &cz, 1);
        assert_eq!((only.front.as_str(), only.back), (mark("en_US", "country").as_str(), mark("en_US", "Korea")));
        assert_eq!(render_card("{{tts en_US:cloze-only:Text}}", "", &cz, 0).front, "", "힌트 없는 질문 면은 읽을 것이 없다");
    }

    #[test]
    fn filters_and_missing_fields() {
        assert_eq!(render_card("{{text:Front}}|{{Nope}}", "", &f(&[("Front", "Q")]), 0).front, "Q|");
        // text: 은 서식을 뺀 글자, 엔티티는 푼다 (실제 Anki 와 같다)
        assert_eq!(render_card("{{text:Back}}", "", &f(&[("Back", "<b>굵은</b> &amp; 뒷면")]), 0).front, "굵은 & 뒷면");
        // hint: 은 스크립트 없는 펼치기, 빈 값이면 없음. 빈칸과 함께 쓰면 빈칸을 먼저 푼다
        let h = render_card("{{Front}}{{hint:Extra}}", "", &f(&[("Front", "Q"), ("Extra", "<i>메모</i>")]), 0);
        assert_eq!(h.front, "Q<details class=\"hint\"><summary>Extra</summary><i>메모</i></details>");
        assert_eq!(render_card("{{hint:Extra}}", "", &f(&[("Extra", " ")]), 0).front, "");
        assert_eq!(render_card("{{hint:text:Extra}}", "", &f(&[("Extra", "<b>a</b>")]), 0).front, "<details class=\"hint\"><summary>Extra</summary>a</details>");
        assert_eq!(render_card("", "{{hint:Extra}}", &f(&[("Extra", "a")]), 0).back, "<details class=\"hint\" open><summary>Extra</summary>a</details>", "답 면은 펼친 채");
        let fs = render_card("{{hint:Extra}}", "{{FrontSide}}", &f(&[("Extra", "a")]), 0);
        assert_eq!((fs.front.contains(" open"), fs.back.contains(" open")), (false, true), "답 면의 앞면 부분도 펼친 채");
        assert_eq!(strip_tags(&h.front), "Q Extra 메모");
        assert_eq!(render_card("{{Front", "", &f(&[]), 0).front, "{{Front", "닫히지 않은 태그는 그대로 둔다");
        // type-in: 앞면은 비우고 뒷면은 정답 (J1)
        let t = render_card("{{Front}}{{type:Back}}", "{{FrontSide}}<hr id=answer>{{type:Back}}", &f(&[("Front", "Q"), ("Back", "A")]), 0);
        assert_eq!((t.front.as_str(), t.back.as_str()), ("Q", "Q<hr id=answer>A"));
    }
}
