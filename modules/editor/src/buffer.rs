//! The text buffer: rope, selections, edits, undo and saving.

use std::path::{Path, PathBuf};

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use crate::history::{Edit, History, Kind, Step};
use crate::io::{self, DiskStamp, LineEnding, OpenError, SaveError};
use crate::search::{self, Match, SearchOptions};

/// A selection: `anchor` stays, `head` moves (both are character indexes). Empty when equal: that is a cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn cursor(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// `(start, end)` with `start <= end`.
    pub fn range(&self) -> (usize, usize) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }
}

/// Where a cursor moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    WordLeft,
    WordRight,
    DocStart,
    DocEnd,
}

/// The editable text of one file (or of no file yet).
#[derive(Debug)]
pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    selections: Vec<Selection>,
    history: History,
    ending: LineEnding,
    bom: bool,
    stamp: Option<DiskStamp>,
    /// `history.top_id()` when the file was read or last saved: the text is clean while it matches.
    saved_top: Option<u64>,
    /// Column kept while moving up and down through shorter lines.
    desired_col: Option<usize>,
    /// Where the last typed character or deletion ended, to join consecutive ones into one undo step.
    last_end: Option<usize>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Default for Buffer {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl Buffer {
    /// A buffer holding `text` (line endings are normalised to `\n`).
    pub fn from_text(text: &str) -> Self {
        Self {
            rope: Rope::from_str(&text.replace("\r\n", "\n")),
            path: None,
            selections: vec![Selection::cursor(0)],
            history: History::default(),
            ending: LineEnding::detect(text),
            bom: false,
            stamp: None,
            saved_top: None,
            desired_col: None,
            last_end: None,
        }
    }

    /// Read a file. Large files and files that are not UTF-8 are refused (view them instead).
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let loaded = io::read(path)?;
        let mut b = Self::from_text(&loaded.text);
        b.path = Some(path.to_path_buf());
        b.ending = loaded.ending;
        b.bom = loaded.bom;
        b.stamp = loaded.stamp;
        Ok(b)
    }

    // ---- reading ----

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Number of lines (an empty buffer, or one ending in a newline, has a last empty line).
    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Text of line `i` without its newline; empty for a line past the end.
    pub fn line(&self, i: usize) -> String {
        if i >= self.rope.len_lines() {
            return String::new();
        }
        let s = self.rope.line(i).to_string();
        s.strip_suffix('\n').unwrap_or(&s).to_owned()
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn line_ending(&self) -> LineEnding {
        self.ending
    }

    /// Whether the text differs from the file: undoing back to the saved state makes it clean again.
    pub fn is_dirty(&self) -> bool {
        self.history.top_id() != self.saved_top
    }

    pub fn selections(&self) -> &[Selection] {
        &self.selections
    }

    /// The primary (first) selection's moving end.
    pub fn cursor(&self) -> usize {
        self.selections[0].head
    }

    /// `(line, column)` of a character index, both zero-based, the column in characters.
    pub fn line_col(&self, idx: usize) -> (usize, usize) {
        let idx = idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        (line, idx - self.rope.line_to_char(line))
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    // ---- selections ----

    /// Replace all selections with one (indexes are clamped to the text).
    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        let n = self.rope.len_chars();
        self.selections = vec![Selection {
            anchor: anchor.min(n),
            head: head.min(n),
        }];
        self.desired_col = None;
        self.break_typing();
    }

    /// Add a cursor (multi-cursor); overlapping selections merge.
    pub fn add_cursor(&mut self, at: usize) {
        self.selections
            .push(Selection::cursor(at.min(self.rope.len_chars())));
        self.normalize();
    }

    pub fn select_all(&mut self) {
        self.set_selection(0, self.rope.len_chars());
    }

    /// The text of the primary selection.
    pub fn selected_text(&self) -> String {
        let (a, b) = self.selections[0].range();
        self.rope.slice(a..b).to_string()
    }

    fn normalize(&mut self) {
        self.selections.sort_by_key(|s| s.range());
        let mut out: Vec<Selection> = Vec::new();
        for s in self.selections.drain(..) {
            match out.last_mut() {
                Some(last)
                    if s.range().0 <= last.range().1
                        && (!s.is_empty() || !last.is_empty() || s == *last) =>
                {
                    let (a, b) = (last.range().0, last.range().1.max(s.range().1));
                    *last = Selection { anchor: a, head: b };
                }
                _ => out.push(s),
            }
        }
        self.selections = out;
    }

    // ---- grapheme and word boundaries ----

    /// Boundaries (character offsets from the line start) of the grapheme clusters of line `line`.
    fn line_boundaries(&self, line: usize) -> Vec<usize> {
        let text = self.line(line);
        let mut out = Vec::new();
        let mut chars = 0;
        for g in text.graphemes(true) {
            out.push(chars);
            chars += g.chars().count();
        }
        out.push(chars);
        out
    }

    fn prev_boundary(&self, idx: usize) -> usize {
        if idx == 0 {
            return 0;
        }
        let line = self.rope.char_to_line(idx);
        let start = self.rope.line_to_char(line);
        if idx == start {
            return idx - 1; // the newline before this line
        }
        let off = idx - start;
        self.line_boundaries(line)
            .into_iter()
            .rev()
            .find(|b| *b < off)
            .map_or(start, |b| start + b)
    }

    fn next_boundary(&self, idx: usize) -> usize {
        let n = self.rope.len_chars();
        if idx >= n {
            return n;
        }
        let line = self.rope.char_to_line(idx);
        let start = self.rope.line_to_char(line);
        let off = idx - start;
        let line_len = self.line(line).chars().count();
        if off >= line_len {
            return (idx + 1).min(n); // step over the newline
        }
        self.line_boundaries(line)
            .into_iter()
            .find(|b| *b > off)
            .map_or(start + line_len, |b| start + b)
    }

    fn word_left(&self, idx: usize) -> usize {
        let mut i = idx;
        while i > 0 && self.rope.char(i - 1).is_whitespace() {
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        let word = is_word(self.rope.char(i - 1));
        while i > 0 {
            let c = self.rope.char(i - 1);
            if c.is_whitespace() || is_word(c) != word {
                break;
            }
            i -= 1;
        }
        i
    }

    fn word_right(&self, idx: usize) -> usize {
        let n = self.rope.len_chars();
        let mut i = idx;
        while i < n && self.rope.char(i).is_whitespace() {
            i += 1;
        }
        if i >= n {
            return n;
        }
        let word = is_word(self.rope.char(i));
        while i < n {
            let c = self.rope.char(i);
            if c.is_whitespace() || is_word(c) != word {
                break;
            }
            i += 1;
        }
        i
    }

    fn grapheme_col(&self, idx: usize) -> usize {
        let line = self.rope.char_to_line(idx);
        let off = idx - self.rope.line_to_char(line);
        self.line_boundaries(line)
            .iter()
            .take_while(|b| **b < off)
            .count()
    }

    fn idx_at_col(&self, line: usize, col: usize) -> usize {
        let b = self.line_boundaries(line);
        self.rope.line_to_char(line) + b[col.min(b.len() - 1)]
    }

    // ---- moving ----

    /// Move every cursor; with `extend` the selection grows instead of collapsing.
    pub fn move_cursor(&mut self, m: Move, extend: bool) {
        let keep_col = matches!(m, Move::Up | Move::Down);
        let lines = self.rope.len_lines();
        let n = self.rope.len_chars();
        let mut desired = self.desired_col;
        for i in 0..self.selections.len() {
            let sel = self.selections[i];
            let head = sel.head;
            let line = self.rope.char_to_line(head);
            let target = match m {
                // Without `extend`, an arrow key first collapses a selection to the side it points to.
                Move::Left if !extend && !sel.is_empty() => sel.range().0,
                Move::Right if !extend && !sel.is_empty() => sel.range().1,
                Move::Left => self.prev_boundary(head),
                Move::Right => self.next_boundary(head),
                Move::WordLeft => self.word_left(head),
                Move::WordRight => self.word_right(head),
                Move::LineStart => self.rope.line_to_char(line),
                Move::LineEnd => self.rope.line_to_char(line) + self.line(line).chars().count(),
                Move::DocStart => 0,
                Move::DocEnd => n,
                Move::Up | Move::Down => {
                    let col = *desired.get_or_insert_with(|| self.grapheme_col(head));
                    if m == Move::Up && line == 0 {
                        0
                    } else if m == Move::Down && line + 1 >= lines {
                        n
                    } else {
                        let to = if m == Move::Up { line - 1 } else { line + 1 };
                        self.idx_at_col(to, col)
                    }
                }
            };
            self.selections[i] = if extend {
                Selection {
                    anchor: sel.anchor,
                    head: target,
                }
            } else {
                Selection::cursor(target)
            };
        }
        self.normalize();
        self.desired_col = if keep_col { desired } else { None };
        self.break_typing();
    }

    // ---- editing ----

    fn break_typing(&mut self) {
        self.last_end = None;
        self.history.seal();
    }

    /// Apply one edit to the rope and return it.
    fn apply_raw(&mut self, e: &Edit) {
        let removed = e.removed.chars().count();
        if removed > 0 {
            self.rope.remove(e.at..e.at + removed);
        }
        if !e.inserted.is_empty() {
            self.rope.insert(e.at, &e.inserted);
        }
    }

    /// Replace `[start, end)` by `text`, as a single undo step; selections move with the text.
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        let n = self.rope.len_chars();
        let (start, end) = (start.min(n), end.min(n));
        let (start, end) = (start.min(end), start.max(end));
        let edit = Edit {
            at: start,
            removed: self.rope.slice(start..end).to_string(),
            inserted: text.to_owned(),
        };
        if edit.removed == edit.inserted {
            return; // nothing changes: no history step, not dirty
        }
        let before = self.sel_pairs();
        self.apply_raw(&edit);
        for s in &mut self.selections {
            s.anchor = map_through(s.anchor, &edit);
            s.head = map_through(s.head, &edit);
        }
        self.desired_col = None;
        self.last_end = None;
        let after = self.sel_pairs();
        self.history.push(
            Step {
                id: 0,
                edits: vec![edit],
                kind: Kind::Other,
                before,
                after,
            },
            false,
        );
    }

    fn sel_pairs(&self) -> Vec<(usize, usize)> {
        self.selections.iter().map(|s| (s.anchor, s.head)).collect()
    }

    fn set_pairs(&mut self, pairs: &[(usize, usize)]) {
        self.selections = pairs
            .iter()
            .map(|(a, h)| Selection {
                anchor: *a,
                head: *h,
            })
            .collect();
        if self.selections.is_empty() {
            self.selections.push(Selection::cursor(0));
        }
    }

    /// Replace every selection by what `make` returns for its range; cursors end after the new text.
    fn edit_selections(
        &mut self,
        kind: Kind,
        make: impl Fn(&Self, usize, usize) -> Option<(usize, usize, String)>,
    ) {
        self.normalize();
        let before = self.sel_pairs();
        // Plan every edit against the current text (ascending), then apply from the end so indexes stay valid.
        let mut plan: Vec<(usize, usize, String)> = Vec::new();
        for s in &self.selections {
            let (a, b) = s.range();
            if let Some(p) = make(self, a, b) {
                plan.push(p);
            }
        }
        if plan.is_empty() {
            return;
        }
        plan.dedup_by(|b, a| b.0 < a.1); // overlapping plans (adjacent backspaces on one grapheme): keep the first
        let mut edits = Vec::new();
        for (a, b, text) in plan.iter().rev() {
            let e = Edit {
                at: *a,
                removed: self.rope.slice(*a..*b).to_string(),
                inserted: text.clone(),
            };
            self.apply_raw(&e);
            edits.push(e);
        }
        // New cursor of each edit: its end, shifted by the size change of every earlier edit.
        let mut shift: isize = 0;
        let mut cursors = Vec::new();
        for (a, b, text) in &plan {
            let end = (*a as isize + shift) as usize + text.chars().count();
            cursors.push(Selection::cursor(end));
            shift += text.chars().count() as isize - (*b - *a) as isize;
        }
        self.selections = cursors;
        self.desired_col = None;
        let after = self.sel_pairs();
        let join = self.selections.len() == 1
            && matches!(kind, Kind::Typing | Kind::Deleting)
            && self.joins(kind, &edits[0]);
        self.last_end = if matches!(kind, Kind::Typing | Kind::Deleting) {
            Some(edits[0].at + edits[0].inserted.chars().count())
        } else {
            None
        };
        self.history.push(
            Step {
                id: 0,
                edits,
                kind,
                before,
                after,
            },
            join,
        );
    }

    /// Does `e` directly continue the previous typing or deleting?
    fn joins(&self, kind: Kind, e: &Edit) -> bool {
        match (kind, self.last_end) {
            (Kind::Typing, Some(end)) => e.at == end && e.removed.is_empty(),
            // Backspace: this deletion ends where the last one started. Delete: it starts where the last one did.
            (Kind::Deleting, Some(end)) => e.at + e.removed.chars().count() == end || e.at == end,
            _ => false,
        }
    }

    /// Type `text` at every cursor (replacing selections). A single character joins the current typing step.
    pub fn insert_str(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n");
        let kind = if text.chars().count() == 1 && text != "\n" {
            Kind::Typing
        } else {
            Kind::Other
        };
        self.edit_selections(kind, |_, a, b| Some((a, b, text.clone())));
    }

    /// Enter: a newline that keeps the indentation of the current line.
    pub fn newline(&mut self) {
        self.edit_selections(Kind::Other, |s, a, b| {
            let line = s.rope.char_to_line(a);
            let indent: String = s
                .line(line)
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let col = a - s.rope.line_to_char(line);
            let indent: String = indent.chars().take(col).collect();
            Some((a, b, format!("\n{indent}")))
        });
    }

    /// Backspace: delete the selection, or the grapheme before the cursor.
    pub fn backspace(&mut self) {
        self.edit_selections(Kind::Deleting, |s, a, b| {
            if a != b {
                Some((a, b, String::new()))
            } else if a > 0 {
                Some((s.prev_boundary(a), a, String::new()))
            } else {
                None
            }
        });
    }

    /// Delete: remove the selection, or the grapheme after the cursor.
    pub fn delete_forward(&mut self) {
        self.edit_selections(Kind::Deleting, |s, a, b| {
            if a != b {
                Some((a, b, String::new()))
            } else if a < s.rope.len_chars() {
                Some((a, s.next_boundary(a), String::new()))
            } else {
                None
            }
        });
    }

    /// Character range `[start, end)` of line `line` without its newline.
    pub fn line_range(&self, line: usize) -> (usize, usize) {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        (start, start + self.line(line).chars().count())
    }

    /// The word (letters, digits, `_`) around `idx`, or an empty range at `idx`.
    pub fn word_range_at(&self, idx: usize) -> (usize, usize) {
        let n = self.rope.len_chars();
        let idx = idx.min(n);
        let (mut a, mut b) = (idx, idx);
        while a > 0 && is_word(self.rope.char(a - 1)) {
            a -= 1;
        }
        while b < n && is_word(self.rope.char(b)) {
            b += 1;
        }
        (a, b)
    }

    /// Add another selection (multi-cursor); overlapping ones merge.
    pub fn add_selection(&mut self, anchor: usize, head: usize) {
        let n = self.rope.len_chars();
        self.selections.push(Selection {
            anchor: anchor.min(n),
            head: head.min(n),
        });
        self.normalize();
        self.last_end = None;
    }

    /// Text of `[start, end)`.
    pub fn slice(&self, start: usize, end: usize) -> String {
        let n = self.rope.len_chars();
        self.rope.slice(start.min(n)..end.min(n)).to_string()
    }

    /// The line numbers touched by the primary selection (a selection ending at a line start excludes that line).
    pub fn primary_lines(&self) -> (usize, usize) {
        let (a, b) = self.selections[0].range();
        let first = self.rope.char_to_line(a);
        let mut last = self.rope.char_to_line(b);
        if b > a && b == self.rope.line_to_char(last) && last > first {
            last -= 1;
        }
        (first, last)
    }

    /// Copy the lines of the primary selection below themselves.
    pub fn duplicate_lines(&mut self) {
        let (first, last) = self.primary_lines();
        let start = self.rope.line_to_char(first);
        let end = self
            .rope
            .line_to_char((last + 1).min(self.rope.len_lines()));
        let mut block = self.rope.slice(start..end).to_string();
        if !block.ends_with('\n') {
            block.insert(0, '\n');
        }
        let at = end;
        let cursor = self.cursor() + block.chars().count();
        self.replace_range(at, at, &block);
        self.set_selection(cursor, cursor);
    }

    /// Delete the lines of the primary selection.
    pub fn delete_lines(&mut self) {
        let (first, last) = self.primary_lines();
        let mut start = self.rope.line_to_char(first);
        let end = self
            .rope
            .line_to_char((last + 1).min(self.rope.len_lines()));
        // Deleting the last line also takes the newline before it, so no empty line stays behind.
        if end == self.rope.len_chars() && start > 0 {
            start -= 1;
        }
        self.replace_range(start, end, "");
        let c = start.min(self.rope.len_chars());
        let line = self.rope.char_to_line(c);
        let at = self.rope.line_to_char(line);
        self.set_selection(at, at);
    }

    /// Move the lines of the primary selection one line up (`-1`) or down (`1`).
    pub fn move_lines(&mut self, delta: isize) {
        let (first, last) = self.primary_lines();
        let total = self.rope.len_lines();
        let last_real = if self.line(total - 1).is_empty() && total > 1 {
            total - 2
        } else {
            total - 1
        };
        if (delta < 0 && first == 0) || (delta > 0 && last >= last_real) {
            return;
        }
        let (other_first, other_last) = if delta < 0 {
            (first - 1, first - 1)
        } else {
            (last + 1, last + 1)
        };
        let block_text = (first..=last)
            .map(|l| self.line(l))
            .collect::<Vec<_>>()
            .join("\n");
        let other_text = (other_first..=other_last)
            .map(|l| self.line(l))
            .collect::<Vec<_>>()
            .join("\n");
        let (lo, hi) = (first.min(other_first), last.max(other_last));
        let start = self.rope.line_to_char(lo);
        let end = self.rope.line_to_char(hi) + self.line(hi).chars().count();
        let swapped = if delta < 0 {
            format!("{block_text}\n{other_text}")
        } else {
            format!("{other_text}\n{block_text}")
        };
        let (a, h) = (self.selections[0].anchor, self.selections[0].head);
        let shift = if delta < 0 {
            -(i64::try_from(other_text.chars().count()).unwrap_or(0) + 1)
        } else {
            i64::try_from(other_text.chars().count()).unwrap_or(0) + 1
        };
        let moved = |x: usize| (i64::try_from(x).unwrap_or(0) + shift).max(0) as usize;
        self.replace_range(start, end, &swapped);
        self.set_selection(moved(a), moved(h));
    }

    // ---- undo and redo ----

    /// Undo the last step. Returns false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.history.pop_undo() else {
            return false;
        };
        for e in step.edits.iter().rev() {
            self.apply_raw(&e.inverse());
        }
        self.set_pairs(&step.before);
        self.history.push_redo(step);
        self.desired_col = None;
        self.last_end = None;
        true
    }

    /// Redo the last undone step. Returns false when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(mut step) = self.history.pop_redo() else {
            return false;
        };
        for e in &step.edits {
            self.apply_raw(e);
        }
        self.set_pairs(&step.after);
        step.kind = Kind::Other;
        self.history.push_undo_keep_redo(step);
        self.desired_col = None;
        self.last_end = None;
        true
    }

    // ---- search ----

    pub fn find_all(&self, query: &str, opts: SearchOptions) -> Vec<Match> {
        search::find_all(&self.rope, query, opts)
    }

    /// Replace every match as one undo step; returns how many were replaced.
    pub fn replace_all(&mut self, query: &str, with: &str, opts: SearchOptions) -> usize {
        let matches = self.find_all(query, opts);
        if matches.is_empty() {
            return 0;
        }
        let before = self.sel_pairs();
        let mut edits = Vec::new();
        for m in matches.iter().rev() {
            let e = Edit {
                at: m.start,
                removed: self.rope.slice(m.start..m.end).to_string(),
                inserted: with.to_owned(),
            };
            self.apply_raw(&e);
            edits.push(e);
        }
        let n = self.rope.len_chars();
        for s in &mut self.selections {
            s.anchor = s.anchor.min(n);
            s.head = s.head.min(n);
        }
        let after = self.sel_pairs();
        self.last_end = None;
        self.history.push(
            Step {
                id: 0,
                edits,
                kind: Kind::Other,
                before,
                after,
            },
            false,
        );
        matches.len()
    }

    // ---- saving ----

    pub(crate) fn bytes_to_write(&self) -> Vec<u8> {
        let mut text = self.rope.to_string();
        if self.ending == LineEnding::CrLf {
            text = text.replace('\n', "\r\n");
        }
        let mut out = Vec::with_capacity(text.len() + 3);
        if self.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        out.extend_from_slice(text.as_bytes());
        out
    }

    fn write_to(&mut self, path: &Path) -> Result<(), SaveError> {
        io::write_atomic(path, &self.bytes_to_write())?;
        self.path = Some(path.to_path_buf());
        self.stamp = io::stamp(path);
        self.saved_top = self.history.top_id();
        self.break_typing();
        Ok(())
    }

    /// Prepare a save without touching the disk: run the returned job in a background task, then call
    /// [`Buffer::finish_save`] with its result. `overwrite` skips the "changed on disk" check.
    pub fn prepare_save(
        &self,
        path: Option<&Path>,
        overwrite: bool,
    ) -> Result<crate::io::SaveJob, SaveError> {
        let target = path
            .map(Path::to_path_buf)
            .or_else(|| self.path.clone())
            .ok_or(SaveError::NoPath)?;
        // A new name has nothing to conflict with.
        let same = Some(target.as_path()) == self.path.as_deref();
        Ok(crate::io::SaveJob {
            path: target,
            bytes: self.bytes_to_write(),
            expect: if same { self.stamp } else { None },
            overwrite: overwrite || !same,
        })
    }

    /// The job finished: remember the new disk state and file name, and mark the text clean.
    pub fn finish_save(&mut self, path: &Path, stamp: crate::io::SavedStamp) {
        self.path = Some(path.to_path_buf());
        self.stamp = stamp.0;
        self.saved_top = self.history.top_id();
        self.break_typing();
    }

    /// Save to the file this buffer came from. Refuses when the file changed on disk since it was read.
    pub fn save(&mut self) -> Result<(), SaveError> {
        let path = self.path.clone().ok_or(SaveError::NoPath)?;
        if let (Some(then), Some(now)) = (self.stamp, io::stamp(&path)) {
            if then != now {
                return Err(SaveError::Conflict {
                    path: path.display().to_string(),
                });
            }
        }
        self.write_to(&path)
    }

    /// Save over the file even if it changed on disk (the user said so).
    pub fn save_overwrite(&mut self) -> Result<(), SaveError> {
        let path = self.path.clone().ok_or(SaveError::NoPath)?;
        self.write_to(&path)
    }

    /// Save under a new name.
    pub fn save_as(&mut self, path: &Path) -> Result<(), SaveError> {
        self.write_to(path)
    }
}

/// Where an index goes when `e` happens.
fn map_through(idx: usize, e: &Edit) -> usize {
    let removed = e.removed.chars().count();
    let inserted = e.inserted.chars().count();
    if idx <= e.at {
        idx
    } else if idx >= e.at + removed {
        idx - removed + inserted
    } else {
        e.at + inserted
    }
}
