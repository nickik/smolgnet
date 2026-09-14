const fn make_crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if (crc & 0x80) != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if (crc & 0x8000_0000) != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const CRC8_TABLE: [u8; 256] = make_crc8_table();
const CRC32_TABLE: [u32; 256] = make_crc32_table();

#[derive(Debug, Clone, Copy)]
pub struct Crc8 {
    value: u8,
}

impl Crc8 {
    pub const fn new() -> Self {
        Self { value: 0 }
    }

    pub fn update_bit(&mut self, bit: bool) {
        let top = (self.value & 0x80) != 0;
        self.value <<= 1;
        if top ^ bit {
            self.value ^= 0x07;
        }
    }

    pub fn update_bits(&mut self, value: u64, bits: usize) {
        for i in (0..bits).rev() {
            self.update_bit(((value >> i) & 1) != 0);
        }
    }

    pub fn update_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.value = CRC8_TABLE[(self.value ^ b) as usize];
        }
    }

    pub const fn finalize(self) -> u8 {
        self.value
    }
}

impl Default for Crc8 {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Crc32 {
    value: u32,
}

impl Crc32 {
    pub const fn new() -> Self {
        Self {
            value: 0xffff_ffff,
        }
    }

    pub fn update_bit(&mut self, bit: bool) {
        let top = (self.value & 0x8000_0000) != 0;
        self.value <<= 1;
        if top ^ bit {
            self.value ^= 0x04c1_1db7;
        }
    }

    pub fn update_bits(&mut self, value: u64, bits: usize) {
        for i in (0..bits).rev() {
            self.update_bit(((value >> i) & 1) != 0);
        }
    }

    /// Table-driven CRC-32-GNET update for one or many discontiguous spans.
    /// Callers can repeatedly invoke this for pseudo-header, metadata and
    /// payload without concatenating them into a temporary buffer.
    pub fn update_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let idx = ((self.value >> 24) as u8 ^ b) as usize;
            self.value = (self.value << 8) ^ CRC32_TABLE[idx];
        }
    }

    pub const fn finalize(self) -> u32 {
        self.value ^ 0xffff_ffff
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

pub fn crc8_gnet(bytes: &[u8]) -> u8 {
    let mut c = Crc8::new();
    c.update_bytes(bytes);
    c.finalize()
}

pub fn crc32_gnet(bytes: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update_bytes(bytes);
    c.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slow_crc32(bytes: &[u8]) -> u32 {
        let mut value = 0xffff_ffffu32;
        for &b in bytes {
            for i in (0..8).rev() {
                let top = (value & 0x8000_0000) != 0;
                value <<= 1;
                if top ^ (((b >> i) & 1) != 0) {
                    value ^= 0x04c1_1db7;
                }
            }
        }
        value ^ 0xffff_ffff
    }

    #[test]
    fn check_crc8() {
        assert_eq!(crc8_gnet(b"123456789"), 0xF4);
    }

    #[test]
    fn check_crc32() {
        assert_eq!(crc32_gnet(b"123456789"), 0xFC89_1918);
    }

    #[test]
    fn table_crc32_matches_bitwise_reference_and_incremental_spans() {
        let bytes = b"GNET table driven CRC regression vector";
        assert_eq!(crc32_gnet(bytes), slow_crc32(bytes));

        let mut incremental = Crc32::new();
        incremental.update_bytes(&bytes[..7]);
        incremental.update_bytes(&bytes[7..19]);
        incremental.update_bytes(&bytes[19..]);
        assert_eq!(incremental.finalize(), slow_crc32(bytes));
    }
}
