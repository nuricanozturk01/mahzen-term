//! Maps Iced keyboard events to terminal keys.

use iced::keyboard::key::Named;
use iced::keyboard::{Key as IKey, Modifiers};
use mahzenterm_term::{Key, Mods};

/// Convert a key press into a terminal key plus modifiers.
/// `text` is the layout-aware text produced by the press (handles dead keys, AltGr, Turkish letters).
pub fn to_term_key(key: &IKey, text: Option<&str>, modifiers: Modifiers) -> Option<(Key, Mods)> {
    let mods = Mods {
        shift: modifiers.shift(),
        alt: modifiers.alt(),
        ctrl: modifiers.control(),
    };

    if let IKey::Named(named) = key {
        let k = match named {
            Named::Enter => Key::Enter,
            Named::Tab => Key::Tab,
            Named::Backspace => Key::Backspace,
            Named::Escape => Key::Escape,
            Named::ArrowUp => Key::Up,
            Named::ArrowDown => Key::Down,
            Named::ArrowLeft => Key::Left,
            Named::ArrowRight => Key::Right,
            Named::Home => Key::Home,
            Named::End => Key::End,
            Named::PageUp => Key::PageUp,
            Named::PageDown => Key::PageDown,
            Named::Insert => Key::Insert,
            Named::Delete => Key::Delete,
            Named::Space => Key::Char(' '),
            Named::F1 => Key::F(1),
            Named::F2 => Key::F(2),
            Named::F3 => Key::F(3),
            Named::F4 => Key::F(4),
            Named::F5 => Key::F(5),
            Named::F6 => Key::F(6),
            Named::F7 => Key::F(7),
            Named::F8 => Key::F(8),
            Named::F9 => Key::F(9),
            Named::F10 => Key::F(10),
            Named::F11 => Key::F(11),
            Named::F12 => Key::F(12),
            _ => return None,
        };
        return Some((k, mods));
    }

    // Ctrl (and Alt) shortcuts need the unmodified base key; Windows reports AltGr as Ctrl+Alt,
    // in which case the produced text must win.
    let altgr_like = modifiers.control() && modifiers.alt();
    if (modifiers.control() || modifiers.alt()) && !altgr_like {
        if let IKey::Character(s) = key {
            let c = s.chars().next()?;
            let c = if modifiers.control() {
                c.to_ascii_lowercase()
            } else {
                c
            };
            return Some((
                Key::Char(c),
                Mods {
                    shift: false,
                    ..mods
                },
            ));
        }
    }

    let t = text?;
    let mut chars = t.chars();
    let c = chars.next()?;
    if c.is_control() {
        return None;
    }
    // Text may hold several chars (compose); send them all as plain characters.
    if chars.next().is_some() {
        return None;
    }
    Some((Key::Char(c), Mods::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(s: &str) -> IKey {
        IKey::Character(s.into())
    }

    #[test]
    fn plain_text_uses_layout_text() {
        let r = to_term_key(&ch("s"), Some("ş"), Modifiers::empty());
        assert_eq!(r, Some((Key::Char('ş'), Mods::default())));
    }

    #[test]
    fn ctrl_c_is_base_key_with_ctrl() {
        let r = to_term_key(&ch("c"), Some("\u{3}"), Modifiers::CTRL).unwrap();
        assert_eq!(r.0, Key::Char('c'));
        assert!(r.1.ctrl);
    }

    #[test]
    fn altgr_as_ctrl_alt_keeps_produced_text() {
        let r = to_term_key(&ch("q"), Some("@"), Modifiers::CTRL | Modifiers::ALT);
        assert_eq!(r, Some((Key::Char('@'), Mods::default())));
    }

    #[test]
    fn named_keys_map() {
        let r = to_term_key(&IKey::Named(Named::ArrowUp), None, Modifiers::SHIFT).unwrap();
        assert_eq!(r.0, Key::Up);
        assert!(r.1.shift);
        assert_eq!(
            to_term_key(&IKey::Named(Named::Shift), None, Modifiers::SHIFT),
            None
        );
    }

    #[test]
    fn bare_control_text_is_dropped() {
        assert_eq!(
            to_term_key(
                &IKey::Named(Named::CapsLock),
                Some("\u{1}"),
                Modifiers::empty()
            ),
            None
        );
    }
}
