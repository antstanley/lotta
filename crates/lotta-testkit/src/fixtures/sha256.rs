const INITIAL: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];
const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

pub(super) fn digest(input: &[u8]) -> [u8; 32] {
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let blocks = input.len().saturating_add(9).div_ceil(64);
    let mut state = INITIAL;
    for block_index in 0..blocks {
        let mut block = [0_u8; 64];
        fill_block(&mut block, input, block_index, blocks, bit_len);
        compress(&mut state, &block);
    }
    state_to_bytes(state)
}

pub(super) fn lowercase_hex(input: &[u8]) -> String {
    let mut output = String::with_capacity(64);
    for byte in digest(input) {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

fn fill_block(block: &mut [u8; 64], input: &[u8], block_index: usize, blocks: usize, bit_len: u64) {
    let start = block_index.saturating_mul(64);
    for (offset, slot) in block.iter_mut().enumerate() {
        let index = start.saturating_add(offset);
        if index < input.len() {
            *slot = input[index];
        } else if index == input.len() {
            *slot = 0x80;
        }
    }
    if block_index + 1 == blocks {
        block[56..].copy_from_slice(&bit_len.to_be_bytes());
    }
}

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut words = [0_u32; 64];
    for (index, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
        words[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for index in 16..64 {
        let s0 = words[index - 15].rotate_right(7)
            ^ words[index - 15].rotate_right(18)
            ^ (words[index - 15] >> 3);
        let s1 = words[index - 2].rotate_right(17)
            ^ words[index - 2].rotate_right(19)
            ^ (words[index - 2] >> 10);
        words[index] = words[index - 16]
            .wrapping_add(s0)
            .wrapping_add(words[index - 7])
            .wrapping_add(s1);
    }
    rounds(state, &words);
}

fn rounds(state: &mut [u32; 8], words: &[u32; 64]) {
    let mut value = *state;
    for index in 0..64 {
        let sigma1 =
            value[4].rotate_right(6) ^ value[4].rotate_right(11) ^ value[4].rotate_right(25);
        let choose = (value[4] & value[5]) ^ (!value[4] & value[6]);
        let first = value[7]
            .wrapping_add(sigma1)
            .wrapping_add(choose)
            .wrapping_add(K[index])
            .wrapping_add(words[index]);
        let sigma0 =
            value[0].rotate_right(2) ^ value[0].rotate_right(13) ^ value[0].rotate_right(22);
        let majority = (value[0] & value[1]) ^ (value[0] & value[2]) ^ (value[1] & value[2]);
        let second = sigma0.wrapping_add(majority);
        value = [
            first.wrapping_add(second),
            value[0],
            value[1],
            value[2],
            value[3].wrapping_add(first),
            value[4],
            value[5],
            value[6],
        ];
    }
    for index in 0..8 {
        state[index] = state[index].wrapping_add(value[index]);
    }
}

fn state_to_bytes(state: [u32; 8]) -> [u8; 32] {
    let mut output = [0_u8; 32];
    for (index, value) in state.into_iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    output
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        _ => char::from(b'a' + value - 10),
    }
}

#[cfg(test)]
mod tests {
    use super::lowercase_hex;

    #[test]
    fn nist_empty_vector() {
        assert_eq!(
            lowercase_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn nist_abc_vector() {
        assert_eq!(
            lowercase_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
