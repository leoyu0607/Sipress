/// DTMF via RTP Events (RFC 4733)
use crate::rtp::packet::RtpPacket;

pub const PT_TELEPHONE_EVENT: u8 = 101;

pub fn digit_to_event(ch: char) -> Option<u8> {
    match ch {
        '0' => Some(0),
        '1' => Some(1),
        '2' => Some(2),
        '3' => Some(3),
        '4' => Some(4),
        '5' => Some(5),
        '6' => Some(6),
        '7' => Some(7),
        '8' => Some(8),
        '9' => Some(9),
        '*' => Some(10),
        '#' => Some(11),
        'A' | 'a' => Some(12),
        'B' | 'b' => Some(13),
        'C' | 'c' => Some(14),
        'D' | 'd' => Some(15),
        _ => None,
    }
}

fn dtmf_payload(event: u8, end: bool, volume: u8, duration: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4);
    buf.push(event);
    buf.push(if end { 0x80 } else { 0x00 } | (volume & 0x3F));
    buf.extend_from_slice(&duration.to_be_bytes());
    buf
}

/// Build a sequence of RTP packets representing a single DTMF digit press.
///
/// Returns `(packets, total_timestamp_advance)`. The caller should send each
/// packet at 20ms intervals.
///
/// The sequence: ~`duration_ms / 20` packets with E=0, then 3 end packets with E=1.
/// All share the same initial RTP timestamp (RFC 4733 §2.5.1.2).
pub fn build_dtmf_packets(
    digit: char,
    duration_ms: u32,
    seq_start: u16,
    ts_start: u32,
    ssrc: u32,
) -> Option<(Vec<RtpPacket>, u32)> {
    let event = digit_to_event(digit)?;
    let volume: u8 = 10;
    let frames = (duration_ms / 20).max(1);
    let mut pkts = Vec::new();
    let mut seq = seq_start;

    for i in 0..frames {
        let dur_ts = ((i + 1) * 160) as u16;
        let payload = dtmf_payload(event, false, volume, dur_ts);
        pkts.push(RtpPacket::new(
            PT_TELEPHONE_EVENT,
            seq,
            ts_start,
            ssrc,
            payload,
        ));
        seq = seq.wrapping_add(1);
    }

    let final_dur = (frames * 160) as u16;
    for _ in 0..3 {
        let payload = dtmf_payload(event, true, volume, final_dur);
        pkts.push(RtpPacket::new(
            PT_TELEPHONE_EVENT,
            seq,
            ts_start,
            ssrc,
            payload,
        ));
        seq = seq.wrapping_add(1);
    }

    let total_advance = frames * 160;
    Some((pkts, total_advance))
}

/// Parse a DTMF sequence string like "1,w2000,2,w500,#"
/// Returns a list of actions: Digit(char, duration_ms) or Wait(ms).
#[derive(Debug, Clone, PartialEq)]
pub enum DtmfAction {
    Digit(char, u32),
    Wait(u32),
}

pub fn parse_dtmf_sequence(seq: &str) -> Vec<DtmfAction> {
    let mut actions = Vec::new();
    for part in seq.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(ms_str) = part.strip_prefix('w').or_else(|| part.strip_prefix('W')) {
            if let Ok(ms) = ms_str.parse::<u32>() {
                actions.push(DtmfAction::Wait(ms));
                continue;
            }
        }
        if let Some(ch) = part.chars().next() {
            if digit_to_event(ch).is_some() {
                actions.push(DtmfAction::Digit(ch, 160));
            }
        }
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_mapping() {
        assert_eq!(digit_to_event('0'), Some(0));
        assert_eq!(digit_to_event('9'), Some(9));
        assert_eq!(digit_to_event('*'), Some(10));
        assert_eq!(digit_to_event('#'), Some(11));
        assert_eq!(digit_to_event('A'), Some(12));
        assert_eq!(digit_to_event('x'), None);
    }

    #[test]
    fn dtmf_packet_sequence() {
        let (pkts, advance) = build_dtmf_packets('1', 100, 1000, 5000, 42).unwrap();
        assert_eq!(pkts.len(), 8); // 5 normal + 3 end
        assert_eq!(advance, 800); // 5 * 160

        for pkt in &pkts {
            assert_eq!(pkt.payload_type, PT_TELEPHONE_EVENT);
            assert_eq!(pkt.timestamp, 5000);
            assert_eq!(pkt.ssrc, 42);
            assert_eq!(pkt.payload.len(), 4);
            assert_eq!(pkt.payload[0], 1); // event=1
        }

        // last 3 have E bit set
        for pkt in &pkts[5..] {
            assert_eq!(pkt.payload[1] & 0x80, 0x80);
        }
        // first 5 don't
        for pkt in &pkts[..5] {
            assert_eq!(pkt.payload[1] & 0x80, 0x00);
        }
    }

    #[test]
    fn parse_sequence() {
        let actions = parse_dtmf_sequence("1,w2000,2,w500,#");
        assert_eq!(
            actions,
            vec![
                DtmfAction::Digit('1', 160),
                DtmfAction::Wait(2000),
                DtmfAction::Digit('2', 160),
                DtmfAction::Wait(500),
                DtmfAction::Digit('#', 160),
            ]
        );
    }

    #[test]
    fn parse_empty_sequence() {
        assert!(parse_dtmf_sequence("").is_empty());
    }

    #[test]
    fn payload_end_bit() {
        let p = dtmf_payload(5, true, 10, 800);
        assert_eq!(p[0], 5);
        assert_eq!(p[1] & 0x80, 0x80);
        assert_eq!(p[1] & 0x3F, 10);
        assert_eq!(u16::from_be_bytes([p[2], p[3]]), 800);
    }
}
