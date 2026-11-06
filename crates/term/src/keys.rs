//! Keyboard input to PTY byte sequences (xterm-compatible).

/// A key independent of any UI toolkit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// Function key F1..=F12.
    F(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    /// xterm modifier parameter (1 + shift + 2*alt + 4*ctrl); 1 means no modifiers.
    fn param(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }
    fn any(self) -> bool {
        self.param() != 1
    }
}

fn csi_final(final_byte: char, app_cursor: bool, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[1;{}{}", mods.param(), final_byte).into_bytes()
    } else if app_cursor {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

fn csi_tilde(n: u8, mods: Mods) -> Vec<u8> {
    if mods.any() {
        format!("\x1b[{};{}~", n, mods.param()).into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

/// Encode a key press. `app_cursor` is the DECCKM (application cursor keys) mode.
/// Returns `None` for keys with no encoding.
pub fn encode_key(key: Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    Some(match key {
        Key::Char(c) => {
            let mut out = Vec::new();
            if mods.alt {
                out.push(0x1b);
            }
            if mods.ctrl {
                let b = match c {
                    'a'..='z' => c as u8 - b'a' + 1,
                    'A'..='Z' => c as u8 - b'A' + 1,
                    ' ' | '@' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '7' | '/' => 0x1f,
                    '8' | '?' => 0x7f,
                    _ => return Some(plain(c, mods.alt)),
                };
                out.push(b);
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
            out
        }
        Key::Enter => alt_prefix(mods, b"\r"),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefix(mods, b"\t"),
        Key::Backspace if mods.ctrl => alt_prefix(mods, b"\x08"),
        Key::Backspace => alt_prefix(mods, b"\x7f"),
        Key::Escape => alt_prefix(mods, b"\x1b"),
        Key::Up => csi_final('A', app_cursor, mods),
        Key::Down => csi_final('B', app_cursor, mods),
        Key::Right => csi_final('C', app_cursor, mods),
        Key::Left => csi_final('D', app_cursor, mods),
        Key::Home => csi_final('H', app_cursor, mods),
        Key::End => csi_final('F', app_cursor, mods),
        Key::PageUp => csi_tilde(5, mods),
        Key::PageDown => csi_tilde(6, mods),
        Key::Insert => csi_tilde(2, mods),
        Key::Delete => csi_tilde(3, mods),
        Key::F(n) => match n {
            1..=4 => {
                let f = ['P', 'Q', 'R', 'S'][usize::from(n - 1)];
                if mods.any() {
                    format!("\x1b[1;{}{}", mods.param(), f).into_bytes()
                } else {
                    format!("\x1bO{f}").into_bytes()
                }
            }
            5 => csi_tilde(15, mods),
            6 => csi_tilde(17, mods),
            7 => csi_tilde(18, mods),
            8 => csi_tilde(19, mods),
            9 => csi_tilde(20, mods),
            10 => csi_tilde(21, mods),
            11 => csi_tilde(23, mods),
            12 => csi_tilde(24, mods),
            _ => return None,
        },
    })
}

fn plain(c: char, alt: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    out
}

fn alt_prefix(mods: Mods, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if mods.alt {
        out.push(0x1b);
    }
    out.extend_from_slice(bytes);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
    };
    const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
    };
    const ALT: Mods = Mods {
        shift: false,
        alt: true,
        ctrl: false,
    };

    fn enc(k: Key, m: Mods) -> Vec<u8> {
        encode_key(k, m, false).unwrap()
    }

    #[test]
    fn printable_and_utf8() {
        assert_eq!(enc(Key::Char('a'), NONE), b"a");
        assert_eq!(enc(Key::Char('ş'), NONE), "ş".as_bytes());
    }

    #[test]
    fn control_characters() {
        assert_eq!(enc(Key::Char('c'), CTRL), [3]);
        assert_eq!(enc(Key::Char('d'), CTRL), [4]);
        assert_eq!(enc(Key::Char('['), CTRL), [0x1b]);
        assert_eq!(enc(Key::Char(' '), CTRL), [0]);
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(enc(Key::Char('b'), ALT), b"\x1bb");
    }

    #[test]
    fn enter_tab_backspace() {
        assert_eq!(enc(Key::Enter, NONE), b"\r");
        assert_eq!(enc(Key::Tab, NONE), b"\t");
        assert_eq!(
            enc(
                Key::Tab,
                Mods {
                    shift: true,
                    ..NONE
                }
            ),
            b"\x1b[Z"
        );
        assert_eq!(enc(Key::Backspace, NONE), [0x7f]);
    }

    #[test]
    fn arrows_respect_application_mode_and_modifiers() {
        assert_eq!(enc(Key::Up, NONE), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, NONE, true).unwrap(), b"\x1bOA");
        assert_eq!(enc(Key::Left, CTRL), b"\x1b[1;5D");
        assert_eq!(
            enc(
                Key::Right,
                Mods {
                    shift: true,
                    alt: false,
                    ctrl: true
                }
            ),
            b"\x1b[1;6C"
        );
    }

    #[test]
    fn editing_and_function_keys() {
        assert_eq!(enc(Key::Delete, NONE), b"\x1b[3~");
        assert_eq!(enc(Key::PageUp, NONE), b"\x1b[5~");
        assert_eq!(enc(Key::Delete, CTRL), b"\x1b[3;5~");
        assert_eq!(enc(Key::F(1), NONE), b"\x1bOP");
        assert_eq!(enc(Key::F(5), NONE), b"\x1b[15~");
        assert_eq!(enc(Key::F(12), NONE), b"\x1b[24~");
        assert_eq!(encode_key(Key::F(13), NONE, false), None);
    }
}
