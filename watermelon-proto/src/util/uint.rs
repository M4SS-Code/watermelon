macro_rules! parse_unsigned {
    ($name:ident, $num:ty) => {
        pub(crate) fn $name(buf: &[u8]) -> Result<$num, ParseUintError> {
            if buf.is_empty() {
                return Err(ParseUintError::Empty);
            }

            let mut val: $num = 0;

            for &b in buf {
                if !b.is_ascii_digit() {
                    return Err(ParseUintError::InvalidByte(b));
                }

                val = val.checked_mul(10).ok_or(ParseUintError::Overflow)?;
                val = val
                    .checked_add(<$num>::from(b - b'0'))
                    .ok_or(ParseUintError::Overflow)?;
            }

            Ok(val)
        }
    };
}

parse_unsigned!(parse_u16, u16);
parse_unsigned!(parse_u64, u64);
parse_unsigned!(parse_usize, usize);

#[derive(Debug, thiserror::Error)]
pub enum ParseUintError {
    #[error("empty input")]
    Empty,
    #[error("invalid byte {0:?}")]
    InvalidByte(u8),
    #[error("overflow")]
    Overflow,
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use claims::{assert_matches, assert_ok_eq};

    use super::{ParseUintError, parse_u16, parse_u64, parse_usize};

    #[test]
    fn parse_empty_input() {
        assert_matches!(parse_u16(b""), Err(ParseUintError::Empty));
        assert_matches!(parse_u64(b""), Err(ParseUintError::Empty));
        assert_matches!(parse_usize(b""), Err(ParseUintError::Empty));
    }

    #[test]
    fn parse_u16_range() {
        for n in 0..=u16::MAX {
            let s = n.to_string();
            assert_ok_eq!(parse_u16(s.as_bytes()), n);
            assert_ok_eq!(parse_usize(s.as_bytes()), usize::from(n));
            assert_ok_eq!(parse_u64(s.as_bytes()), u64::from(n));
        }
    }
}
