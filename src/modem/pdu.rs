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