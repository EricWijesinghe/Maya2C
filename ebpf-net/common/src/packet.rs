//! Ethernet, IPv4/IPv6 and UDP, as far as the relay needs them.
//!
//! User space reads AF_XDP frames with [`parse`]; tests and benchmarks build
//! frames with [`encode_ipv4_udp`]. The XDP program applies the same rules with
//! pointer arithmetic, because the verifier tracks packet bounds through
//! pointers and not through slice lengths; `ebpf-net/tests/xdp_veth.rs` is what
//! holds the two to the same behaviour.
//!
//! The rules are deliberately narrow. Anything that is not *certainly* a UDP
//! datagram with a readable destination port is [`Parsed::Other`], and the
//! kernel passes it untouched: the accelerator judges relay traffic, and must
//! never black-hole a node's SSH session because an IP header looked odd.

/// Ethernet header length, without VLAN tags.
pub const ETH_HEADER_LEN: usize = 14;
/// EtherType for IPv4.
pub const ETHERTYPE_IPV4: u16 = 0x0800;
/// EtherType for IPv6.
pub const ETHERTYPE_IPV6: u16 = 0x86DD;
/// IP protocol number for UDP.
pub const IPPROTO_UDP: u8 = 17;
/// IPv4 header length without options.
pub const IPV4_MIN_HEADER_LEN: usize = 20;
/// IPv6 fixed header length.
pub const IPV6_HEADER_LEN: usize = 40;
/// UDP header length.
pub const UDP_HEADER_LEN: usize = 8;

const IPV4_MORE_FRAGMENTS: u16 = 0x2000;
const IPV4_FRAGMENT_OFFSET: u16 = 0x1fff;
const DEFAULT_TTL: u8 = 64;

/// A datagram's source address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceAddr {
    /// IPv4, as on the wire.
    V4([u8; 4]),
    /// IPv6, as on the wire.
    V6([u8; 16]),
}

/// A UDP datagram inside a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UdpView<'a> {
    /// Source address.
    pub source: SourceAddr,
    /// Source port.
    pub source_port: u16,
    /// Destination port.
    pub destination_port: u16,
    /// The UDP payload, bounded by the UDP length field.
    pub payload: &'a [u8],
}

/// What a frame turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsed<'a> {
    /// Not a UDP datagram this parser can read. Passed by the kernel.
    Other,
    /// The first fragment of a fragmented UDP datagram. Relay datagrams are
    /// sized never to fragment, so one addressed to the relay port is dropped.
    Fragment {
        /// Destination port from the fragment's UDP header.
        destination_port: u16,
    },
    /// A UDP header whose length field disagrees with the packet.
    MalformedUdp {
        /// Destination port from the UDP header.
        destination_port: u16,
    },
    /// A well-formed UDP datagram.
    Udp(UdpView<'a>),
}

impl Parsed<'_> {
    /// The UDP destination port, if one could be read.
    #[must_use]
    pub const fn destination_port(&self) -> Option<u16> {
        match self {
            Self::Other => None,
            Self::Fragment { destination_port } | Self::MalformedUdp { destination_port } => {
                Some(*destination_port)
            }
            Self::Udp(view) => Some(view.destination_port),
        }
    }
}

/// Parses an Ethernet frame.
#[must_use]
pub fn parse(frame: &[u8]) -> Parsed<'_> {
    let Some((eth, rest)) = frame.split_first_chunk::<ETH_HEADER_LEN>() else {
        return Parsed::Other;
    };
    match u16::from_be_bytes([eth[12], eth[13]]) {
        ETHERTYPE_IPV4 => parse_ipv4(rest),
        ETHERTYPE_IPV6 => parse_ipv6(rest),
        _ => Parsed::Other,
    }
}

fn parse_ipv4(packet: &[u8]) -> Parsed<'_> {
    let Some(fixed) = packet.first_chunk::<IPV4_MIN_HEADER_LEN>() else {
        return Parsed::Other;
    };
    if fixed[0] >> 4 != 4 || fixed[9] != IPPROTO_UDP {
        return Parsed::Other;
    }
    let header_len = usize::from(fixed[0] & 0x0f) * 4;
    let total_len = usize::from(u16::from_be_bytes([fixed[2], fixed[3]]));
    let flags = u16::from_be_bytes([fixed[6], fixed[7]]);
    if header_len < IPV4_MIN_HEADER_LEN || flags & IPV4_FRAGMENT_OFFSET != 0 {
        // A later fragment carries no UDP header, so no port to judge by.
        return Parsed::Other;
    }
    let Some(segment) = packet.get(header_len..total_len) else {
        return Parsed::Other;
    };
    let source = SourceAddr::V4([fixed[12], fixed[13], fixed[14], fixed[15]]);
    parse_udp(segment, source, flags & IPV4_MORE_FRAGMENTS != 0)
}

fn parse_ipv6(packet: &[u8]) -> Parsed<'_> {
    let Some(fixed) = packet.first_chunk::<IPV6_HEADER_LEN>() else {
        return Parsed::Other;
    };
    // Extension headers, a fragment header among them, hide the transport
    // header behind a chain the relay never produces.
    if fixed[0] >> 4 != 6 || fixed[6] != IPPROTO_UDP {
        return Parsed::Other;
    }
    let payload_len = usize::from(u16::from_be_bytes([fixed[4], fixed[5]]));
    let Some(segment) = packet.get(IPV6_HEADER_LEN..IPV6_HEADER_LEN + payload_len) else {
        return Parsed::Other;
    };
    let mut source = [0u8; 16];
    source.copy_from_slice(&fixed[8..24]);
    parse_udp(segment, SourceAddr::V6(source), false)
}

fn parse_udp(segment: &[u8], source: SourceAddr, is_fragment: bool) -> Parsed<'_> {
    let Some(fixed) = segment.first_chunk::<UDP_HEADER_LEN>() else {
        return Parsed::Other;
    };
    let source_port = u16::from_be_bytes([fixed[0], fixed[1]]);
    let destination_port = u16::from_be_bytes([fixed[2], fixed[3]]);
    if is_fragment {
        return Parsed::Fragment { destination_port };
    }
    let udp_len = usize::from(u16::from_be_bytes([fixed[4], fixed[5]]));
    if udp_len < UDP_HEADER_LEN {
        return Parsed::MalformedUdp { destination_port };
    }
    let Some(payload) = segment.get(UDP_HEADER_LEN..udp_len) else {
        return Parsed::MalformedUdp { destination_port };
    };
    Parsed::Udp(UdpView {
        source,
        source_port,
        destination_port,
        payload,
    })
}

/// Addresses and ports for [`encode_ipv4_udp`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ipv4Endpoints {
    /// Source address.
    pub source: [u8; 4],
    /// Destination address.
    pub destination: [u8; 4],
    /// Source port.
    pub source_port: u16,
    /// Destination port.
    pub destination_port: u16,
}

/// Writes an Ethernet + IPv4 + UDP frame carrying `payload` into `out`.
///
/// Returns the frame length, or `None` if `out` is too small or the payload
/// does not fit an IPv4 datagram. The UDP checksum is zero (permitted over
/// IPv4); the IPv4 header checksum is computed.
#[must_use]
pub fn encode_ipv4_udp(endpoints: Ipv4Endpoints, payload: &[u8], out: &mut [u8]) -> Option<usize> {
    let udp_len = UDP_HEADER_LEN.checked_add(payload.len())?;
    let ip_len = IPV4_MIN_HEADER_LEN.checked_add(udp_len)?;
    let frame_len = ETH_HEADER_LEN.checked_add(ip_len)?;
    let ip_len_field = u16::try_from(ip_len).ok()?;
    let udp_len_field = u16::try_from(udp_len).ok()?;
    let frame = out.get_mut(..frame_len)?;

    let (eth, rest) = frame.split_at_mut(ETH_HEADER_LEN);
    eth[..12].fill(0);
    eth[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());

    let (ip, rest) = rest.split_at_mut(IPV4_MIN_HEADER_LEN);
    ip.fill(0);
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&ip_len_field.to_be_bytes());
    ip[8] = DEFAULT_TTL;
    ip[9] = IPPROTO_UDP;
    ip[12..16].copy_from_slice(&endpoints.source);
    ip[16..20].copy_from_slice(&endpoints.destination);
    let checksum = ipv4_checksum(ip);
    ip[10..12].copy_from_slice(&checksum.to_be_bytes());

    let (udp, body) = rest.split_at_mut(UDP_HEADER_LEN);
    udp[0..2].copy_from_slice(&endpoints.source_port.to_be_bytes());
    udp[2..4].copy_from_slice(&endpoints.destination_port.to_be_bytes());
    udp[4..6].copy_from_slice(&udp_len_field.to_be_bytes());
    udp[6..8].fill(0);
    body.copy_from_slice(payload);
    Some(frame_len)
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = header
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    #[allow(clippy::cast_possible_truncation)] // folded into 16 bits above.
    let folded = sum as u16;
    !folded
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENDPOINTS: Ipv4Endpoints = Ipv4Endpoints {
        source: [10, 0, 0, 1],
        destination: [10, 0, 0, 2],
        source_port: 4000,
        destination_port: 30_334,
    };

    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0u8; 2048];
        let len = encode_ipv4_udp(ENDPOINTS, payload, &mut out).unwrap();
        out.truncate(len);
        out
    }

    #[test]
    fn an_encoded_ipv4_udp_frame_parses_back() {
        let bytes = frame(b"hello relay");
        let Parsed::Udp(view) = parse(&bytes) else {
            panic!("expected a datagram")
        };
        assert_eq!(view.source, SourceAddr::V4([10, 0, 0, 1]));
        assert_eq!(view.source_port, 4000);
        assert_eq!(view.destination_port, 30_334);
        assert_eq!(view.payload, b"hello relay");
        assert_eq!(ipv4_checksum(&bytes[14..34]), 0, "a valid header sums to zero");
    }

    #[test]
    fn trailing_ethernet_padding_is_not_payload() {
        let mut bytes = frame(b"x");
        bytes.extend_from_slice(&[0; 20]);
        let Parsed::Udp(view) = parse(&bytes) else {
            panic!("expected a datagram")
        };
        assert_eq!(view.payload, b"x");
    }

    #[test]
    fn a_udp_length_beyond_the_packet_is_malformed_and_keeps_its_port() {
        let mut bytes = frame(b"abc");
        bytes[38..40].copy_from_slice(&500u16.to_be_bytes());
        assert_eq!(
            parse(&bytes),
            Parsed::MalformedUdp {
                destination_port: 30_334
            }
        );
        bytes[38..40].copy_from_slice(&3u16.to_be_bytes());
        assert!(matches!(parse(&bytes), Parsed::MalformedUdp { .. }));
    }

    #[test]
    fn fragments_are_recognised_and_later_fragments_are_not_judged() {
        let mut first = frame(b"abc");
        first[20] = 0x20; // more fragments
        assert_eq!(
            parse(&first),
            Parsed::Fragment {
                destination_port: 30_334
            }
        );
        let mut later = frame(b"abc");
        later[21] = 0x08; // offset 8
        assert_eq!(parse(&later), Parsed::Other);
    }

    #[test]
    fn anything_unreadable_is_other_and_never_malformed() {
        assert_eq!(parse(&[]), Parsed::Other);
        let good = frame(b"abc");
        let mut arp = good.clone();
        arp[12..14].copy_from_slice(&0x0806u16.to_be_bytes());
        assert_eq!(parse(&arp), Parsed::Other);
        let mut tcp = good.clone();
        tcp[23] = 6;
        assert_eq!(parse(&tcp), Parsed::Other);
        let mut short_ihl = good.clone();
        short_ihl[14] = 0x44;
        assert_eq!(parse(&short_ihl), Parsed::Other);
        let mut long_total = good.clone();
        long_total[16..18].copy_from_slice(&9_000u16.to_be_bytes());
        assert_eq!(parse(&long_total), Parsed::Other);
        assert_eq!(parse(&good[..30]), Parsed::Other);
    }

    #[test]
    fn ipv6_udp_parses_and_extension_headers_are_other() {
        let payload = b"six";
        let mut bytes = vec![0u8; ETH_HEADER_LEN + IPV6_HEADER_LEN + UDP_HEADER_LEN + 3];
        bytes[12..14].copy_from_slice(&ETHERTYPE_IPV6.to_be_bytes());
        bytes[14] = 0x60;
        bytes[18..20].copy_from_slice(&11u16.to_be_bytes());
        bytes[20] = IPPROTO_UDP;
        bytes[22..38].copy_from_slice(&[0xfe; 16]);
        let udp = ETH_HEADER_LEN + IPV6_HEADER_LEN;
        bytes[udp + 2..udp + 4].copy_from_slice(&30_334u16.to_be_bytes());
        bytes[udp + 4..udp + 6].copy_from_slice(&11u16.to_be_bytes());
        bytes[udp + 8..].copy_from_slice(payload);
        let Parsed::Udp(view) = parse(&bytes) else {
            panic!("expected a datagram")
        };
        assert_eq!(view.source, SourceAddr::V6([0xfe; 16]));
        assert_eq!(view.payload, payload);

        bytes[20] = 44; // fragment extension header
        assert_eq!(parse(&bytes), Parsed::Other);
    }

    #[test]
    fn encoding_refuses_a_buffer_too_small() {
        let mut tiny = [0u8; 20];
        assert_eq!(encode_ipv4_udp(ENDPOINTS, b"abc", &mut tiny), None);
    }
}
