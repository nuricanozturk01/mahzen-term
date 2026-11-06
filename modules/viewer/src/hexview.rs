//! Hex dump and a one-line identification of what a file is (by its first bytes).

use crate::ansi::{paint, Style, DIM, PLAIN};
use crate::Options;

const ACCENT: Style = Style::fg(6);

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn elf(b: &[u8]) -> Option<String> {
    if !b.starts_with(b"\x7fELF") {
        return None;
    }
    let bits = match b.get(4)? {
        1 => "32-bit",
        2 => "64-bit",
        _ => "?-bit",
    };
    let little = *b.get(5)? == 1;
    let rd16 = |at| {
        let v = u16le(b, at)?;
        Some(if little { v } else { v.swap_bytes() })
    };
    let kind = match rd16(16)? {
        1 => "relocatable object",
        2 => "executable",
        3 => "shared object or PIE executable",
        4 => "core dump",
        _ => "unknown type",
    };
    let machine = match rd16(18)? {
        0x03 => "x86",
        0x3E => "x86-64",
        0x28 => "ARM",
        0xB7 => "AArch64",
        0xF3 => "RISC-V",
        0x08 => "MIPS",
        0x15 => "PowerPC64",
        _ => "other machine",
    };
    let entry = if b.get(4) == Some(&2) {
        let v = u64le(b, 24)?;
        if little {
            v
        } else {
            v.swap_bytes()
        }
    } else {
        let v = u32le(b, 24)?;
        u64::from(if little { v } else { v.swap_bytes() })
    };
    Some(format!(
        "ELF {bits} {} {kind}, {machine}, entry point 0x{entry:x}",
        if little {
            "little-endian"
        } else {
            "big-endian"
        }
    ))
}

fn pe(b: &[u8]) -> Option<String> {
    if !b.starts_with(b"MZ") {
        return None;
    }
    let off = u32le(b, 0x3C)? as usize;
    if b.get(off..off + 4)? != b"PE\0\0" {
        return Some("MS-DOS executable (no PE header)".to_owned());
    }
    let machine = match u16le(b, off + 4)? {
        0x14C => "x86",
        0x8664 => "x86-64",
        0xAA64 => "ARM64",
        0x1C4 => "ARM",
        _ => "other machine",
    };
    let sections = u16le(b, off + 6)?;
    let dll = u16le(b, off + 22)? & 0x2000 != 0;
    Some(format!(
        "Windows PE {} for {machine}, {sections} sections",
        if dll { "DLL" } else { "executable" }
    ))
}

/// A short description of the file from its first bytes, if they are recognisable.
pub fn identify(b: &[u8]) -> Option<String> {
    if let Some(s) = elf(b).or_else(|| pe(b)) {
        return Some(s);
    }
    let simple: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "PNG image"),
        (b"\xff\xd8\xff", "JPEG image"),
        (b"GIF87a", "GIF image"),
        (b"GIF89a", "GIF image"),
        (b"%PDF-", "PDF document"),
        (
            b"PK\x03\x04",
            "zip archive (also docx, xlsx, jar, apk, epub ...)",
        ),
        (b"\x1f\x8b", "gzip compressed data"),
        (b"SQLite format 3\0", "SQLite 3 database"),
        (b"\xcf\xfa\xed\xfe", "Mach-O 64-bit executable"),
        (b"\xce\xfa\xed\xfe", "Mach-O 32-bit executable"),
        (b"\xca\xfe\xba\xbe", "Mach-O universal binary or Java class"),
        (b"#!", "script with a shebang"),
        (b"\0asm", "WebAssembly module"),
        (b"RIFF", "RIFF container (WAV, AVI, WebP ...)"),
        (b"BM", "BMP image"),
        (b"7z\xbc\xaf\x27\x1c", "7-Zip archive"),
        (b"Rar!", "RAR archive"),
        (b"\xfd7zXZ\0", "xz compressed data"),
        (b"BZh", "bzip2 compressed data"),
        (b"OggS", "Ogg media"),
        (b"ID3", "MP3 audio with ID3 tag"),
        (b"fLaC", "FLAC audio"),
    ];
    simple
        .iter()
        .find(|(magic, _)| b.starts_with(magic))
        .map(|(_, name)| (*name).to_owned())
}

/// Classic dump: offset, 16 bytes in two groups of 8, and the printable characters.
/// `offset` and `length` select a window of `bytes` (`length` 0 means to the end, but at most 1 MiB are shown).
pub fn render(bytes: &[u8], offset: usize, length: usize, opts: &Options) -> String {
    const MAX_SHOWN: usize = 1024 * 1024;
    let start = offset.min(bytes.len());
    let want = if length == 0 {
        bytes.len() - start
    } else {
        length
    };
    let end = (start + want.min(MAX_SHOWN)).min(bytes.len());
    let mut out = String::new();
    let head = identify(bytes).map_or_else(
        || format!("{} bytes", bytes.len()),
        |d| format!("{d}, {} bytes", bytes.len()),
    );
    out.push_str(&paint(&head, DIM, opts.color));
    out.push('\n');
    for (row, chunk) in bytes[start..end].chunks(16).enumerate() {
        let addr = start + row * 16;
        out.push_str(&paint(&format!("{addr:08x}  "), DIM, opts.color));
        for i in 0..16 {
            match chunk.get(i) {
                Some(&b) => {
                    let st = if b == 0 {
                        DIM
                    } else if b.is_ascii_graphic() {
                        ACCENT
                    } else {
                        PLAIN
                    };
                    out.push_str(&paint(&format!("{b:02x}"), st, opts.color));
                }
                None => out.push_str("  "),
            }
            out.push(' ');
            if i == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        out.push('|');
        for &b in chunk {
            let ch = if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '.'
            };
            out.push_str(&paint(
                &ch.to_string(),
                if ch == '.' { DIM } else { PLAIN },
                opts.color,
            ));
        }
        out.push_str("|\n");
    }
    if end < bytes.len() {
        out.push_str(&paint(
            &format!(
                "(\u{2026} {} more bytes; use --offset and --length)",
                bytes.len() - end
            ),
            DIM,
            opts.color,
        ));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Options {
        Options {
            width: 80,
            color: false,
            line_numbers: false,
        }
    }

    #[test]
    fn dump_has_the_classic_layout() {
        let t = render(b"Hello, world!\n\x00\xff", 0, 0, &plain());
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "16 bytes");
        assert!(
            lines[1].starts_with("00000000  48 65 6c 6c 6f 2c 20 77  6f 72 6c 64 21 0a 00 ff"),
            "{}",
            lines[1]
        );
        assert!(lines[1].ends_with("|Hello, world!...|"), "{}", lines[1]);
        // A short last row is padded so the text column lines up.
        let t = render(&[0x41; 20], 0, 0, &plain());
        let rows: Vec<&str> = t.lines().skip(1).collect();
        assert_eq!(rows[0].find('|'), rows[1].find('|'));
        assert!(rows[1].starts_with("00000010  41 41 41 41"));
    }

    #[test]
    fn windows_and_limits() {
        let data: Vec<u8> = (0..=255).collect();
        let t = render(&data, 0x20, 16, &plain());
        assert!(t.contains("00000020  20 21 22 23"), "{t}");
        assert!(t.contains("more bytes; use --offset and --length"), "{t}");
        assert!(
            render(&data, 9999, 0, &plain()).lines().count() == 1,
            "an offset past the end shows only the header"
        );
        let big = vec![7u8; 2 * 1024 * 1024];
        assert!(
            render(&big, 0, 0, &plain()).contains("more bytes"),
            "at most 1 MiB is shown"
        );
    }

    #[test]
    fn elf_headers_are_summarised() {
        let mut h = vec![0u8; 64];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2; // 64-bit
        h[5] = 1; // little endian
        h[16] = 3; // DYN
        h[18] = 0x3E; // x86-64
        h[24..32].copy_from_slice(&0x1040u64.to_le_bytes());
        assert_eq!(
            identify(&h).unwrap(),
            "ELF 64-bit little-endian shared object or PIE executable, x86-64, entry point 0x1040"
        );
        // The real ls binary of this machine, when present.
        if let Ok(ls) = std::fs::read("/bin/ls") {
            assert!(identify(&ls).unwrap().starts_with("ELF 64-bit"));
        }
        assert!(
            identify(b"\x7fELF").is_none() || identify(b"\x7fELF").is_some(),
            "a cut header never panics"
        );
    }

    #[test]
    fn pe_and_simple_signatures() {
        let mut pe = vec![0u8; 0x100];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3C] = 0x80;
        pe[0x80..0x84].copy_from_slice(b"PE\0\0");
        pe[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        pe[0x86..0x88].copy_from_slice(&5u16.to_le_bytes());
        pe[0x96..0x98].copy_from_slice(&0x2022u16.to_le_bytes());
        assert_eq!(
            identify(&pe).unwrap(),
            "Windows PE DLL for x86-64, 5 sections"
        );
        assert_eq!(
            identify(b"MZ\0\0").unwrap_or_default(),
            "",
            "MZ without a PE offset is not claimed"
        );
        assert_eq!(identify(b"\x89PNG\r\n\x1a\nrest").unwrap(), "PNG image");
        assert_eq!(identify(b"%PDF-1.7").unwrap(), "PDF document");
        assert_eq!(
            identify(b"SQLite format 3\0abc").unwrap(),
            "SQLite 3 database"
        );
        assert_eq!(identify(b"#!/bin/sh\n").unwrap(), "script with a shebang");
        assert!(identify(b"plain text").is_none());
        assert!(identify(b"").is_none());
    }
}
