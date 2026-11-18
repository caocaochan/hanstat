//! A split-block Bloom filter (as in Apache Parquet): each key sets one bit in
//! each of the 8 words of one 32-byte block, so a test reads one cache line.
//! About 10 bits a key: roughly 1% false positives. It lets a lookup skip a
//! dictionary that has no such headword without touching its mapped files.

const SALT: [u32; 8] = [
    0x47b6137b, 0x44974d91, 0x8824ad5b, 0xa2b7289d, 0x705495c7, 0x2df1424b, 0x9efc4947, 0x5c6bfb31,
];

pub struct Bloom {
    blocks: Vec<[u32; 8]>,
}

impl Bloom {
    pub fn with_keys(n: usize) -> Bloom {
        let blocks = (n * 10).div_ceil(256).max(1);
        Bloom {
            blocks: vec![[0; 8]; blocks],
        }
    }

    fn block(&self, h: u64) -> usize {
        (((h >> 32) * self.blocks.len() as u64) >> 32) as usize
    }

    pub fn insert(&mut self, h: u64) {
        let b = self.block(h);
        let x = h as u32;
        for (i, w) in self.blocks[b].iter_mut().enumerate() {
            *w |= 1 << (x.wrapping_mul(SALT[i]) >> 27);
        }
    }

    pub fn may_contain(&self, h: u64) -> bool {
        let x = h as u32;
        let block = &self.blocks[self.block(h)];
        (0..8).all(|i| block[i] & (1 << (x.wrapping_mul(SALT[i]) >> 27)) != 0)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.blocks.len() * 32);
        for b in &self.blocks {
            for w in b {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Bloom> {
        if b.is_empty() || !b.len().is_multiple_of(32) {
            return None;
        }
        Some(Bloom {
            blocks: b
                .as_chunks::<32>()
                .0
                .iter()
                .map(|c| {
                    std::array::from_fn(|i| {
                        u32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap())
                    })
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dict::xxh64::key;

    #[test]
    fn no_false_negatives_and_few_false_positives() {
        let n = 50_000;
        let mut b = Bloom::with_keys(n);
        for i in 0..n {
            b.insert(key(&format!("词{i}")));
        }
        let b = Bloom::from_bytes(&b.to_bytes()).unwrap();
        for i in 0..n {
            assert!(b.may_contain(key(&format!("词{i}"))), "{i}");
        }
        let fp = (0..n)
            .filter(|i| b.may_contain(key(&format!("无{i}"))))
            .count();
        let rate = fp as f64 / n as f64;
        assert!(rate < 0.03, "false positives {rate}");
    }
}
