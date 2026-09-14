#[derive(Debug, Clone, Copy)]
pub struct Crc8 {
    value: u8,
}

impl Crc8 {
    pub const fn new() -> Self { Self { value: 0 } }
    pub fn update_bit(&mut self, bit: bool) {
        let top = (self.value & 0x80) != 0;
        self.value <<= 1;
        if top ^ bit { self.value ^= 0x07; }
    }
    pub fn update_bits(&mut self, value: u64, bits: usize) {
        for i in (0..bits).rev() { self.update_bit(((value >> i) & 1) != 0); }
    }
    pub fn update_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes { self.update_bits(b as u64, 8); }
    }
    pub const fn finalize(self) -> u8 { self.value }
}

impl Default for Crc8 { fn default() -> Self { Self::new() } }

#[derive(Debug, Clone, Copy)]
pub struct Crc32 {
    value: u32,
}

impl Crc32 {
    pub const fn new() -> Self { Self { value: 0xffff_ffff } }
    pub fn update_bit(&mut self, bit: bool) {
        let top = (self.value & 0x8000_0000) != 0;
        self.value <<= 1;
        if top ^ bit { self.value ^= 0x04c1_1db7; }
    }
    pub fn update_bits(&mut self, value: u64, bits: usize) {
        for i in (0..bits).rev() { self.update_bit(((value >> i) & 1) != 0); }
    }
    pub fn update_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes { self.update_bits(b as u64, 8); }
    }
    pub const fn finalize(self) -> u32 { self.value ^ 0xffff_ffff }
}

impl Default for Crc32 { fn default() -> Self { Self::new() } }

pub fn crc8_gnet(bytes: &[u8]) -> u8 {
    let mut c = Crc8::new(); c.update_bytes(bytes); c.finalize()
}

pub fn crc32_gnet(bytes: &[u8]) -> u32 {
    let mut c = Crc32::new(); c.update_bytes(bytes); c.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn check_crc8() { assert_eq!(crc8_gnet(b"123456789"), 0xF4); }
    #[test] fn check_crc32() { assert_eq!(crc32_gnet(b"123456789"), 0xFC89_1918); }
}
