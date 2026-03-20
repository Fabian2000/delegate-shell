use crate::interpreter::value::Value;
use super::registry::{BuiltinRegistry, Param, Type};

fn md5(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else { unreachable!() };
    let result = md5_compute(s.as_bytes());
    Ok(Value::string_from(&bytes_to_hex(&result)))
}

fn sha256(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else { unreachable!() };
    let result = sha256_compute(s.as_bytes());
    Ok(Value::string_from(&bytes_to_hex(&result)))
}

fn sha512(args: &[Value]) -> Result<Value, String> {
    let Some(s) = args[0].as_str_ref() else { unreachable!() };
    let result = sha512_compute(s.as_bytes());
    Ok(Value::string_from(&bytes_to_hex(&result)))
}

fn uuid(_args: &[Value]) -> Result<Value, String> {
    let bytes = random_bytes_16();
    // UUID v4 format
    Ok(Value::string_from(&format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-4{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6] & 0x0F, bytes[7],
        (bytes[8] & 0x3F) | 0x80, bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )))
}

// --- Helpers ---

fn bytes_to_hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(data.len() * 2);
    for b in data { let _ = write!(hex, "{b:02x}"); }
    hex
}

fn random_bytes_16() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let seed = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ).unwrap_or(0);
    let mut state = seed;
    for byte in &mut bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    bytes
}

// === MD5 ===

fn md5_compute(msg: &[u8]) -> [u8; 16] {
    const SHIFT: [u32; 64] = [
        7,12,17,22, 7,12,17,22, 7,12,17,22, 7,12,17,22,
        5, 9,14,20, 5, 9,14,20, 5, 9,14,20, 5, 9,14,20,
        4,11,16,23, 4,11,16,23, 4,11,16,23, 4,11,16,23,
        6,10,15,21, 6,10,15,21, 6,10,15,21, 6,10,15,21,
    ];
    const KTAB: [u32; 64] = [
        0xd76a_a478, 0xe8c7_b756, 0x2420_70db, 0xc1bd_ceee, 0xf57c_0faf, 0x4787_c62a, 0xa830_4613, 0xfd46_9501,
        0x6980_98d8, 0x8b44_f7af, 0xffff_5bb1, 0x895c_d7be, 0x6b90_1122, 0xfd98_7193, 0xa679_438e, 0x49b4_0821,
        0xf61e_2562, 0xc040_b340, 0x265e_5a51, 0xe9b6_c7aa, 0xd62f_105d, 0x0244_1453, 0xd8a1_e681, 0xe7d3_fbc8,
        0x21e1_cde6, 0xc337_07d6, 0xf4d5_0d87, 0x455a_14ed, 0xa9e3_e905, 0xfcef_a3f8, 0x676f_02d9, 0x8d2a_4c8a,
        0xfffa_3942, 0x8771_f681, 0x6d9d_6122, 0xfde5_380c, 0xa4be_ea44, 0x4bde_cfa9, 0xf6bb_4b60, 0xbebf_bc70,
        0x289b_7ec6, 0xeaa1_27fa, 0xd4ef_3085, 0x0488_1d05, 0xd9d4_d039, 0xe6db_99e5, 0x1fa2_7cf8, 0xc4ac_5665,
        0xf429_2244, 0x432a_ff97, 0xab94_23a7, 0xfc93_a039, 0x655b_59c3, 0x8f0c_cc92, 0xffef_f47d, 0x8584_5dd1,
        0x6fa8_7e4f, 0xfe2c_e6e0, 0xa301_4314, 0x4e08_11a1, 0xf753_7e82, 0xbd3a_f235, 0x2ad7_d2bb, 0xeb86_d391,
    ];
    let (mut a0, mut b0, mut c0, mut d0): (u32, u32, u32, u32) = (0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476);
    let orig_len_bits = (msg.len() as u64) * 8;
    let mut padded = msg.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 { padded.push(0); }
    padded.extend_from_slice(&orig_len_bits.to_le_bytes());
    for chunk in padded.chunks(64) {
        let mut md5_m = [0u32; 16];
        for (idx, word) in chunk.chunks(4).enumerate() { md5_m[idx] = u32::from_le_bytes([word[0], word[1], word[2], word[3]]); }
        let (mut aa, mut bb, mut cc, mut dd) = (a0, b0, c0, d0);
        for idx in 0..64 {
            let (ff, gg) = match idx {
                0..=15 => ((bb & cc) | ((!bb) & dd), idx),
                16..=31 => ((dd & bb) | ((!dd) & cc), (5 * idx + 1) % 16),
                32..=47 => (bb ^ cc ^ dd, (3 * idx + 5) % 16),
                _ => (cc ^ (bb | (!dd)), (7 * idx) % 16),
            };
            let temp = dd; dd = cc; cc = bb;
            bb = bb.wrapping_add((aa.wrapping_add(ff).wrapping_add(KTAB[idx]).wrapping_add(md5_m[gg])).rotate_left(SHIFT[idx]));
            aa = temp;
        }
        a0 = a0.wrapping_add(aa); b0 = b0.wrapping_add(bb); c0 = c0.wrapping_add(cc); d0 = d0.wrapping_add(dd);
    }
    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&a0.to_le_bytes());
    result[4..8].copy_from_slice(&b0.to_le_bytes());
    result[8..12].copy_from_slice(&c0.to_le_bytes());
    result[12..16].copy_from_slice(&d0.to_le_bytes());
    result
}

// === SHA-256 ===

fn sha256_compute(msg: &[u8]) -> [u8; 32] {
    const KTAB: [u32; 64] = [
        0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1, 0x923f_82a4, 0xab1c_5ed5,
        0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3, 0x72be_5d74, 0x80de_b1fe, 0x9bdc_06a7, 0xc19b_f174,
        0xe49b_69c1, 0xefbe_4786, 0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f, 0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da,
        0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7, 0xc6e0_0bf3, 0xd5a7_9147, 0x06ca_6351, 0x1429_2967,
        0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc, 0x5338_0d13, 0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85,
        0xa2bf_e8a1, 0xa81a_664b, 0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070,
        0x19a4_c116, 0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a, 0x5b9c_ca4f, 0x682e_6ff3,
        0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208, 0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7, 0xc671_78f2,
    ];
    let mut hash: [u32; 8] = [0x6a09_e667, 0xbb67_ae85, 0x3c6e_f372, 0xa54f_f53a, 0x510e_527f, 0x9b05_688c, 0x1f83_d9ab, 0x5be0_cd19];
    let orig_len_bits = (msg.len() as u64) * 8;
    let mut padded = msg.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 { padded.push(0); }
    padded.extend_from_slice(&orig_len_bits.to_be_bytes());
    for chunk in padded.chunks(64) {
        let mut ww = [0u32; 64];
        for (idx, word) in chunk.chunks(4).enumerate() { ww[idx] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]); }
        for idx in 16..64 {
            let s0 = ww[idx-15].rotate_right(7) ^ ww[idx-15].rotate_right(18) ^ (ww[idx-15] >> 3);
            let s1 = ww[idx-2].rotate_right(17) ^ ww[idx-2].rotate_right(19) ^ (ww[idx-2] >> 10);
            ww[idx] = ww[idx-16].wrapping_add(s0).wrapping_add(ww[idx-7]).wrapping_add(s1);
        }
        let [mut va, mut vb, mut vc, mut vd, mut ve, mut vf, mut vg, mut vh] = hash;
        for idx in 0..64 {
            let s1 = ve.rotate_right(6) ^ ve.rotate_right(11) ^ ve.rotate_right(25);
            let ch = (ve & vf) ^ ((!ve) & vg);
            let t1 = vh.wrapping_add(s1).wrapping_add(ch).wrapping_add(KTAB[idx]).wrapping_add(ww[idx]);
            let s0 = va.rotate_right(2) ^ va.rotate_right(13) ^ va.rotate_right(22);
            let maj = (va & vb) ^ (va & vc) ^ (vb & vc);
            let t2 = s0.wrapping_add(maj);
            vh = vg; vg = vf; vf = ve; ve = vd.wrapping_add(t1); vd = vc; vc = vb; vb = va; va = t1.wrapping_add(t2);
        }
        hash[0] = hash[0].wrapping_add(va); hash[1] = hash[1].wrapping_add(vb); hash[2] = hash[2].wrapping_add(vc); hash[3] = hash[3].wrapping_add(vd);
        hash[4] = hash[4].wrapping_add(ve); hash[5] = hash[5].wrapping_add(vf); hash[6] = hash[6].wrapping_add(vg); hash[7] = hash[7].wrapping_add(vh);
    }
    let mut result = [0u8; 32];
    for (idx, val) in hash.iter().enumerate() { result[idx*4..(idx+1)*4].copy_from_slice(&val.to_be_bytes()); }
    result
}

// === SHA-512 ===

fn sha512_compute(msg: &[u8]) -> [u8; 64] {
    const KTAB: [u64; 80] = [
        0x428a_2f98_d728_ae22, 0x7137_4491_23ef_65cd, 0xb5c0_fbcf_ec4d_3b2f, 0xe9b5_dba5_8189_dbbc,
        0x3956_c25b_f348_b538, 0x59f1_11f1_b605_d019, 0x923f_82a4_af19_4f9b, 0xab1c_5ed5_da6d_8118,
        0xd807_aa98_a303_0242, 0x1283_5b01_4570_6fbe, 0x2431_85be_4ee4_b28c, 0x550c_7dc3_d5ff_b4e2,
        0x72be_5d74_f27b_896f, 0x80de_b1fe_3b16_96b1, 0x9bdc_06a7_25c7_1235, 0xc19b_f174_cf69_2694,
        0xe49b_69c1_9ef1_4ad2, 0xefbe_4786_384f_25e3, 0x0fc1_9dc6_8b8c_d5b5, 0x240c_a1cc_77ac_9c65,
        0x2de9_2c6f_592b_0275, 0x4a74_84aa_6ea6_e483, 0x5cb0_a9dc_bd41_fbd4, 0x76f9_88da_8311_53b5,
        0x983e_5152_ee66_dfab, 0xa831_c66d_2db4_3210, 0xb003_27c8_98fb_213f, 0xbf59_7fc7_beef_0ee4,
        0xc6e0_0bf3_3da8_8fc2, 0xd5a7_9147_930a_a725, 0x06ca_6351_e003_826f, 0x1429_2967_0a0e_6e70,
        0x27b7_0a85_46d2_2ffc, 0x2e1b_2138_5c26_c926, 0x4d2c_6dfc_5ac4_2aed, 0x5338_0d13_9d95_b3df,
        0x650a_7354_8baf_63de, 0x766a_0abb_3c77_b2a8, 0x81c2_c92e_47ed_aee6, 0x9272_2c85_1482_353b,
        0xa2bf_e8a1_4cf1_0364, 0xa81a_664b_bc42_3001, 0xc24b_8b70_d0f8_9791, 0xc76c_51a3_0654_be30,
        0xd192_e819_d6ef_5218, 0xd699_0624_5565_a910, 0xf40e_3585_5771_202a, 0x106a_a070_32bb_d1b8,
        0x19a4_c116_b8d2_d0c8, 0x1e37_6c08_5141_ab53, 0x2748_774c_df8e_eb99, 0x34b0_bcb5_e19b_48a8,
        0x391c_0cb3_c5c9_5a63, 0x4ed8_aa4a_e341_8acb, 0x5b9c_ca4f_7763_e373, 0x682e_6ff3_d6b2_b8a3,
        0x748f_82ee_5def_b2fc, 0x78a5_636f_4317_2f60, 0x84c8_7814_a1f0_ab72, 0x8cc7_0208_1a64_39ec,
        0x90be_fffa_2363_1e28, 0xa450_6ceb_de82_bde9, 0xbef9_a3f7_b2c6_7915, 0xc671_78f2_e372_532b,
        0xca27_3ece_ea26_619c, 0xd186_b8c7_21c0_c207, 0xeada_7dd6_cde0_eb1e, 0xf57d_4f7f_ee6e_d178,
        0x06f0_67aa_7217_6fba, 0x0a63_7dc5_a2c8_98a6, 0x113f_9804_bef9_0dae, 0x1b71_0b35_131c_471b,
        0x28db_77f5_2304_7d84, 0x32ca_ab7b_40c7_2493, 0x3c9e_be0a_15c9_bebc, 0x431d_67c4_9c10_0d4c,
        0x4cc5_d4be_cb3e_42b6, 0x597f_299c_fc65_7e2a, 0x5fcb_6fab_3ad6_faec, 0x6c44_198c_4a47_5817,
    ];
    let mut hash: [u64; 8] = [
        0x6a09_e667_f3bc_c908, 0xbb67_ae85_84ca_a73b, 0x3c6e_f372_fe94_f82b, 0xa54f_f53a_5f1d_36f1,
        0x510e_527f_ade6_82d1, 0x9b05_688c_2b3e_6c1f, 0x1f83_d9ab_fb41_bd6b, 0x5be0_cd19_137e_2179,
    ];
    let orig_len_bits = (msg.len() as u128) * 8;
    let mut padded = msg.to_vec();
    padded.push(0x80);
    while padded.len() % 128 != 112 { padded.push(0); }
    padded.extend_from_slice(&orig_len_bits.to_be_bytes());
    for chunk in padded.chunks(128) {
        let mut ww = [0u64; 80];
        for (idx, word) in chunk.chunks(8).enumerate() {
            ww[idx] = u64::from_be_bytes([word[0], word[1], word[2], word[3], word[4], word[5], word[6], word[7]]);
        }
        for idx in 16..80 {
            let s0 = ww[idx-15].rotate_right(1) ^ ww[idx-15].rotate_right(8) ^ (ww[idx-15] >> 7);
            let s1 = ww[idx-2].rotate_right(19) ^ ww[idx-2].rotate_right(61) ^ (ww[idx-2] >> 6);
            ww[idx] = ww[idx-16].wrapping_add(s0).wrapping_add(ww[idx-7]).wrapping_add(s1);
        }
        let [mut va, mut vb, mut vc, mut vd, mut ve, mut vf, mut vg, mut vh] = hash;
        for idx in 0..80 {
            let s1 = ve.rotate_right(14) ^ ve.rotate_right(18) ^ ve.rotate_right(41);
            let ch = (ve & vf) ^ ((!ve) & vg);
            let t1 = vh.wrapping_add(s1).wrapping_add(ch).wrapping_add(KTAB[idx]).wrapping_add(ww[idx]);
            let s0 = va.rotate_right(28) ^ va.rotate_right(34) ^ va.rotate_right(39);
            let maj = (va & vb) ^ (va & vc) ^ (vb & vc);
            let t2 = s0.wrapping_add(maj);
            vh = vg; vg = vf; vf = ve; ve = vd.wrapping_add(t1); vd = vc; vc = vb; vb = va; va = t1.wrapping_add(t2);
        }
        hash[0] = hash[0].wrapping_add(va); hash[1] = hash[1].wrapping_add(vb); hash[2] = hash[2].wrapping_add(vc); hash[3] = hash[3].wrapping_add(vd);
        hash[4] = hash[4].wrapping_add(ve); hash[5] = hash[5].wrapping_add(vf); hash[6] = hash[6].wrapping_add(vg); hash[7] = hash[7].wrapping_add(vh);
    }
    let mut result = [0u8; 64];
    for (idx, val) in hash.iter().enumerate() { result[idx*8..(idx+1)*8].copy_from_slice(&val.to_be_bytes()); }
    result
}

pub fn register(reg: &mut BuiltinRegistry) -> Result<(), String> {
    reg.add("md5", &[Param::Required(Type::String)], Type::String, md5)?;
    reg.add("sha256", &[Param::Required(Type::String)], Type::String, sha256)?;
    reg.add("sha512", &[Param::Required(Type::String)], Type::String, sha512)?;
    reg.add("uuid", &[], Type::String, uuid)?;

    Ok(())
}
