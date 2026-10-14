//! Stable 64-bit FNV-1a. Used for ids that are persisted (hunk ids in session files),
//! so it must not depend on std's unspecified `DefaultHasher` algorithm.

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0100_0000_01b3;

#[derive(Debug, Clone, Copy)]
pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(OFFSET)
    }
}

impl Fnv {
    pub fn write(&mut self, bytes: &[u8]) -> &mut Self {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(PRIME);
        }
        self
    }

    /// Write a field followed by a separator byte so `("ab","c")` != `("a","bc")`.
    pub fn field(&mut self, bytes: &[u8]) -> &mut Self {
        self.write(bytes).write(&[0xff])
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    Fnv::default().write(bytes).finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn fields_are_separated() {
        let a = Fnv::default().field(b"ab").field(b"c").finish();
        let b = Fnv::default().field(b"a").field(b"bc").finish();
        assert_ne!(a, b);
    }
}
