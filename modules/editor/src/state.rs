//! The state of one editor pane: a buffer plus everything the screen needs (scroll, find bar, go-to-line prompt, notices)
//! and the handling of key presses. Pure data and logic, so every behaviour is tested without a window.

use mahzenterm_core::command::Chord;
use mahzenterm_core::syntax;

use crate::keymap::{apply, Applied, Keymap};
use crate::search::{Match, SearchOptions};
use crate::Buffer;

/// Spaces a Tab key inserts (VS Code's default).
pub const TAB_WIDTH: usize = 4;

/// A problem to underline (from a language server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub severity: Severity,
    pub message: String,
}

/// How bad a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Hint,
    Info,
    Warning,
    Error,
}

/// What the editor is doing besides editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Edit,
    /// The find bar: query, replacement, which field has the keyboard, matches and the current one.
    Find {
        query: String,
        replace: String,
        /// The replacement field is shown (Ctrl+H) and can take the keyboard (Tab).
        show_replace: bool,
        /// The replacement field has the keyboard.
        in_replace: bool,
        matches: Vec<Match>,
        current: usize,
    },
    /// "Go to line": the digits typed so far.
    Goto(String),
    /// "Save as": the file name typed so far.
    SaveAs(String),
}

/// What the app must do after a key press (things the state cannot do itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Save,
    SaveAs,
    /// Save under this name (typed in the prompt; relative names are relative to the file's folder).
    SaveAsTo(String),
    /// Put this text on the clipboard.
    Copy(String),
    /// Same, after the selection was removed.
    Cut(String),
    /// Ask the clipboard for text and call [`EditorState::paste`].
    Paste,
    GotoDefinition,
    Suggest,
    /// Ask for what is known about the symbol under the cursor.
    Hover,
    QuickOpen,
}

/// One suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    /// What replaces the word before the cursor.
    pub insert: String,
}

/// The suggestion list at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completions {
    all: Vec<CompletionItem>,
    /// Indices into `all` that match what is typed, best first.
    shown: Vec<usize>,
    pub selected: usize,
    /// Character index where the word being completed starts.
    start: usize,
}

impl Completions {
    /// The suggestions that match, in order.
    pub fn items(&self) -> impl Iterator<Item = &CompletionItem> {
        self.shown.iter().map(|i| &self.all[*i])
    }

    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }
}

/// Most suggestions listed at once.
pub const MAX_SUGGESTIONS: usize = 50;

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Rank `items` for `prefix`: prefix matches first, then other subsequence matches (case-insensitive).
fn matching(all: &[CompletionItem], prefix: &str) -> Vec<usize> {
    let p = prefix.to_lowercase();
    let mut scored: Vec<(u8, usize)> = all
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let l = c.label.to_lowercase();
            if p.is_empty() || l.starts_with(&p) {
                return Some((0, i));
            }
            let mut it = l.chars();
            p.chars().all(|pc| it.any(|lc| lc == pc)).then_some((1, i))
        })
        .collect();
    scored.sort_by_key(|(rank, i)| (*rank, all[*i].label.len()));
    scored
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, i)| i)
        .collect()
}

/// One open file in an editor pane.
#[derive(Debug)]
pub struct EditorState {
    pub buffer: Buffer,
    pub keymap: Keymap,
    pub lang: &'static str,
    /// First visible line.
    pub scroll: usize,
    /// Lines that fit on screen (set by the view each frame).
    pub rows: usize,
    pub mode: Mode,
    pub notice: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    /// Text of the symbol under the cursor (from a language server), shown until the next key.
    pub hover: Option<Vec<String>>,
    /// Suggestions being offered at the cursor.
    pub completion: Option<Completions>,
    /// Bumped on every change that needs a redraw.
    pub revision: u64,
}

impl EditorState {
    pub fn new(buffer: Buffer) -> Self {
        let lang = buffer
            .path()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
            .and_then(|e| syntax::language_for_extension(&e.to_ascii_lowercase()))
            .unwrap_or("");
        Self {
            buffer,
            keymap: Keymap::vscode(),
            lang,
            scroll: 0,
            rows: 30,
            mode: Mode::Edit,
            notice: None,
            diagnostics: Vec::new(),
            hover: None,
            completion: None,
            revision: 0,
        }
    }

    /// Character index where the word that ends at the cursor starts.
    fn word_start(&self) -> usize {
        let cur = self.buffer.cursor();
        let (ls, _) = self.buffer.line_range(self.buffer.line_col(cur).0);
        let line = self.buffer.line(self.buffer.line_col(cur).0);
        let upto: Vec<char> = line.chars().take(cur - ls).collect();
        let n = upto.iter().rev().take_while(|c| is_word(**c)).count();
        cur - n
    }

    /// Offer `items` at the cursor, filtered by the word typed so far. An empty answer says so.
    pub fn show_completions(&mut self, items: Vec<CompletionItem>) {
        let start = self.word_start();
        let prefix = self.buffer.slice(start, self.buffer.cursor());
        let shown = matching(&items, &prefix);
        if shown.is_empty() {
            self.completion = None;
            self.notice = Some("no suggestions".into());
        } else {
            self.completion = Some(Completions {
                all: items,
                shown,
                selected: 0,
                start,
            });
        }
        self.touch();
    }

    /// Show what a language server said about the symbol at the cursor.
    pub fn show_hover(&mut self, lines: Vec<String>) {
        if lines.is_empty() {
            self.notice = Some("nothing known about this".into());
        } else {
            self.hover = Some(lines);
        }
        self.touch();
    }

    fn move_completion(&mut self, delta: isize) {
        if let Some(c) = self.completion.as_mut() {
            let n = c.shown.len();
            if n > 0 {
                c.selected = (c.selected as isize + delta).rem_euclid(n as isize) as usize;
            }
        }
        self.touch();
    }

    /// Replace the word before the cursor with the chosen suggestion.
    fn accept_completion(&mut self) {
        let Some(c) = self.completion.take() else {
            return;
        };
        if let Some(item) = c.shown.get(c.selected).map(|i| c.all[*i].clone()) {
            let cur = self.buffer.cursor();
            if c.start <= cur {
                self.buffer.replace_range(c.start, cur, &item.insert);
                self.reveal();
            }
        }
        self.touch();
    }

    /// Keep the list in step with typing: closes when the word is gone or nothing matches any more.
    fn refilter_completion(&mut self) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let cur = self.buffer.cursor();
        if cur < c.start || self.buffer.line_col(cur).0 != self.buffer.line_col(c.start).0 {
            self.touch();
            return;
        }
        let prefix = self.buffer.slice(c.start, cur);
        if !prefix.chars().all(is_word) {
            self.touch();
            return;
        }
        let shown = matching(&c.all, &prefix);
        if !shown.is_empty() {
            let selected = c.selected.min(shown.len() - 1);
            self.completion = Some(Completions {
                shown,
                selected,
                ..c
            });
        }
        self.touch();
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Scroll so the primary cursor is on screen.
    pub fn reveal(&mut self) {
        let (line, _) = self.buffer.line_col(self.buffer.cursor());
        let rows = self.rows.max(1);
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + rows {
            self.scroll = line + 1 - rows;
        }
        self.touch();
    }

    /// Scroll by `delta` lines (the wheel); the cursor stays where it is.
    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.buffer.line_count().saturating_sub(1);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
        self.touch();
    }

    /// Put the cursor at a line and column (a click); `extend` keeps the anchor (shift-click or drag).
    pub fn click(&mut self, line: usize, col: usize, extend: bool) {
        let (start, end) = self
            .buffer
            .line_range(line.min(self.buffer.line_count().saturating_sub(1)));
        let at = (start + col).min(end);
        let anchor = if extend {
            self.buffer.selections()[0].anchor
        } else {
            at
        };
        self.buffer.set_selection(anchor, at);
        self.mode = if matches!(self.mode, Mode::Find { .. }) {
            std::mem::replace(&mut self.mode, Mode::Edit)
        } else {
            Mode::Edit
        };
        self.touch();
    }

    /// Select the word under a double click.
    pub fn double_click(&mut self, line: usize, col: usize) {
        let (start, end) = self.buffer.line_range(line);
        let (a, b) = self.buffer.word_range_at((start + col).min(end));
        self.buffer.set_selection(a, b);
        self.touch();
    }

    /// Jump to a 1-based line (and column), as from a search result or an error message.
    pub fn goto(&mut self, line1: usize, col1: usize) {
        let line = line1
            .saturating_sub(1)
            .min(self.buffer.line_count().saturating_sub(1));
        let (start, end) = self.buffer.line_range(line);
        let at = (start + col1.saturating_sub(1)).min(end);
        self.buffer.set_selection(at, at);
        self.reveal();
        // Centre it when it was off screen.
        let rows = self.rows.max(1);
        self.scroll = line.saturating_sub(rows / 2);
    }

    /// Type text at the cursors.
    pub fn type_text(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Edit => {
                self.buffer.insert_str(text);
                self.reveal();
            }
            Mode::Find {
                query,
                replace,
                in_replace,
                ..
            } => {
                let field = if *in_replace { replace } else { query };
                field.extend(text.chars().filter(|c| !c.is_control()));
                if !*in_replace {
                    self.refresh_matches(true);
                }
                self.touch();
            }
            Mode::Goto(digits) => {
                digits.extend(text.chars().filter(char::is_ascii_digit));
                self.touch();
            }
            Mode::SaveAs(name) => {
                name.extend(text.chars().filter(|c| !c.is_control()));
                self.touch();
            }
        }
    }

    /// Paste clipboard text.
    pub fn paste(&mut self, text: &str) {
        if matches!(self.mode, Mode::Edit) {
            self.buffer.insert_str(text);
            self.reveal();
        } else {
            self.type_text(&text.replace(['\n', '\r'], " "));
        }
    }

    fn refresh_matches(&mut self, jump: bool) {
        let Mode::Find {
            query,
            matches,
            current,
            ..
        } = &mut self.mode
        else {
            return;
        };
        *matches = self.buffer.find_all(query, SearchOptions::default());
        let cursor = self.buffer.selections()[0].range().0;
        *current = matches.iter().position(|m| m.start >= cursor).unwrap_or(0);
        let target = matches.get(*current).copied();
        if let (true, Some(m)) = (jump, target) {
            self.buffer.set_selection(m.start, m.end);
            self.reveal();
        }
    }

    fn step_match(&mut self, forward: bool) {
        let Mode::Find {
            matches, current, ..
        } = &mut self.mode
        else {
            return;
        };
        if matches.is_empty() {
            return;
        }
        let n = matches.len();
        *current = if forward {
            (*current + 1) % n
        } else {
            (*current + n - 1) % n
        };
        let m = matches[*current];
        self.buffer.set_selection(m.start, m.end);
        self.reveal();
    }

    fn open_find(&mut self, replace: bool) {
        let seed = self.buffer.selected_text();
        let query = if seed.is_empty() || seed.contains('\n') {
            String::new()
        } else {
            seed
        };
        self.mode = Mode::Find {
            query,
            replace: String::new(),
            show_replace: replace,
            in_replace: false,
            matches: Vec::new(),
            current: 0,
        };
        self.refresh_matches(false);
        self.touch();
    }

    /// Ctrl+D: select the word under the cursor; again: add the next occurrence as another cursor.
    fn add_next_occurrence(&mut self) {
        let sel = self.buffer.selections()[0];
        if sel.is_empty() {
            let (a, b) = self.buffer.word_range_at(sel.head);
            if a < b {
                self.buffer.set_selection(a, b);
            }
            return;
        }
        let text = self.buffer.selected_text();
        let last_end = self
            .buffer
            .selections()
            .iter()
            .map(|s| s.range().1)
            .max()
            .unwrap_or(0);
        let all = self.buffer.find_all(
            &text,
            SearchOptions {
                case_sensitive: true,
                whole_word: false,
            },
        );
        // The next match after the last selection, wrapping to the first one not already selected.
        let taken: Vec<(usize, usize)> =
            self.buffer.selections().iter().map(|s| s.range()).collect();
        let next = all
            .iter()
            .find(|m| m.start >= last_end && !taken.contains(&(m.start, m.end)))
            .or_else(|| all.iter().find(|m| !taken.contains(&(m.start, m.end))));
        if let Some(m) = next {
            self.buffer.add_selection(m.start, m.end);
            self.reveal();
        } else {
            self.notice = Some("no more occurrences".into());
        }
    }

    /// Ctrl+/: comment the selected lines, or uncomment them when they all are.
    fn toggle_comment(&mut self) {
        let Some(prefix) = syntax::line_comment(self.lang) else {
            self.notice = Some("this file type has no line comment".into());
            return;
        };
        let (first, last) = self.buffer.primary_lines();
        let lines: Vec<String> = (first..=last).map(|l| self.buffer.line(l)).collect();
        let indent_of = |l: &str| l.chars().take_while(|c| c.is_whitespace()).count();
        let all_commented = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| l.trim_start().starts_with(prefix));
        let min_indent = lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| indent_of(l))
            .min()
            .unwrap_or(0);
        let out: Vec<String> = lines
            .iter()
            .map(|l| {
                if l.trim().is_empty() {
                    l.clone()
                } else if all_commented {
                    let ind = indent_of(l);
                    let rest = &l.trim_start()[prefix.len()..];
                    let rest = rest.strip_prefix(' ').unwrap_or(rest);
                    format!("{}{rest}", " ".repeat(ind))
                } else {
                    let (head, tail): (String, String) = (
                        l.chars().take(min_indent).collect(),
                        l.chars().skip(min_indent).collect(),
                    );
                    format!("{head}{prefix} {tail}")
                }
            })
            .collect();
        let (start, _) = self.buffer.line_range(first);
        let (_, end) = self.buffer.line_range(last);
        let before = self.buffer.selections()[0];
        self.buffer.replace_range(start, end, &out.join("\n"));
        let n_end = self.buffer.line_range(last).1;
        self.buffer
            .set_selection(start, n_end.min(self.buffer.len_chars()));
        let _ = before;
        self.reveal();
    }

    /// Shift+Tab: remove up to one tab width of leading spaces from the selected lines.
    fn dedent(&mut self) {
        let (first, last) = self.buffer.primary_lines();
        let lines: Vec<String> = (first..=last).map(|l| self.buffer.line(l)).collect();
        let out: Vec<String> = lines
            .iter()
            .map(|l| {
                let n = l.chars().take(TAB_WIDTH).take_while(|c| *c == ' ').count();
                l.chars().skip(n).collect()
            })
            .collect();
        let (start, _) = self.buffer.line_range(first);
        let (_, end) = self.buffer.line_range(last);
        self.buffer.replace_range(start, end, &out.join("\n"));
        self.buffer
            .set_selection(start, self.buffer.line_range(last).1);
        self.reveal();
    }

    /// Handle one key press. `None` means "not mine": the app may use the key for its own shortcuts.
    pub fn key(&mut self, chord: &Chord, text: Option<&str>) -> Option<Effect> {
        if self.hover.take().is_some() {
            self.touch();
        }
        let plain = !chord.ctrl && !chord.alt && !chord.logo;
        if self.completion.is_some() && matches!(self.mode, Mode::Edit) && !chord.shift {
            match chord.key.as_str() {
                "up" if plain => {
                    self.move_completion(-1);
                    return Some(Effect::None);
                }
                "down" if plain => {
                    self.move_completion(1);
                    return Some(Effect::None);
                }
                "enter" | "tab" if plain => {
                    self.accept_completion();
                    return Some(Effect::None);
                }
                "escape" if plain => {
                    self.completion = None;
                    self.touch();
                    return Some(Effect::None);
                }
                _ => {}
            }
        }
        let had = self.completion.is_some();
        let result = self.key_inner(chord, text);
        if had {
            self.refilter_completion();
        }
        result
    }

    fn key_inner(&mut self, chord: &Chord, text: Option<&str>) -> Option<Effect> {
        // The find bar and the go-to-line prompt take their own keys first, then plain typing.
        let prompt = match &self.mode {
            Mode::Find { .. } => Some(self.key_in_find(chord)),
            Mode::Goto(_) => Some(self.key_in_goto(chord)),
            Mode::SaveAs(_) => Some(self.key_in_save_as(chord)),
            Mode::Edit => None,
        };
        if let Some(handled) = prompt {
            if handled.is_some() {
                return handled;
            }
            let typing = !chord.ctrl && !chord.alt && !chord.logo;
            return match text
                .filter(|t| typing && !t.is_empty() && t.chars().all(|c| !c.is_control()))
            {
                Some(t) => {
                    self.type_text(t);
                    Some(Effect::None)
                }
                None => None,
            };
        }
        self.notice = None;
        let plain = !chord.ctrl && !chord.alt && !chord.logo;
        // Keys that need a decision of the state itself.
        match (chord.key.as_str(), chord.ctrl, chord.shift, chord.alt) {
            ("tab", false, false, false) => {
                if self.buffer.selected_text().contains('\n') {
                    let (first, last) = self.buffer.primary_lines();
                    let (start, _) = self.buffer.line_range(first);
                    let (_, end) = self.buffer.line_range(last);
                    let block: Vec<String> = (first..=last)
                        .map(|l| self.buffer.line(l))
                        .map(|l| {
                            if l.is_empty() {
                                l
                            } else {
                                format!("{}{l}", " ".repeat(TAB_WIDTH))
                            }
                        })
                        .collect();
                    self.buffer.replace_range(start, end, &block.join("\n"));
                    self.buffer
                        .set_selection(start, self.buffer.line_range(last).1);
                } else {
                    self.buffer.insert_str(&" ".repeat(TAB_WIDTH));
                }
                self.reveal();
                return Some(Effect::None);
            }
            ("tab", false, true, false) => {
                self.dedent();
                return Some(Effect::None);
            }
            ("escape", false, _, false) => {
                let c = self.buffer.cursor();
                self.buffer.set_selection(c, c);
                self.touch();
                return Some(Effect::None);
            }
            ("pageup", ..) | ("pagedown", ..) => {
                let rows = self.rows.max(2) - 1;
                let m = if chord.key == "pageup" {
                    crate::Move::Up
                } else {
                    crate::Move::Down
                };
                for _ in 0..rows {
                    self.buffer.move_cursor(m, chord.shift);
                }
                self.reveal();
                return Some(Effect::None);
            }
            _ => {}
        }
        let resolved = self.keymap.resolve(chord).map(str::to_owned);
        if let Some(cmd) = resolved {
            return Some(self.run(&cmd));
        }
        if plain || (chord.shift && !chord.ctrl && !chord.alt) {
            if let Some(t) = text.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
                self.type_text(t);
                return Some(Effect::None);
            }
        }
        None
    }

    /// Run a command id (also used by menus and the palette).
    pub fn run(&mut self, cmd: &str) -> Effect {
        match apply(&mut self.buffer, cmd) {
            Applied::Done => {
                self.reveal();
                return Effect::None;
            }
            Applied::Unknown => return Effect::None,
            Applied::NeedsUi => {}
        }
        match cmd {
            "editor.save" => Effect::Save,
            "editor.save_as" => {
                let seed = self
                    .buffer
                    .path()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.mode = Mode::SaveAs(seed);
                self.touch();
                Effect::None
            }
            "editor.copy" => {
                let t = self.buffer.selected_text();
                if t.is_empty() {
                    // VS Code copies the whole line when nothing is selected.
                    let (l, _) = self.buffer.line_col(self.buffer.cursor());
                    Effect::Copy(format!("{}\n", self.buffer.line(l)))
                } else {
                    Effect::Copy(t)
                }
            }
            "editor.cut" => {
                let t = self.buffer.selected_text();
                if t.is_empty() {
                    let (l, _) = self.buffer.line_col(self.buffer.cursor());
                    let text = format!("{}\n", self.buffer.line(l));
                    self.buffer.delete_lines();
                    self.reveal();
                    Effect::Cut(text)
                } else {
                    self.buffer.backspace();
                    self.reveal();
                    Effect::Cut(t)
                }
            }
            "editor.paste" => Effect::Paste,
            "editor.find" => {
                self.open_find(false);
                Effect::None
            }
            "editor.replace" => {
                self.open_find(true);
                Effect::None
            }
            "editor.goto_line" => {
                self.mode = Mode::Goto(String::new());
                self.touch();
                Effect::None
            }
            "editor.add_next_occurrence" => {
                self.add_next_occurrence();
                Effect::None
            }
            "editor.toggle_comment" => {
                self.toggle_comment();
                Effect::None
            }
            "editor.goto_definition" => Effect::GotoDefinition,
            "editor.suggest" => Effect::Suggest,
            "editor.hover" => Effect::Hover,
            "editor.quick_open" => Effect::QuickOpen,
            _ => Effect::None,
        }
    }

    fn key_in_find(&mut self, chord: &Chord) -> Option<Effect> {
        match (chord.key.as_str(), chord.ctrl, chord.alt, chord.shift) {
            ("escape", ..) => {
                self.mode = Mode::Edit;
                self.touch();
            }
            ("enter", true, true, _) => {
                // Replace all.
                if let Mode::Find { query, replace, .. } = &self.mode {
                    let (q, r) = (query.clone(), replace.clone());
                    let n = self.buffer.replace_all(&q, &r, SearchOptions::default());
                    self.notice = Some(format!("replaced {n}"));
                    self.refresh_matches(false);
                }
            }
            ("enter", false, false, shift) => {
                let replacing = matches!(
                    &self.mode,
                    Mode::Find {
                        in_replace: true,
                        ..
                    }
                );
                if replacing {
                    // Replace the current match, then move to the next.
                    if let Mode::Find {
                        replace,
                        matches,
                        current,
                        ..
                    } = &self.mode
                    {
                        if let Some(m) = matches.get(*current).copied() {
                            let r = replace.clone();
                            self.buffer.replace_range(m.start, m.end, &r);
                        }
                    }
                    self.refresh_matches(true);
                } else {
                    self.step_match(!shift);
                }
            }
            ("tab", false, false, _) => {
                if let Mode::Find {
                    in_replace,
                    show_replace,
                    ..
                } = &mut self.mode
                {
                    if *show_replace {
                        *in_replace = !*in_replace;
                    }
                    self.touch();
                }
            }
            ("backspace", ..) => {
                if let Mode::Find {
                    query,
                    replace,
                    in_replace,
                    ..
                } = &mut self.mode
                {
                    if *in_replace {
                        replace.pop()
                    } else {
                        query.pop()
                    };
                    if !*in_replace {
                        self.refresh_matches(true);
                    }
                    self.touch();
                }
            }
            ("f3", _, _, shift) => self.step_match(!shift),
            _ => return None,
        }
        Some(Effect::None)
    }

    fn key_in_save_as(&mut self, chord: &Chord) -> Option<Effect> {
        match chord.key.as_str() {
            "escape" => self.mode = Mode::Edit,
            "backspace" => {
                if let Mode::SaveAs(n) = &mut self.mode {
                    n.pop();
                }
            }
            "enter" => {
                let name = if let Mode::SaveAs(n) = &self.mode {
                    n.trim().to_owned()
                } else {
                    String::new()
                };
                self.mode = Mode::Edit;
                self.touch();
                return Some(if name.is_empty() {
                    Effect::None
                } else {
                    Effect::SaveAsTo(name)
                });
            }
            _ => return None,
        }
        self.touch();
        Some(Effect::None)
    }

    fn key_in_goto(&mut self, chord: &Chord) -> Option<Effect> {
        match chord.key.as_str() {
            "escape" => self.mode = Mode::Edit,
            "backspace" => {
                if let Mode::Goto(d) = &mut self.mode {
                    d.pop();
                }
            }
            "enter" => {
                let n = if let Mode::Goto(d) = &self.mode {
                    d.parse::<usize>().ok()
                } else {
                    None
                };
                self.mode = Mode::Edit;
                if let Some(n) = n.filter(|n| *n > 0) {
                    self.goto(n, 1);
                }
            }
            _ => return None,
        }
        self.touch();
        Some(Effect::None)
    }

    /// `Ln 3, Col 5` of the primary cursor.
    pub fn position_label(&self) -> String {
        let (l, c) = self.buffer.line_col(self.buffer.cursor());
        let sels = self.buffer.selections().len();
        if sels > 1 {
            format!("Ln {}, Col {} ({sels} cursors)", l + 1, c + 1)
        } else {
            format!("Ln {}, Col {}", l + 1, c + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    fn state(text: &str, ext: &str) -> EditorState {
        let mut b = Buffer::from_text(text);
        // A path only for its extension (the language).
        let _ = &mut b;
        let mut s = EditorState::new(b);
        s.lang = syntax::language_for_extension(ext).unwrap_or("");
        s
    }

    fn press(s: &mut EditorState, keys: &str) -> Option<Effect> {
        s.key(&chord(keys), None)
    }

    fn typed(s: &mut EditorState, text: &str) {
        for c in text.chars() {
            s.key(&chord(&c.to_string()), Some(&c.to_string()));
        }
    }

    fn item(label: &str, insert: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            detail: String::new(),
            insert: insert.into(),
        }
    }

    #[test]
    fn suggestions_filter_as_you_type_and_replace_the_word_on_enter() {
        let mut s = state("", "rs");
        s.type_text("let v = ve");
        s.show_completions(vec![
            item("vec!", "vec![]"),
            item("Vec", "Vec"),
            item("value", "value"),
            item("zzz", "zzz"),
        ]);
        let c = s.completion.as_ref().unwrap();
        assert_eq!(
            c.items().map(|i| i.label.as_str()).collect::<Vec<_>>(),
            ["Vec", "vec!", "value"],
            "prefix matches first, shorter first"
        );
        // Typing narrows the list; Down moves; Enter replaces `ve`+typed word.
        typed(&mut s, "c");
        assert_eq!(s.completion.as_ref().unwrap().len(), 2);
        press(&mut s, "Down");
        assert_eq!(press(&mut s, "Enter"), Some(Effect::None));
        assert_eq!(s.buffer.text(), "let v = vec![]");
        assert!(s.completion.is_none());
    }

    #[test]
    fn suggestions_close_on_escape_on_a_non_word_key_and_when_nothing_matches() {
        let mut s = state("", "rs");
        typed(&mut s, "a");
        s.show_completions(vec![item("abc", "abc")]);
        press(&mut s, "Escape");
        assert!(s.completion.is_none());
        s.show_completions(vec![item("abc", "abc")]);
        typed(&mut s, "(");
        assert!(s.completion.is_none(), "a non-word key ends the word");
        s.show_completions(vec![item("abc", "abc")]);
        typed(&mut s, "z");
        assert!(s.completion.is_none(), "no match left");
        s.show_completions(vec![item("nomatch", "x")]);
        assert!(s.completion.is_none());
        assert_eq!(s.notice.as_deref(), Some("no suggestions"));
    }

    #[test]
    fn hover_text_shows_until_the_next_key() {
        let mut s = state("fn main() {}", "rs");
        s.show_hover(vec!["fn main()".into(), "Entry point.".into()]);
        assert_eq!(s.hover.as_ref().unwrap().len(), 2);
        press(&mut s, "Right");
        assert!(s.hover.is_none());
        s.show_hover(Vec::new());
        assert!(s.hover.is_none());
        assert_eq!(s.notice.as_deref(), Some("nothing known about this"));
        assert_eq!(press(&mut s, "Ctrl+Alt+I"), Some(Effect::Hover));
    }

    #[test]
    fn typing_moving_and_undo_work_through_keys() {
        let mut s = state("", "rs");
        typed(&mut s, "hello");
        assert_eq!(s.buffer.text(), "hello");
        press(&mut s, "Home");
        typed(&mut s, ">");
        assert_eq!(s.buffer.text(), ">hello");
        press(&mut s, "Ctrl+Z");
        assert_eq!(s.buffer.text(), "hello");
        assert_eq!(press(&mut s, "Ctrl+Shift+Z"), Some(Effect::None));
        assert_eq!(s.buffer.text(), ">hello");
        // Keys the editor does not own are left for the app.
        assert_eq!(press(&mut s, "Ctrl+Alt+K"), None);
        assert_eq!(press(&mut s, "F9"), None);
    }

    #[test]
    fn tab_inserts_four_spaces_and_shift_tab_removes_them() {
        let mut s = state("a\nb", "txt");
        press(&mut s, "Tab");
        assert_eq!(s.buffer.line(0), "    a");
        s.buffer.select_all();
        press(&mut s, "Tab");
        assert_eq!(
            s.buffer.text(),
            "        a\n    b",
            "a multi-line selection indents every line"
        );
        press(&mut s, "Shift+Tab");
        assert_eq!(s.buffer.text(), "    a\nb");
    }

    #[test]
    fn effects_are_reported_for_the_app() {
        let mut s = state("one two", "txt");
        s.buffer.select_all();
        assert_eq!(
            press(&mut s, "Ctrl+C"),
            Some(Effect::Copy("one two".into()))
        );
        assert_eq!(press(&mut s, "Ctrl+X"), Some(Effect::Cut("one two".into())));
        assert_eq!(s.buffer.text(), "");
        assert_eq!(press(&mut s, "Ctrl+V"), Some(Effect::Paste));
        s.paste("pasted\ntext");
        assert_eq!(s.buffer.text(), "pasted\ntext");
        assert_eq!(press(&mut s, "Ctrl+S"), Some(Effect::Save));
        press(&mut s, "Ctrl+Shift+S");
        assert!(matches!(s.mode, Mode::SaveAs(_)));
        typed(&mut s, "x.txt");
        assert_eq!(
            press(&mut s, "Enter"),
            Some(Effect::SaveAsTo("x.txt".into()))
        );
        assert_eq!(s.mode, Mode::Edit);
        assert_eq!(press(&mut s, "F12"), Some(Effect::GotoDefinition));
        assert_eq!(press(&mut s, "Ctrl+P"), Some(Effect::QuickOpen));
        // With nothing selected, copy takes the whole line and cut removes it.
        let mut t = state("first\nsecond\nthird", "txt");
        t.buffer.set_selection(8, 8);
        assert_eq!(
            press(&mut t, "Ctrl+C"),
            Some(Effect::Copy("second\n".into()))
        );
        assert_eq!(
            press(&mut t, "Ctrl+X"),
            Some(Effect::Cut("second\n".into()))
        );
        assert_eq!(t.buffer.text(), "first\nthird");
    }

    #[test]
    fn find_replace_and_goto_prompts() {
        let mut s = state("foo bar\nfoo baz\nqux foo", "txt");
        press(&mut s, "Ctrl+F");
        typed(&mut s, "foo");
        let Mode::Find {
            matches, current, ..
        } = &s.mode
        else {
            panic!("find bar")
        };
        assert_eq!((matches.len(), *current), (3, 0));
        assert_eq!(
            s.buffer.selected_text(),
            "foo",
            "the first match is selected"
        );
        press(&mut s, "Enter");
        assert_eq!(
            s.buffer.line_col(s.buffer.cursor()).0,
            1,
            "next match is on line 2"
        );
        press(&mut s, "Shift+Enter");
        assert_eq!(s.buffer.line_col(s.buffer.selections()[0].range().0).0, 0);
        press(&mut s, "Escape");
        assert_eq!(s.mode, Mode::Edit);
        press(&mut s, "Escape"); // collapse the selection: the find bar is seeded from a selection
                                 // Replace all with Ctrl+H, Tab to the replacement field, Ctrl+Alt+Enter.
        press(&mut s, "Ctrl+H");
        typed(&mut s, "ba");
        press(&mut s, "Tab");
        typed(&mut s, "BA");
        press(&mut s, "Ctrl+Alt+Enter");
        assert_eq!(s.buffer.text(), "foo BAr\nfoo BAz\nqux foo");
        assert_eq!(s.notice.as_deref(), Some("replaced 2"));
        press(&mut s, "Escape");
        // Go to line.
        press(&mut s, "Ctrl+G");
        typed(&mut s, "3x"); // letters are ignored
        press(&mut s, "Enter");
        assert_eq!(s.buffer.line_col(s.buffer.cursor()).0, 2);
        press(&mut s, "Ctrl+G");
        press(&mut s, "Escape");
        assert_eq!(s.mode, Mode::Edit);
    }

    #[test]
    fn ctrl_d_selects_a_word_then_adds_the_next_occurrence() {
        let mut s = state("let x = 1;\nlet y = x;\nx", "rs");
        s.buffer.set_selection(4, 4); // on the first x
        press(&mut s, "Ctrl+D");
        assert_eq!(s.buffer.selected_text(), "x");
        press(&mut s, "Ctrl+D");
        assert_eq!(s.buffer.selections().len(), 2);
        press(&mut s, "Ctrl+D");
        assert_eq!(s.buffer.selections().len(), 3);
        typed(&mut s, "z");
        assert_eq!(
            s.buffer.text(),
            "let z = 1;\nlet y = z;\nz",
            "all three change together"
        );
        press(&mut s, "Ctrl+D");
        press(&mut s, "Ctrl+D");
        press(&mut s, "Ctrl+D");
        press(&mut s, "Ctrl+D");
        assert!(
            s.notice.is_some() || s.buffer.selections().len() >= 3,
            "running out of matches is reported, not an error"
        );
    }

    #[test]
    fn toggle_comment_adds_and_removes_markers() {
        let mut s = state("fn a() {\n    x();\n\n    y();\n}", "rs");
        s.buffer.select_all();
        press(&mut s, "Ctrl+/");
        assert_eq!(
            s.buffer.text(),
            "// fn a() {\n//     x();\n\n//     y();\n// }"
        );
        press(&mut s, "Ctrl+/");
        assert_eq!(s.buffer.text(), "fn a() {\n    x();\n\n    y();\n}");
        press(&mut s, "Ctrl+Z");
        assert_eq!(
            s.buffer.text(),
            "// fn a() {\n//     x();\n\n//     y();\n// }",
            "one undo per toggle"
        );
        let mut py = state("a = 1\nb = 2", "py");
        py.buffer.select_all();
        press(&mut py, "Ctrl+/");
        assert_eq!(py.buffer.text(), "# a = 1\n# b = 2");
        let mut txt = state("plain", "txt");
        press(&mut txt, "Ctrl+/");
        assert!(txt.notice.as_deref().unwrap().contains("no line comment"));
        assert_eq!(txt.buffer.text(), "plain");
    }

    #[test]
    fn clicks_scroll_and_goto_keep_the_cursor_visible() {
        let text: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let mut s = state(&text, "txt");
        s.rows = 20;
        s.click(50, 3, false);
        assert_eq!(s.buffer.line_col(s.buffer.cursor()), (50, 3));
        s.click(55, 99, true);
        assert_eq!(
            s.buffer.selections()[0].anchor,
            s.buffer.line_range(50).0 + 3,
            "drag keeps the anchor"
        );
        assert_eq!(
            s.buffer.cursor(),
            s.buffer.line_range(55).1,
            "a column past the end stops at the end"
        );
        s.goto(150, 1);
        assert!(
            s.scroll <= 149 && s.scroll + s.rows > 149,
            "line 150 is on screen: scroll {}",
            s.scroll
        );
        s.scroll_by(-10_000);
        assert_eq!(s.scroll, 0);
        s.scroll_by(10_000);
        assert!(s.scroll < 201);
        s.double_click(0, 2);
        assert_eq!(s.buffer.selected_text(), "line");
        assert_eq!(s.position_label(), "Ln 1, Col 5");
    }
}
