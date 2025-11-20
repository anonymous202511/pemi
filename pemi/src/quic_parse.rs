/* QUIC packet parsing. */
use crate::common::Error;

use log::trace;

const FORM_BIT: u8 = 0x80;
const FIXED_BIT: u8 = 0x40;
const SPIN_BIT: u8 = 0x20;

const TYPE_MASK: u8 = 0x30;
pub const MAX_CID_LEN: u8 = 20;

/// Supported QUIC versions.
const PROTOCOL_VERSION_V1: u32 = 0x0000_0001;

#[inline]
pub fn version_is_supported(version: u32) -> bool {
    matches!(version, PROTOCOL_VERSION_V1)
}

/// QUIC packet type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// Initial packet.
    Initial,

    /// Retry packet.
    Retry,

    /// Handshake packet.
    Handshake,

    /// 0-RTT packet.
    ZeroRTT,

    /// Version negotiation packet.
    VersionNegotiation,

    /// 1-RTT short header packet.
    Short,
}

/// A QUIC connection ID.
pub struct ConnectionId<'a>(ConnectionIdInner<'a>);

enum ConnectionIdInner<'a> {
    Vec(Vec<u8>),
    Ref(&'a [u8]),
}

impl<'a> ConnectionId<'a> {
    /// Creates a new connection ID from the given vector.
    #[inline]
    pub const fn from_vec(cid: Vec<u8>) -> Self {
        Self(ConnectionIdInner::Vec(cid))
    }

    /// Creates a new connection ID from the given slice.
    #[inline]
    pub const fn from_ref(cid: &'a [u8]) -> Self {
        Self(ConnectionIdInner::Ref(cid))
    }

    /// Returns a new owning connection ID from the given existing one.
    #[inline]
    pub fn into_owned(self) -> ConnectionId<'static> {
        ConnectionId::from_vec(self.into())
    }
}

impl<'a> Default for ConnectionId<'a> {
    #[inline]
    fn default() -> Self {
        Self::from_vec(Vec::new())
    }
}

impl<'a> From<Vec<u8>> for ConnectionId<'a> {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        Self::from_vec(v)
    }
}

impl<'a> From<ConnectionId<'a>> for Vec<u8> {
    #[inline]
    fn from(id: ConnectionId<'a>) -> Self {
        match id.0 {
            ConnectionIdInner::Vec(cid) => cid,
            ConnectionIdInner::Ref(cid) => cid.to_vec(),
        }
    }
}

impl<'a> PartialEq for ConnectionId<'a> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl<'a> Eq for ConnectionId<'a> {}

impl<'a> AsRef<[u8]> for ConnectionId<'a> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        match &self.0 {
            ConnectionIdInner::Vec(v) => v.as_ref(),
            ConnectionIdInner::Ref(v) => v,
        }
    }
}

impl<'a> std::hash::Hash for ConnectionId<'a> {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state);
    }
}

impl<'a> std::ops::Deref for ConnectionId<'a> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        match &self.0 {
            ConnectionIdInner::Vec(v) => v.as_ref(),
            ConnectionIdInner::Ref(v) => v,
        }
    }
}

impl<'a> Clone for ConnectionId<'a> {
    #[inline]
    fn clone(&self) -> Self {
        Self::from_vec(self.as_ref().to_vec())
    }
}

impl<'a> std::fmt::Debug for ConnectionId<'a> {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        for c in self.as_ref() {
            write!(f, "{c:02x}")?;
        }

        Ok(())
    }
}

/// A QUIC packet's header.
#[derive(PartialEq, Eq)]
pub struct Header<'a> {
    /// The type of the packet.
    pub ty: Type,

    /// The spin bit of the packet. Only present in `Short` packets.
    pub spin: bool,

    /// The version of the packet.
    pub version: u32,

    /// The destination connection ID of the packet.
    pub dcid: ConnectionId<'a>,

    /// The source connection ID of the packet.
    pub scid: ConnectionId<'a>,

    /// The length of the payload.(payload + packet number)
    pub length: usize,
}

impl<'a> Header<'a> {
    /// In some QUIC implementations, the packets in handshake phase may have padding outside the QUIC packets.
    /// see https://github.com/quicwg/base-drafts/issues/3333
    pub fn is_udp_padding(b: &mut octets::Octets) -> Result<bool, Error> {
        let first = b.peek_u8()?;
        Ok(first == 0)
    }

    /// Parses a QUIC packet header from the given buffer.
    ///
    /// The `dcid_len` parameter is the length of the destination connection ID,
    /// required to parse short header packets.
    #[inline]
    pub fn from_slice(buf: &[u8], dcid_len: usize) -> Result<Header<'a>, Error> {
        let mut b = octets::Octets::with_slice(buf);
        Header::from_bytes(&mut b, dcid_len)
    }

    pub fn from_bytes(b: &mut octets::Octets, dcid_len: usize) -> Result<Header<'a>, Error> {
        let first = b.get_u8()?;

        // decode fixed bit and spin bit
        if !Header::fixed_bit(first) {
            trace!("Fix bit==0, not QUIC or grease_quic_bit transport parameter set");
        }

        if !Header::is_long(first) {
            // Decode short header.

            let spin_bit = Header::spin_state(first);

            // Decode dcid
            if dcid_len == 0 {
                // Encounter short header without dcid length
                // Connection is in invalid state
                return Err(Error::InvalidState);
            }
            let dcid = b.get_bytes(dcid_len)?;

            return Ok(Header {
                ty: Type::Short,
                spin: spin_bit,
                version: 0,
                dcid: dcid.to_vec().into(),
                scid: ConnectionId::default(),
                length: b.cap(), // A packet with a short header does not include a length, so it can only be the last packet included in a UDP datagram.
            });
        }

        // Decode long header.
        let version = b.get_u32()?;

        let ty = if version == 0 {
            Type::VersionNegotiation
        } else {
            match (first & TYPE_MASK) >> 4 {
                0x00 => Type::Initial,
                0x01 => Type::ZeroRTT,
                0x02 => Type::Handshake,
                0x03 => Type::Retry,
                _ => unreachable!(),
            }
        };

        let dcid_len = b.get_u8()?;
        if version_is_supported(version) && dcid_len > MAX_CID_LEN {
            panic!("dcid_len > MAX_CID_LEN");
        }
        let dcid = b.get_bytes(dcid_len as usize)?.to_vec();

        let scid_len = b.get_u8()?;
        if version_is_supported(version) && scid_len > MAX_CID_LEN {
            panic!("scid_len > MAX_CID_LEN");
        }
        let scid = b.get_bytes(scid_len as usize)?.to_vec();

        // parse the length
        // Initial, Handshake, and 0-RTT packets have a length field.
        // Retry and Version Negotiation packets do not have a length field. But MUST be the last packet in the UDP datagram.
        let length: usize = match ty {
            Type::Initial => {
                _ = Some(b.get_bytes_with_varint_length()?.to_vec()); // token. Not used but need to consume
                b.get_varint()? as usize
            }
            Type::Handshake => b.get_varint()? as usize,
            Type::ZeroRTT => b.get_varint()? as usize,
            Type::Retry => b.cap(),
            Type::VersionNegotiation => b.cap(),
            Type::Short => unreachable!(),
        };

        Ok(Header {
            ty,
            spin: false,
            version,
            dcid: dcid.into(),
            scid: scid.into(),
            length,
        })
    }

    /// Returns true if the packet has a long header.
    ///
    /// The `b` parameter represents the first byte of the QUIC header.
    fn is_long(b: u8) -> bool {
        b & FORM_BIT != 0
    }

    /// Returns true if the packet has a fixed bit set as 1.
    ///
    /// The `b` parameter represents the first byte of the QUIC header.
    fn fixed_bit(b: u8) -> bool {
        b & FIXED_BIT != 0
    }

    /// Returns true if the spin bit is 1. Should be called only for short header.
    ///
    /// The `b` parameter represents the first byte of the QUIC header.
    fn spin_state(b: u8) -> bool {
        b & SPIN_BIT != 0
    }
}

impl<'a> std::fmt::Debug for Header<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{:?}", self.ty)?;

        if self.ty != Type::Short {
            write!(f, " version={:x}", self.version)?;
        }

        write!(f, " dcid={:?}", self.dcid)?;

        if self.ty != Type::Short {
            write!(f, " scid={:?}", self.scid)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex;

    #[test]
    fn initial() {
        // from a pcap and wireshark decode result
        // UDP payload:
        let pkt = "c40000000110f44df81582d3b6f067b182f6b3c5caa8141ab213fc50df36f8791d09d293df6e43b41f72be004113cf596b00603ff64b70db409bf89fa57050c6462a223003c9d49492e62b86ddf32ed05d1e85903725d1f7827c562dfad04ca2229190d970c235907a9363d7f15e026ffaa1180efe89347fbb8cc6ffdd188517f98b22016805d0104de5b6f1e20ebc7b64e5cf3a88fff831fb0a4b8daab1e721ed1bfc16f5fcfa42eb8e9c596b107b7386052a8b070506133a9f7bed479d960345992620355aa2adea1e9f355cd8d8018ec3406ad7976b94f4f837b13f67e19e65709e4afdf0a8db954c29154870d24d31ad75391d752d1650a63a6909edcf8fae1a11f86ad22b6d1ac9f10eea107c445e7a6d45bdc4d092aecd37b46d919718f5180846b93e401a72ec4155462a64340ba7bc26b923fae55ba2f13462dd70d5b8000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        // wireshark decode:
        // QUIC IETF
        // QUIC Connection information
        //     [Connection Number: 0]
        // [Packet Length: 321]
        // 1... .... = Header Form: Long Header (1)
        // .1.. .... = Fixed Bit: True
        // ..00 .... = Packet Type: Initial (0)
        // [.... 00.. = Reserved: 0]
        // [.... ..00 = Packet Number Length: 1 bytes (0)]
        // Version: 1 (0x00000001)
        // Destination Connection ID Length: 16
        // Destination Connection ID: f44df81582d3b6f067b182f6b3c5caa8
        // Source Connection ID Length: 20
        // Source Connection ID: 1ab213fc50df36f8791d09d293df6e43b41f72be
        // Token Length: 0
        // Length: 275

        let bytes = hex::decode(pkt).unwrap();
        let mut b = octets::Octets::with_slice(&bytes);
        let hdr = Header::from_bytes(&mut b, 0).unwrap();
        assert_eq!(hdr.ty, Type::Initial);
        assert_eq!(hdr.spin, false);
        assert_eq!(hdr.version, 1);
        let dcid = hex::decode("f44df81582d3b6f067b182f6b3c5caa8").unwrap();
        assert_eq!(hdr.dcid.as_ref(), dcid.as_slice());
        let scid = hex::decode("1ab213fc50df36f8791d09d293df6e43b41f72be").unwrap();
        assert_eq!(hdr.scid.as_ref(), scid.as_slice());
    }

    #[test]
    fn handshake() {
        // from a pcap and wireshark decode result
        // UDP payload:
        let pkt = "ee00000001141ab213fc50df36f8791d09d293df6e43b41f72be14a0e5ef94e277a0e9f0cfbf1e16ae5dd6ecf6913d410687bf40e2c344eb8f308f336523565793a585601768fb119011dc31cd441f4b0a1a418f5af1f8d24eb864d171c1a19a60a89a0c4975f9c44abf2daf45314f0b56f59670b09ed6f4ada6db70410f0baf490bd19d08e1e147e9526c4beaeea7cc75f93425ac5e1c86456b0ecaaa445b40df791590ba15fcef7376b8ee61a4bb202c9efc319190a1e816b6b743d764d9f069e43c65706743faed9c547232e16c45284c18186443f43ce11930595c4ec5a0475c83d3cd1dab3768bf3428e6683a6446c44b0e5c02424acb3cc879f5a24ef7564c3b675b77d5a50bfd3e031b924829a8fd777f1a0a4b5768fb49cc745d96c925c451e4c0d3fa56aed51e2142163ec787d093c22ede9c";
        // wireshark decode:
        // QUIC IETF
        // QUIC Connection information
        //     [Connection Number: 0]
        // [Packet Length: 311]
        // 1... .... = Header Form: Long Header (1)
        // .1.. .... = Fixed Bit: True
        // ..10 .... = Packet Type: Handshake (2)
        // Version: 1 (0x00000001)
        // Destination Connection ID Length: 20
        // Destination Connection ID: 1ab213fc50df36f8791d09d293df6e43b41f72be
        // Source Connection ID Length: 20
        // Source Connection ID: a0e5ef94e277a0e9f0cfbf1e16ae5dd6ecf6913d
        // Length: 262

        let bytes = hex::decode(pkt).unwrap();
        let mut b = octets::Octets::with_slice(&bytes);
        let hdr = Header::from_bytes(&mut b, 0).unwrap();
        assert_eq!(hdr.ty, Type::Handshake);
        assert_eq!(hdr.spin, false);
        assert_eq!(hdr.version, 1);
        let dcid = hex::decode("1ab213fc50df36f8791d09d293df6e43b41f72be").unwrap();
        assert_eq!(hdr.dcid.as_ref(), dcid.as_slice());
        let scid = hex::decode("a0e5ef94e277a0e9f0cfbf1e16ae5dd6ecf6913d").unwrap();
        assert_eq!(hdr.scid.as_ref(), scid.as_slice());
    }
}
