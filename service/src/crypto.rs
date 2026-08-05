//! Hand-written primitives for the two places the collector needs them: the
//! Kasa KLAP transport (SHA-1/SHA-256/AES-128-CBC) and HTTP Basic auth
//! (base64). Every primitive is pinned to published test vectors below —
//! FIPS-180 for the hashes, FIPS-197 and SP 800-38A for AES, RFC 4648 for
//! base64. This protects device credentials on the local LAN; nothing here
//! is a general-purpose TLS stack.

// ---------------------------------------------------------------------------
// SHA-256 (FIPS 180-4)

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    for block in padded_blocks(data) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
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
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
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
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// SHA-1 (FIPS 180-4). Used only because KLAP's auth hash is defined as
// sha256(sha1(user) + sha1(pass)); no security property beyond matching the
// protocol is claimed.

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    for block in padded_blocks(data) {
        let mut w = [0u32; 80];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5a827999u32),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Merkle–Damgård padding shared by SHA-1 and SHA-256: 0x80, zeros, 64-bit
/// big-endian bit length, to a multiple of 64 bytes.
fn padded_blocks(data: &[u8]) -> Vec<[u8; 64]> {
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());
    padded
        .chunks_exact(64)
        .map(|c| {
            let mut block = [0u8; 64];
            block.copy_from_slice(c);
            block
        })
        .collect()
}

// ---------------------------------------------------------------------------
// AES-128 (FIPS 197) + CBC mode (SP 800-38A) + PKCS#7

const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab,
    0x76, 0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4,
    0x72, 0xc0, 0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71,
    0xd8, 0x31, 0x15, 0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2,
    0xeb, 0x27, 0xb2, 0x75, 0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6,
    0xb3, 0x29, 0xe3, 0x2f, 0x84, 0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb,
    0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf, 0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45,
    0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8, 0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5,
    0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2, 0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44,
    0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73, 0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a,
    0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb, 0xe0, 0x32, 0x3a, 0x0a, 0x49,
    0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79, 0xe7, 0xc8, 0x37, 0x6d,
    0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08, 0xba, 0x78, 0x25,
    0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a, 0x70, 0x3e,
    0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e, 0xe1,
    0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb,
    0x16,
];

fn inv_sbox() -> [u8; 256] {
    let mut inv = [0u8; 256];
    for (i, &v) in SBOX.iter().enumerate() {
        inv[v as usize] = i as u8;
    }
    inv
}

fn xtime(b: u8) -> u8 {
    (b << 1) ^ (if b & 0x80 != 0 { 0x1b } else { 0 })
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    p
}

fn expand_key(key: &[u8; 16]) -> [[u8; 16]; 11] {
    let mut round_keys = [[0u8; 16]; 11];
    round_keys[0] = *key;
    let mut rcon = 1u8;
    for round in 1..11 {
        let prev = round_keys[round - 1];
        let mut word = [prev[12], prev[13], prev[14], prev[15]];
        word.rotate_left(1);
        for b in &mut word {
            *b = SBOX[*b as usize];
        }
        word[0] ^= rcon;
        rcon = xtime(rcon);
        for i in 0..4 {
            round_keys[round][i] = prev[i] ^ word[i];
        }
        for i in 4..16 {
            round_keys[round][i] = prev[i] ^ round_keys[round][i - 4];
        }
    }
    round_keys
}

fn encrypt_block(block: &mut [u8; 16], round_keys: &[[u8; 16]; 11]) {
    for (b, k) in block.iter_mut().zip(round_keys[0].iter()) {
        *b ^= k;
    }
    for round in 1..11 {
        // SubBytes
        for b in block.iter_mut() {
            *b = SBOX[*b as usize];
        }
        // ShiftRows (state is column-major: byte index = col*4 + row)
        let s = *block;
        for row in 1..4 {
            for col in 0..4 {
                block[col * 4 + row] = s[((col + row) % 4) * 4 + row];
            }
        }
        // MixColumns (skipped in the final round)
        if round != 10 {
            let s = *block;
            for col in 0..4 {
                let c = &s[col * 4..col * 4 + 4];
                block[col * 4] = gmul(c[0], 2) ^ gmul(c[1], 3) ^ c[2] ^ c[3];
                block[col * 4 + 1] = c[0] ^ gmul(c[1], 2) ^ gmul(c[2], 3) ^ c[3];
                block[col * 4 + 2] = c[0] ^ c[1] ^ gmul(c[2], 2) ^ gmul(c[3], 3);
                block[col * 4 + 3] = gmul(c[0], 3) ^ c[1] ^ c[2] ^ gmul(c[3], 2);
            }
        }
        for (b, k) in block.iter_mut().zip(round_keys[round].iter()) {
            *b ^= k;
        }
    }
}

fn decrypt_block(block: &mut [u8; 16], round_keys: &[[u8; 16]; 11], inv: &[u8; 256]) {
    for (b, k) in block.iter_mut().zip(round_keys[10].iter()) {
        *b ^= k;
    }
    for round in (1..11).rev() {
        // InvShiftRows
        let s = *block;
        for row in 1..4 {
            for col in 0..4 {
                block[((col + row) % 4) * 4 + row] = s[col * 4 + row];
            }
        }
        // InvSubBytes
        for b in block.iter_mut() {
            *b = inv[*b as usize];
        }
        for (b, k) in block.iter_mut().zip(round_keys[round - 1].iter()) {
            *b ^= k;
        }
        // InvMixColumns (skipped after the first processed round, which
        // corresponds to the encryption side's final round)
        if round != 1 {
            let s = *block;
            for col in 0..4 {
                let c = &s[col * 4..col * 4 + 4];
                block[col * 4] =
                    gmul(c[0], 0x0e) ^ gmul(c[1], 0x0b) ^ gmul(c[2], 0x0d) ^ gmul(c[3], 0x09);
                block[col * 4 + 1] =
                    gmul(c[0], 0x09) ^ gmul(c[1], 0x0e) ^ gmul(c[2], 0x0b) ^ gmul(c[3], 0x0d);
                block[col * 4 + 2] =
                    gmul(c[0], 0x0d) ^ gmul(c[1], 0x09) ^ gmul(c[2], 0x0e) ^ gmul(c[3], 0x0b);
                block[col * 4 + 3] =
                    gmul(c[0], 0x0b) ^ gmul(c[1], 0x0d) ^ gmul(c[2], 0x09) ^ gmul(c[3], 0x0e);
            }
        }
    }
}

/// AES-128-CBC encrypt with PKCS#7 padding.
pub fn aes128_cbc_encrypt(key: &[u8; 16], iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    let round_keys = expand_key(key);
    let pad = 16 - (plaintext.len() % 16);
    let mut data = plaintext.to_vec();
    data.extend(std::iter::repeat(pad as u8).take(pad));
    let mut prev = *iv;
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.chunks_exact(16) {
        let mut block = [0u8; 16];
        for i in 0..16 {
            block[i] = chunk[i] ^ prev[i];
        }
        encrypt_block(&mut block, &round_keys);
        out.extend_from_slice(&block);
        prev = block;
    }
    out
}

/// AES-128-CBC decrypt, stripping PKCS#7 padding. Errors on malformed input
/// rather than panicking: device responses are untrusted bytes.
pub fn aes128_cbc_decrypt(
    key: &[u8; 16],
    iv: &[u8; 16],
    ciphertext: &[u8],
) -> Result<Vec<u8>, String> {
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(format!("ciphertext length {} not a positive multiple of 16", ciphertext.len()));
    }
    let round_keys = expand_key(key);
    let inv = inv_sbox();
    let mut prev = *iv;
    let mut out = Vec::with_capacity(ciphertext.len());
    for chunk in ciphertext.chunks_exact(16) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        let saved = block;
        decrypt_block(&mut block, &round_keys, &inv);
        for i in 0..16 {
            block[i] ^= prev[i];
        }
        out.extend_from_slice(&block);
        prev = saved;
    }
    let pad = *out.last().unwrap() as usize;
    if pad == 0 || pad > 16 || out.len() < pad {
        return Err("invalid PKCS#7 padding".into());
    }
    if !out[out.len() - pad..].iter().all(|&b| b as usize == pad) {
        return Err("invalid PKCS#7 padding".into());
    }
    out.truncate(out.len() - pad);
    Ok(out)
}

// ---------------------------------------------------------------------------
// base64 (RFC 4648, standard alphabet)

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: Vec<u8> = text
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if cleaned.len() % 4 != 0 {
        return Err("base64 length not a multiple of 4".into());
    }
    let mut out = Vec::with_capacity(cleaned.len() / 4 * 3);
    for chunk in cleaned.chunks_exact(4) {
        let mut vals = [0u32; 4];
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            vals[i] = match c {
                b'A'..=b'Z' => u32::from(c - b'A'),
                b'a'..=b'z' => u32::from(c - b'a') + 26,
                b'0'..=b'9' => u32::from(c - b'0') + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if i >= 2 => {
                    pad += 1;
                    0
                }
                _ => return Err(format!("invalid base64 byte {c}")),
            };
        }
        let n = (vals[0] << 18) | (vals[1] << 12) | (vals[2] << 6) | vals[3];
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

pub fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time byte comparison for token checks: the loop always visits
/// every byte so timing does not leak the first mismatch position.
pub fn eq_constant_time(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_fips_vectors() {
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_encode(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_encode(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha1_fips_vectors() {
        assert_eq!(
            hex_encode(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex_encode(&sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex_encode(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    fn from_hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn aes128_fips197_block_vector() {
        let key: [u8; 16] = from_hex("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .unwrap();
        let mut block: [u8; 16] = from_hex("00112233445566778899aabbccddeeff")
            .try_into()
            .unwrap();
        let round_keys = expand_key(&key);
        encrypt_block(&mut block, &round_keys);
        assert_eq!(hex_encode(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");
        let inv = inv_sbox();
        decrypt_block(&mut block, &round_keys, &inv);
        assert_eq!(hex_encode(&block), "00112233445566778899aabbccddeeff");
    }

    #[test]
    fn aes128_cbc_sp800_38a_vector() {
        let key: [u8; 16] = from_hex("2b7e151628aed2a6abf7158809cf4f3c")
            .try_into()
            .unwrap();
        let iv: [u8; 16] = from_hex("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .unwrap();
        let plaintext = from_hex("6bc1bee22e409f96e93d7e117393172a");
        let ciphertext = aes128_cbc_encrypt(&key, &iv, &plaintext);
        // First block matches the NIST vector; the second block is the
        // PKCS#7 padding block, which the NIST vector (no padding) lacks.
        assert_eq!(
            hex_encode(&ciphertext[..16]),
            "7649abac8119b246cee98e9b12e9197d"
        );
        let decrypted = aes128_cbc_decrypt(&key, &iv, &ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn cbc_round_trips_arbitrary_lengths() {
        let key = [7u8; 16];
        let iv = [9u8; 16];
        for len in [0usize, 1, 15, 16, 17, 100] {
            let data: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let ct = aes128_cbc_encrypt(&key, &iv, &data);
            assert_eq!(aes128_cbc_decrypt(&key, &iv, &ct).unwrap(), data, "len {len}");
        }
    }

    #[test]
    fn cbc_decrypt_rejects_garbage() {
        let key = [1u8; 16];
        let iv = [2u8; 16];
        assert!(aes128_cbc_decrypt(&key, &iv, &[0u8; 15]).is_err());
        assert!(aes128_cbc_decrypt(&key, &iv, &[]).is_err());
    }

    #[test]
    fn base64_rfc4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in cases {
            assert_eq!(base64_encode(plain.as_bytes()), encoded);
            assert_eq!(base64_decode(encoded).unwrap(), plain.as_bytes());
        }
        assert!(base64_decode("a").is_err());
        assert!(base64_decode("a!==").is_err());
    }

    #[test]
    fn constant_time_eq() {
        assert!(eq_constant_time(b"same", b"same"));
        assert!(!eq_constant_time(b"same", b"diff"));
        assert!(!eq_constant_time(b"short", b"longer"));
    }
}
