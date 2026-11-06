//! Scanner for OSC 133 shell-integration marks in the raw PTY byte stream.
//!
//! Shells that support it print `ESC ] 133 ; A` before the prompt, `B` after it, `C` before command output and
//! `D ; <status>` when the command ends. The terminal emulator ignores these, so the reader watches the stream
//! itself. The bytes are still passed on untouched; the scanner only reports where a mark ended.

/// One shell-integration mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// `A`: a new prompt starts.
    PromptStart,
    /// `B`: the prompt ended, the user types.
    PromptEnd,
    /// `C`: command output starts.
    OutputStart,
    /// `D`: the command finished, with its exit status when the shell sent one.
    CommandDone(Option<i32>),
}

/// Longest OSC payload kept; anything longer cannot be a 133 mark.
const MAX_PAYLOAD: usize = 32;

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    Esc,
    /// Inside `ESC ]`, collecting the payload.
    Osc,
    /// Saw `ESC` inside an OSC (probably the `ESC \` terminator).
    OscEsc,
}

/// Incremental scanner; sequences may be split across reads.
#[derive(Debug, Default)]
pub struct Scanner {
    state: State,
    payload: Vec<u8>,
}

impl Scanner {
    /// Scan `chunk`; returns each mark with the offset just after its terminator.
    pub fn scan(&mut self, chunk: &[u8]) -> Vec<(usize, Mark)> {
        let mut found = Vec::new();
        for (i, &b) in chunk.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.state = State::Osc;
                        self.payload.clear();
                    } else if b != 0x1b {
                        self.state = State::Ground;
                    }
                }
                State::Osc => match b {
                    0x07 => self.finish(i + 1, &mut found),
                    0x1b => self.state = State::OscEsc,
                    _ if self.payload.len() < MAX_PAYLOAD => self.payload.push(b),
                    _ => {
                        self.state = State::Ground;
                        self.payload.clear();
                    }
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish(i + 1, &mut found);
                    } else {
                        self.state = State::Ground;
                        self.payload.clear();
                    }
                }
            }
        }
        found
    }

    fn finish(&mut self, end: usize, found: &mut Vec<(usize, Mark)>) {
        self.state = State::Ground;
        if let Some(mark) = parse(&self.payload) {
            found.push((end, mark));
        }
        self.payload.clear();
    }
}

fn parse(payload: &[u8]) -> Option<Mark> {
    let rest = payload.strip_prefix(b"133;")?;
    let (kind, args) = match rest.iter().position(|&b| b == b';') {
        Some(p) => (&rest[..p], &rest[p + 1..]),
        None => (rest, &rest[rest.len()..]),
    };
    match kind {
        b"A" => Some(Mark::PromptStart),
        b"B" => Some(Mark::PromptEnd),
        b"C" => Some(Mark::OutputStart),
        b"D" => {
            let code = std::str::from_utf8(args)
                .ok()
                .and_then(|s| s.split(';').next())
                .and_then(|s| s.parse::<i32>().ok());
            Some(Mark::CommandDone(code))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(chunks: &[&[u8]]) -> Vec<(usize, Mark)> {
        let mut s = Scanner::default();
        chunks.iter().flat_map(|c| s.scan(c)).collect()
    }

    #[test]
    fn finds_marks_with_bel_and_st_terminators() {
        let got = all(&[b"x\x1b]133;A\x07yy\x1b]133;D;130\x1b\\z"]);
        assert_eq!(
            got,
            vec![(9, Mark::PromptStart), (24, Mark::CommandDone(Some(130)))]
        );
    }

    #[test]
    fn marks_split_across_reads_are_found() {
        let got = all(&[b"\x1b]13", b"3;", b"C\x07"]);
        assert_eq!(got, vec![(2, Mark::OutputStart)]);
    }

    #[test]
    fn other_sequences_and_junk_are_ignored() {
        assert!(all(&[b"\x1b]0;title\x07\x1b[31m\x1b]133;Z\x07"]).is_empty());
        assert!(all(&[b"\x1b]133;A"]).is_empty(), "unterminated");
        let long = [b"\x1b]133;".as_slice(), &[b'A'; 100], b"\x07"].concat();
        assert!(all(&[&long]).is_empty());
    }

    #[test]
    fn done_without_status_is_none() {
        assert_eq!(
            all(&[b"\x1b]133;D\x07"]),
            vec![(8, Mark::CommandDone(None))]
        );
        assert_eq!(
            all(&[b"\x1b]133;D;x\x07"]),
            vec![(10, Mark::CommandDone(None))]
        );
    }
}
