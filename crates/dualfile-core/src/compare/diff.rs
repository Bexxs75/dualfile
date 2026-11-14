use crate::error::{Error, Result};
use crate::vfs::{VPath, Vfs};
use std::collections::HashMap;

pub const MSG_TOO_LARGE: &str = "Die Datei ist zu groß für den Textvergleich";
pub const MSG_BINARY: &str = "Binärdateien können nicht als Text verglichen werden";

/// Bytes at the start that are searched for NUL to detect binary files.
const BINARY_PROBE: usize = 8 * 1024;
/// Largest edit distance the exact search follows (memory grows with its square). Beyond
/// it the differing middle part is reported as one replaced block: still a correct
/// diff, only not a minimal one.
const MAX_EDIT_DISTANCE: usize = 2000;

#[derive(Clone, Debug)]
pub struct DiffOptions {
    /// Ignore all white space (including a trailing `\r`) when comparing lines.
    pub ignore_whitespace: bool,
    pub ignore_case: bool,
    /// Largest accepted size per side in bytes.
    pub max_bytes: u64,
    /// Most lines per side.
    pub max_lines: usize,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            ignore_case: false,
            max_bytes: 5 * 1024 * 1024,
            max_lines: 100_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HunkKind {
    Equal,
    /// Only on the left.
    Delete,
    /// Only on the right.
    Insert,
    /// Lines on both sides that differ.
    Replace,
}

/// A run of lines. The hunks of a result cover both files completely and in order.
/// Line numbers are 0-based; lines carry no `\n` (a `\r` stays). With `Equal` hunks the
/// two sides may still differ in ignored white space or case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffHunk {
    pub kind: HunkKind,
    pub left_start: usize,
    pub left: Vec<String>,
    pub right_start: usize,
    pub right: Vec<String>,
}

struct Lines {
    text: Vec<String>,
    keys: Vec<String>,
}

fn split_lines(data: &[u8], opts: &DiffOptions) -> Result<Lines> {
    if data.len() as u64 > opts.max_bytes {
        return Err(Error::Unsupported(MSG_TOO_LARGE));
    }
    if data[..data.len().min(BINARY_PROBE)].contains(&0) {
        return Err(Error::Unsupported(MSG_BINARY));
    }
    let content = String::from_utf8_lossy(data);
    let mut text = Vec::new();
    let mut keys = Vec::new();
    let mut rest: &str = &content;
    while !rest.is_empty() {
        let (line, ended) = match rest.find('\n') {
            Some(i) => {
                let line = &rest[..i];
                rest = &rest[i + 1..];
                (line, true)
            }
            None => {
                let line = rest;
                rest = "";
                (line, false)
            }
        };
        if text.len() >= opts.max_lines {
            return Err(Error::Unsupported(MSG_TOO_LARGE));
        }
        let mut key: String = if opts.ignore_whitespace {
            line.chars().filter(|c| !c.is_whitespace()).collect()
        } else {
            line.to_string()
        };
        if opts.ignore_case {
            key = key.to_lowercase();
        }
        // A last line without line break differs from the same line with one. Real lines
        // never contain `\n`, so the marker cannot collide.
        if !ended && !opts.ignore_whitespace {
            key.push('\n');
        }
        text.push(line.to_string());
        keys.push(key);
    }
    Ok(Lines { text, keys })
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Myers O(ND) shortest edit script; `None` if the distance exceeds `MAX_EDIT_DISTANCE`.
fn myers(a: &[u32], b: &[u32]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        let mut ops = vec![Op::Delete; a.len()];
        ops.extend(std::iter::repeat_n(Op::Insert, b.len()));
        return Some(ops);
    }
    let dmax = ((n + m) as usize).min(MAX_EDIT_DISTANCE) as isize;
    let off = dmax + 1;
    let mut v = vec![0isize; (2 * dmax + 3) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'outer: for d in 0..=dmax {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let idx = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && y >= 0 && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x == n && y == m {
                found = Some(d);
                break 'outer;
            }
            k += 2;
        }
    }
    let found = found?;
    let (mut x, mut y) = (n, m);
    let mut ops = Vec::new();
    for d in (0..=found).rev() {
        let snap = &trace[d as usize];
        let get = |k: isize| snap[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = get(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            if x == prev_x {
                ops.push(Op::Insert);
                y -= 1;
            } else {
                ops.push(Op::Delete);
                x -= 1;
            }
        }
    }
    ops.reverse();
    Some(ops)
}

/// Compares two texts line by line. Refuses binary data (NUL in the first 8 KiB) and
/// input over the limits.
pub fn diff_text(left: &[u8], right: &[u8], opts: &DiffOptions) -> Result<Vec<DiffHunk>> {
    let l = split_lines(left, opts)?;
    let r = split_lines(right, opts)?;
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut ids_of = |keys: &[String]| -> Vec<u32> {
        keys.iter()
            .map(|k| {
                let next = ids.len() as u32;
                *ids.entry(k.clone()).or_insert(next)
            })
            .collect()
    };
    let a = ids_of(&l.keys);
    let b = ids_of(&r.keys);

    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut ops = vec![Op::Equal; prefix];
    match myers(mid_a, mid_b) {
        Some(middle) => ops.extend(middle),
        None => {
            ops.extend(std::iter::repeat_n(Op::Delete, mid_a.len()));
            ops.extend(std::iter::repeat_n(Op::Insert, mid_b.len()));
        }
    }
    ops.extend(std::iter::repeat_n(Op::Equal, suffix));

    let mut hunks = Vec::new();
    let (mut li, mut ri, mut i) = (0usize, 0usize, 0usize);
    while i < ops.len() {
        let (ls, rs) = (li, ri);
        if ops[i] == Op::Equal {
            while i < ops.len() && ops[i] == Op::Equal {
                li += 1;
                ri += 1;
                i += 1;
            }
            hunks.push(DiffHunk {
                kind: HunkKind::Equal,
                left_start: ls,
                left: l.text[ls..li].to_vec(),
                right_start: rs,
                right: r.text[rs..ri].to_vec(),
            });
        } else {
            while i < ops.len() && ops[i] != Op::Equal {
                match ops[i] {
                    Op::Delete => li += 1,
                    _ => ri += 1,
                }
                i += 1;
            }
            let kind = match (li > ls, ri > rs) {
                (true, true) => HunkKind::Replace,
                (true, false) => HunkKind::Delete,
                _ => HunkKind::Insert,
            };
            hunks.push(DiffHunk {
                kind,
                left_start: ls,
                left: l.text[ls..li].to_vec(),
                right_start: rs,
                right: r.text[rs..ri].to_vec(),
            });
        }
    }
    Ok(hunks)
}

fn read_limited(vfs: &dyn Vfs, path: &VPath, opts: &DiffOptions) -> Result<Vec<u8>> {
    // stat_file refuses folders and special files (pipes block forever when opened).
    let stat = vfs.stat_file(path)?;
    if stat.size > opts.max_bytes {
        return Err(Error::Unsupported(MSG_TOO_LARGE));
    }
    // One byte more than the limit: a file that grew after stat_file is caught as well.
    let len = usize::try_from(opts.max_bytes.saturating_add(1)).unwrap_or(usize::MAX);
    let data = vfs.read_range(path, 0, len)?;
    if data.len() as u64 > opts.max_bytes {
        return Err(Error::Unsupported(MSG_TOO_LARGE));
    }
    Ok(data)
}

/// `diff_text` on two files (each through its own `Vfs`).
pub fn diff_files(
    left_vfs: &dyn Vfs,
    left: &VPath,
    right_vfs: &dyn Vfs,
    right: &VPath,
    opts: &DiffOptions,
) -> Result<Vec<DiffHunk>> {
    let l = read_limited(left_vfs, left, opts)?;
    let r = read_limited(right_vfs, right, opts)?;
    diff_text(&l, &r, opts)
}
