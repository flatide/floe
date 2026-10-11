//! A 4096-error chunk's bit plane: bit i (little-endian bytes, the Python
//! reference's `int.from_bytes(blob, "little")`) is the chunk's error i.
//! Stored as a 512-byte blob, or empty when no bit is set.
use crate::{Error, Result};

pub const CHUNK: u64 = 4096;
pub const BYTES: usize = (CHUNK / 8) as usize;
const WORDS: usize = BYTES / 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bits(pub [u64; WORDS]);

impl Default for Bits {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Bits {
    pub const ZERO: Bits = Bits([0; WORDS]);

    /// The first `n` bits set.
    pub fn first(n: u64) -> Self {
        let mut b = Self::ZERO;
        for (w, word) in b.0.iter_mut().enumerate() {
            let lo = w as u64 * 64;
            if n >= lo + 64 {
                *word = u64::MAX;
            } else if n > lo {
                *word = (1u64 << (n - lo)) - 1;
            }
        }
        b
    }
    pub fn from_blob(blob: &[u8], what: &str) -> Result<Self> {
        match blob.len() {
            0 => Ok(Self::ZERO),
            BYTES => {
                let mut b = Self::ZERO;
                for (w, c) in b.0.iter_mut().zip(blob.chunks_exact(8)) {
                    *w = u64::from_le_bytes(c.try_into().unwrap());
                }
                Ok(b)
            }
            _ => Err(Error::input(format!("corrupt {what}"))),
        }
    }
    /// The stored blob: 512 bytes, or none for an empty plane.
    pub fn blob(&self) -> Vec<u8> {
        if self.is_zero() {
            return Vec::new();
        }
        self.bytes().to_vec()
    }
    pub fn bytes(&self) -> [u8; BYTES] {
        let mut out = [0u8; BYTES];
        for (c, w) in out.chunks_exact_mut(8).zip(self.0) {
            c.copy_from_slice(&w.to_le_bytes());
        }
        out
    }
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&w| w == 0)
    }
    pub fn count(&self) -> u64 {
        self.0.iter().map(|w| u64::from(w.count_ones())).sum()
    }
    pub fn get(&self, i: u64) -> bool {
        self.0[(i / 64) as usize] >> (i % 64) & 1 == 1
    }
    pub fn set(&mut self, i: u64) {
        self.0[(i / 64) as usize] |= 1 << (i % 64);
    }
    /// Set bits below position `i`.
    pub fn count_below(&self, i: u64) -> u64 {
        let w = (i / 64) as usize;
        let mut n: u64 = self.0[..w].iter().map(|x| u64::from(x.count_ones())).sum();
        if i % 64 != 0 {
            n += u64::from((self.0[w] & ((1u64 << (i % 64)) - 1)).count_ones());
        }
        n
    }
    /// The set positions, ascending.
    pub fn ones(&self) -> impl Iterator<Item = u64> + '_ {
        self.0.iter().enumerate().flat_map(|(w, &word)| {
            let mut x = word;
            std::iter::from_fn(move || {
                if x == 0 {
                    return None;
                }
                let t = x.trailing_zeros();
                x &= x - 1;
                Some(w as u64 * 64 + u64::from(t))
            })
        })
    }
    /// Any bit at or above position `n`.
    pub fn beyond(&self, n: u64) -> bool {
        !(*self & !Self::first(n)).is_zero()
    }
    /// From a boolean mask (at most 4096 entries).
    pub fn from_mask(mask: &[bool]) -> Self {
        let mut b = Self::ZERO;
        for (i, &m) in mask.iter().enumerate() {
            if m {
                b.set(i as u64);
            }
        }
        b
    }
}

macro_rules! binop {
    ($tr:ident, $f:ident, $op:tt, $tra:ident, $fa:ident) => {
        impl std::ops::$tr for Bits {
            type Output = Bits;
            fn $f(self, o: Bits) -> Bits {
                let mut r = self;
                for (a, b) in r.0.iter_mut().zip(o.0) {
                    *a = *a $op b;
                }
                r
            }
        }
        impl std::ops::$tra for Bits {
            fn $fa(&mut self, o: Bits) {
                *self = *self $op o;
            }
        }
    };
}
binop!(BitAnd, bitand, &, BitAndAssign, bitand_assign);
binop!(BitOr, bitor, |, BitOrAssign, bitor_assign);
binop!(BitXor, bitxor, ^, BitXorAssign, bitxor_assign);

impl std::ops::Not for Bits {
    type Output = Bits;
    fn not(self) -> Bits {
        let mut r = self;
        for a in r.0.iter_mut() {
            *a = !*a;
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_round_trip_and_counts() {
        let mut b = Bits::ZERO;
        assert!(b.blob().is_empty());
        for i in [0, 7, 8, 63, 64, 4095] {
            b.set(i);
        }
        let blob = b.blob();
        assert_eq!(blob.len(), BYTES);
        assert_eq!(blob[0], 0x81);
        assert_eq!(blob[1], 0x01);
        assert_eq!(Bits::from_blob(&blob, "x").unwrap(), b);
        assert_eq!(b.count(), 6);
        assert_eq!(b.count_below(64), 4);
        assert_eq!(b.count_below(65), 5);
        assert_eq!(b.ones().collect::<Vec<_>>(), vec![0, 7, 8, 63, 64, 4095]);
        assert!(b.beyond(4095));
        assert!(!b.beyond(4096));
        assert_eq!(Bits::first(70).count(), 70);
        assert!(Bits::from_blob(&[0; 3], "x").is_err());
    }
}
