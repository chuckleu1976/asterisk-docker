use std::sync::atomic::{AtomicU8, Ordering};

static MSG_REF: AtomicU8 = AtomicU8::new(1);

fn string_to_ucs2(message: &str) -> anyhow::Result<String> {
    let encoded: Vec<u16> = message.encode_utf16().collect();

    let mut bytes = Vec::with_capacity(encoded.len() * 2);
    for code_unit in encoded {
        bytes.extend_from_slice(&code_unit.to_be_bytes());
    }

    Ok(hex::encode_upper(bytes))
}

/// Encode a slice of chars to UCS2 hex, handling surrogate pairs correctly.
fn chars_to_ucs2(chars: &[char]) -> String {
    let mut bytes = Vec::with_capacity(chars.len() * 2);
    for &ch in chars {
        let mut buf = [0u16; 2];
        let n = ch.encode_utf16(&mut buf).len();
        for &unit in &buf[..n] {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
    }
    hex::encode_upper(bytes)
}

fn parse_number(number: &str) -> anyhow::Result<(u8, String)> {
    let cleaned_number = number.trim_start_matches('+').replace(' ', "");
    let addr_type = if number.starts_with('+') { 0x91 } else { 0x81 };

    let mut chars: Vec<char> = cleaned_number.chars().collect();
    if chars.len() % 2 != 0 {
        chars.push('F');
    }

    let mut swapped = String::with_capacity(chars.len());
    for chunk in chars.chunks(2) {
        if chunk.len() == 2 {
            swapped.push(chunk[1]);
            swapped.push(chunk[0]);
        }
    }

    Ok((addr_type, swapped))
}

fn encode_tpdu(mobile: &str, message: &str) -> anyhow::Result<(String, usize)> {
    const FIRST_OCTET: &str = "11";
    const MESSAGE_REF: &str = "00";
    const PID: &str = "00";
    const DCS: &str = "08";
    const VP: &str = "00";

    let destination_phone = mobile.trim_start_matches('+').replace(' ', "");
    let phone_len = format!("{:02X}", destination_phone.len());
    let (addr_type, swapped_number) = parse_number(mobile)?;
    let destination = format!("{}{:02X}{}", phone_len, addr_type, swapped_number);

    let encoded_text = string_to_ucs2(message)?;
    let udl = format!("{:02X}", message.chars().count() * 2);

    let tpdu = format!(
        "{}{}{}{}{}{}{}{}",
        FIRST_OCTET, MESSAGE_REF, destination, PID, DCS, VP, udl, encoded_text
    );

    let tpdu_length = tpdu.len() / 2;
    Ok((tpdu, tpdu_length))
}

pub fn build_pdu(mobile: &str, message: &str) -> anyhow::Result<(String, usize)> {
    const SMSC_INFO: &str = "00";
    let (tpdu, tpdu_length) = encode_tpdu(mobile, message)?;
    let full_pdu = format!("{}{}", SMSC_INFO, tpdu);
    Ok((full_pdu, tpdu_length))
}

fn encode_multipart_segment_tpdu(
    mobile: &str,
    segment: &[char],
    ref_num: u8,
    total: u8,
    part: u8,
) -> anyhow::Result<(String, usize)> {
    const FIRST_OCTET: &str = "51"; // 0x11 | 0x40: TP-UDHI bit set
    const MESSAGE_REF: &str = "00";
    const PID: &str = "00";
    const DCS: &str = "08";
    const VP: &str = "00";

    let destination_phone = mobile.trim_start_matches('+').replace(' ', "");
    let phone_len = format!("{:02X}", destination_phone.len());
    let (addr_type, swapped_number) = parse_number(mobile)?;
    let destination = format!("{}{:02X}{}", phone_len, addr_type, swapped_number);

    // UDH: UDHL(05) + IEI(00) + IEL(03) + ref + total + part = 6 bytes
    let udh = format!("050003{:02X}{:02X}{:02X}", ref_num, total, part);

    let ucs2_text = chars_to_ucs2(segment);
    // UDL is byte count: 6 (UDH) + actual UCS2 text bytes
    let text_byte_count = ucs2_text.len() / 2;
    let udl = format!("{:02X}", 6 + text_byte_count);

    let tpdu = format!(
        "{}{}{}{}{}{}{}{}{}",
        FIRST_OCTET, MESSAGE_REF, destination, PID, DCS, VP, udl, udh, ucs2_text
    );
    let tpdu_length = tpdu.len() / 2;
    Ok((tpdu, tpdu_length))
}

/// Build one or more PDUs for a message.
/// Single PDU for messages <=70 chars; multiple PDUs with UDH concatenation header for longer.
pub fn build_pdus(mobile: &str, message: &str) -> anyhow::Result<Vec<(String, usize)>> {
    let chars: Vec<char> = message.chars().collect();

    if chars.len() <= 70 {
        let (pdu, tpdu_len) = build_pdu(mobile, message)?;
        return Ok(vec![(pdu, tpdu_len)]);
    }

    // Split into 67-char segments (67 chars * 2 bytes + 6 bytes UDH = 140 bytes/PDU)
    let ref_num = MSG_REF.fetch_add(1, Ordering::Relaxed);
    let raw_segments: Vec<&[char]> = chars.chunks(67).collect();
    let total = raw_segments.len() as u8;

    let mut result = Vec::new();
    for (i, seg) in raw_segments.iter().enumerate() {
        let (tpdu, tpdu_len) =
            encode_multipart_segment_tpdu(mobile, seg, ref_num, total, (i + 1) as u8)?;
        result.push((format!("00{}", tpdu), tpdu_len));
    }
    Ok(result)
}

pub fn string_to_ucs2_pub(message: &str) -> anyhow::Result<String> {
    string_to_ucs2(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_message_produces_single_pdu() {
        let pdus = build_pdus("+8613800001234", "Hello").unwrap();
        assert_eq!(pdus.len(), 1);
    }

    #[test]
    fn exactly_70_chars_produces_single_pdu() {
        let msg = "A".repeat(70);
        let pdus = build_pdus("+8613800001234", &msg).unwrap();
        assert_eq!(pdus.len(), 1);
    }

    #[test]
    fn exactly_71_chars_produces_two_pdus() {
        let msg = "A".repeat(71);
        let pdus = build_pdus("+8613800001234", &msg).unwrap();
        assert_eq!(pdus.len(), 2);
    }

    #[test]
    fn long_message_correct_segment_count() {
        // 134 chars → 2 segments of 67
        let msg = "A".repeat(134);
        let pdus = build_pdus("+8613800001234", &msg).unwrap();
        assert_eq!(pdus.len(), 2);

        // 135 chars → 3 segments (67+67+1)
        let msg2 = "A".repeat(135);
        let pdus2 = build_pdus("+8613800001234", &msg2).unwrap();
        assert_eq!(pdus2.len(), 3);
    }

    #[test]
    fn multipart_pdu_starts_with_smsc_00_and_first_octet_51() {
        let msg = "A".repeat(71);
        let pdus = build_pdus("+8613800001234", &msg).unwrap();
        // full PDU hex: "00" (SMSC) + "51" (TP-UDHI first octet) + ...
        for (pdu, _) in &pdus {
            assert!(pdu.starts_with("00"), "PDU should start with SMSC 00");
            assert_eq!(&pdu[2..4], "51", "First octet should be 0x51 (TP-UDHI set)");
        }
    }

    #[test]
    fn single_pdu_has_no_udhi_bit() {
        let (pdu, _) = build_pdu("+8613800001234", "Hello").unwrap();
        // First octet after SMSC ("00") should be "11", not "51"
        assert_eq!(&pdu[2..4], "11", "Single PDU should not have TP-UDHI set");
    }

    #[test]
    fn chinese_chars_produce_correct_segment_count() {
        // 70 Chinese chars → single PDU
        let msg: String = "你好".repeat(35); // 70 chars
        let pdus = build_pdus("+8613800001234", &msg).unwrap();
        assert_eq!(pdus.len(), 1);

        // 71 Chinese chars → 2 PDUs
        let msg2: String = "你".repeat(71);
        let pdus2 = build_pdus("+8613800001234", &msg2).unwrap();
        assert_eq!(pdus2.len(), 2);
    }
}