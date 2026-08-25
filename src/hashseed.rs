//! The "hashed seed" the server sends the client, and using it to pick the one
//! true world seed out of the 65 536 a structure seed lifts to.
//!
//! Every cracking path here recovers a *structure seed* — the low 48 bits of the
//! world seed. Lifting to a full 64-bit world seed leaves 2¹⁶ = 65 536
//! candidates, one per choice of the top 16 bits, and normally you cannot tell
//! them apart: they generate identical structures, biomes, and terrain.
//!
//! But the server also computes a **hashed seed** from the *full* 64-bit world
//! seed and sends it to the client (it drives client-side biome blending and is
//! visible to a mod). Minecraft computes it with Guava:
//!
//! ```text
//! hashedSeed = Hashing.sha256().hashLong(worldSeed).asLong()
//! ```
//!
//! `hashLong` feeds the long to SHA-256 as 8 **little-endian** bytes, and
//! `asLong` reads the first 8 bytes of the digest back as a **little-endian**
//! long. Because it depends on all 64 bits, testing the 65 536 lift candidates
//! against it picks out the exact world seed. The SHA-256 here is verified
//! against the standard `"abc"` vector and against ten seeds hashed by the real
//! Guava the game ships (see the tests).

use crate::random::MASK;

// -- SHA-256 (FIPS 180-4), self-contained so the crate needs no crypto dep. --

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

fn sha256(message: &[u8]) -> [u8; 32] {
    let mut h = H0;

    // Padding: 0x80, then zeros, then the 64-bit big-endian bit length.
    let bit_len = (message.len() as u64).wrapping_mul(8);
    let mut data = message.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());

    for block in 0..data.len() / 64 {
        let base = block * 64;
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().enumerate().take(16) {
            let j = base + i * 4;
            *word = u32::from_be_bytes([data[j], data[j + 1], data[j + 2], data[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (hi, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *hi = hi.wrapping_add(v);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// -- Minecraft's hashed seed and the disambiguation it enables. --

/// The hashed seed Minecraft derives from a full 64-bit world seed.
pub fn hashed_seed(world_seed: i64) -> i64 {
    // Guava's hashLong writes the long little-endian; asLong reads the first
    // eight digest bytes little-endian.
    let digest = sha256(&(world_seed as u64).to_le_bytes());
    i64::from_le_bytes(digest[..8].try_into().unwrap())
}

/// The full world seeds whose hashed seed equals `observed`, given a structure
/// seed (its low 48 bits).
///
/// A structure seed fixes the low 48 bits; this tries all 2¹⁶ choices of the top
/// 16 and keeps those whose hashed seed matches. Normally exactly one survives —
/// which is the whole point: it turns "65 536 indistinguishable world seeds"
/// into a single answer. A 64-bit hash collision among 65 536 candidates is
/// vanishingly unlikely, but more than one is returned honestly if it happens.
pub fn world_seeds_matching_hash(structure_seed: i64, observed: i64) -> Vec<i64> {
    let low = (structure_seed as u64) & MASK;
    (0u64..65_536)
        .map(|hi| ((hi << 48) | low) as i64)
        .filter(|&ws| hashed_seed(ws) == observed)
        .collect()
}

/// The biome-zoom seed the client stores — the hashed seed hashed again.
///
/// On joining a world the client runs the server's hashed seed through
/// `BiomeManager.obfuscateSeed`, which is byte-for-byte the same SHA-256 framing
/// as [`hashed_seed`] (verified against the game). The result is kept in a field
/// the exporter mod can read by reflection with no mixin, so it is the
/// server-friendly way to get the same disambiguation power as the raw hashed
/// seed — you just match against the double hash.
pub fn biome_hash(world_seed: i64) -> i64 {
    hashed_seed(hashed_seed(world_seed))
}

/// Like [`world_seeds_matching_hash`], but for the doubly-hashed biome seed the
/// mod reads from `BiomeManager`.
pub fn world_seeds_matching_biome_hash(structure_seed: i64, observed: i64) -> Vec<i64> {
    let low = (structure_seed as u64) & MASK;
    (0u64..65_536)
        .map(|hi| ((hi << 48) | low) as i64)
        .filter(|&ws| biome_hash(ws) == observed)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_standard_abc_vector() {
        // FIPS 180-4 example: SHA-256("abc").
        let want = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let got: String = sha256(b"abc").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn sha256_matches_the_empty_vector() {
        let want = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let got: String = sha256(b"").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, want);
    }

    /// Ground truth generated by the real Guava the game ships:
    /// `Hashing.sha256().hashLong(seed).asLong()`. If either the SHA-256 or the
    /// little-endian framing were wrong, these would not match.
    #[test]
    fn hashed_seed_matches_guava() {
        let vectors: [(i64, i64); 10] = [
            (0, 8794265229978523055),
            (1, -6467378160175308932),
            (-1, 6759447113877070610),
            (42, -4111196313959201555),
            (1234567890123, 5318346877068299412),
            (-4172144997902289642, 2159143436479834350),
            (123456789, -5458710421630621949),
            (8675309, 8580917108473614843),
            (i64::MAX, 7179146226492139882),
            (i64::MIN, 6374347445474471398),
        ];
        for (seed, want) in vectors {
            assert_eq!(hashed_seed(seed), want, "hashed seed for {seed}");
        }
    }

    #[test]
    fn disambiguation_recovers_the_exact_world_seed() {
        // A world seed with non-zero high bits; its structure seed is the low 48.
        let world: i64 = 0x0123_4567_89AB_CDEF_u64 as i64;
        let structure = world & (MASK as i64);
        let observed = hashed_seed(world);

        let found = world_seeds_matching_hash(structure, observed);
        assert!(found.contains(&world), "the true world seed must survive");
        // In practice a single candidate matches.
        assert_eq!(found.len(), 1, "expected a unique match, got {found:?}");
        assert_eq!(found[0] as u64 & MASK, structure as u64 & MASK);
    }

    #[test]
    fn a_candidate_set_with_decoys_collapses_to_one_world_seed() {
        // The realistic Tier-1 case: the structure sieve leaves several
        // structure-seed candidates, only one of which is real. The hashed seed
        // matches the full world seed, so decoys contribute nothing and exactly
        // the true world seed survives — the structures do not have to pin the
        // structure seed uniquely on their own.
        let world: i64 = 0x0007_1357_9BDF_2468u64 as i64;
        let truth = world & (MASK as i64);
        let observed = hashed_seed(world);

        let candidates = [
            truth,
            (truth ^ 0x3) & (MASK as i64),
            (truth ^ 0x2A0) & (MASK as i64),
            (truth.wrapping_add(1)) & (MASK as i64),
            (truth ^ 0x1_0000) & (MASK as i64),
        ];

        let worlds: Vec<i64> = candidates
            .iter()
            .flat_map(|s| world_seeds_matching_hash(*s, observed))
            .collect();

        assert_eq!(worlds, vec![world], "only the true world seed should survive");
    }

    /// Ground truth for the biome-zoom seed — `obfuscateSeed(hashedSeed(seed))` —
    /// straight from the game's `BiomeManager`. Confirms the double hash and,
    /// implicitly, that obfuscateSeed is the same SHA-256 as hashLong.
    #[test]
    fn biome_hash_matches_the_game() {
        let vectors: [(i64, i64); 8] = [
            (0, 4978243150091466422),
            (1, 4399471924234691836),
            (-1, -8069622019703028862),
            (42, 7401262386151203154),
            (123456789, 5797260164526851119),
            (-4172144997902289642, 8632423987145108184),
            (8675309, -4615120266122708361),
            (i64::MAX, 8253616770716532245),
        ];
        for (seed, want) in vectors {
            assert_eq!(biome_hash(seed), want, "biome hash for {seed}");
        }
    }

    #[test]
    fn biome_hash_disambiguation_recovers_the_world_seed() {
        let world: i64 = 0x0042_1357_9BDF_2468u64 as i64;
        let structure = world & (MASK as i64);
        let observed = biome_hash(world);
        let found = world_seeds_matching_biome_hash(structure, observed);
        assert_eq!(found, vec![world]);
    }

    #[test]
    fn a_wrong_hash_yields_no_seed() {
        // A hash no lift candidate produces (flip a bit of a real one) must
        // return nothing rather than a false seed.
        let world: i64 = 0x00AB_CDEF_1234_5678u64 as i64;
        let structure = world & (MASK as i64);
        let bogus = hashed_seed(world) ^ 0x1;
        assert!(world_seeds_matching_hash(structure, bogus).is_empty());
    }
}
