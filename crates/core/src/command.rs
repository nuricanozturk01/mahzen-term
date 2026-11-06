//! Command registry, `when` expressions, key chords and keymap resolution.
//!
//! One registry feeds the palette, menus and keyboard shortcuts, so every action is searchable and rebindable.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------------------------------------------
// `when` expressions
// ---------------------------------------------------------------------------------------------------------------

/// Parsed `when` expression: identifiers combined with `!`, `&&`, `||` and parentheses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum When {
    Ident(String),
    Not(Box<When>),
    And(Box<When>, Box<When>),
    Or(Box<When>, Box<When>),
}

/// Error from parsing a `when` expression.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WhenError {
    #[error("empty `when` expression")]
    Empty,
    #[error("unexpected `{found}` at position {pos} in `when` expression; expected an identifier, `!` or `(`")]
    Unexpected { found: String, pos: usize },
    #[error("unexpected end of `when` expression; add the missing operand or `)`")]
    UnexpectedEnd,
    #[error("unbalanced `)` at position {0} in `when` expression")]
    Unbalanced(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Not,
    And,
    Or,
    Open,
    Close,
}

fn lex(src: &str) -> Result<Vec<(Tok, usize)>, WhenError> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '!' => {
                out.push((Tok::Not, pos));
                i += 1;
            }
            '(' => {
                out.push((Tok::Open, pos));
                i += 1;
            }
            ')' => {
                out.push((Tok::Close, pos));
                i += 1;
            }
            '&' | '|' => {
                if chars.get(i + 1).map(|x| x.1) == Some(c) {
                    out.push((if c == '&' { Tok::And } else { Tok::Or }, pos));
                    i += 2;
                } else {
                    return Err(WhenError::Unexpected {
                        found: c.to_string(),
                        pos,
                    });
                }
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].1.is_alphanumeric() || matches!(chars[i].1, '_' | '.' | '-'))
                {
                    i += 1;
                }
                let s: String = chars[start..i].iter().map(|x| x.1).collect();
                out.push((Tok::Ident(s), pos));
            }
            other => {
                return Err(WhenError::Unexpected {
                    found: other.to_string(),
                    pos,
                })
            }
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<(Tok, usize)>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&(Tok, usize)> {
        self.toks.get(self.at)
    }

    fn or(&mut self) -> Result<When, WhenError> {
        let mut left = self.and()?;
        while matches!(self.peek(), Some((Tok::Or, _))) {
            self.at += 1;
            let right = self.and()?;
            left = When::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<When, WhenError> {
        let mut left = self.unary()?;
        while matches!(self.peek(), Some((Tok::And, _))) {
            self.at += 1;
            let right = self.unary()?;
            left = When::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<When, WhenError> {
        match self.peek().cloned() {
            Some((Tok::Not, _)) => {
                self.at += 1;
                Ok(When::Not(Box::new(self.unary()?)))
            }
            Some((Tok::Open, _)) => {
                self.at += 1;
                let inner = self.or()?;
                match self.peek() {
                    Some((Tok::Close, _)) => {
                        self.at += 1;
                        Ok(inner)
                    }
                    Some((t, pos)) => Err(WhenError::Unexpected {
                        found: tok_text(t),
                        pos: *pos,
                    }),
                    None => Err(WhenError::UnexpectedEnd),
                }
            }
            Some((Tok::Ident(s), _)) => {
                self.at += 1;
                Ok(When::Ident(s))
            }
            Some((t, pos)) => Err(WhenError::Unexpected {
                found: tok_text(&t),
                pos,
            }),
            None => Err(WhenError::UnexpectedEnd),
        }
    }
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => s.clone(),
        Tok::Not => "!".into(),
        Tok::And => "&&".into(),
        Tok::Or => "||".into(),
        Tok::Open => "(".into(),
        Tok::Close => ")".into(),
    }
}

impl When {
    /// Parse an expression such as `terminal.focused && !palette.open`.
    pub fn parse(src: &str) -> Result<Self, WhenError> {
        let toks = lex(src)?;
        if toks.is_empty() {
            return Err(WhenError::Empty);
        }
        let mut p = Parser { toks, at: 0 };
        let expr = p.or()?;
        match p.peek() {
            None => Ok(expr),
            Some((Tok::Close, pos)) => Err(WhenError::Unbalanced(*pos)),
            Some((t, pos)) => Err(WhenError::Unexpected {
                found: tok_text(t),
                pos: *pos,
            }),
        }
    }

    /// Evaluate against a context: `ctx(name)` says whether the named flag is set.
    pub fn eval(&self, ctx: &dyn Fn(&str) -> bool) -> bool {
        match self {
            Self::Ident(s) => ctx(s),
            Self::Not(e) => !e.eval(ctx),
            Self::And(a, b) => a.eval(ctx) && b.eval(ctx),
            Self::Or(a, b) => a.eval(ctx) || b.eval(ctx),
        }
    }
}

impl fmt::Display for When {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ident(s) => write!(f, "{s}"),
            Self::Not(e) => write!(f, "!{}", Paren(e)),
            Self::And(a, b) => write!(f, "{} && {}", Paren(a), Paren(b)),
            Self::Or(a, b) => write!(f, "{} || {}", Paren(a), Paren(b)),
        }
    }
}

struct Paren<'a>(&'a When);

impl fmt::Display for Paren<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            When::Ident(_) => write!(f, "{}", self.0),
            other => write!(f, "({other})"),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Key chords
// ---------------------------------------------------------------------------------------------------------------

/// One key press with modifiers, in a normalized form (lowercase key name).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Windows/Command/Super key.
    pub logo: bool,
    /// Lowercase key name: a single character (`p`, `1`, `,`) or a name (`enter`, `tab`, `f5`, `pageup`).
    pub key: String,
}

/// Error parsing a key chord or sequence.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChordError {
    #[error("empty key binding; write something like `Ctrl+Shift+P`")]
    Empty,
    #[error("unknown modifier `{0}` in `{1}`; use Ctrl, Alt, Shift or Super")]
    UnknownModifier(String, String),
    #[error("binding `{0}` has no key, only modifiers")]
    NoKey(String),
}

impl Chord {
    /// Parse `Ctrl+Shift+P`. Case-insensitive; `Cmd`, `Win`, `Meta` and `Super` all mean the logo key.
    pub fn parse(src: &str) -> Result<Self, ChordError> {
        let src = src.trim();
        if src.is_empty() {
            return Err(ChordError::Empty);
        }
        let mut chord = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            logo: false,
            key: String::new(),
        };
        // A trailing `+` is the plus key itself (`Ctrl++`).
        let (mods, key) = match src.strip_suffix('+') {
            Some(rest) if rest.is_empty() || rest.ends_with('+') => {
                (rest.trim_end_matches('+'), "+".to_string())
            }
            _ => match src.rsplit_once('+') {
                Some((m, k)) => (m, k.to_string()),
                None => ("", src.to_string()),
            },
        };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "alt" | "option" => chord.alt = true,
                "shift" => chord.shift = true,
                "super" | "cmd" | "command" | "win" | "meta" => chord.logo = true,
                other => {
                    return Err(ChordError::UnknownModifier(other.into(), src.into()));
                }
            }
        }
        let key = key.trim().to_lowercase();
        if key.is_empty() {
            return Err(ChordError::NoKey(src.into()));
        }
        // Modifier names alone are not keys.
        if matches!(
            key.as_str(),
            "ctrl"
                | "control"
                | "alt"
                | "option"
                | "shift"
                | "super"
                | "cmd"
                | "command"
                | "win"
                | "meta"
        ) {
            return Err(ChordError::NoKey(src.into()));
        }
        chord.key = match key.as_str() {
            "return" => "enter".into(),
            "esc" => "escape".into(),
            "del" => "delete".into(),
            "space" | "spacebar" => "space".into(),
            _ => key,
        };
        Ok(chord)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            write!(f, "Ctrl+")?;
        }
        if self.alt {
            write!(f, "Alt+")?;
        }
        if self.shift {
            write!(f, "Shift+")?;
        }
        if self.logo {
            write!(f, "Super+")?;
        }
        let mut cs = self.key.chars();
        match (cs.next(), cs.next()) {
            (Some(c), None) => write!(f, "{}", c.to_ascii_uppercase()),
            _ => {
                let mut k = self.key.clone();
                if let Some(first) = k.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                write!(f, "{k}")
            }
        }
    }
}

/// A sequence of chords (`Ctrl+K Ctrl+S`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sequence(pub Vec<Chord>);

impl Sequence {
    /// Parse whitespace-separated chords.
    pub fn parse(src: &str) -> Result<Self, ChordError> {
        let chords: Result<Vec<_>, _> = src.split_whitespace().map(Chord::parse).collect();
        let chords = chords?;
        if chords.is_empty() {
            return Err(ChordError::Empty);
        }
        Ok(Self(chords))
    }
}

impl fmt::Display for Sequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, c) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, " ")?;
            }
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------------------------------------------

/// Declarative description of an invocable command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Namespaced id: lowercase ASCII, digits, `_`, with at least one `.` (`tab.new`).
    pub id: String,
    pub title: String,
    pub category: String,
    pub default_keys: Vec<String>,
    /// Optional `when` expression controlling availability.
    pub when: Option<String>,
}

impl CommandSpec {
    /// Convenience constructor.
    pub fn new(id: &str, title: &str, category: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            category: category.into(),
            default_keys: Vec::new(),
            when: None,
        }
    }

    /// Add a default shortcut.
    #[must_use]
    pub fn key(mut self, keys: &str) -> Self {
        self.default_keys.push(keys.into());
        self
    }

    /// Set the availability expression.
    #[must_use]
    pub fn when(mut self, expr: &str) -> Self {
        self.when = Some(expr.into());
        self
    }
}

/// Registry errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("invalid command id `{0}`: use lowercase letters, digits and `_`, with a namespace dot like `tab.new`")]
    BadId(String),
    #[error("command `{0}` is already registered")]
    Duplicate(String),
    #[error("command `{id}` has an invalid `when`: {source}")]
    BadWhen { id: String, source: WhenError },
    #[error("command `{id}` has an invalid default key: {source}")]
    BadKey { id: String, source: ChordError },
}

#[derive(Debug, Clone)]
struct Entry {
    spec: CommandSpec,
    when: Option<When>,
}

/// All registered commands, ordered by id.
#[derive(Debug, Default, Clone)]
pub struct Registry {
    entries: BTreeMap<String, Entry>,
}

fn valid_id(id: &str) -> bool {
    id.contains('.')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

impl Registry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command; validates id, `when` and default keys up front.
    pub fn register(&mut self, spec: CommandSpec) -> Result<(), RegistryError> {
        if !valid_id(&spec.id) {
            return Err(RegistryError::BadId(spec.id));
        }
        if self.entries.contains_key(&spec.id) {
            return Err(RegistryError::Duplicate(spec.id));
        }
        let when = spec
            .when
            .as_deref()
            .map(When::parse)
            .transpose()
            .map_err(|source| RegistryError::BadWhen {
                id: spec.id.clone(),
                source,
            })?;
        for k in &spec.default_keys {
            Sequence::parse(k).map_err(|source| RegistryError::BadKey {
                id: spec.id.clone(),
                source,
            })?;
        }
        self.entries.insert(spec.id.clone(), Entry { spec, when });
        Ok(())
    }

    /// Look up a command.
    pub fn get(&self, id: &str) -> Option<&CommandSpec> {
        self.entries.get(id).map(|e| &e.spec)
    }

    /// Iterate over all commands in id order.
    pub fn iter(&self) -> impl Iterator<Item = &CommandSpec> {
        self.entries.values().map(|e| &e.spec)
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `id` exists and its `when` holds in `ctx`.
    pub fn available(&self, id: &str, ctx: &dyn Fn(&str) -> bool) -> bool {
        self.entries
            .get(id)
            .is_some_and(|e| e.when.as_ref().is_none_or(|w| w.eval(ctx)))
    }

    /// Fuzzy search for the palette: available commands matching `query`, best first.
    /// An empty query lists everything by category then title. `recent` ids get a boost, in given order.
    pub fn search(
        &self,
        query: &str,
        ctx: &dyn Fn(&str) -> bool,
        recent: &[String],
    ) -> Vec<&CommandSpec> {
        let mut scored: Vec<(i64, &CommandSpec)> = self
            .entries
            .values()
            .filter(|e| e.when.as_ref().is_none_or(|w| w.eval(ctx)))
            .filter_map(|e| {
                let hay = format!("{}: {}", e.spec.category, e.spec.title);
                let base = if query.trim().is_empty() {
                    Some(0)
                } else {
                    fuzzy_score(query, &hay)
                }?;
                let boost = recent
                    .iter()
                    .position(|r| *r == e.spec.id)
                    .map_or(0, |i| 1000 - i as i64);
                Some((base + boost, &e.spec))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.category.cmp(&b.1.category))
                .then_with(|| a.1.title.cmp(&b.1.title))
        });
        scored.into_iter().map(|(_, s)| s).collect()
    }
}

/// Case-insensitive subsequence match. Higher is better; `None` when `query` is not a subsequence.
/// Rewards consecutive runs, word starts and early matches.
pub fn fuzzy_score(query: &str, hay: &str) -> Option<i64> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let h: Vec<char> = hay.chars().flat_map(char::to_lowercase).collect();
    let mut qi = 0;
    let mut score = 0i64;
    let mut prev_match: Option<usize> = None;
    for (hi, &hc) in h.iter().enumerate() {
        if qi < q.len() && hc == q[qi] {
            score += 10;
            if prev_match == Some(hi.wrapping_sub(1)) && hi > 0 {
                score += 30; // consecutive
            }
            if hi == 0 || !h[hi - 1].is_alphanumeric() {
                score += 20; // word start
            }
            score -= i64::try_from(hi.min(50)).unwrap_or(50) / 5; // earlier is better
            prev_match = Some(hi);
            qi += 1;
        }
    }
    (qi == q.len()).then_some(score)
}

// ---------------------------------------------------------------------------------------------------------------
// Keymap
// ---------------------------------------------------------------------------------------------------------------

/// A user-supplied binding (from `keymap.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserBinding {
    pub keys: String,
    pub command: String,
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Debug, Clone)]
struct Binding {
    seq: Sequence,
    command: String,
    when: Option<When>,
    user: bool,
}

/// Problem found while building a keymap. Reported to the user; never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeymapIssue {
    /// A user binding could not be parsed or names an unknown command.
    Invalid {
        keys: String,
        command: String,
        reason: String,
    },
    /// Two bindings from the same layer use the same keys and `when`.
    Conflict { keys: String, a: String, b: String },
}

/// Result of feeding the pressed chords so far into the keymap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Complete match: run this command and clear pending chords.
    Matched(String),
    /// A prefix of a longer sequence: wait for more chords.
    Pending,
    /// Nothing matches: clear pending chords and pass the key through.
    NoMatch,
}

/// Resolved key bindings: defaults from the registry overlaid by user bindings.
#[derive(Debug, Default, Clone)]
pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Keymap {
    /// Build from registry defaults plus user bindings. A user binding with the same keys and `when`
    /// shadows a default (user bindings take priority at lookup).
    /// A user `command` starting with `-` removes that command's default binding for those keys.
    pub fn build(reg: &Registry, user: &[UserBinding]) -> (Self, Vec<KeymapIssue>) {
        let mut issues = Vec::new();
        let mut bindings: Vec<Binding> = Vec::new();
        for e in reg.entries.values() {
            for k in &e.spec.default_keys {
                if let Ok(seq) = Sequence::parse(k) {
                    bindings.push(Binding {
                        seq,
                        command: e.spec.id.clone(),
                        when: e.when.clone(),
                        user: false,
                    });
                }
            }
        }
        detect_conflicts(&bindings, &mut issues);
        let mut user_bindings: Vec<Binding> = Vec::new();
        for ub in user {
            let seq = match Sequence::parse(&ub.keys) {
                Ok(s) => s,
                Err(e) => {
                    issues.push(KeymapIssue::Invalid {
                        keys: ub.keys.clone(),
                        command: ub.command.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            let when = match ub.when.as_deref().map(When::parse).transpose() {
                Ok(w) => w,
                Err(e) => {
                    issues.push(KeymapIssue::Invalid {
                        keys: ub.keys.clone(),
                        command: ub.command.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            };
            if let Some(target) = ub.command.strip_prefix('-') {
                bindings.retain(|b| !(b.seq == seq && b.command == target && !b.user));
                continue;
            }
            if reg.get(&ub.command).is_none() {
                issues.push(KeymapIssue::Invalid {
                    keys: ub.keys.clone(),
                    command: ub.command.clone(),
                    reason: format!(
                        "unknown command `{}`; check the id in the command palette",
                        ub.command
                    ),
                });
                continue;
            }
            user_bindings.push(Binding {
                seq,
                command: ub.command.clone(),
                when,
                user: true,
            });
        }
        detect_conflicts(&user_bindings, &mut issues);
        bindings.extend(user_bindings);
        (Self { bindings }, issues)
    }

    /// Resolve `pending` (chords pressed so far, oldest first). User bindings win over defaults;
    /// later bindings win over earlier ones within a layer. Bindings whose `when` fails are ignored.
    pub fn resolve(&self, pending: &[Chord], ctx: &dyn Fn(&str) -> bool) -> Resolution {
        let live = |b: &&Binding| b.when.as_ref().is_none_or(|w| w.eval(ctx));
        let exact = self
            .bindings
            .iter()
            .rev()
            .filter(live)
            .filter(|b| b.seq.0 == pending)
            .max_by_key(|b| b.user);
        let longer = self
            .bindings
            .iter()
            .filter(live)
            .any(|b| b.seq.0.len() > pending.len() && b.seq.0.starts_with(pending));
        match (exact, longer) {
            (Some(b), false) => Resolution::Matched(b.command.clone()),
            // An exact match that is also a prefix: prefer the longer sequence (VS Code behaviour).
            (_, true) => Resolution::Pending,
            (None, false) => Resolution::NoMatch,
        }
    }

    /// The first (highest-priority) shortcut shown for a command, e.g. in the palette.
    pub fn shortcut_for(&self, command: &str) -> Option<String> {
        self.bindings
            .iter()
            .rev()
            .filter(|b| b.command == command)
            .max_by_key(|b| b.user)
            .map(|b| b.seq.to_string())
    }
}

fn detect_conflicts(bindings: &[Binding], issues: &mut Vec<KeymapIssue>) {
    for (i, a) in bindings.iter().enumerate() {
        for b in &bindings[i + 1..] {
            if a.seq == b.seq && a.when == b.when && a.command != b.command {
                issues.push(KeymapIssue::Conflict {
                    keys: a.seq.to_string(),
                    a: a.command.clone(),
                    b: b.command.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ctx<'a>(flags: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |n| flags.contains(&n)
    }

    #[test]
    fn when_precedence_and_eval() {
        let w = When::parse("a || b && !c").unwrap();
        assert!(w.eval(&ctx(&["a", "c"])));
        assert!(w.eval(&ctx(&["b"])));
        assert!(!w.eval(&ctx(&["b", "c"])));
        let w = When::parse("(a || b) && c").unwrap();
        assert!(!w.eval(&ctx(&["a"])));
        assert!(w.eval(&ctx(&["a", "c"])));
    }

    #[test]
    fn when_dotted_identifiers() {
        let w = When::parse("terminal.focused && !palette.open").unwrap();
        assert!(w.eval(&ctx(&["terminal.focused"])));
        assert!(!w.eval(&ctx(&["terminal.focused", "palette.open"])));
    }

    #[test]
    fn when_errors_are_specific() {
        assert_eq!(When::parse("  "), Err(WhenError::Empty));
        assert_eq!(When::parse("a &&"), Err(WhenError::UnexpectedEnd));
        assert_eq!(
            When::parse("a b"),
            Err(WhenError::Unexpected {
                found: "b".into(),
                pos: 2
            })
        );
        assert_eq!(When::parse("a)"), Err(WhenError::Unbalanced(1)));
        assert_eq!(When::parse("(a"), Err(WhenError::UnexpectedEnd));
        assert_eq!(
            When::parse("a & b"),
            Err(WhenError::Unexpected {
                found: "&".into(),
                pos: 2
            })
        );
    }

    #[test]
    fn chord_parse_and_display() {
        let c = Chord::parse("ctrl+shift+p").unwrap();
        assert!(c.ctrl && c.shift && !c.alt && c.key == "p");
        assert_eq!(c.to_string(), "Ctrl+Shift+P");
        assert_eq!(Chord::parse("Ctrl++").unwrap().key, "+");
        assert_eq!(Chord::parse("Alt+Return").unwrap().key, "enter");
        assert_eq!(Chord::parse("F5").unwrap().to_string(), "F5");
    }

    #[test]
    fn chord_errors() {
        assert_eq!(Chord::parse(""), Err(ChordError::Empty));
        assert!(matches!(
            Chord::parse("Hyper+X"),
            Err(ChordError::UnknownModifier(..))
        ));
        assert!(matches!(
            Chord::parse("Ctrl+Shift"),
            Err(ChordError::NoKey(_))
        ));
    }

    #[test]
    fn sequence_parses_multi_step() {
        let s = Sequence::parse("Ctrl+K Ctrl+S").unwrap();
        assert_eq!(s.0.len(), 2);
        assert_eq!(s.to_string(), "Ctrl+K Ctrl+S");
    }

    fn reg() -> Registry {
        let mut r = Registry::new();
        r.register(CommandSpec::new("tab.new", "New Tab", "Tab").key("Ctrl+Shift+T"))
            .unwrap();
        r.register(
            CommandSpec::new("tab.close", "Close Tab", "Tab")
                .key("Ctrl+Shift+W")
                .when("tabs.multiple"),
        )
        .unwrap();
        r.register(
            CommandSpec::new("pane.split_right", "Split Right", "Pane")
                .key("Ctrl+Shift+D")
                .key("Ctrl+K Ctrl+D"),
        )
        .unwrap();
        r.register(
            CommandSpec::new("palette.open", "Open Command Palette", "View").key("Ctrl+Shift+P"),
        )
        .unwrap();
        r
    }

    #[test]
    fn register_validates() {
        let mut r = Registry::new();
        assert!(matches!(
            r.register(CommandSpec::new("nodot", "x", "c")),
            Err(RegistryError::BadId(_))
        ));
        assert!(matches!(
            r.register(CommandSpec::new("Tab.New", "x", "c")),
            Err(RegistryError::BadId(_))
        ));
        assert!(matches!(
            r.register(CommandSpec::new("a..b", "x", "c")),
            Err(RegistryError::BadId(_))
        ));
        assert!(matches!(
            r.register(CommandSpec::new("a.b", "x", "c").when("(")),
            Err(RegistryError::BadWhen { .. })
        ));
        assert!(matches!(
            r.register(CommandSpec::new("a.b", "x", "c").key("Hyper+X")),
            Err(RegistryError::BadKey { .. })
        ));
        r.register(CommandSpec::new("a.b", "x", "c")).unwrap();
        assert_eq!(
            r.register(CommandSpec::new("a.b", "y", "c")),
            Err(RegistryError::Duplicate("a.b".into()))
        );
    }

    #[test]
    fn availability_follows_when() {
        let r = reg();
        assert!(!r.available("tab.close", &ctx(&[])));
        assert!(r.available("tab.close", &ctx(&["tabs.multiple"])));
        assert!(!r.available("missing.cmd", &ctx(&[])));
    }

    #[test]
    fn search_ranks_and_filters() {
        let r = reg();
        let hits: Vec<_> = r
            .search("nt", &ctx(&[]), &[])
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(hits.first(), Some(&"tab.new"));
        // unavailable commands are hidden
        let all: Vec<_> = r
            .search("", &ctx(&[]), &[])
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert!(!all.contains(&"tab.close"));
        // recent boost
        let boosted: Vec<_> = r
            .search("", &ctx(&[]), &["palette.open".into()])
            .iter()
            .map(|s| s.id.as_str())
            .collect();
        assert_eq!(boosted.first(), Some(&"palette.open"));
    }

    #[test]
    fn fuzzy_prefers_consecutive_and_word_start() {
        let a = fuzzy_score("spl", "Pane: Split Right").unwrap();
        let b = fuzzy_score("spl", "Pane: Some Pretty Long").unwrap();
        assert!(a > b);
        assert_eq!(fuzzy_score("xyz", "abc"), None);
    }

    #[test]
    fn keymap_resolves_default_and_pending() {
        let (km, issues) = Keymap::build(&reg(), &[]);
        assert!(issues.is_empty());
        let p = Chord::parse("Ctrl+Shift+T").unwrap();
        assert_eq!(
            km.resolve(&[p], &ctx(&[])),
            Resolution::Matched("tab.new".into())
        );
        let k = Chord::parse("Ctrl+K").unwrap();
        assert_eq!(
            km.resolve(std::slice::from_ref(&k), &ctx(&[])),
            Resolution::Pending
        );
        let d = Chord::parse("Ctrl+D").unwrap();
        assert_eq!(
            km.resolve(&[k.clone(), d], &ctx(&[])),
            Resolution::Matched("pane.split_right".into())
        );
        let x = Chord::parse("Ctrl+X").unwrap();
        assert_eq!(km.resolve(&[x], &ctx(&[])), Resolution::NoMatch);
    }

    #[test]
    fn keymap_respects_when() {
        let (km, _) = Keymap::build(&reg(), &[]);
        let w = Chord::parse("Ctrl+Shift+W").unwrap();
        assert_eq!(
            km.resolve(std::slice::from_ref(&w), &ctx(&[])),
            Resolution::NoMatch
        );
        assert_eq!(
            km.resolve(&[w], &ctx(&["tabs.multiple"])),
            Resolution::Matched("tab.close".into())
        );
    }

    #[test]
    fn user_binding_overrides_default() {
        let user = [UserBinding {
            keys: "Ctrl+Shift+T".into(),
            command: "palette.open".into(),
            when: None,
        }];
        let (km, issues) = Keymap::build(&reg(), &user);
        assert!(issues.is_empty());
        let p = Chord::parse("Ctrl+Shift+T").unwrap();
        assert_eq!(
            km.resolve(&[p], &ctx(&[])),
            Resolution::Matched("palette.open".into())
        );
        assert_eq!(
            km.shortcut_for("palette.open").as_deref(),
            Some("Ctrl+Shift+T")
        );
    }

    #[test]
    fn user_unbind_removes_default() {
        let user = [UserBinding {
            keys: "Ctrl+Shift+T".into(),
            command: "-tab.new".into(),
            when: None,
        }];
        let (km, _) = Keymap::build(&reg(), &user);
        let p = Chord::parse("Ctrl+Shift+T").unwrap();
        assert_eq!(km.resolve(&[p], &ctx(&[])), Resolution::NoMatch);
    }

    #[test]
    fn bad_user_bindings_are_reported_not_dropped_silently() {
        let user = [
            UserBinding {
                keys: "Hyper+X".into(),
                command: "tab.new".into(),
                when: None,
            },
            UserBinding {
                keys: "Ctrl+Q".into(),
                command: "nope.nothing".into(),
                when: None,
            },
            UserBinding {
                keys: "Ctrl+Q".into(),
                command: "tab.new".into(),
                when: Some("(".into()),
            },
        ];
        let (_, issues) = Keymap::build(&reg(), &user);
        assert_eq!(issues.len(), 3);
        insta::assert_debug_snapshot!(issues);
    }

    #[test]
    fn same_layer_conflict_is_reported() {
        let mut r = Registry::new();
        r.register(CommandSpec::new("a.one", "One", "A").key("Ctrl+1"))
            .unwrap();
        r.register(CommandSpec::new("a.two", "Two", "A").key("Ctrl+1"))
            .unwrap();
        let (_, issues) = Keymap::build(&r, &[]);
        assert_eq!(
            issues,
            vec![KeymapIssue::Conflict {
                keys: "Ctrl+1".into(),
                a: "a.one".into(),
                b: "a.two".into()
            }]
        );
    }

    proptest! {
        #[test]
        fn when_display_round_trips(src in "[a-c]( (&&|\\|\\|) !?[a-c]){0,5}") {
            let w = When::parse(&src).unwrap();
            let again = When::parse(&w.to_string()).unwrap();
            for mask in 0u8..8 {
                let c = |n: &str| match n { "a" => mask & 1 != 0, "b" => mask & 2 != 0, _ => mask & 4 != 0 };
                prop_assert_eq!(w.eval(&c), again.eval(&c));
            }
        }

        #[test]
        fn when_parser_never_panics(src in ".{0,40}") {
            let _ = When::parse(&src);
        }

        #[test]
        fn chord_display_round_trips(ctrl: bool, alt: bool, shift: bool, key in "[a-z0-9]") {
            let c = Chord { ctrl, alt, shift, logo: false, key };
            prop_assert_eq!(Chord::parse(&c.to_string()).unwrap(), c);
        }
    }
}
