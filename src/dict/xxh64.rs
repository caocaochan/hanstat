//! XXH64 (Yann Collet's xxHash, 64-bit), written out so the dictionary files'
//! hashes never change with a crate's version. Checked against the reference
//! implementation's sanity vectors (`cli/xsum_sanity_check.c`).

const P1: u64 = 0x9E37_79B1_85EB_CA87;
const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
const P3: u64 = 0x1656_67B1_9E37_79F9;
const P4: u64 = 0x85EB_CA77_C2B2_AE63;
const P5: u64 = 0x27D4_EB2F_1656_67C5;

#[inline]
fn read64(b: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}

#[inline]
fn read32(b: &[u8], i: usize) -> u64 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as u64
}

#[inline]
fn round(acc: u64, input: u64) -> u64 {
    acc.wrapping_add(input.wrapping_mul(P2))
        .rotate_left(31)
        .wrapping_mul(P1)
}

#[inline]
fn merge(acc: u64, val: u64) -> u64 {
    (acc ^ round(0, val)).wrapping_mul(P1).wrapping_add(P4)
}

pub fn xxh64(data: &[u8], seed: u64) -> u64 {
    let len = data.len();
    let mut i = 0;
    let mut h = if len >= 32 {
        let mut v1 = seed.wrapping_add(P1).wrapping_add(P2);
        let mut v2 = seed.wrapping_add(P2);
        let mut v3 = seed;
        let mut v4 = seed.wrapping_sub(P1);
        while i + 32 <= len {
            v1 = round(v1, read64(data, i));
            v2 = round(v2, read64(data, i + 8));
            v3 = round(v3, read64(data, i + 16));
            v4 = round(v4, read64(data, i + 24));
            i += 32;
        }
        let mut h = v1
            .rotate_left(1)
            .wrapping_add(v2.rotate_left(7))
            .wrapping_add(v3.rotate_left(12))
            .wrapping_add(v4.rotate_left(18));
        h = merge(h, v1);
        h = merge(h, v2);
        h = merge(h, v3);
        merge(h, v4)
    } else {
        seed.wrapping_add(P5)
    };
    h = h.wrapping_add(len as u64);
    while i + 8 <= len {
        h ^= round(0, read64(data, i));
        h = h.rotate_left(27).wrapping_mul(P1).wrapping_add(P4);
        i += 8;
    }
    if i + 4 <= len {
        h ^= read32(data, i).wrapping_mul(P1);
        h = h.rotate_left(23).wrapping_mul(P2).wrapping_add(P3);
        i += 4;
    }
    while i < len {
        h ^= (data[i] as u64).wrapping_mul(P5);
        h = h.rotate_left(11).wrapping_mul(P1);
        i += 1;
    }
    h ^= h >> 33;
    h = h.wrapping_mul(P2);
    h ^= h >> 29;
    h = h.wrapping_mul(P3);
    h ^ (h >> 32)
}

/// A headword's key in the indexes.
pub fn key(s: &str) -> u64 {
    xxh64(s.as_bytes(), 0)
}

/// A glossary's identity for storing it once: two seeds, 128 bits, so
/// different glossaries never share one in practice.
pub fn wide(b: &[u8]) -> u128 {
    ((xxh64(b, 0) as u128) << 64) | xxh64(b, 0x6861_6e73_7461_7421) as u128
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `XSUM_fillTestBuffer` and `XSUM_XXH64_testdata` from xxHash's
    /// `cli/xsum_sanity_check.c`.
    #[test]
    fn reference_vectors() {
        const PRIME32: u64 = 2654435761;
        const PRIME64: u64 = 11400714785074694797;
        let mut buf = vec![0u8; 222];
        let mut g = PRIME32;
        for b in &mut buf {
            *b = (g >> 56) as u8;
            g = g.wrapping_mul(PRIME64);
        }
        for (len, seed, want) in [
            (0, 0, 0xEF46DB3751D8E999u64),
            (0, PRIME32, 0xAC75FDA2929B17EF),
            (1, 0, 0xE934A84ADB052768),
            (1, PRIME32, 0x5014607643A9B4C3),
            (4, 0, 0x9136A0DCA57457EE),
            (14, 0, 0x8282DCC4994E35C8),
            (14, PRIME32, 0xC3BD6BF63DEB6DF0),
            (222, 0, 0xB641AE8CB691C174),
            (222, PRIME32, 0x20CB8AB7AE10C14A),
        ] {
            assert_eq!(xxh64(&buf[..len], seed), want, "len {len} seed {seed}");
        }
    }
}
