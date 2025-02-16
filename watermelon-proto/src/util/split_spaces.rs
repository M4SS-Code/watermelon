use core::{iter, mem};

use bytes::{Buf, Bytes};

pub(crate) fn split_spaces(mut bytes: Bytes) -> impl Iterator<Item = Bytes> {
    iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }

        let Some(i) = memchr::memchr2(b' ', b'\t', &bytes) else {
            return Some(mem::take(&mut bytes));
        };

        let chunk = bytes.split_to(i);

        let spaces = bytes
            .iter()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        debug_assert!(spaces > 0);
        bytes.advance(spaces);
        Some(chunk)
    })
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::split_spaces;

    #[test]
    fn combinations() {
        let tests: &[(&str, &[&str])] = &[
            ("", &[]),
            ("0123456789abcdef", &["0123456789abcdef"]),
            ("012345 6789abcdef", &["012345", "6789abcdef"]),
            ("012345\t6789abcdef", &["012345", "6789abcdef"]),
            ("012345  6789abcdef", &["012345", "6789abcdef"]),
            ("012345        6789abcdef", &["012345", "6789abcdef"]),
            ("012345\t\t6789abcdef", &["012345", "6789abcdef"]),
            ("012345\t\t\t\t6789abcdef", &["012345", "6789abcdef"]),
            ("012345 \t \t\t\t 6789abcdef", &["012345", "6789abcdef"]),
            ("012345 678 9abcdef", &["012345", "678", "9abcdef"]),
            ("012345 678\t9abcdef", &["012345", "678", "9abcdef"]),
            ("012345\t678 9abcdef", &["012345", "678", "9abcdef"]),
            ("012345\t678\t9abcdef", &["012345", "678", "9abcdef"]),
            ("012345\t678\t 9abcdef", &["012345", "678", "9abcdef"]),
            ("012345 \t678\t 9abcdef", &["012345", "678", "9abcdef"]),
        ];

        for (input, output) in tests {
            let spaces = split_spaces(Bytes::from_static(input.as_bytes())).collect::<Vec<Bytes>>();
            assert_eq!(spaces, output.to_vec());
        }
    }
}
