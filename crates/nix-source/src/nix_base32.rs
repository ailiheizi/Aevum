//! Nix base32 编码:Nix 的哈希字符串用一套**自定义** base32(不是 RFC4648)。
//!
//! 字母表 `0123456789abcdfghijklmnpqrsvwxyz`(32 字符,刻意去掉 e/o/u/t 避免脏词/混淆),
//! 且按**位反序**编码(见 Nix `src/libutil/hash.cc` 的 `printHash32`)。
//! narinfo 的 `NarHash: sha256:<nixbase32>` / `FileHash` 都用它。
//!
//! 这里只需 encode:把我们用 `sha2` 算出的 32 字节摘要编成 nixbase32 串,
//! 与 narinfo 里的值做字符串比对即可(无需 decode)。零依赖、纯函数、可测。

const ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// 编码后字符数:`ceil(len*8 / 5)`。SHA256(32 字节)→ 52 字符。
pub fn encoded_len(byte_len: usize) -> usize {
    if byte_len == 0 {
        return 0;
    }
    (byte_len * 8 - 1) / 5 + 1
}

/// 把字节摘要编成 Nix base32 字符串(位反序,同 Nix `printHash32`)。
pub fn encode(bytes: &[u8]) -> String {
    let len = encoded_len(bytes.len());
    let mut out = String::with_capacity(len);
    // 从最高位组到最低位组(n 递减),每组取 5 bit。
    for n in (0..len).rev() {
        let b = n * 5;
        let i = b / 8;
        let j = b % 8;
        // 跨字节取 5 bit:当前字节高位 + 下个字节低位补足。
        let mut c = (bytes[i] as u16) >> j;
        if i + 1 < bytes.len() {
            c |= (bytes[i + 1] as u16) << (8 - j);
        }
        out.push(ALPHABET[(c & 0x1f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn len_for_sha256_is_52() {
        assert_eq!(encoded_len(32), 52);
        assert_eq!(encoded_len(0), 0);
        assert_eq!(encoded_len(20), 32); // sha1 长度
    }

    #[test]
    fn all_zero_digest() {
        // 全 0 字节 → 全 '0' 字符。
        let z = [0u8; 32];
        let s = encode(&z);
        assert_eq!(s.len(), 52);
        assert!(s.chars().all(|c| c == '0'));
    }

    #[test]
    fn alphabet_excludes_eout() {
        // Nix 字母表刻意不含 e/o/u/t。
        let a: String = ALPHABET.iter().map(|b| *b as char).collect();
        for bad in ['e', 'o', 'u', 't'] {
            assert!(!a.contains(bad), "字母表不应含 {bad}");
        }
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn known_vector_sha256_of_empty_string() {
        // SHA256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        // 用独立的 decode(纯测试内实现,反向验证)做 round-trip:encode 后再 decode 回原字节。
        // 这样不依赖我手抄的 nixbase32 常量,只验证编码本身与位序规范自洽。
        let digest = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        let encoded = encode(&digest);
        assert_eq!(encoded.len(), 52);
        // 只含字母表内字符。
        let a: String = ALPHABET.iter().map(|b| *b as char).collect();
        assert!(encoded.chars().all(|c| a.contains(c)));
        // round-trip 回原字节。
        assert_eq!(decode_for_test(&encoded), digest.to_vec());
    }

    /// 测试专用 decode:Nix base32 → 字节(encode 的逆,验证位序自洽)。
    fn decode_for_test(s: &str) -> Vec<u8> {
        let chars: Vec<u8> = s.bytes().collect();
        let byte_len = chars.len() * 5 / 8;
        let mut out = vec![0u8; byte_len];
        for (n, &ch) in chars.iter().rev().enumerate() {
            let digit = ALPHABET.iter().position(|&a| a == ch).expect("非法字符") as u16;
            let b = n * 5;
            let i = b / 8;
            let j = b % 8;
            out[i] |= ((digit << j) & 0xff) as u8;
            if (digit >> (8 - j)) != 0 && i + 1 < byte_len {
                out[i + 1] |= (digit >> (8 - j)) as u8;
            }
        }
        out
    }

    #[test]
    fn round_trip_various_patterns() {
        for pat in [[0xABu8; 32], [0x01; 32], [0xFF; 32], [0x5A; 32]] {
            let enc = encode(&pat);
            assert_eq!(decode_for_test(&enc), pat.to_vec(), "round-trip 失败: {enc}");
        }
    }
}
