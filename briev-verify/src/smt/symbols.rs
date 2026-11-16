// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//! A disjoint self namespace and hexadecimal UTF-8 names prevent collisions
//! and keep arbitrary identifier contents within SMT-LIB simple symbols.

pub(super) fn encode(name: &str) -> String {
    if name == "#Self" || name == "#self" {
        return "b_self".into();
    }
    let mut result = String::from("v_");
    for byte in name.bytes() {
        let [high, low] = hex_pair(byte);
        result.push(char::from(high));
        result.push(char::from(low));
    }
    result
}

fn hex_pair(byte: u8) -> [u8; 2] {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    [
        DIGITS[usize::from(byte >> 4)],
        DIGITS[usize::from(byte & 15)],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn namespaces_keep_self_aliases_but_separate_ordinary_names() {
        assert_eq!(encode("#Self"), encode("#self"));
        for (a, b) in [
            ("#Foo", "hash_Foo"),
            ("#Self", "self_var"),
            ("#Self", "b_self"),
            ("α", "v_ceb1"),
            ("", "v_"),
        ] {
            assert_ne!(encode(a), encode(b), "{a:?} / {b:?}");
        }
    }
    #[test]
    fn every_byte_has_a_distinct_two_digit_encoding() {
        let encodings: std::collections::HashSet<_> = (0..=255).map(hex_pair).collect();
        assert_eq!(encodings.len(), 256);
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;
    #[kani::proof]
    #[kani::unwind(3)]
    fn symbol_byte_codec_is_injective() {
        let a: u8 = kani::any();
        let b: u8 = kani::any();
        assert_eq!(hex_pair(a) == hex_pair(b), a == b);
        for digit in hex_pair(a) {
            assert!(digit.is_ascii_digit() || (b'a'..=b'f').contains(&digit));
        }
        kani::cover!(a == 0);
        kani::cover!(a == 255);
        kani::cover!(a != b);
    }
}
