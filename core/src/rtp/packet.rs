//! RTP 封包（RFC 3550）
//! 僅實作壓測所需的最小欄位集合

/// RTP 固定 header 大小（bytes）
pub const RTP_HEADER_SIZE: usize = 12;

/// G.711 PCMU payload type
pub const PT_PCMU: u8 = 0;
/// G.711 PCMA payload type
pub const PT_PCMA: u8 = 8;

#[derive(Debug, Clone)]
pub struct RtpPacket {
    /// Payload Type (7 bits)
    pub payload_type:    u8,
    /// 序號（每包 +1，用於掉包偵測）
    pub sequence:        u16,
    /// 時間戳記（G.711 = 8000Hz，每 20ms 增加 160）
    pub timestamp:       u32,
    /// SSRC 同步源識別碼
    pub ssrc:            u32,
    /// 音訊 payload（G.711 PCM 樣本）
    pub payload:         Vec<u8>,
    /// 接收時間（用於 jitter 計算，非封包內欄位）
    pub received_at_us:  Option<u64>,
}

impl RtpPacket {
    /// 建構一個傳送用的 RTP 封包
    pub fn new(
        payload_type: u8,
        sequence:     u16,
        timestamp:    u32,
        ssrc:         u32,
        payload:      Vec<u8>,
    ) -> Self {
        Self {
            payload_type,
            sequence,
            timestamp,
            ssrc,
            payload,
            received_at_us: None,
        }
    }

    /// 序列化為 bytes（固定 header + payload）
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(RTP_HEADER_SIZE + self.payload.len());

        // Byte 0: V=2, P=0, X=0, CC=0
        buf.push(0b1000_0000u8);
        // Byte 1: M=0, PT
        buf.push(self.payload_type & 0x7F);
        // Byte 2-3: Sequence Number (big-endian)
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        // Byte 4-7: Timestamp (big-endian)
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        // Byte 8-11: SSRC (big-endian)
        buf.extend_from_slice(&self.ssrc.to_be_bytes());
        // Payload
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// 從 bytes 解析（用於接收端 jitter/loss 分析）
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < RTP_HEADER_SIZE {
            return None;
        }
        // 檢查 version = 2
        if (data[0] >> 6) != 2 {
            return None;
        }
        let payload_type = data[1] & 0x7F;
        let sequence     = u16::from_be_bytes([data[2], data[3]]);
        let timestamp    = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ssrc         = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        let payload      = data[RTP_HEADER_SIZE..].to_vec();

        Some(Self {
            payload_type,
            sequence,
            timestamp,
            ssrc,
            payload,
            received_at_us: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let pkt = RtpPacket::new(PT_PCMU, 42, 1000, 0xDEADBEEF, vec![0u8; 160]);
        let bytes  = pkt.encode();
        assert_eq!(bytes.len(), RTP_HEADER_SIZE + 160);

        let decoded = RtpPacket::decode(&bytes).unwrap();
        assert_eq!(decoded.sequence,     42);
        assert_eq!(decoded.timestamp,    1000);
        assert_eq!(decoded.ssrc,         0xDEADBEEF);
        assert_eq!(decoded.payload.len(), 160);
    }
}
