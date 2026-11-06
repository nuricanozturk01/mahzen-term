use std::fs;

use proptest::prelude::*;

use crate::{Buffer, Move, SaveError, SearchOptions};

fn typed(text: &str) -> Buffer {
    let mut b = Buffer::default();
    for c in text.chars() {
        b.insert_str(&c.to_string());
    }
    b
}

#[test]
fn typing_builds_text_and_one_undo_removes_the_whole_word() {
    let mut b = typed("hello");
    assert_eq!(b.text(), "hello");
    assert_eq!(b.cursor(), 5);
    assert!(b.undo(), "typing joined into one step");
    assert_eq!(b.text(), "");
    assert!(!b.can_undo());
    assert!(b.redo());
    assert_eq!(b.text(), "hello");
    assert_eq!(b.cursor(), 5, "the cursor comes back with the text");
}

#[test]
fn moving_the_cursor_ends_a_typing_step() {
    let mut b = typed("ab");
    b.move_cursor(Move::Left, false);
    b.insert_str("X");
    assert_eq!(b.text(), "aXb");
    b.undo();
    assert_eq!(b.text(), "ab", "only the typing after the move is undone");
    b.undo();
    assert_eq!(b.text(), "");
}

#[test]
fn newline_keeps_the_indentation() {
    let mut b = Buffer::from_text("    let x = 1;");
    b.move_cursor(Move::DocEnd, false);
    b.newline();
    b.insert_str("y");
    assert_eq!(b.text(), "    let x = 1;\n    y");
    // A cursor inside the indentation only carries what is left of it.
    let mut c = Buffer::from_text("        deep");
    c.set_selection(3, 3);
    c.newline();
    assert_eq!(c.text(), "   \n   ".to_owned() + "     deep");
}

#[test]
fn backspace_and_delete_remove_whole_grapheme_clusters() {
    // Thumbs up with a skin tone (2 chars), `e` plus a combining accent (2 chars), a flag (2 chars).
    let mut b = Buffer::from_text("a\u{1F44D}\u{1F3FD}b");
    b.move_cursor(Move::DocEnd, false);
    b.move_cursor(Move::Left, false);
    b.backspace();
    assert_eq!(b.text(), "ab", "the emoji and its skin tone go together");
    let mut c = Buffer::from_text("e\u{301}x");
    c.set_selection(0, 0);
    c.delete_forward();
    assert_eq!(c.text(), "x");
    let mut d = Buffer::from_text("\u{1F1E9}\u{1F1EA}z");
    d.set_selection(0, 0);
    d.move_cursor(Move::Right, false);
    assert_eq!(d.cursor(), 2, "a flag is one step");
    // Backspace at the start and delete at the end do nothing.
    let mut e = Buffer::from_text("q");
    e.backspace();
    e.set_selection(1, 1);
    e.delete_forward();
    assert_eq!(e.text(), "q");
    assert!(!e.can_undo());
}

#[test]
fn consecutive_backspaces_undo_together() {
    let mut b = Buffer::from_text("abcd");
    b.set_selection(4, 4);
    b.backspace();
    b.backspace();
    b.backspace();
    assert_eq!(b.text(), "a");
    b.undo();
    assert_eq!(b.text(), "abcd");
    assert_eq!(b.selections()[0], crate::Selection::cursor(4));
}

#[test]
fn movement_across_lines_remembers_the_column() {
    let mut b = Buffer::from_text("abcdef\nab\nabcdef");
    b.set_selection(5, 5);
    b.move_cursor(Move::Down, false);
    assert_eq!(b.line_col(b.cursor()), (1, 2), "short line: end of line");
    b.move_cursor(Move::Down, false);
    assert_eq!(b.line_col(b.cursor()), (2, 5), "the wanted column returns");
    b.move_cursor(Move::Down, false);
    assert_eq!(
        b.cursor(),
        b.len_chars(),
        "down on the last line goes to the end"
    );
    b.move_cursor(Move::DocStart, false);
    b.move_cursor(Move::Up, false);
    assert_eq!(b.cursor(), 0);
    b.move_cursor(Move::LineEnd, false);
    assert_eq!(b.cursor(), 6);
    b.move_cursor(Move::Right, false);
    assert_eq!(
        b.line_col(b.cursor()),
        (1, 0),
        "right steps over the newline"
    );
}

#[test]
fn words_selection_and_collapsing() {
    let mut b = Buffer::from_text("foo_bar  baz.qux");
    b.set_selection(0, 0);
    b.move_cursor(Move::WordRight, false);
    assert_eq!(b.cursor(), 7);
    b.move_cursor(Move::WordRight, false);
    assert_eq!(b.cursor(), 12, "after `baz`");
    b.move_cursor(Move::WordRight, false);
    assert_eq!(b.cursor(), 13, "punctuation is its own word");
    b.move_cursor(Move::WordLeft, false);
    b.move_cursor(Move::WordLeft, false);
    assert_eq!(b.cursor(), 9);
    // Extending grows a selection; a plain arrow collapses it to the side it points to.
    b.set_selection(2, 2);
    b.move_cursor(Move::Right, true);
    b.move_cursor(Move::Right, true);
    assert_eq!(b.selected_text(), "o_");
    b.move_cursor(Move::Left, false);
    assert_eq!(b.cursor(), 2);
    b.select_all();
    assert_eq!(b.selected_text(), "foo_bar  baz.qux");
    b.insert_str("x");
    assert_eq!(b.text(), "x", "typing replaces the selection");
}

#[test]
fn several_cursors_edit_together_and_undo_as_one() {
    let mut b = Buffer::from_text("a\nb\nc");
    b.set_selection(0, 0);
    b.add_cursor(2);
    b.add_cursor(4);
    assert_eq!(b.selections().len(), 3);
    b.insert_str("> ");
    assert_eq!(b.text(), "> a\n> b\n> c");
    assert_eq!(
        b.selections().iter().map(|s| s.head).collect::<Vec<_>>(),
        [2, 6, 10]
    );
    b.backspace();
    b.backspace();
    assert_eq!(b.text(), "a\nb\nc");
    b.undo();
    b.undo();
    assert_eq!(
        b.text(),
        "> a\n> b\n> c",
        "each backspace is one step for all cursors"
    );
    b.undo();
    assert_eq!(b.text(), "a\nb\nc", "the insertion is one step too");
    assert_eq!(b.selections().len(), 3, "the cursors come back");
    // Cursors that touch merge.
    b.add_cursor(0);
    assert_eq!(b.selections().len(), 3);
}

#[test]
fn line_operations() {
    let mut b = Buffer::from_text("one\ntwo\nthree");
    b.set_selection(5, 5);
    b.duplicate_lines();
    assert_eq!(b.text(), "one\ntwo\ntwo\nthree");
    b.undo();
    assert_eq!(b.text(), "one\ntwo\nthree");
    b.set_selection(5, 5);
    b.move_lines(1);
    assert_eq!(b.text(), "one\nthree\ntwo");
    b.move_lines(1);
    assert_eq!(
        b.text(),
        "one\nthree\ntwo",
        "the last line cannot go further down"
    );
    b.move_lines(-1);
    b.move_lines(-1);
    b.move_lines(-1);
    assert_eq!(b.text(), "two\none\nthree");
    b.set_selection(0, 0);
    b.delete_lines();
    assert_eq!(b.text(), "one\nthree");
    b.set_selection(b.len_chars(), b.len_chars());
    b.delete_lines();
    assert_eq!(
        b.text(),
        "one",
        "deleting the last line leaves no empty line behind"
    );
}

#[test]
fn undoing_back_to_the_saved_text_is_clean_again() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    fs::write(&f, "abc").unwrap();
    let mut b = Buffer::open(&f).unwrap();
    assert!(!b.is_dirty());
    b.set_selection(3, 3);
    b.insert_str("d");
    assert!(b.is_dirty());
    b.undo();
    assert!(!b.is_dirty(), "back to what the file holds");
    b.redo();
    assert!(b.is_dirty());
    b.save().unwrap();
    assert!(!b.is_dirty());
    // Typing right after a save starts a new step instead of joining the saved one.
    b.insert_str("e");
    assert!(b.is_dirty());
    b.undo();
    assert!(!b.is_dirty());
    assert_eq!(b.text(), "abcd");
    // Replacing text with the same text is not a change.
    b.replace_range(0, 1, "a");
    assert!(!b.is_dirty());
}

#[test]
fn replace_all_is_one_undo_step() {
    let mut b = Buffer::from_text("foo bar foo FOO");
    let n = b.replace_all("foo", "x", SearchOptions::default());
    assert_eq!(n, 3, "smart case matches all spellings");
    assert_eq!(b.text(), "x bar x x");
    assert!(b.undo());
    assert_eq!(b.text(), "foo bar foo FOO");
    assert_eq!(b.replace_all("zzz", "y", SearchOptions::default()), 0);
}

#[test]
fn files_round_trip_with_line_endings_and_bom_and_save_atomically() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    fs::write(&f, b"\xEF\xBB\xBFone\r\ntwo\r\n").unwrap();
    let mut b = Buffer::open(&f).unwrap();
    assert_eq!(b.text(), "one\ntwo\n", "held with \\n");
    assert!(!b.is_dirty());
    b.set_selection(3, 3);
    b.insert_str("!");
    assert!(b.is_dirty());
    b.save().unwrap();
    assert!(!b.is_dirty());
    assert_eq!(
        fs::read(&f).unwrap(),
        b"\xEF\xBB\xBFone!\r\ntwo\r\n",
        "CRLF and BOM are written back"
    );
    assert_eq!(
        fs::read_dir(d.path()).unwrap().count(),
        1,
        "no temp file left"
    );
}

#[test]
fn saving_refuses_to_overwrite_outside_changes_unless_told() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    fs::write(&f, "mine").unwrap();
    let mut b = Buffer::open(&f).unwrap();
    b.insert_str("!");
    fs::write(&f, "someone else wrote a longer text").unwrap();
    assert!(matches!(b.save(), Err(SaveError::Conflict { .. })));
    assert_eq!(
        fs::read_to_string(&f).unwrap(),
        "someone else wrote a longer text",
        "nothing was written"
    );
    assert!(b.is_dirty());
    b.save_overwrite().unwrap();
    assert_eq!(fs::read_to_string(&f).unwrap(), "!mine");
    // A buffer with no file needs a name.
    let mut n = Buffer::from_text("x");
    assert!(matches!(n.save(), Err(SaveError::NoPath)));
    n.save_as(&d.path().join("new.txt")).unwrap();
    assert_eq!(n.path().unwrap().file_name().unwrap(), "new.txt");
    assert!(!n.is_dirty());
}

#[test]
fn a_save_job_runs_apart_from_the_buffer_and_detects_outside_changes() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.txt");
    fs::write(&f, "one").unwrap();
    let mut b = Buffer::open(&f).unwrap();
    b.insert_str("!");
    // The job holds the bytes and can run anywhere (here: on this thread).
    let job = b.prepare_save(None, false).unwrap();
    assert_eq!(job.path, f);
    let stamp = job.run().unwrap();
    assert_eq!(fs::read_to_string(&f).unwrap(), "!one");
    assert!(
        b.is_dirty(),
        "the buffer is clean only after it is told the job finished"
    );
    b.finish_save(&f, stamp);
    assert!(!b.is_dirty());
    // Someone else writes the file; the next job refuses, an overwrite job does not.
    b.insert_str("?");
    fs::write(&f, "other and longer").unwrap();
    assert!(matches!(
        b.prepare_save(None, false).unwrap().run(),
        Err(SaveError::Conflict { .. })
    ));
    assert_eq!(fs::read_to_string(&f).unwrap(), "other and longer");
    b.prepare_save(None, true).unwrap().run().unwrap();
    assert_eq!(fs::read_to_string(&f).unwrap(), "!?one");
    // A new name never conflicts and is remembered after finish_save.
    let g = d.path().join("b.txt");
    let stamp = b.prepare_save(Some(&g), false).unwrap().run().unwrap();
    b.finish_save(&g, stamp);
    assert_eq!(b.path(), Some(g.as_path()));
    assert!(matches!(
        Buffer::from_text("x").prepare_save(None, false),
        Err(SaveError::NoPath)
    ));
}

#[test]
fn lines_and_positions() {
    let b = Buffer::from_text("ab\r\ncd\r\n");
    assert_eq!(
        b.line_count(),
        3,
        "a final newline leaves an empty last line"
    );
    assert_eq!(
        (
            b.line(0).as_str(),
            b.line(1).as_str(),
            b.line(2).as_str(),
            b.line(9).as_str()
        ),
        ("ab", "cd", "", "")
    );
    assert_eq!(b.line_col(4), (1, 1));
    assert_eq!(b.line_col(999), (2, 0), "clamped to the end");
    assert_eq!(b.line_ending(), crate::LineEnding::CrLf);
}

#[derive(Debug, Clone)]
enum Op {
    Replace { at: u8, len: u8, text: String },
}

fn op() -> impl Strategy<Value = Op> {
    (any::<u8>(), 0u8..6, "[a-z\u{e9}\u{1F600}\n ]{0,5}").prop_map(|(at, len, text)| Op::Replace {
        at,
        len,
        text,
    })
}

proptest! {
    /// Random edit scripts agree with a naive `Vec<char>` model, and undo and redo walk through the same texts.
    #[test]
    fn edits_match_a_naive_model_and_undo_redo_round_trip(ops in proptest::collection::vec(op(), 1..40)) {
        let mut b = Buffer::from_text("start\nof the text");
        let mut model: Vec<char> = "start\nof the text".chars().collect();
        let mut states = vec![model.iter().collect::<String>()];
        for Op::Replace { at, len, text } in ops {
            let start = (at as usize) % (model.len() + 1);
            let end = (start + len as usize).min(model.len());
            b.replace_range(start, end, &text);
            let text = text.replace("\r\n", "\n");
            model.splice(start..end, text.chars());
            let now: String = model.iter().collect();
            prop_assert_eq!(b.text(), now.clone());
            if states.last() != Some(&now) {
                states.push(now);
            }
        }
        // Undo walks back through every distinct state; redo walks forward again.
        let mut back = states.clone();
        while b.undo() {
            back.pop();
            if let Some(expected) = back.last() {
                prop_assert_eq!(&b.text(), expected);
            }
        }
        prop_assert_eq!(b.text(), states[0].clone());
        while b.redo() {}
        prop_assert_eq!(b.text(), states.last().unwrap().clone());
        // Selections never point past the text.
        for s in b.selections() {
            prop_assert!(s.anchor <= b.len_chars() && s.head <= b.len_chars());
        }
    }

    /// Moving and deleting by graphemes never panics and never leaves the text.
    #[test]
    fn movement_and_deletion_are_total(text in "[a-z \u{e9}\u{301}\u{1F44D}\u{1F3FD}\n]{0,30}", moves in proptest::collection::vec(0u8..12, 0..30)) {
        let mut b = Buffer::from_text(&text);
        for m in moves {
            match m {
                0 => b.move_cursor(Move::Left, false),
                1 => b.move_cursor(Move::Right, false),
                2 => b.move_cursor(Move::Up, false),
                3 => b.move_cursor(Move::Down, true),
                4 => b.move_cursor(Move::WordLeft, false),
                5 => b.move_cursor(Move::WordRight, true),
                6 => b.move_cursor(Move::LineStart, false),
                7 => b.move_cursor(Move::LineEnd, true),
                8 => b.backspace(),
                9 => b.delete_forward(),
                10 => b.insert_str("x"),
                _ => { b.undo(); }
            }
            prop_assert!(b.cursor() <= b.len_chars());
        }
    }
}
