//! NAR 格式解包器:Nix Archive → 文件树。
//!
//! NAR 是 Nix 的确定性归档格式(无时间戳/uid/gid,只有 type+name+contents+executable+symlink)。
//! 格式:字符串以 u64-LE 长度前缀 + 8 字节对齐 padding;递归节点。
//!
//! 直译自 `nix-master/src/libutil/archive.cc:178` parse()。

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NarError {
    #[error("NAR 格式错误: {0}")]
    Format(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("NAR 嵌套深度超限(max {0})")]
    TooDeep(usize),
}

/// 安全限制(同 Nix)。
const MAX_DEPTH: usize = 64;
const MAX_NAME: usize = 255;
const MAX_TARGET: usize = 4095;

/// 从 Reader 解包 NAR 到目标目录。返回解包的文件/symlink 总数。
///
/// ```text
/// "nix-archive-1" → 递归节点
/// ```
pub fn unpack(reader: &mut impl Read, dest: &Path) -> Result<usize, NarError> {
    let magic = read_string(reader)?;
    if magic != "nix-archive-1" {
        return Err(NarError::Format(format!("bad magic: {magic:?}")));
    }
    let mut count = 0;
    parse_node(reader, dest, 0, &mut count)?;
    Ok(count)
}

fn parse_node(r: &mut impl Read, dest: &Path, depth: usize, count: &mut usize) -> Result<(), NarError> {
    if depth >= MAX_DEPTH {
        return Err(NarError::TooDeep(MAX_DEPTH));
    }
    expect(r, "(")?;
    expect(r, "type")?;
    let typ = read_string(r)?;

    match typ.as_str() {
        "regular" => {
            let mut executable = false;
            let mut tag = read_string(r)?;
            if tag == "executable" {
                executable = true;
                let marker = read_string(r)?;
                if !marker.is_empty() {
                    return Err(NarError::Format("executable marker non-empty".into()));
                }
                tag = read_string(r)?;
            }
            if tag != "contents" {
                return Err(NarError::Format(format!("expected 'contents', got '{tag}'")));
            }
            let size = read_u64(r)?;
            // 读内容
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = fs::File::create(dest)?;
            copy_exact(r, &mut file, size)?;
            // 对齐 padding
            let pad = (8 - (size % 8)) % 8;
            if pad > 0 {
                let mut skip = vec![0u8; pad as usize];
                r.read_exact(&mut skip)?;
            }
            if executable {
                fs::set_permissions(dest, fs::Permissions::from_mode(0o755))?;
            }
            expect(r, ")")?;
            *count += 1;
        }
        "directory" => {
            fs::create_dir_all(dest)?;
            loop {
                let tag = read_string(r)?;
                if tag == ")" {
                    break;
                }
                if tag != "entry" {
                    return Err(NarError::Format(format!("expected 'entry' or ')', got '{tag}'")));
                }
                expect(r, "(")?;
                expect(r, "name")?;
                let name = read_string(r)?;
                if name.len() > MAX_NAME || name.contains('/') || name == "." || name == ".." {
                    return Err(NarError::Format(format!("invalid entry name: {name:?}")));
                }
                expect(r, "node")?;
                parse_node(r, &dest.join(&name), depth + 1, count)?;
                expect(r, ")")?;
            }
        }
        "symlink" => {
            expect(r, "target")?;
            let target = read_string(r)?;
            if target.len() > MAX_TARGET {
                return Err(NarError::Format(format!("symlink target too long: {}", target.len())));
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            if dest.exists() || dest.symlink_metadata().is_ok() {
                fs::remove_file(dest)?;
            }
            std::os::unix::fs::symlink(&target, dest)?;
            expect(r, ")")?;
            *count += 1;
        }
        other => {
            return Err(NarError::Format(format!("unknown type: {other:?}")));
        }
    }
    Ok(())
}

/// 读一个 NAR 字符串:u64-LE 长度 + 数据 + 对齐到 8 字节。
fn read_string(r: &mut impl Read) -> Result<String, NarError> {
    let len = read_u64(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    let pad = (8 - (len % 8)) % 8;
    if pad > 0 {
        let mut skip = vec![0u8; pad];
        r.read_exact(&mut skip)?;
    }
    String::from_utf8(buf).map_err(|e| NarError::Format(format!("non-utf8 string: {e}")))
}

fn read_u64(r: &mut impl Read) -> Result<u64, NarError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn expect(r: &mut impl Read, expected: &str) -> Result<(), NarError> {
    let got = read_string(r)?;
    if got != expected {
        Err(NarError::Format(format!("expected '{expected}', got '{got}'")))
    } else {
        Ok(())
    }
}

/// 精确拷贝 n 字节(分块,避免大文件一次性分配)。
fn copy_exact(r: &mut impl Read, w: &mut impl std::io::Write, n: u64) -> Result<(), NarError> {
    let mut remaining = n;
    let mut buf = [0u8; 65536];
    while remaining > 0 {
        let to_read = std::cmp::min(remaining as usize, buf.len());
        r.read_exact(&mut buf[..to_read])?;
        w.write_all(&buf[..to_read])?;
        remaining -= to_read as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 构造一个最小的 NAR(单文件,内容 "hello")。
    fn make_nar_regular(content: &[u8], executable: bool) -> Vec<u8> {
        let mut buf = Vec::new();
        write_str(&mut buf, "nix-archive-1");
        write_str(&mut buf, "(");
        write_str(&mut buf, "type");
        write_str(&mut buf, "regular");
        if executable {
            write_str(&mut buf, "executable");
            write_str(&mut buf, "");
        }
        write_str(&mut buf, "contents");
        buf.extend_from_slice(&(content.len() as u64).to_le_bytes());
        buf.extend_from_slice(content);
        let pad = (8 - (content.len() % 8)) % 8;
        buf.extend_from_slice(&vec![0u8; pad]);
        write_str(&mut buf, ")");
        buf
    }

    fn write_str(buf: &mut Vec<u8>, s: &str) {
        buf.extend_from_slice(&(s.len() as u64).to_le_bytes());
        buf.extend_from_slice(s.as_bytes());
        let pad = (8 - (s.len() % 8)) % 8;
        buf.extend_from_slice(&vec![0u8; pad]);
    }

    #[test]
    fn unpack_regular_file() {
        let nar = make_nar_regular(b"hello world", false);
        let dest = std::env::temp_dir().join(format!("nar-test-reg-{}", std::process::id()));
        let _ = fs::remove_file(&dest);
        let count = unpack(&mut Cursor::new(nar), &dest).unwrap();
        assert_eq!(count, 1);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "hello world");
        let _ = fs::remove_file(&dest);
    }

    #[test]
    fn unpack_executable_file() {
        let nar = make_nar_regular(b"#!/bin/sh\necho hi", true);
        let dest = std::env::temp_dir().join(format!("nar-test-exec-{}", std::process::id()));
        let _ = fs::remove_file(&dest);
        unpack(&mut Cursor::new(nar), &dest).unwrap();
        let mode = fs::metadata(&dest).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "should be executable");
        let _ = fs::remove_file(&dest);
    }

    #[test]
    fn unpack_directory() {
        // NAR: directory with one regular file "hello.txt"
        let mut buf = Vec::new();
        write_str(&mut buf, "nix-archive-1");
        write_str(&mut buf, "(");
        write_str(&mut buf, "type");
        write_str(&mut buf, "directory");
        // entry
        write_str(&mut buf, "entry");
        write_str(&mut buf, "(");
        write_str(&mut buf, "name");
        write_str(&mut buf, "hello.txt");
        write_str(&mut buf, "node");
        // nested regular
        write_str(&mut buf, "(");
        write_str(&mut buf, "type");
        write_str(&mut buf, "regular");
        write_str(&mut buf, "contents");
        let content = b"hi";
        buf.extend_from_slice(&(content.len() as u64).to_le_bytes());
        buf.extend_from_slice(content);
        buf.extend_from_slice(&[0u8; 6]); // pad to 8
        write_str(&mut buf, ")");
        // end entry
        write_str(&mut buf, ")");
        // end directory
        write_str(&mut buf, ")");

        let dest = std::env::temp_dir().join(format!("nar-test-dir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        let count = unpack(&mut Cursor::new(buf), &dest).unwrap();
        assert_eq!(count, 1);
        assert_eq!(fs::read_to_string(dest.join("hello.txt")).unwrap(), "hi");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut buf = Vec::new();
        write_str(&mut buf, "not-a-nar");
        let dest = std::env::temp_dir().join("nar-bad-magic");
        assert!(matches!(unpack(&mut Cursor::new(buf), &dest), Err(NarError::Format(_))));
    }

    // ── P1-9:NAR 摄入来自不可信镜像,补 symlink 分支 + 路径穿越防御 + 边界的测试 ──

    /// 构造一个 symlink 节点 NAR(target 任意)。
    fn make_nar_symlink(target: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        write_str(&mut buf, "nix-archive-1");
        write_str(&mut buf, "(");
        write_str(&mut buf, "type");
        write_str(&mut buf, "symlink");
        write_str(&mut buf, "target");
        write_str(&mut buf, target);
        write_str(&mut buf, ")");
        buf
    }

    /// 构造一个含**单个具名 entry**(空目录子节点)的 directory NAR,entry 名可控(用于穿越测试)。
    fn make_nar_dir_with_entry(entry_name: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        write_str(&mut buf, "nix-archive-1");
        write_str(&mut buf, "(");
        write_str(&mut buf, "type");
        write_str(&mut buf, "directory");
        write_str(&mut buf, "entry");
        write_str(&mut buf, "(");
        write_str(&mut buf, "name");
        write_str(&mut buf, entry_name);
        write_str(&mut buf, "node");
        // 子节点:空目录(最简合法节点)。
        write_str(&mut buf, "(");
        write_str(&mut buf, "type");
        write_str(&mut buf, "directory");
        write_str(&mut buf, ")");
        write_str(&mut buf, ")"); // end entry
        write_str(&mut buf, ")"); // end directory
        buf
    }

    #[test]
    fn unpack_symlink_node_preserved() {
        // 几乎每个真实 Nix 包都有 symlink 节点,此前完全无单测。
        let nar = make_nar_symlink("libfoo.so.1.2.3");
        let dest = std::env::temp_dir().join(format!("nar-symlink-{}", std::process::id()));
        let _ = fs::remove_file(&dest);
        let count = unpack(&mut Cursor::new(nar), &dest).unwrap();
        assert_eq!(count, 1);
        assert!(dest.symlink_metadata().unwrap().file_type().is_symlink(), "应建成 symlink");
        assert_eq!(fs::read_link(&dest).unwrap(), Path::new("libfoo.so.1.2.3"));
        let _ = fs::remove_file(&dest);
    }

    #[test]
    fn reject_path_traversal_entry_names() {
        // 不可信镜像给的目录项名若含 `..`、`/` 或 `.` 必须被拒(zip-slip 防御)。
        for bad in ["..", ".", "evil/sub", "../escape"] {
            let nar = make_nar_dir_with_entry(bad);
            let dest = std::env::temp_dir().join(format!("nar-trav-{}-{}", bad.len(), std::process::id()));
            let _ = fs::remove_dir_all(&dest);
            let r = unpack(&mut Cursor::new(nar), &dest);
            assert!(
                matches!(r, Err(NarError::Format(_))),
                "穿越名 {bad:?} 必须被拒,实得 {r:?}"
            );
            let _ = fs::remove_dir_all(&dest);
        }
    }

    #[test]
    fn reject_overlong_entry_name() {
        // 超 MAX_NAME(255)的目录项名被拒。
        let long = "a".repeat(256);
        let nar = make_nar_dir_with_entry(&long);
        let dest = std::env::temp_dir().join(format!("nar-longname-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        assert!(matches!(unpack(&mut Cursor::new(nar), &dest), Err(NarError::Format(_))));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn reject_overlong_symlink_target() {
        // 超 MAX_TARGET(4095)的 symlink target 被拒。
        let long = "x".repeat(4096);
        let nar = make_nar_symlink(&long);
        let dest = std::env::temp_dir().join(format!("nar-longtarget-{}", std::process::id()));
        let _ = fs::remove_file(&dest);
        assert!(matches!(unpack(&mut Cursor::new(nar), &dest), Err(NarError::Format(_))));
        let _ = fs::remove_file(&dest);
    }

    #[test]
    fn reject_truncated_stream() {
        // 截断流(只有 magic,后续节点缺失)→ IO 错误(read_exact 失败),不 panic。
        let mut buf = Vec::new();
        write_str(&mut buf, "nix-archive-1");
        write_str(&mut buf, "("); // 开了节点就断
        let dest = std::env::temp_dir().join(format!("nar-trunc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        assert!(unpack(&mut Cursor::new(buf), &dest).is_err(), "截断流应报错而非 panic");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn reject_non_utf8_string() {
        // 字符串字段含非 UTF-8 字节 → Format 错(read_string 的 from_utf8 守卫)。
        let mut buf = Vec::new();
        write_str(&mut buf, "nix-archive-1");
        write_str(&mut buf, "(");
        write_str(&mut buf, "type");
        // 写一个长度 4 的非 UTF-8 字符串(0xff 字节)。
        buf.extend_from_slice(&(4u64).to_le_bytes());
        buf.extend_from_slice(&[0xff, 0xfe, 0xfd, 0xfc]);
        buf.extend_from_slice(&[0u8; 4]); // pad to 8
        let dest = std::env::temp_dir().join(format!("nar-nonutf8-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dest);
        assert!(matches!(unpack(&mut Cursor::new(buf), &dest), Err(NarError::Format(_))));
        let _ = fs::remove_dir_all(&dest);
    }
}
