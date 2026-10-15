//! OSC 7（工作目录通知）旁路扫描器。
//!
//! 约定（iTerm2/VS Code 等通行的 shell 集成）：shell 在每次提示符前输出
//! `ESC ] 7 ; file://<host><path> ST`（ST = `ESC \` 或 BEL）。远端可能由
//! PROMPT_COMMAND / precmd 钩子产生，也可能本来就有（vte.sh 集成）。
//!
//! 本扫描器在字节流喂给 alacritty 解析器**之前**旁路识别该序列（alacritty
//! 对未识别的 OSC 码静默忽略，双通道互不干扰）；跨块截断的序列由内部
//! 缓冲拼接。路径按 file URL 规则做百分号解码。

/// 跨块缓冲上限：正常 OSC 7 载荷远小于此；超长视为脏数据整体丢弃。
const MAX_PENDING: usize = 4096;

/// 有状态的 OSC 7 扫描器（每个终端实例一个）。
#[derive(Default)]
pub struct Osc7Scanner {
    /// 尚未消费的字节（含可能跨块截断的序列头/载荷）。
    buf: Vec<u8>,
}

impl Osc7Scanner {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// 喂入一批新字节，返回本批识别到的 cwd 路径（多条时取最后一条）。
    pub fn feed(&mut self, bytes: &[u8]) -> Option<String> {
        self.buf.extend_from_slice(bytes);
        let mut last = None;
        loop {
            let Some(start) = find_sub(&self.buf, b"\x1b]7;") else {
                // 无序列头：只留尾部 3 字节（头可能跨块，超出部分全是普通数据）。
                if self.buf.len() > 3 {
                    let keep = self.buf.len() - 3;
                    self.buf.drain(..keep);
                }
                return last;
            };
            let payload_at = start + 4;
            // 找终结符：BEL 或 ESC \。ESC 后字节未到时等待下一批。
            let mut end = None;
            let mut consumed = 0;
            let mut i = payload_at;
            while i < self.buf.len() {
                match self.buf[i] {
                    0x07 => {
                        end = Some(i);
                        consumed = i + 1;
                        break;
                    }
                    0x1b => {
                        if let Some(&b'\\') = self.buf.get(i + 1) {
                            end = Some(i);
                            consumed = i + 2;
                            break;
                        }
                        // 裸 ESC（非 ST）：视为载荷脏数据，放弃本序列。
                        end = Some(i);
                        consumed = i + 1;
                        break;
                    }
                    _ => i += 1,
                }
            }
            match end {
                Some(e) => {
                    if let Some(p) = extract_path(&self.buf[payload_at..e]) {
                        last = Some(p);
                    }
                    self.buf.drain(..consumed);
                }
                None => {
                    // 头已到、尾未到：保留头之后等待续块；拖过久判脏丢弃。
                    if self.buf.len() - payload_at > MAX_PENDING {
                        self.buf.clear();
                    } else if start > 0 {
                        self.buf.drain(..start);
                    }
                    return last;
                }
            }
        }
    }
}

/// 朴素子串查找（buf 小，无需 KMP）。
fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 载荷 `file://host/path` → 路径（仅接受 file 协议；百分号解码）。
fn extract_path(payload: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(payload).ok()?;
    let rest = s.strip_prefix("file://")?;
    // 跳过 authority（host），路径从其后的 '/' 开始。
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        None => return None,
    };
    if path.len() <= 1 {
        return None;
    }
    Some(percent_decode(path))
}

/// 百分号解码（%XX）；非法转义原样保留。
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_st_and_bel() {
        let mut s = Osc7Scanner::new();
        assert_eq!(
            s.feed(b"before \x1b]7;file://host/root\x1b\\ after"),
            Some("/root".into())
        );
        assert_eq!(s.feed(b"\x1b]7;file://h/home/t\x07tail"), Some("/home/t".into()));
        assert_eq!(s.feed(b"plain text"), None);
    }

    #[test]
    fn split_across_chunks() {
        let mut s = Osc7Scanner::new();
        assert_eq!(s.feed(b"\x1b"), None);
        assert_eq!(s.feed(b"]"), None);
        assert_eq!(s.feed(b"7;fi"), None);
        assert_eq!(s.feed(b"le://host/a b"), None);
        assert_eq!(s.feed(b"\x1b\\"), Some("/a b".into()));
    }

    #[test]
    fn percent_decoded_and_last_wins() {
        let mut s = Osc7Scanner::new();
        let out = s.feed(b"\x1b]7;file://h/my%20dir\x07\x1b]7;file://h/other\x07");
        assert_eq!(out, Some("/other".into()));
        let mut s2 = Osc7Scanner::new();
        assert_eq!(s2.feed(b"\x1b]7;file://h/a%2Fb\x07"), Some("/a/b".into()));
    }

    #[test]
    fn non_file_and_garbage_ignored() {
        let mut s = Osc7Scanner::new();
        assert_eq!(s.feed(b"\x1b]0;title\x07"), None);
        assert_eq!(s.feed(b"\x1b]7;http://x/y\x07"), None);
        assert_eq!(s.feed(b"\x1b]7;file://hostonly\x07"), None);
        assert_eq!(s.feed(b"\x1b]7;file://h/\x07"), None);
        // 载荷里裸 ESC（非 ST）：放弃该序列，继续扫后面的。
        assert_eq!(s.feed(b"\x1b]7;bad\x1bX\x1b]7;file://h/ok\x07"), Some("/ok".into()));
    }

    #[test]
    fn ordinary_bytes_do_not_accumulate() {
        let mut s = Osc7Scanner::new();
        s.feed(&vec![b'x'; 100_000]);
        assert!(s.buf.len() <= 3, "普通数据不留缓冲: {}", s.buf.len());
        // 大量数据中夹一条仍能识别。
        let mut big = vec![b'y'; 100_000];
        big.extend_from_slice(b"\x1b]7;file://h/z\x07");
        assert_eq!(s.feed(&big), Some("/z".into()));
    }
}
