#!/usr/bin/env python3
"""
sms_pdu_watcher.py — Decode SMS PDUs from Asterisk debug logs.

Handles DCS=0x8a (reserved coding group) and other non-standard encodings
that Asterisk's res_pjsip_messaging.c leaves as UD='' (empty body).

Strategy:
  - Tail docker logs looking for pairs:
      DEBUG: SMS RP-DATA 'hex...'
      DEBUG: SMS UD='...' OA='...'
  - If UD is empty but OA is not: decode the RP-DATA ourselves
  - Append result to /home/ht/docker/logs/<instance>/otp_sms.txt

Run on HOST:
  sudo python3 sms_pdu_watcher.py [instance]
  instance: 1 or 2 (default: 2)
"""

import sys
import re
import os
import subprocess
from datetime import datetime

# ── GSM 7-bit default alphabet (TS 23.038 Table 1) ──────────────────────────
GSM7 = (
    "@£$¥èéùìòÇ\nØø\rÅåΔ_ΦΓΛΩΠΨΣΘΞ\x1bÆæßÉ "
    "!\"#¤%&'()*+,-./0123456789:;<=>?"
    "¡ABCDEFGHIJKLMNOPQRSTUVWXYZÄÖÑÜ§"
    "¿abcdefghijklmnopqrstuvwxyzäöñüà"
)


def gsm7_decode(octets, num_septets):
    """Decode packed GSM 7-bit octets into a string of num_septets characters."""
    acc, acc_bits = 0, 0
    result = []
    for byte in octets:
        acc |= byte << acc_bits
        acc_bits += 8
        while acc_bits >= 7 and len(result) < num_septets:
            idx = acc & 0x7F
            result.append(GSM7[idx] if idx < len(GSM7) else '\ufffd')
            acc >>= 7
            acc_bits -= 7
    return ''.join(result)


def decode_ud(ud_bytes, dcs, udhi, tp_udl):
    """
    Decode TP-UD based on DCS.
    Returns decoded text string.
    """
    # Determine charset from DCS
    # 0x08 or top nibble 0x1_ / 0x9_: UCS-2
    if dcs == 0x08 or (dcs & 0xF0) in (0x10, 0x90):
        charset = 'ucs2'
    # 0x04 / 0x44 / 0xF4 / 0xF5: 8-bit data
    elif dcs in (0x04, 0x44, 0xF4, 0xF5) or (dcs & 0x0C) == 0x04:
        charset = '8bit'
    else:
        # Default / 0x00 / reserved (0x80-0xBF) → GSM 7-bit
        # DCS=0x8a falls here per TS 23.038: reserved groups → default alphabet
        charset = 'gsm7'

    if not ud_bytes:
        return ''

    # Handle User Data Header
    if udhi:
        udh_len_byte = ud_bytes[0]
        udh_total = udh_len_byte + 1  # includes the length byte itself
        if udh_total >= len(ud_bytes):
            # Invalid UDH length — ignore UDHI, decode entire UD
            udhi = False
        else:
            if charset == 'gsm7':
                # UDH occupies udh_total octets = ceil(udh_total*8/7) septets
                udh_septets = (udh_total * 8 + 6) // 7
                # Decode all tp_udl septets, skip first udh_septets
                all_chars = gsm7_decode(ud_bytes, tp_udl)
                return all_chars[udh_septets:].strip('\x00')
            elif charset == 'ucs2':
                ud_data = ud_bytes[udh_total:]
                if udh_total % 2 != 0:
                    ud_data = ud_data[1:]  # align to 2-byte boundary
                return ud_data.decode('utf-16-be', errors='replace')
            else:
                return bytes(ud_bytes[udh_total:]).decode('latin-1', errors='replace')

    # No UDH
    if charset == 'gsm7':
        return gsm7_decode(ud_bytes, tp_udl).strip('\x00')
    elif charset == 'ucs2':
        return bytes(ud_bytes).decode('utf-16-be', errors='replace')
    else:
        return bytes(ud_bytes).decode('latin-1', errors='replace')


def decode_rp_data(hex_str):
    """
    Parse SMS RP-DATA and return decoded UD text.
    Returns (text, dcs) or (None, None) on error.
    """
    try:
        data = bytearray.fromhex(hex_str)
        p = 0

        p += 1  # RP-MTI
        p += 1  # RP-MR

        # RP-OA (SMSC address) — skip
        rp_oa_len = data[p]; p += 1
        p += rp_oa_len

        # RP-DA — skip
        rp_da_len = data[p]; p += 1
        p += rp_da_len

        # RP-UI (TPDU)
        rp_ui_len = data[p]; p += 1
        tpdu = data[p:p + rp_ui_len]

        if not tpdu:
            return None, None

        # ── Parse SMS-DELIVER TPDU ─────────────────────────────────────────
        tp = 0
        tp_flags = tpdu[tp]; tp += 1
        tp_mti  = tp_flags & 0x03
        tp_udhi = bool(tp_flags & 0x40)

        if tp_mti != 0:  # Not SMS-DELIVER
            return None, None

        # TP-OA: skip (we get OA from Asterisk's debug line)
        oa_len = tpdu[tp]; tp += 1   # address length (semi-octets)
        tp += 1                       # TON/NPI byte
        tp += (oa_len + 1) // 2       # address value bytes

        tp_pid = tpdu[tp]; tp += 1
        tp_dcs = tpdu[tp]; tp += 1
        tp += 7                       # TP-SCTS
        tp_udl = tpdu[tp]; tp += 1
        ud_bytes = tpdu[tp:]

        text = decode_ud(ud_bytes, tp_dcs, tp_udhi, tp_udl)
        return text, tp_dcs

    except Exception as e:
        return None, None


# ── OTP extraction ────────────────────────────────────────────────────────────
OTP_PATTERNS = [
    re.compile(r'(?:verification\s+)?code[:\s]+(\d{4,8})', re.IGNORECASE),
    re.compile(r'\bOTP[:\s]+(\d{4,8})', re.IGNORECASE),
    re.compile(r'\bPIN[:\s]+(\d{4,8})', re.IGNORECASE),
    re.compile(r'passcode[:\s]+(\d{4,8})', re.IGNORECASE),
    re.compile(r'(?:use|enter|is)\s+(\d{4,8})\b', re.IGNORECASE),
    re.compile(r'\b(\d{4,8})\b'),
]

def extract_otp(text):
    for pat in OTP_PATTERNS:
        m = pat.search(text)
        if m:
            return m.group(1)
    return None


# ── Log patterns ──────────────────────────────────────────────────────────────
RE_RP_DATA = re.compile(r"parse_rpdata: SMS RP-DATA '([0-9a-fA-F]+)'")
RE_UD_OA   = re.compile(r"parse_tpdu: SMS UD='(.*?)' OA='(.*?)'")


def main():
    instance = sys.argv[1] if len(sys.argv) > 1 else '2'
    if instance == '1':
        container = 'asterisk-docker-asterisk-1'
        log_dir   = '/home/ht/docker/logs/1'
    else:
        container = 'asterisk-docker-asterisk2-1'
        log_dir   = '/home/ht/docker/logs/2'

    os.makedirs(log_dir, exist_ok=True)
    otp_file = os.path.join(log_dir, 'otp_sms.txt')

    print(f"[sms_pdu_watcher] Watching {container} for SMS PDUs...", flush=True)

    proc = subprocess.Popen(
        ['sudo', 'docker', 'logs', '--follow', '--since', '0s', container],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1
    )

    pending_rp_data = None  # last seen RP-DATA hex, waiting for its UD line

    try:
        for line in proc.stdout:
            line = line.strip()

            # Buffer RP-DATA hex
            m = RE_RP_DATA.search(line)
            if m:
                pending_rp_data = m.group(1)
                continue

            # Check the UD decode result
            m = RE_UD_OA.search(line)
            if m:
                ud_text = m.group(1)
                oa      = m.group(2)

                if ud_text == '' and oa and pending_rp_data:
                    # Asterisk couldn't decode — try ourselves
                    text, dcs = decode_rp_data(pending_rp_data)
                    if text:
                        otp = extract_otp(text)
                        timestamp = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
                        print(f"[{timestamp}] From: {oa} | DCS: 0x{dcs:02x} | OTP: {otp} | {text}", flush=True)
                        if otp:
                            entry = (
                                f"[{timestamp}] From: {oa}\n"
                                f"SMS: {text}\n"
                                f"OTP: {otp}\n"
                                f"{'=' * 50}\n\n"
                            )
                            with open(otp_file, 'a') as f:
                                f.write(entry)

                pending_rp_data = None

    except KeyboardInterrupt:
        print("\n[sms_pdu_watcher] Stopped.", flush=True)
    finally:
        proc.terminate()


if __name__ == '__main__':
    main()
