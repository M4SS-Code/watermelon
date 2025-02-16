use core::array;

use bytes::{Buf, Bytes};

pub(crate) fn split_spaces(mut bytes: Bytes) -> impl Iterator<Item = Bytes> {
    let mut chunks = array::from_fn::<_, 6, _>(|_| Bytes::new());
    let mut found = 0;

    for chunk in &mut chunks {
        let Some(i) = memchr::memchr2(b' ', b'\t', &bytes) else {
            if !bytes.is_empty() {
                *chunk = bytes;
                found += 1;
            }
            break;
        };

        *chunk = bytes.split_to(i);
        found += 1;

        let spaces = bytes
            .iter()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        bytes.advance(spaces);
    }

    chunks.into_iter().take(found)
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
