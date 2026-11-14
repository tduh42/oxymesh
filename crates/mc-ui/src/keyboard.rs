//! Two-button text entry (`docs/spec/ui.md` §7), on two levels.
//!
//! The first level is a ring of stops: quick replies (empty text only), the letters most likely
//! to come next, completions of the word being typed, then rows of characters, then the final
//! action (`Send`, `Save`, `Add`) last, so that a double click from the first stop reaches it.
//! A click moves to the next stop, a double click to the previous one, a long A inserts a
//! letter or a word, or opens a row. In an open row a click moves along it (starting on its
//! most likely character), a long A inserts and goes back to the first stop, B leaves the row.
//! Outside a row, B deletes the last character.
//!
//! The predictions come from the built-in French and English word lists (written for
//! oxymesh, most frequent first) and the words of your own sent messages: each letter scores
//! the words that continue the text with it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

const FR: &str = include_str!("wordlist/fr.txt");
const EN: &str = include_str!("wordlist/en.txt");

/// Letters by frequency in French and English messages; accented ones after their base. The
/// predictions' fallback, for a word no list knows.
const LETTERS: &str = "eéèêaàâsitnrulodcçmpvqfbghjxyzwkœôûùîïëü";

/// Default quick replies (`docs/spec/ui.md` §6).
pub const QUICK_REPLIES: [&str; 8] = [
    "OK",
    "Yes",
    "No",
    "On my way",
    "Call me",
    "Where are you?",
    "Running late",
    "Thanks",
];

/// Predicted letters offered as stops of their own.
pub const PREDICTED: usize = 4;
/// Word completions offered.
pub const WORDS: usize = 2;

/// What the text is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A message; `limit` in bytes.
    Message { limit: usize },
    /// The node's name (31 bytes).
    Name,
    /// A `#hashtag` channel name (without the `#`).
    Hashtag,
    /// A server password: shown, lower and upper case, no completions.
    Password,
    /// Digits only (PIN, frequency).
    Digits { limit: usize },
}

impl Mode {
    pub fn limit(self) -> usize {
        match self {
            Mode::Message { limit } => limit,
            Mode::Name => 31,
            Mode::Hashtag => 30,
            Mode::Password => 15,
            Mode::Digits { limit } => limit,
        }
    }

    /// The final action's label.
    pub fn action(self) -> &'static str {
        match self {
            Mode::Message { .. } => "Send",
            Mode::Name => "Save",
            Mode::Hashtag => "Add",
            Mode::Password => "Login",
            Mode::Digits { .. } => "Save",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cand {
    /// The final action.
    Done,
    Space,
    Char(char),
    /// Completes the word being typed (and adds a space).
    Word(String),
    /// Replaces the whole text.
    Reply(String),
}

impl Cand {
    pub fn label(&self, mode: Mode) -> String {
        match self {
            Cand::Done => String::from(mode.action()),
            Cand::Space => String::from("space"),
            Cand::Char(c) => c.to_string(),
            Cand::Word(w) => w.clone(),
            Cand::Reply(r) => r.clone(),
        }
    }
}

/// A stop of the first level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// Inserted (or done) with a long A.
    One(Cand),
    /// Opened with a long A; its characters in a fixed order.
    Row(Vec<Cand>),
}

/// One gesture of a route to a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// A click.
    Next,
    /// A double click.
    Prev,
    /// A long A.
    Open,
    /// B.
    Back,
}

/// What the screen does after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Typing,
    /// The final action was chosen with this text.
    Done(String),
    /// B on an empty text.
    Cancel,
}

#[derive(Debug, Clone)]
pub struct Keyboard {
    pub mode: Mode,
    pub text: String,
    /// The stop selected on the first level.
    pub sel: usize,
    /// Inside the row at `sel`: the character selected.
    pub inner: Option<usize>,
    stops: Vec<Stop>,
    /// Scores of the next character, best first (`' '` for a space).
    scores: Vec<(char, u32)>,
    recent: Vec<String>,
    /// "My position: …", offered with the quick replies while the GPS has a fix.
    position: Option<String>,
}

/// The word being typed: after the last space.
fn partial(text: &str) -> &str {
    text.rsplit(' ').next().unwrap_or("")
}

/// Bytes after the last apostrophe of the word being typed.
fn tail_len(typed: &str) -> usize {
    typed.rfind('\'').map_or(0, |i| typed.len() - i - 1)
}

/// Upper-case the next letter: at the start and after a sentence.
fn sentence_start(text: &str) -> bool {
    let t = text.trim_end();
    t.is_empty() || (text.ends_with(' ') && t.ends_with(['.', '!', '?']))
}

fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn upper(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

/// A word's weight from its rank in a list, most frequent first (Zipf-like).
fn weight(rank: usize) -> u32 {
    4_000_000 / (rank as u32).saturating_add(40)
}

/// Every known word with its weight: your own words first, then the lists.
fn vocabulary(recent: &[String]) -> impl Iterator<Item = (&str, u32)> {
    recent
        .iter()
        .enumerate()
        .map(|(i, w)| (w.as_str(), 200_000 / (i as u32 + 2)))
        .chain(FR.lines().enumerate().map(|(r, w)| (w, weight(r))))
        .chain(EN.lines().enumerate().map(|(r, w)| (w, weight(r))))
}

fn add<K: PartialEq>(acc: &mut Vec<(K, u32)>, k: K, w: u32) {
    match acc.iter_mut().find(|e| e.0 == k) {
        Some(e) => e.1 = e.1.saturating_add(w),
        None => acc.push((k, w)),
    }
}

/// A row as one label: its characters side by side, `_` for the space.
pub fn row_label(items: &[Cand]) -> String {
    items
        .iter()
        .map(|c| match c {
            Cand::Char(ch) => *ch,
            _ => '_',
        })
        .collect()
}

/// The rows of characters, by mode.
fn rows(mode: Mode, caps: bool) -> Vec<Vec<Cand>> {
    let chars = |s: &str, caps: bool| -> Vec<Cand> {
        s.chars()
            .map(|c| Cand::Char(if caps { upper(c) } else { c }))
            .collect()
    };
    match mode {
        Mode::Message { .. } => {
            // The common accents with y and z; space, punctuation, digits and the rare accents
            // in the last row (the mockup's six rows).
            let mut last = alloc::vec![Cand::Space];
            last.extend(chars(".,?!'-:0123456789", false));
            last.extend(chars("âôîûœëïü", caps));
            last.extend(chars("@#/()+&;\"*", false));
            alloc::vec![
                chars("abcdef", caps),
                chars("ghijkl", caps),
                chars("mnopqr", caps),
                chars("stuvwx", caps),
                chars("yzéèàçêù", caps),
                last,
            ]
        }
        Mode::Name => {
            let mut last = alloc::vec![Cand::Space];
            last.extend(chars("-_.'0123456789", false));
            last.extend(chars("âôîûœëïü", caps));
            alloc::vec![
                chars("abcdef", caps),
                chars("ghijkl", caps),
                chars("mnopqr", caps),
                chars("stuvwx", caps),
                chars("yzéèàçêù", caps),
                last,
            ]
        }
        Mode::Hashtag => alloc::vec![
            chars("abcdef", false),
            chars("ghijkl", false),
            chars("mnopqr", false),
            chars("stuvwx", false),
            chars("yz-", false),
            chars("0123456789", false),
        ],
        Mode::Password => alloc::vec![
            chars("abcdef", false),
            chars("ghijkl", false),
            chars("mnopqr", false),
            chars("stuvwxyz", false),
            chars("ABCDEFGHIJKLM", false),
            chars("NOPQRSTUVWXYZ", false),
            chars("0123456789", false),
            chars(".,?!'-:@#/()+&", false),
        ],
        Mode::Digits { .. } => Vec::new(),
    }
}

impl Keyboard {
    /// `recent`: words from the messages sent from the device, most recent first.
    pub fn new(mode: Mode, text: &str, recent: Vec<String>) -> Self {
        let mut k = Self {
            mode,
            text: String::from(text),
            sel: 0,
            inner: None,
            stops: Vec::new(),
            scores: Vec::new(),
            recent,
            position: None,
        };
        k.refresh();
        k
    }

    /// Offers the position (degrees × 1e6) as a quick reply.
    pub fn with_position(mut self, pos: Option<(i32, i32)>) -> Self {
        self.position =
            pos.map(|(lat, lon)| alloc::format!("My position: {}, {}", degrees(lat), degrees(lon)));
        self.refresh();
        self
    }

    pub fn stops(&self) -> &[Stop] {
        &self.stops
    }

    /// The candidate a long A would insert, if any (not on a closed row).
    pub fn selected(&self) -> Option<&Cand> {
        match (self.stops.get(self.sel), self.inner) {
            (Some(Stop::One(c)), _) => Some(c),
            (Some(Stop::Row(items)), Some(i)) => items.get(i),
            _ => None,
        }
    }

    pub fn bytes(&self) -> usize {
        self.text.len()
    }

    /// The prefixes predictions and completions start from: the word being typed, then, after
    /// an apostrophe (`J'arr`), what follows it.
    fn prefixes(&self) -> (String, Option<String>) {
        let p = partial(&self.text).to_lowercase();
        let tail = p
            .rfind('\'')
            .map(|i| String::from(p.get(i + 1..).unwrap_or("")));
        (p, tail)
    }

    /// Next-character scores and the best completions, in one pass over the words.
    fn predict(&self) -> (Vec<(char, u32)>, Vec<String>) {
        let (full, tail) = self.prefixes();
        let mut scores: Vec<(char, u32)> = Vec::new();
        // Matches as (weight, word, after the apostrophe): strings only for the few shown.
        let mut matches: Vec<(u32, &str, bool)> = Vec::new();
        let typed = partial(&self.text);
        for (w, weight) in vocabulary(&self.recent) {
            for (p, scale, keep) in [(Some(full.as_str()), 1, 0), (tail.as_deref(), 4, 1)] {
                let Some(p) = p else { continue };
                if !w.starts_with(p) {
                    continue;
                }
                let wt = weight / scale;
                match w.get(p.len()..).and_then(|r| r.chars().next()) {
                    Some(c) => add(&mut scores, c, wt),
                    None if !p.is_empty() => add(&mut scores, ' ', wt),
                    None => {}
                }
                if !p.is_empty() && w.len() > p.len() {
                    matches.push((wt, w, keep == 1));
                }
            }
        }
        // Punctuation after a whole word, a share of the space's score; `?` and `!` also after
        // a space, French style (`là ?`).
        let space = scores
            .iter()
            .find(|(c, _)| *c == ' ')
            .map_or(0, |(_, w)| *w);
        if space > 0 {
            for (c, pct) in [(',', 25), ('.', 20), ('?', 15), ('!', 10)] {
                add(&mut scores, c, space / 100 * pct);
            }
        } else if typed.is_empty()
            && self
                .text
                .trim_end()
                .chars()
                .last()
                .is_some_and(char::is_alphabetic)
        {
            let top = scores.iter().map(|(_, w)| *w).max().unwrap_or(0);
            add(&mut scores, '?', top / 20);
            add(&mut scores, '!', top / 30);
        }
        scores.sort_by_key(|(_, w)| core::cmp::Reverse(*w));
        matches.sort_by_key(|(w, _, _)| core::cmp::Reverse(*w));
        let mut words: Vec<String> = Vec::new();
        for (_, w, tail) in matches {
            if words.len() == WORDS {
                break;
            }
            // After an apostrophe the text before it stays (`J'` + `arrive`).
            let shown = if tail {
                let p = tail_len(typed);
                alloc::format!("{}{w}", typed.get(..typed.len() - p).unwrap_or(""))
            } else if typed.chars().next().is_some_and(char::is_uppercase) {
                capitalize(w)
            } else {
                String::from(w)
            };
            if !words.contains(&shown) {
                words.push(shown);
            }
        }
        if scores.is_empty() {
            // A word no list knows: most likely it ends here.
            if !typed.is_empty() {
                scores.push((' ', 2));
            }
            scores.extend(LETTERS.chars().map(|c| (c, 1)));
        }
        (scores, words)
    }

    fn refresh(&mut self) {
        self.sel = 0;
        self.inner = None;
        let mut stops: Vec<Stop> = Vec::new();
        if let Mode::Digits { limit } = self.mode {
            stops.extend(('0'..='9').map(|c| Stop::One(Cand::Char(c))));
            if self.text.len() == limit {
                // Full: the action first.
                stops.insert(0, Stop::One(Cand::Done));
            } else if !self.text.is_empty() {
                stops.push(Stop::One(Cand::Done));
            }
            self.stops = stops;
            self.scores.clear();
            return;
        }
        let msg = matches!(self.mode, Mode::Message { .. });
        if msg && self.text.is_empty() {
            stops.extend(
                QUICK_REPLIES
                    .iter()
                    .map(|r| Stop::One(Cand::Reply(String::from(*r)))),
            );
            stops.extend(self.position.clone().map(|r| Stop::One(Cand::Reply(r))));
        }
        let caps =
            matches!(self.mode, Mode::Message { .. } | Mode::Name) && sentence_start(&self.text);
        let ended = self.text.is_empty() || self.text.ends_with(' ');
        let (scores, words) = if matches!(self.mode, Mode::Message { .. } | Mode::Hashtag) {
            self.predict()
        } else {
            (Vec::new(), Vec::new())
        };
        let allowed = |c: char| match self.mode {
            Mode::Hashtag => c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-',
            _ => true,
        };
        let mut shown = 0;
        for (c, _) in &scores {
            if shown == PREDICTED {
                break;
            }
            let cand = match *c {
                ' ' if ended || self.mode == Mode::Hashtag => continue,
                ' ' => Cand::Space,
                c if !allowed(c) => continue,
                c if caps => Cand::Char(upper(c)),
                c => Cand::Char(c),
            };
            stops.push(Stop::One(cand));
            shown += 1;
        }
        if msg {
            stops.extend(
                words
                    .into_iter()
                    .take(WORDS)
                    .map(|w| Stop::One(Cand::Word(w))),
            );
        }
        if self.mode == Mode::Name && !ended {
            stops.push(Stop::One(Cand::Space));
        }
        stops.extend(rows(self.mode, caps).into_iter().map(Stop::Row));
        if !self.text.trim().is_empty() {
            stops.push(Stop::One(Cand::Done));
        }
        self.scores = scores;
        self.stops = stops;
    }

    /// Where an opened row starts: its most likely character among those not already a stop
    /// of their own (opening the row means it is none of those).
    fn row_start(&self, items: &[Cand]) -> usize {
        let score = |c: &Cand| {
            if self.stops.iter().any(|s| *s == Stop::One(c.clone())) {
                return 0;
            }
            match c {
                Cand::Space => self.score(' '),
                Cand::Char(ch) => self.score(ch.to_lowercase().next().unwrap_or(*ch)),
                _ => 0,
            }
        };
        let mut best = 0;
        for (i, c) in items.iter().enumerate() {
            if score(c) > items.get(best).map_or(0, score) {
                best = i;
            }
        }
        best
    }

    fn score(&self, c: char) -> u32 {
        self.scores
            .iter()
            .find(|(k, _)| *k == c)
            .map_or(0, |(_, w)| *w)
    }

    fn len_here(&self) -> usize {
        match (self.inner, self.stops.get(self.sel)) {
            (Some(_), Some(Stop::Row(items))) => items.len(),
            _ => self.stops.len(),
        }
    }

    pub fn next(&mut self, n: usize) {
        let len = self.len_here();
        if len == 0 {
            return;
        }
        match &mut self.inner {
            Some(i) => *i = (*i + n) % len,
            None => self.sel = (self.sel + n) % len,
        }
    }

    pub fn prev(&mut self) {
        let len = self.len_here();
        if len == 0 {
            return;
        }
        match &mut self.inner {
            Some(i) => *i = (*i + len - 1) % len,
            None => self.sel = (self.sel + len - 1) % len,
        }
    }

    fn fits(&self, extra: &str) -> bool {
        self.text.len() + extra.len() <= self.mode.limit()
    }

    /// Long A: insert the selected candidate, open the selected row, or finish.
    pub fn insert(&mut self) -> Key {
        let c = match (self.stops.get(self.sel), self.inner) {
            (Some(Stop::Row(items)), None) => {
                self.inner = Some(self.row_start(items));
                return Key::Typing;
            }
            _ => match self.selected() {
                Some(c) => c.clone(),
                None => return Key::Typing,
            },
        };
        match c {
            Cand::Done => return Key::Done(String::from(self.text.trim())),
            Cand::Space => {
                if self.fits(" ") {
                    self.text.push(' ');
                }
            }
            Cand::Char(ch) => {
                let mut b = [0u8; 4];
                if self.fits(ch.encode_utf8(&mut b)) {
                    self.text.push(ch);
                }
            }
            Cand::Word(w) => {
                let keep = self.text.len() - partial(&self.text).len();
                let mut t = String::from(self.text.get(..keep).unwrap_or(""));
                t.push_str(&w);
                t.push(' ');
                if t.len() <= self.mode.limit() {
                    self.text = t;
                }
            }
            Cand::Reply(r) => {
                if r.len() <= self.mode.limit() {
                    self.text = r;
                }
                self.refresh();
                // A quick reply is complete: the cursor goes to Send.
                self.sel = self
                    .stops
                    .iter()
                    .position(|s| *s == Stop::One(Cand::Done))
                    .unwrap_or(0);
                return Key::Typing;
            }
        }
        self.refresh();
        Key::Typing
    }

    /// B: leave an open row; else delete the last character, or cancel on an empty text.
    pub fn delete(&mut self) -> Key {
        if self.inner.take().is_some() {
            return Key::Typing;
        }
        if self.text.pop().is_none() {
            return Key::Cancel;
        }
        self.refresh();
        Key::Typing
    }

    /// The fewest gestures that insert `want` from here, if it is offered.
    pub fn route(&self, want: &Cand) -> Option<Vec<Move>> {
        let steps = |from: usize, to: usize, len: usize| -> Vec<Move> {
            let fwd = (to + len - from) % len;
            let back = (from + len - to) % len;
            if fwd <= back {
                alloc::vec![Move::Next; fwd]
            } else {
                alloc::vec![Move::Prev; back]
            }
        };
        let mut best: Option<Vec<Move>> = None;
        let mut consider = |r: Vec<Move>| {
            if best.as_ref().is_none_or(|b| r.len() < b.len()) {
                best = Some(r);
            }
        };
        let (lead, from) = match (self.inner, self.stops.get(self.sel)) {
            (Some(i), Some(Stop::Row(items))) => {
                if let Some(j) = items.iter().position(|c| c == want) {
                    let mut r = steps(i, j, items.len());
                    r.push(Move::Open);
                    consider(r);
                }
                (alloc::vec![Move::Back], self.sel)
            }
            _ => (Vec::new(), self.sel),
        };
        let n = self.stops.len();
        for (k, stop) in self.stops.iter().enumerate() {
            let mut r = lead.clone();
            r.extend(steps(from, k, n));
            match stop {
                Stop::One(c) if c == want => {
                    r.push(Move::Open);
                    consider(r);
                }
                Stop::Row(items) => {
                    if let Some(j) = items.iter().position(|c| c == want) {
                        r.push(Move::Open);
                        r.extend(steps(self.row_start(items), j, items.len()));
                        r.push(Move::Open);
                        consider(r);
                    }
                }
                Stop::One(_) => {}
            }
        }
        best
    }
}

/// Degrees × 1e6 as `45.12345` (five decimals, about a metre).
fn degrees(v: i32) -> String {
    let sign = if v < 0 { "-" } else { "" };
    let a = v.unsigned_abs();
    alloc::format!("{sign}{}.{:05}", a / 1_000_000, a % 1_000_000 / 10)
}

/// Distinct words of `texts`, most recent first (texts are given oldest first).
pub fn recent_words<'a>(texts: impl DoubleEndedIterator<Item = &'a [u8]>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in texts.rev() {
        let s = String::from_utf8_lossy(t);
        for w in s.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-') {
            let w = w.to_lowercase();
            if w.chars().count() >= 3 && !out.contains(&w) {
                out.push(w);
                if out.len() >= 64 {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Inserts `want` the cheapest way.
    fn pick(k: &mut Keyboard, want: &Cand) -> usize {
        let route = k
            .route(want)
            .unwrap_or_else(|| panic!("{want:?} not offered"));
        for m in &route {
            match m {
                Move::Next => k.next(1),
                Move::Prev => k.prev(),
                Move::Open => {
                    k.insert();
                }
                Move::Back => {
                    k.delete();
                }
            }
        }
        route.len()
    }

    #[test]
    fn my_position_is_a_quick_reply_with_a_fix() {
        let k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        assert!(
            !k.stops()
                .iter()
                .any(|s| matches!(s, Stop::One(Cand::Reply(r)) if r.starts_with("My position")))
        );
        let mut k = k.with_position(Some((45_123_456, -5_000_042)));
        pick(
            &mut k,
            &Cand::Reply(String::from("My position: 45.12345, -5.00004")),
        );
        assert_eq!(k.text, "My position: 45.12345, -5.00004");
        assert_eq!(k.selected(), Some(&Cand::Done));
    }

    #[test]
    fn quick_reply_then_send_is_two_long_presses() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        assert_eq!(k.selected(), Some(&Cand::Reply(String::from("OK"))));
        k.insert();
        assert_eq!(k.text, "OK");
        assert_eq!(k.selected(), Some(&Cand::Done));
        assert_eq!(k.insert(), Key::Done(String::from("OK")));
    }

    #[test]
    fn typing_capitalizes_predicts_and_completes_words() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        pick(&mut k, &Cand::Char('B'));
        // After "B", "o" is among the predicted letters: one long press.
        assert_eq!(pick(&mut k, &Cand::Char('o')), 1 + k_index_of_o_before());
        assert!(
            k.stops()
                .iter()
                .any(|s| matches!(s, Stop::One(Cand::Word(w)) if w.starts_with("Bo"))),
            "{:?}",
            k.stops()
        );
        pick(&mut k, &Cand::Word(String::from("Bonjour")));
        assert_eq!(k.text, "Bonjour ");
        // Double click from the first stop: the last one, Send.
        k.prev();
        assert_eq!(k.selected(), Some(&Cand::Done));
        assert_eq!(k.insert(), Key::Done(String::from("Bonjour")));
    }

    /// Where "o" stood among the stops after "B" (checked by `pick`'s count).
    fn k_index_of_o_before() -> usize {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        pick(&mut k, &Cand::Char('B'));
        k.stops()
            .iter()
            .position(|s| *s == Stop::One(Cand::Char('o')))
            .expect("o predicted after B")
    }

    #[test]
    fn a_row_opens_on_its_likeliest_new_letter_and_b_leaves_it() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        pick(&mut k, &Cand::Char('Q'));
        // "q" is nearly always followed by "u": predicted first.
        assert_eq!(k.stops().first(), Some(&Stop::One(Cand::Char('u'))));
        let row = k
            .stops()
            .iter()
            .position(|s| matches!(s, Stop::Row(items) if items.contains(&Cand::Char('u'))))
            .unwrap();
        k.sel = row;
        assert_eq!(k.selected(), None, "a closed row inserts nothing");
        k.insert();
        let open = k.inner.unwrap();
        let Stop::Row(items) = &k.stops()[row] else {
            unreachable!()
        };
        assert_ne!(
            items[open],
            Cand::Char('u'),
            "u is a stop of its own already"
        );
        assert_eq!(k.delete(), Key::Typing);
        assert_eq!(k.inner, None);
        assert_eq!(k.text, "Q", "B left the row without deleting");
    }

    #[test]
    fn an_unknown_word_ends_with_a_space_first() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        for c in ['X', 'q', 'z'] {
            pick(&mut k, &Cand::Char(c));
        }
        assert_eq!(k.stops().first(), Some(&Stop::One(Cand::Space)));
    }

    #[test]
    fn punctuation_is_predicted_after_a_word() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "Merci", vec![]);
        k.refresh();
        let route = k.route(&Cand::Char('!')).unwrap();
        assert!(route.len() <= 8, "{route:?}");
    }

    #[test]
    fn recent_words_come_first_and_limit_is_bytes() {
        let recent = recent_words([b"meet at the Rifugio".as_slice()].into_iter());
        let mut k = Keyboard::new(Mode::Message { limit: 3 }, "", recent);
        pick(&mut k, &Cand::Char('R'));
        pick(&mut k, &Cand::Char('i'));
        assert!(
            k.stops()
                .iter()
                .any(|s| *s == Stop::One(Cand::Word(String::from("Rifugio")))),
            "{:?}",
            k.stops()
        );
        k.text.clear();
        k.refresh();
        pick(&mut k, &Cand::Char('R'));
        pick(&mut k, &Cand::Char('é'));
        assert_eq!(k.text, "Ré", "3 bytes");
        pick(&mut k, &Cand::Char('a'));
        assert_eq!(k.text, "Ré", "a fourth byte does not fit");
        assert_eq!(k.delete(), Key::Typing);
        assert_eq!(k.delete(), Key::Typing);
        assert_eq!(k.delete(), Key::Cancel);
    }

    #[test]
    fn hashtag_mode_is_lowercase_ascii() {
        let k = Keyboard::new(Mode::Hashtag, "", vec![]);
        let ok = |c: &Cand| match c {
            Cand::Char(ch) => ch.is_ascii_lowercase() || ch.is_ascii_digit() || *ch == '-',
            _ => false,
        };
        assert!(k.stops().iter().all(|s| match s {
            Stop::One(c) => ok(c),
            Stop::Row(items) => items.iter().all(ok),
        }));
    }

    #[test]
    fn a_route_from_inside_a_row_leaves_it_first() {
        let mut k = Keyboard::new(Mode::Message { limit: 160 }, "", vec![]);
        pick(&mut k, &Cand::Char('T'));
        let row = k
            .stops()
            .iter()
            .position(|s| matches!(s, Stop::Row(i) if i.contains(&Cand::Char('a'))))
            .unwrap();
        k.sel = row;
        k.insert();
        // Inside "a b c d e f": a digit is in another row.
        let r = k.route(&Cand::Char('7')).unwrap();
        assert_eq!(r.first(), Some(&Move::Back));
        pick(&mut k, &Cand::Char('7'));
        assert_eq!(k.text, "T7");
    }
}
