//! crossterm KeyEvent → 终端输入字节序列（xterm 约定）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 把一次按键编码为应写入 pty 的字节。返回 None 表示忽略（KeyUp 等）。
pub fn encode_key(ev: &KeyEvent) -> Option<Vec<u8>> {
    if !matches!(ev.kind, crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat) {
        return None;
    }
    let mods = ev.modifiers;
    let out = match ev.code {
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => b"\x1b[Z".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Backspace if mods.contains(KeyModifiers::ALT) => b"\x1b\x7f".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Left => arrow(b'D', mods),
        KeyCode::Right => arrow(b'C', mods),
        KeyCode::Up => arrow(b'A', mods),
        KeyCode::Down => arrow(b'B', mods),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::F(n) => fkey(n)?,
        KeyCode::Char(c) => {
            if mods.contains(KeyModifiers::CONTROL) {
                // 仅处理 Ctrl+字母/@.._（0x00-0x1f），其余 Ctrl 组合忽略
                let b = c.to_ascii_lowercase() as u8;
                if c.is_ascii_alphabetic() || (b'@'..=b'_').contains(&b) {
                    vec![b & 0x1f]
                } else {
                    return None;
                }
            } else if mods.contains(KeyModifiers::ALT) {
                let mut v = vec![0x1b];
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                v
            } else {
                let mut v = Vec::new();
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                v
            }
        }
        _ => return None,
    };
    Some(out)
}

fn arrow(last: u8, mods: KeyModifiers) -> Vec<u8> {
    let mut m = 1u8;
    if mods.contains(KeyModifiers::SHIFT) {
        m += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        m += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        m += 4;
    }
    if m == 1 {
        vec![0x1b, b'[', last]
    } else {
        vec![0x1b, b'[', b'1', b';', b'0' + m, last]
    }
}

fn fkey(n: u8) -> Option<Vec<u8>> {
    Some(match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => return None,
    })
}
