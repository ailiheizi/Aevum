//! Debian 版本比较(确定性偏序)。
//!
//! 直译自 PoC-3 `solver.py` 的 `_split_ver` / `_cmp_segment` / `_cmp_nonnum` / `deb_ver_cmp`。
//! 简化的 Debian 版本算法:`[epoch:]upstream[-revision]`,交替比较非数字段与数字段,
//! `~` 排在最前(预发布)。确定且对 PoC 数据集够用。
//!
//! 单测复用 PoC 的关键用例,保证与 Python 参照实现行为一致。

use std::cmp::Ordering;

/// 拆出 epoch 与剩余部分。无 epoch 时默认 `"0"`。
fn split_ver(v: &str) -> (i64, &str) {
    if let Some((epoch, rest)) = v.split_once(':') {
        let e = epoch.parse::<i64>().unwrap_or(0);
        (e, rest)
    } else {
        (0, v)
    }
}

/// 非数字段比较:`~` < 段尾(空)< 字母 < 其他。
///
/// 这是 dpkg 的规范序:逐位比较,缺位字符权重 `0`,`~` 权重 `-1`。
/// 关键点——`~`(预发布)必须比「段已结束」还小,纯列表字典序表达不了(空列表恒为最小),
/// 故必须逐位带哨兵比较,而非把整段映射成列表再比。
fn cmp_nonnum(a: &str, b: &str) -> Ordering {
    // 字符序权重:`~` 最小(-1),段尾(缺位)为 0,字母按 ASCII,其余 +256。
    fn ord(c: Option<char>) -> i32 {
        match c {
            None => 0,
            Some('~') => -1,
            Some(c) if c.is_ascii_alphabetic() => c as i32,
            Some(c) => c as i32 + 256,
        }
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let n = a.len().max(b.len());
    for i in 0..n {
        let c = ord(a.get(i).copied()).cmp(&ord(b.get(i).copied()));
        if c != Ordering::Equal {
            return c;
        }
    }
    Ordering::Equal
}

/// 比较 upstream/revision 段:交替吃「非数字段」与「数字段」。
fn cmp_segment(a: &str, b: &str) -> Ordering {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (mut ia, mut ib) = (0usize, 0usize);

    while ia < a.len() || ib < b.len() {
        // 非数字部分
        let mut na = String::new();
        while ia < a.len() && !a[ia].is_ascii_digit() {
            na.push(a[ia]);
            ia += 1;
        }
        let mut nb = String::new();
        while ib < b.len() && !b[ib].is_ascii_digit() {
            nb.push(b[ib]);
            ib += 1;
        }
        if na != nb {
            return cmp_nonnum(&na, &nb);
        }
        // 数字部分(前导零无意义,转整数比较)
        let mut da = String::new();
        while ia < a.len() && a[ia].is_ascii_digit() {
            da.push(a[ia]);
            ia += 1;
        }
        let mut db = String::new();
        while ib < b.len() && b[ib].is_ascii_digit() {
            db.push(b[ib]);
            ib += 1;
        }
        let va: u64 = da.parse().unwrap_or(0);
        let vb: u64 = db.parse().unwrap_or(0);
        if va != vb {
            return va.cmp(&vb);
        }
    }
    Ordering::Equal
}

/// Debian 版本比较主入口。先比 epoch,再比 upstream+revision 段。
pub fn deb_ver_cmp(v1: &str, v2: &str) -> Ordering {
    let (e1, r1) = split_ver(v1);
    let (e2, r2) = split_ver(v2);
    if e1 != e2 {
        return e1.cmp(&e2);
    }
    cmp_segment(r1, r2)
}

/// 版本约束运算符(对应 Debian 依赖语法 `(>= x)` 等)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerOp {
    Ge, // >=
    Le, // <=
    Eq, // =
    Gt, // >> / >
    Lt, // << / <
}

impl VerOp {
    /// 解析依赖原子里的运算符字符串。未知运算符返回 `None`(当作无约束处理)。
    pub fn parse(s: &str) -> Option<VerOp> {
        match s {
            ">=" => Some(VerOp::Ge),
            "<=" => Some(VerOp::Le),
            "=" => Some(VerOp::Eq),
            ">>" | ">" => Some(VerOp::Gt),
            "<<" | "<" => Some(VerOp::Lt),
            _ => None,
        }
    }
}

/// `ver` 是否满足 `op target` 约束。对应 PoC 的 `ver_satisfies`。
pub fn ver_satisfies(ver: &str, op: VerOp, target: &str) -> bool {
    let c = deb_ver_cmp(ver, target);
    match op {
        VerOp::Ge => c != Ordering::Less,
        VerOp::Le => c != Ordering::Greater,
        VerOp::Eq => c == Ordering::Equal,
        VerOp::Gt => c == Ordering::Greater,
        VerOp::Lt => c == Ordering::Less,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering::*;

    #[test]
    fn basic_numeric() {
        assert_eq!(deb_ver_cmp("1.0", "2.0"), Less);
        assert_eq!(deb_ver_cmp("2.0", "1.0"), Greater);
        assert_eq!(deb_ver_cmp("1.0", "1.0"), Equal);
    }

    #[test]
    fn numeric_not_lexical() {
        // 10 > 9,数字段按整数比而非字典序
        assert_eq!(deb_ver_cmp("1.10", "1.9"), Greater);
    }

    #[test]
    fn tilde_is_prerelease() {
        // ~ 排在最前:预发布 < 正式版
        assert_eq!(deb_ver_cmp("1.0~rc1", "1.0"), Less);
        assert_eq!(deb_ver_cmp("1.0~beta", "1.0~rc1"), Less);
    }

    #[test]
    fn epoch_dominates() {
        // epoch 高者更新,即使 upstream 更小
        assert_eq!(deb_ver_cmp("2:1.0", "1:9.9"), Greater);
    }

    #[test]
    fn revision_compared() {
        assert_eq!(deb_ver_cmp("1.0-1", "1.0-2"), Less);
    }

    #[test]
    fn satisfies() {
        assert!(ver_satisfies("3.0.1", VerOp::Ge, "3.0"));
        assert!(!ver_satisfies("2.9", VerOp::Ge, "3.0"));
        assert!(ver_satisfies("3.0", VerOp::Eq, "3.0"));
        assert!(ver_satisfies("2.9", VerOp::Lt, "3.0"));
    }
}
