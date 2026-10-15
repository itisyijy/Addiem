//! Anki 템플릿 -> HTML. MVP 6.4.2 의 지원 범위만 다룬다:
//! {{필드}}, {{FrontSide}}, {{#필드}}..{{/필드}}, {{^필드}}..{{/필드}}, {{cloze:필드}}, {{필터:필드}}.
//! 스파이크 1: 실측 컬렉션의 템플릿은 전부 이 범위 안이었다. 그림 가리기(Anki IO) 칸은 occlusion.rs 가 그린다.
//!
//! 결과 HTML 은 신뢰하지 않는다. 화면에서 반드시 살균한 뒤 넣는다 (ADR 0001 2장).

use std::collections::{BTreeSet, HashMap};

pub struct Rendered {
    pub front: String,
    pub back: String,
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
        return front_side.to_string();
    }
    match tag.rsplit_once(':') {
        Some((filters, field)) => {
            let value = fields.get(field.trim()).cloned().unwrap_or_default();
            // type-in 입력칸은 앞면에서 비운다 (답이 새지 않게. 입력은 학습 화면의 J1 칸이 받는다).
            // 뒷면에서는 Anki 처럼 정답을 보여준다. 읽어주기는 지원하지 않는다 (가져오기 결과에 보고한다)
            if filters.split(':').any(|f| f.trim() == "type") {
                return if is_back { value } else { String::new() };
            }
            if filters.split(':').any(|f| f.trim().starts_with("tts")) {
                return String::new();
            }
            if filters.split(':').any(|f| f.trim() == "cloze") {
                cloze(&value, ord + 1, is_back)
            } else {
                value
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
    const BLOCK: [&str; 14] = ["br", "div", "p", "li", "tr", "td", "th", "hr", "h1", "h2", "h3", "h4", "h5", "h6"];
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
    fn filters_and_missing_fields() {
        assert_eq!(render_card("{{text:Front}}|{{Nope}}", "", &f(&[("Front", "Q")]), 0).front, "Q|");
        assert_eq!(render_card("{{Front", "", &f(&[]), 0).front, "{{Front", "닫히지 않은 태그는 그대로 둔다");
        // type-in: 앞면은 비우고 뒷면은 정답 (J1)
        let t = render_card("{{Front}}{{type:Back}}", "{{FrontSide}}<hr id=answer>{{type:Back}}", &f(&[("Front", "Q"), ("Back", "A")]), 0);
        assert_eq!((t.front.as_str(), t.back.as_str()), ("Q", "Q<hr id=answer>A"));
    }
}
