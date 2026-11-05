//! CRC-32/ISO-HDLC (reflected polynomial 0xEDB88320, initial and final XOR 0xFFFFFFFF), the CRC
//! of the AU table's `irap_crc32` and of `java.util.zip.CRC32`. A const table, so the crate needs
//! no dependency for it.

const fn table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut crc = index as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[index] = crc;
        index += 1;
    }
    table
}

const TABLE: [u32; 256] = table();

/// CRC-32/ISO-HDLC of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &byte| {
        TABLE[usize::from((crc as u8) ^ byte)] ^ (crc >> 8)
    })
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn check_values() {
        // The catalogue check value of CRC-32/ISO-HDLC.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        // python3 -c "import zlib; print(hex(zlib.crc32(bytes([0, 1, 2]))))"
        assert_eq!(crc32(&[0, 1, 2]), 0x0854_897F);
        assert_eq!(crc32(&[]), 0);
    }
}
