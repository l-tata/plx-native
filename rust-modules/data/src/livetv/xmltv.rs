//! **XMLTV**, read as a stream: the guide Tunarr publishes at `/api/xmltv.xml`.
//!
//! Two layers, both small and both strict about what they accept:
//!
//! * [`Tokens`] — a pull tokenizer for the well-formed XML subset a guide uses: elements,
//!   attributes, text, the five named entities plus numeric references, CDATA, comments, the
//!   declaration and a DOCTYPE. No namespaces, no DTD processing, no external entities (an
//!   `&name;` it does not know stays literal text, so nothing is ever fetched or expanded).
//! * [`parse`] — the XMLTV walk over it: every `<channel>` (id, display names, icon) and the
//!   `<programme>`s that overlap a time window, with the fields a guide draws (title, episode
//!   title, description, episode number, categories, year, icon). Everything else — credits, ratings,
//!   images, star ratings — is skipped without being kept.
//!
//! The repository has no XML dependency on purpose: every Plex read is JSON, and a guide is a
//! fixed small schema, so a few hundred lines of tokenizer cost less than a vendored crate and the
//! `make build-bench` table a dependency change owes. A document is the whole body in memory
//! (Tunarr's default 12-hour guide is small), and the walk keeps only the window the caller asks
//! for, so a long horizon costs parse time, not resident memory.

/// One channel of the guide.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuideChannel {
    /// The XMLTV `id` (Tunarr's is `C{number}.{code}.tunarr.com`).
    pub id: String,
    /// Every `<display-name>`, in document order. Tunarr writes `"{number} {name}"`, `"{number}"`
    /// and `"{name}"`; the bare number is what joins a lineup entry to this channel.
    pub display_names: Vec<String>,
    /// The channel logo URL, empty when absent.
    pub icon: String,
}

/// One airing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Programme {
    /// The `<channel>` id this airing belongs to.
    pub channel: String,
    /// Start and stop, milliseconds since the epoch. `stop` is `start` when the document gave none
    /// (the next airing then bounds it — see `guide::Lineup::join`).
    pub start_ms: i64,
    pub stop_ms: i64,
    pub title: String,
    /// The episode's own title (`<sub-title>`), empty for a film.
    pub sub_title: String,
    pub desc: String,
    /// `S01E02`-style, from `episode-num system="onscreen"`, else built from `xmltv_ns`.
    pub episode: String,
    /// The first `<category>`, empty when absent.
    pub category: String,
    /// Every `<category>`, in document order (the first is [`Self::category`]), at most
    /// [`MAX_CATEGORIES`]. A guide often leads with a generic word (`Series`) and names the genre
    /// second, so the genre a cell is coloured by is read across all of them.
    pub categories: Vec<String>,
    /// The year from `<date>` (`2019`, `20190514`), when the guide gives one — what tells two films
    /// of one title apart when the airing is looked up in Plex.
    pub year: Option<u16>,
    /// Artwork URL (`<icon src>`), empty when absent.
    pub icon: String,
}

/// How many `<category>` elements one programme keeps.
pub const MAX_CATEGORIES: usize = 6;

/// A parsed guide: every channel, and the airings that overlap the requested window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guide {
    pub channels: Vec<GuideChannel>,
    pub programmes: Vec<Programme>,
    /// The UTC offset (seconds east) the first timestamp that declared one was written in — the
    /// guide server's own zone when it writes local times, as Tunarr does.
    pub source_offset_s: Option<i32>,
}

/// Why a document was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not UTF-8.
    NotUtf8,
    /// The XML is malformed at this byte offset.
    Malformed(usize),
    /// The root element is not `<tv>`.
    NotXmltv,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotUtf8 => f.write_str("guide is not UTF-8"),
            Error::Malformed(at) => write!(f, "guide XML is malformed at byte {at}"),
            Error::NotXmltv => f.write_str("document is not an XMLTV guide"),
        }
    }
}

/// Bounds on what one document may make us keep. A guide past these is truncated, not refused:
/// the channels and airings before the bound are still a usable guide.
const MAX_CHANNELS: usize = 4_000;
const MAX_PROGRAMMES: usize = 100_000;
/// A text field longer than this is cut at a character boundary (a description is prose for one
/// panel, and nothing a guide draws needs more).
const MAX_TEXT: usize = 4_096;

/// Parse an XMLTV document, keeping the airings that overlap `[from_ms, to_ms)`.
pub fn parse(bytes: &[u8], from_ms: i64, to_ms: i64) -> Result<Guide, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::NotUtf8)?;
    let mut tokens = Tokens::new(text);
    let mut guide = Guide::default();
    let mut root_seen = false;
    // What we are inside, as far as the walk cares.
    let mut channel: Option<GuideChannel> = None;
    let mut programme: Option<Programme> = None;
    // The field the next text belongs to, and the depth it was opened at.
    let mut field: Option<(Field, usize)> = None;
    let mut episode_ns = String::new();
    let mut depth = 0usize;
    while let Some(token) = tokens.next() {
        match token.map_err(Error::Malformed)? {
            Token::Start { name, attrs, empty } => {
                depth += 1;
                if !root_seen {
                    if name != "tv" {
                        return Err(Error::NotXmltv);
                    }
                    root_seen = true;
                } else if depth == 2 && name == "channel" {
                    channel = Some(GuideChannel { id: attr(&attrs, "id"), ..Default::default() });
                } else if depth == 2 && name == "programme" {
                    let start = attr(&attrs, "start");
                    let stop = attr(&attrs, "stop");
                    let start_ms = parse_time(&start);
                    if guide.source_offset_s.is_none() && start_ms.is_some() {
                        guide.source_offset_s = offset_of(&start);
                    }
                    programme = start_ms.map(|start_ms| Programme {
                        channel: attr(&attrs, "channel"),
                        start_ms,
                        stop_ms: parse_time(&stop).unwrap_or(start_ms),
                        ..Default::default()
                    });
                    episode_ns.clear();
                } else if depth == 3 {
                    if let Some(c) = channel.as_mut() {
                        match name {
                            "display-name" => field = Some((Field::DisplayName, depth)),
                            "icon" if c.icon.is_empty() => c.icon = attr(&attrs, "src"),
                            _ => {}
                        }
                    } else if let Some(p) = programme.as_mut() {
                        match name {
                            "title" if p.title.is_empty() => field = Some((Field::Title, depth)),
                            "sub-title" if p.sub_title.is_empty() => field = Some((Field::SubTitle, depth)),
                            "desc" if p.desc.is_empty() => field = Some((Field::Desc, depth)),
                            "category" if p.categories.len() < MAX_CATEGORIES => {
                                p.categories.push(String::new());
                                field = Some((Field::Category, depth));
                            }
                            "date" if p.year.is_none() => field = Some((Field::Date, depth)),
                            "episode-num" => {
                                field = Some((match attr(&attrs, "system").as_str() {
                                    "onscreen" => Field::EpisodeOnscreen,
                                    "xmltv_ns" => Field::EpisodeNs,
                                    _ => Field::Ignored,
                                }, depth));
                            }
                            "icon" if p.icon.is_empty() => p.icon = attr(&attrs, "src"),
                            _ => {}
                        }
                    }
                }
                if empty {
                    close(&mut depth, &mut field, &mut channel, &mut programme, &mut episode_ns, &mut guide, from_ms, to_ms);
                }
            }
            Token::End => {
                close(&mut depth, &mut field, &mut channel, &mut programme, &mut episode_ns, &mut guide, from_ms, to_ms);
            }
            Token::Text(t) => {
                let Some((which, _)) = field else { continue };
                if let Some(c) = channel.as_mut() {
                    if which == Field::DisplayName {
                        let name = clip(t.trim());
                        if !name.is_empty() {
                            c.display_names.push(name);
                        }
                    }
                } else if let Some(p) = programme.as_mut() {
                    match which {
                        Field::Title => push_text(&mut p.title, &t),
                        Field::SubTitle => push_text(&mut p.sub_title, &t),
                        Field::Desc => push_text(&mut p.desc, &t),
                        Field::Category => {
                            if let Some(c) = p.categories.last_mut() {
                                push_text(c, &t);
                            }
                        }
                        Field::Date => p.year = year_of(&t),
                        Field::EpisodeOnscreen => push_text(&mut p.episode, &t),
                        Field::EpisodeNs => push_text(&mut episode_ns, &t),
                        Field::DisplayName | Field::Ignored => {}
                    }
                }
            }
        }
    }
    if !root_seen {
        return Err(Error::NotXmltv);
    }
    Ok(guide)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    DisplayName,
    Title,
    SubTitle,
    Desc,
    Category,
    Date,
    EpisodeOnscreen,
    EpisodeNs,
    Ignored,
}

/// An element closed: finish whatever it was.
#[allow(clippy::too_many_arguments)]
fn close(
    depth: &mut usize,
    field: &mut Option<(Field, usize)>,
    channel: &mut Option<GuideChannel>,
    programme: &mut Option<Programme>,
    episode_ns: &mut String,
    guide: &mut Guide,
    from_ms: i64,
    to_ms: i64,
) {
    if field.is_some_and(|(_, at)| at == *depth) {
        *field = None;
    }
    if *depth == 2 {
        if let Some(c) = channel.take() {
            if !c.id.is_empty() && guide.channels.len() < MAX_CHANNELS {
                guide.channels.push(c);
            }
        }
        if let Some(mut p) = programme.take() {
            p.title = p.title.trim().to_owned();
            p.sub_title = p.sub_title.trim().to_owned();
            p.desc = p.desc.trim().to_owned();
            for c in &mut p.categories {
                *c = c.trim().to_owned();
            }
            p.categories.retain(|c| !c.is_empty());
            p.category = p.categories.first().cloned().unwrap_or_default();
            p.episode = p.episode.trim().to_owned();
            if p.episode.is_empty() {
                p.episode = episode_from_ns(episode_ns);
            }
            let overlaps = p.start_ms < to_ms && p.stop_ms.max(p.start_ms + 1) > from_ms;
            if overlaps && !p.channel.is_empty() && guide.programmes.len() < MAX_PROGRAMMES {
                guide.programmes.push(p);
            }
        }
    }
    *depth = depth.saturating_sub(1);
}

fn attr(attrs: &[(String, String)], name: &str) -> String {
    attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default()
}

fn push_text(into: &mut String, text: &str) {
    if into.len() >= MAX_TEXT {
        return;
    }
    into.push_str(text);
    if into.len() > MAX_TEXT {
        let mut cut = MAX_TEXT;
        while !into.is_char_boundary(cut) {
            cut -= 1;
        }
        into.truncate(cut);
    }
}

fn clip(text: &str) -> String {
    let mut s = String::new();
    push_text(&mut s, text);
    s
}

/// The year an XMLTV `<date>` names: its first four digits (`2019`, `20190514`, `2019-05-14`), when
/// they are a plausible year.
fn year_of(text: &str) -> Option<u16> {
    let t = text.trim();
    let y: u16 = t.get(..4).filter(|d| d.bytes().all(|b| b.is_ascii_digit()))?.parse().ok()?;
    (1880..=2200).contains(&y).then_some(y)
}

/// `xmltv_ns` is `season.episode.part`, each ZERO-based and any of them empty: `0.4.` is S1E5.
fn episode_from_ns(ns: &str) -> String {
    let mut parts = ns.trim().split('.');
    let num = |s: Option<&str>| -> Option<u32> {
        let s = s?.trim();
        // A part may be written `4/12` (episode 5 of 12); the total is not shown.
        let s = s.split('/').next()?.trim();
        s.parse::<u32>().ok().map(|n| n + 1)
    };
    let season = num(parts.next());
    let episode = num(parts.next());
    match (season, episode) {
        (Some(s), Some(e)) => format!("S{s:02}E{e:02}"),
        (None, Some(e)) => format!("E{e:02}"),
        (Some(s), None) => format!("S{s:02}"),
        (None, None) => String::new(),
    }
}

/// An XMLTV timestamp — `YYYYMMDDhhmmss ±hhmm`, where the seconds, the minutes-and-seconds and the
/// offset may each be missing (a missing offset is UTC, the format's own rule) — as epoch ms.
/// The offset an XMLTV timestamp declares, in seconds east of UTC; `None` when it declares none.
pub fn offset_of(s: &str) -> Option<i32> {
    let s = s.trim();
    let i = s.find(|c: char| c == '+' || c == '-')?;
    let (sign, digits) = (if s.as_bytes()[i] == b'-' { -1 } else { 1 }, &s[i + 1..]);
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(sign * (digits[0..2].parse::<i32>().ok()? * 3600 + digits[2..4].parse::<i32>().ok()? * 60))
}

pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    let (stamp, offset) = match s.find(|c: char| c == ' ' || c == '+' || c == '-') {
        Some(i) => (&s[..i], s[i..].trim()),
        None => (s, ""),
    };
    if stamp.len() < 8 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let field = |from: usize, to: usize| -> Option<i64> {
        if stamp.len() >= to { stamp[from..to].parse::<i64>().ok() } else { Some(0) }
    };
    let (y, mo, d) = (stamp[0..4].parse::<i64>().ok()?, stamp[4..6].parse::<i64>().ok()?, stamp[6..8].parse::<i64>().ok()?);
    let (h, mi, sec) = (field(8, 10)?, field(10, 12)?, field(12, 14)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let offset_s = if offset.is_empty() {
        0
    } else {
        let (sign, digits) = match offset.as_bytes()[0] {
            b'+' => (1, &offset[1..]),
            b'-' => (-1, &offset[1..]),
            _ => return None,
        };
        if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        sign * (digits[0..2].parse::<i64>().ok()? * 3600 + digits[2..4].parse::<i64>().ok()? * 60)
    };
    let days = days_from_civil(y, mo, d);
    Some((days * 86_400 + h * 3600 + mi * 60 + sec - offset_s) * 1000)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---- the tokenizer ------------------------------------------------------------------------------

/// One token of the document.
#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    /// `<name a="v">` (`empty: false`) or `<name a="v"/>` (`empty: true`, no `End` follows).
    Start { name: &'a str, attrs: Vec<(String, String)>, empty: bool },
    /// `</name>`. The tokenizer checks that it matches the open element.
    End,
    /// Character data with entities decoded (CDATA arrives here verbatim).
    Text(String),
}

/// A pull tokenizer over a whole document. `Err(offset)` is malformed input; after an error the
/// iterator yields nothing more.
struct Tokens<'a> {
    src: &'a str,
    at: usize,
    open: Vec<&'a str>,
    done: bool,
}

impl<'a> Tokens<'a> {
    fn new(src: &'a str) -> Self {
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        Self { src, at: 0, open: Vec::new(), done: false }
    }

    fn fail(&mut self, at: usize) -> Option<Result<Token<'a>, usize>> {
        self.done = true;
        Some(Err(at))
    }
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Result<Token<'a>, usize>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.done {
                return None;
            }
            let rest = &self.src[self.at..];
            if rest.is_empty() {
                self.done = true;
                return if self.open.is_empty() { None } else { Some(Err(self.at)) };
            }
            if let Some(after) = rest.strip_prefix("<!--") {
                let Some(end) = after.find("-->") else { return self.fail(self.at) };
                self.at += 4 + end + 3;
                continue;
            }
            if let Some(after) = rest.strip_prefix("<![CDATA[") {
                let Some(end) = after.find("]]>") else { return self.fail(self.at) };
                let text = after[..end].to_owned();
                self.at += 9 + end + 3;
                if self.open.is_empty() {
                    return self.fail(self.at);
                }
                return Some(Ok(Token::Text(text)));
            }
            if rest.starts_with("<?") {
                let Some(end) = rest.find("?>") else { return self.fail(self.at) };
                self.at += end + 2;
                continue;
            }
            if rest.starts_with("<!") {
                // DOCTYPE (with or without an internal subset in brackets).
                let bytes = rest.as_bytes();
                let mut i = 2;
                let mut bracket = 0i32;
                while i < bytes.len() {
                    match bytes[i] {
                        b'[' => bracket += 1,
                        b']' => bracket -= 1,
                        b'>' if bracket <= 0 => break,
                        _ => {}
                    }
                    i += 1;
                }
                if i >= bytes.len() {
                    return self.fail(self.at);
                }
                self.at += i + 1;
                continue;
            }
            if let Some(after) = rest.strip_prefix("</") {
                let Some(end) = after.find('>') else { return self.fail(self.at) };
                let name = after[..end].trim();
                if self.open.pop() != Some(name) {
                    return self.fail(self.at);
                }
                self.at += 2 + end + 1;
                return Some(Ok(Token::End));
            }
            if rest.starts_with('<') {
                let start = self.at;
                let Some((name, attrs, empty, len)) = start_tag(rest) else { return self.fail(start) };
                self.at += len;
                if !empty {
                    self.open.push(name);
                }
                return Some(Ok(Token::Start { name, attrs, empty }));
            }
            // Character data up to the next markup.
            let end = rest.find('<').unwrap_or(rest.len());
            let raw = &rest[..end];
            self.at += end;
            if self.open.is_empty() {
                if raw.trim().is_empty() {
                    continue;
                }
                return self.fail(self.at - end);
            }
            return Some(Ok(Token::Text(unescape(raw))));
        }
    }
}

/// Parse `<name attr="v" ...>` at the head of `s`: (name, attributes, self-closing, byte length).
fn start_tag(s: &str) -> Option<(&str, Vec<(String, String)>, bool, usize)> {
    let bytes = s.as_bytes();
    let mut i = 1;
    let name_start = i;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
        i += 1;
    }
    let name = &s[name_start..i];
    if name.is_empty() {
        return None;
    }
    let mut attrs = Vec::new();
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        match bytes[i] {
            b'>' => return Some((name, attrs, false, i + 1)),
            b'/' => return (bytes.get(i + 1) == Some(&b'>')).then_some((name, attrs, true, i + 2)),
            _ => {}
        }
        let key_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
            i += 1;
        }
        let key = &s[key_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if key.is_empty() || bytes.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let quote = *bytes.get(i)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            return None;
        }
        attrs.push((key.to_owned(), unescape(&s[value_start..i])));
        i += 1;
    }
}

/// Decode the five named entities and numeric character references. An unknown or malformed
/// reference is kept literally rather than refused: a guide with one stray `&` is still a guide.
fn unescape(raw: &str) -> String {
    if !raw.contains('&') {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail.find(';').filter(|&semi| semi <= 12).and_then(|semi| {
            let name = &tail[1..semi];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let code = if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        name.strip_prefix('#').and_then(|dec| dec.parse::<u32>().ok())
                    };
                    code.and_then(char::from_u32)
                }
            }?;
            Some((c, semi + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape Tunarr writes (`XmlTvWriter.ts`), trimmed to two channels.
    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE tv SYSTEM "xmltv.dtd">
<tv generator-info-name="tunarr" source-data-url="http://192.0.2.20:8000/api/xmltv.xml">
  <channel id="C1.49.tunarr.com">
    <display-name>1 Cartoons</display-name>
    <display-name>1</display-name>
    <display-name>Cartoons</display-name>
    <icon src="http://192.0.2.20:8000/images/tunarr.png" width="250"/>
  </channel>
  <channel id="C12.99.tunarr.com">
    <display-name>12 Films &amp; More</display-name>
    <display-name>12</display-name>
    <display-name>Films &amp; More</display-name>
  </channel>
  <programme start="20261009180000 +0000" stop="20261009183000 +0000" channel="C1.49.tunarr.com">
    <title>The Simpsons</title>
    <sub-title>Itchy &amp; Scratchy &#38; Marge</sub-title>
    <desc>Marge objects to the &quot;violence&quot;.</desc>
    <credits><actor role="Homer">Dan Castellaneta</actor></credits>
    <category>Animation</category>
    <category> Comedy </category>
    <category></category>
    <date>19901011</date>
    <icon src="http://192.0.2.20:8000/api/programs/x/artwork/poster"/>
    <episode-num system="onscreen">S02E09</episode-num>
    <episode-num system="xmltv_ns">1.8.</episode-num>
  </programme>
  <programme start="20261009183000 +0000" stop="20261009190000 +0000" channel="C1.49.tunarr.com">
    <title>Futurama</title>
    <episode-num system="xmltv_ns">0.4.</episode-num>
  </programme>
  <programme start="20261009120000 +0000" stop="20261009140000 +0000" channel="C12.99.tunarr.com">
    <title><![CDATA[Old <Film>]]></title>
  </programme>
  <!-- a comment between elements -->
</tv>
"#;

    fn ms(s: &str) -> i64 {
        parse_time(s).unwrap()
    }

    #[test]
    fn channels_keep_their_id_names_and_icon() {
        let g = parse(DOC.as_bytes(), i64::MIN, i64::MAX).unwrap();
        assert_eq!(g.channels.len(), 2);
        assert_eq!(g.channels[0].id, "C1.49.tunarr.com");
        assert_eq!(g.channels[0].display_names, ["1 Cartoons", "1", "Cartoons"]);
        assert_eq!(g.channels[0].icon, "http://192.0.2.20:8000/images/tunarr.png");
        assert_eq!(g.channels[1].display_names[2], "Films & More", "entities decode once");
        assert_eq!(g.channels[1].icon, "");
    }

    #[test]
    fn programmes_carry_the_drawn_fields_and_skip_the_rest() {
        let g = parse(DOC.as_bytes(), i64::MIN, i64::MAX).unwrap();
        assert_eq!(g.programmes.len(), 3);
        let p = &g.programmes[0];
        assert_eq!(p.channel, "C1.49.tunarr.com");
        assert_eq!(p.start_ms, ms("20261009180000 +0000"));
        assert_eq!(p.stop_ms - p.start_ms, 30 * 60 * 1000);
        assert_eq!(p.title, "The Simpsons");
        assert_eq!(p.sub_title, "Itchy & Scratchy & Marge");
        assert_eq!(p.desc, "Marge objects to the \"violence\".");
        assert_eq!(p.category, "Animation", "the first category");
        assert_eq!(p.categories, ["Animation", "Comedy"], "every category, trimmed, empty ones dropped");
        assert_eq!(p.year, Some(1990));
        assert_eq!(g.programmes[1].year, None);
        assert!(g.programmes[1].categories.is_empty());
        assert_eq!(p.episode, "S02E09", "onscreen wins over xmltv_ns");
        assert_eq!(p.icon, "http://192.0.2.20:8000/api/programs/x/artwork/poster");
        assert_eq!(g.programmes[1].episode, "S01E05", "xmltv_ns is zero-based");
        assert_eq!(g.programmes[2].title, "Old <Film>", "CDATA is verbatim");
    }

    #[test]
    fn the_window_keeps_only_overlapping_airings() {
        let from = ms("20261009181500 +0000");
        let to = ms("20261009183000 +0000");
        let g = parse(DOC.as_bytes(), from, to).unwrap();
        assert_eq!(g.programmes.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), ["The Simpsons"]);
        assert_eq!(g.channels.len(), 2, "channels are kept whatever the window");
    }

    #[test]
    fn timestamps_honour_offsets_and_short_forms() {
        assert_eq!(ms("19700101000000 +0000"), 0);
        assert_eq!(ms("19700101000000"), 0, "no offset is UTC");
        assert_eq!(ms("197001010100 +0100"), 0, "seconds omitted, an eastern offset");
        assert_eq!(ms("19691231190000 -0500"), 0, "a western offset");
        assert_eq!(ms("20261009"), ms("20261009000000 +0000"));
        assert_eq!(ms("20000301000000 +0000") - ms("20000228000000 +0000"), 2 * 86_400_000, "2000 is a leap year");
        assert_eq!(parse_time("2026-10-09"), None);
        assert_eq!(parse_time("20261309000000"), None);
        assert_eq!(parse_time("20261009000000 0100"), None);
    }

    #[test]
    fn the_guide_remembers_the_zone_it_was_written_in() {
        assert_eq!(offset_of("20261009180000 -0500"), Some(-5 * 3600));
        assert_eq!(offset_of("20261009180000 +0530"), Some(5 * 3600 + 1800));
        assert_eq!(offset_of("20261009180000"), None);
        let g = parse(DOC.as_bytes(), i64::MIN, i64::MAX).unwrap();
        assert_eq!(g.source_offset_s, Some(0), "the fixture writes UTC");
    }

    #[test]
    fn malformed_and_foreign_documents_are_refused() {
        assert_eq!(parse(b"<tv><channel id=\"x\"></tv>", 0, 1), Err(Error::Malformed(20)), "a mismatched close");
        assert!(matches!(parse(b"<tv><channel id=\"x\">", 0, 1), Err(Error::Malformed(_))));
        assert_eq!(parse(b"<html></html>", 0, 1), Err(Error::NotXmltv));
        assert_eq!(parse(b"", 0, 1), Err(Error::NotXmltv));
        assert_eq!(parse(&[0xff, 0xfe, 0x00], 0, 1), Err(Error::NotUtf8));
        assert!(matches!(parse(b"<tv a=b></tv>", 0, 1), Err(Error::Malformed(_))), "unquoted attribute");
    }

    #[test]
    fn a_date_gives_a_year_only_when_it_reads_as_one() {
        assert_eq!(year_of("2019"), Some(2019));
        assert_eq!(year_of(" 2019-05-14 "), Some(2019));
        assert_eq!(year_of("19"), None);
        assert_eq!(year_of("abcd"), None);
        assert_eq!(year_of("0001"), None);
    }

    #[test]
    fn unknown_entities_stay_literal_and_numeric_ones_decode() {
        assert_eq!(unescape("a &nbsp; b"), "a &nbsp; b");
        assert_eq!(unescape("Tom &amp; Jerry &#x2014; &#8212;"), "Tom & Jerry \u{2014} \u{2014}");
        assert_eq!(unescape("AT&T"), "AT&T");
        assert_eq!(unescape("&#xD800;"), "&#xD800;", "a surrogate is not a character");
    }

    #[test]
    fn long_text_is_clipped_at_a_character_boundary() {
        let long = "é".repeat(MAX_TEXT);
        let doc = format!("<tv><programme start=\"20261009180000\" channel=\"c\"><desc>{long}</desc></programme></tv>");
        let g = parse(doc.as_bytes(), i64::MIN, i64::MAX).unwrap();
        assert!(g.programmes[0].desc.len() <= MAX_TEXT);
        assert!(g.programmes[0].desc.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_programme_without_a_start_is_dropped_and_one_without_a_stop_is_instantaneous() {
        let doc = "<tv><programme channel=\"c\"><title>x</title></programme><programme start=\"20261009180000\" channel=\"c\"/></tv>";
        let g = parse(doc.as_bytes(), i64::MIN, i64::MAX).unwrap();
        assert_eq!(g.programmes.len(), 1);
        assert_eq!(g.programmes[0].stop_ms, g.programmes[0].start_ms);
    }
}
