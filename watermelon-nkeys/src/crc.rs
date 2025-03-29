#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Crc16(u16);

impl Crc16 {
    pub(crate) fn compute(buf: &[u8]) -> Self {
        Self(crc::Crc::<u16>::new(&crc::CRC_16_XMODEM).checksum(buf))
    }

    pub(crate) fn from_raw_encoded(val: [u8; 2]) -> Self {
        Self::from_raw(u16::from_le_bytes(val))
    }

    pub(crate) fn from_raw(val: u16) -> Self {
        Self(val)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn to_raw(&self) -> u16 {
        self.0
    }

    pub(crate) fn to_raw_encoded(&self) -> [u8; 2] {
        self.0.to_le_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::Crc16;

    #[test]
    fn compute() {
        let input = [
            127, 237, 118, 35, 51, 69, 160, 148, 48, 70, 89, 182, 167, 81, 102, 237, 1, 143, 113,
            171, 162, 163, 101, 161, 49, 2, 57, 163, 167, 13, 106, 97, 249, 213,
        ];
        let crc = Crc16::compute(&input);
        assert_eq!(14592, crc.to_raw());
        assert_eq!(crc, Crc16::from_raw(crc.to_raw()));
        assert_eq!(crc, Crc16::from_raw_encoded(crc.to_raw_encoded()));
    }
}
