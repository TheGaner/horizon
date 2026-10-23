//! Experimental `friendq:` invitations: Iroh endpoint identity and a 256-bit
//! authorization capability. This format is deliberately distinct from legacy
//! `friend:` SPAKE2/TCP invites. It does not enable QUIC in the game client.
//!
//! V3 byte layout: magic `HIFQ` | version 3 | endpoint ID (32) |
//! capability (32) | direct endpoint count (u8) | (tag 4/6, IP, port u16)* |
//! relay URL length (u8, zero for none) | UTF-8 URL | SHA256 prefix (8).
//! A checksum detects typos; it is NOT authentication or a signature.

use crate::{code::ALPHABET, invite::CodeError, iroh_transport::Ticket};
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

pub const PREFIX: &str = "friendq:";
const MAGIC: &[u8; 4] = b"HIFQ";
const VERSION: u8 = 3;
const CHECKSUM: usize = 8;
const MAX_ENDPOINTS: usize = 8;
const MAX_BINARY: usize = 512;
const MAX_INPUT: usize = 1024;
const GROUP: usize = 5;

fn pack(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len() * 8 / 5 + 10);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            result.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
        }
    }
    if bits != 0 {
        result.push(char::from(ALPHABET[((acc << (5 - bits)) & 31) as usize]));
    }
    let grouped = result
        .as_bytes()
        .chunks(GROUP)
        .map(|chunk| std::str::from_utf8(chunk).expect("base32 is ASCII"))
        .collect::<Vec<_>>()
        .join("-");
    format!("{PREFIX}{grouped}")
}

fn unpack(text: &str) -> Result<Vec<u8>, CodeError> {
    let encoded = text.strip_prefix(PREFIX).ok_or(CodeError::Version)?;
    if encoded.len() > MAX_INPUT || encoded.is_empty() {
        return Err(CodeError::Characters);
    }
    let mut out = Vec::with_capacity(encoded.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in encoded.bytes() {
        if c == b'-' {
            continue;
        }
        let upper = c.to_ascii_uppercase();
        let upper = match upper {
            b'O' => b'0',
            b'I' | b'L' => b'1',
            x => x,
        };
        let v = ALPHABET
            .iter()
            .position(|&s| s == upper)
            .ok_or(CodeError::Characters)? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            if out.len() > MAX_BINARY {
                return Err(CodeError::Characters);
            }
        }
        acc &= (1 << bits) - 1;
    }
    if bits != 0 && acc != 0 {
        return Err(CodeError::Typo);
    }
    Ok(out)
}

fn checksum(bytes: &[u8]) -> [u8; CHECKSUM] {
    let digest = Sha256::digest(bytes);
    let mut result = [0u8; CHECKSUM];
    result.copy_from_slice(&digest[..CHECKSUM]);
    result
}

/// Encode an experimental QUIC ticket. Secret-bearing output must never be
/// written to a log or telemetry. Only IP and relay transport kinds are
/// supported; custom Iroh transports are rejected instead of silently lost.
pub fn encode(ticket: &Ticket) -> Result<String, CodeError> {
    let mut bytes = Vec::with_capacity(128);
    bytes.extend_from_slice(MAGIC);
    bytes.push(VERSION);
    bytes.extend_from_slice(ticket.endpoint.id.as_bytes());
    bytes.extend_from_slice(ticket.capability());
    let ips: Vec<_> = ticket.endpoint.ip_addrs().copied().collect();
    if ips.len() > MAX_ENDPOINTS {
        return Err(CodeError::Characters);
    }
    if ticket
        .endpoint
        .addrs
        .iter()
        .any(|a| matches!(a, TransportAddr::Custom(_)))
    {
        return Err(CodeError::Version);
    }
    bytes.push(ips.len() as u8);
    for ip in &ips {
        if ip.port() == 0 {
            return Err(CodeError::Characters);
        }
        match ip.ip() {
            IpAddr::V4(v4) => {
                bytes.push(4);
                bytes.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                bytes.push(6);
                bytes.extend_from_slice(&v6.octets());
            }
        }
        bytes.extend_from_slice(&ip.port().to_be_bytes());
    }
    let urls: Vec<_> = ticket.endpoint.relay_urls().collect();
    if ips.is_empty() && urls.is_empty() {
        return Err(CodeError::Characters);
    }
    if urls.len() > 1 {
        return Err(CodeError::Version);
    }
    if let Some(url) = urls.first() {
        let addr = url.to_string();
        if addr.len() > u8::MAX as usize {
            return Err(CodeError::Characters);
        }
        bytes.push(addr.len() as u8);
        bytes.extend_from_slice(addr.as_bytes());
    } else {
        bytes.push(0);
    }
    if bytes.len() + CHECKSUM > MAX_BINARY {
        return Err(CodeError::Characters);
    }
    bytes.extend_from_slice(&checksum(&bytes));
    Ok(pack(&bytes))
}

/// Decode a V3 QUIC capability without making network connections. The
/// `friend:` v2 decoder is neither invoked nor modified by this function.
pub fn decode(text: &str) -> Result<Ticket, CodeError> {
    if text.len() > MAX_INPUT + PREFIX.len() {
        return Err(CodeError::Characters);
    }
    let bytes = unpack(text)?;
    if bytes.len() < 4 + 1 + 32 + 32 + 1 + 1 + CHECKSUM {
        return Err(CodeError::Characters);
    }
    let (body, given) = bytes.split_at(bytes.len() - CHECKSUM);
    if checksum(body).as_slice() != given {
        return Err(CodeError::Typo);
    }
    if body.get(..4) != Some(MAGIC.as_slice()) || body[4] != VERSION {
        return Err(CodeError::Version);
    }
    let mut pos = 5usize;
    let mut take = |count: usize| -> Result<&[u8], CodeError> {
        let end = pos.checked_add(count).ok_or(CodeError::Characters)?;
        let item = body.get(pos..end).ok_or(CodeError::Characters)?;
        pos = end;
        Ok(item)
    };
    let key: [u8; 32] = take(32)?.try_into().map_err(|_| CodeError::Characters)?;
    let id = EndpointId::from_bytes(&key).map_err(|_| CodeError::Version)?;
    let secret: [u8; 32] = take(32)?.try_into().map_err(|_| CodeError::Characters)?;
    let count = usize::from(take(1)?[0]);
    if count > MAX_ENDPOINTS {
        return Err(CodeError::Characters);
    }
    let mut addrs = BTreeSet::new();
    for _ in 0..count {
        let ip = match take(1)?[0] {
            4 => IpAddr::V4(Ipv4Addr::from(
                <[u8; 4]>::try_from(take(4)?).map_err(|_| CodeError::Characters)?,
            )),
            6 => IpAddr::V6(Ipv6Addr::from(
                <[u8; 16]>::try_from(take(16)?).map_err(|_| CodeError::Characters)?,
            )),
            _ => return Err(CodeError::Version),
        };
        let port = u16::from_be_bytes(take(2)?.try_into().map_err(|_| CodeError::Characters)?);
        if port == 0 || !addrs.insert(TransportAddr::Ip(SocketAddr::new(ip, port))) {
            return Err(CodeError::Characters);
        }
    }
    let length = usize::from(take(1)?[0]);
    if length != 0 {
        let url = std::str::from_utf8(take(length)?).map_err(|_| CodeError::Characters)?;
        let parsed = url.parse().map_err(|_| CodeError::Version)?;
        if !addrs.insert(TransportAddr::Relay(parsed)) {
            return Err(CodeError::Characters);
        }
    }
    if pos != body.len() {
        return Err(CodeError::Characters);
    }
    let endpoint = EndpointAddr::from_parts(id, addrs);
    // Re-encode to reject alternate binary endpoint ordering, noncanonical
    // relay URL text and redundant base32 padding digits. Case/lookalike
    // symbols and optional separators are still accepted for pasting.
    let ticket = Ticket::new(endpoint, secret);
    let canonical = encode(&ticket)?;
    let supplied_symbols = text[PREFIX.len()..].bytes().filter(|&c| c != b'-').count();
    let encoded_symbols = canonical[PREFIX.len()..]
        .bytes()
        .filter(|&c| c != b'-')
        .count();
    if supplied_symbols != encoded_symbols || unpack(&canonical)? != bytes {
        return Err(CodeError::Characters);
    }
    Ok(ticket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invite::{CodeError, parse_detailed};
    use iroh::SecretKey;
    fn sample() -> Ticket {
        let id = SecretKey::from_bytes(&[17; 32]).public();
        let endpoint = EndpointAddr::new(id)
            .with_ip_addr("127.0.0.1:34567".parse().unwrap())
            .with_ip_addr("[::1]:56789".parse().unwrap());
        Ticket::new(endpoint, [37u8; 32])
    }
    fn rechecksum(mut b: Vec<u8>) -> String {
        b.truncate(b.len() - CHECKSUM);
        b.extend_from_slice(&checksum(&b));
        pack(&b)
    }
    #[test]
    fn round_trip_and_existing_v2_is_unaffected() {
        let text = encode(&sample()).unwrap();
        assert!(text.starts_with(PREFIX));
        let decoded = decode(&text).unwrap();
        assert_eq!(decoded.endpoint, sample().endpoint);
        assert_eq!(decoded.capability(), &[37; 32]);
        assert_eq!(parse_detailed(&text), Err(CodeError::Characters));
    }
    #[test]
    fn relay_url_round_trip_and_reject_secret_debug_output() {
        let mut ticket = sample();
        ticket.endpoint = ticket
            .endpoint
            .with_relay_url("https://relay.example.com".parse().unwrap());
        let decoded = decode(&encode(&ticket).unwrap()).unwrap();
        assert_eq!(decoded.endpoint, ticket.endpoint);
        assert!(!format!("{:?}", decoded.endpoint).contains("37, 37"));
    }
    #[test]
    fn bad_checksum_length_version_and_tag_are_rejected() {
        let original = encode(&sample()).unwrap();
        let mut corrupted = original.into_bytes();
        let idx = PREFIX.len() + 6;
        corrupted[idx] = if corrupted[idx] == b'Z' { b'Y' } else { b'Z' };
        assert!(matches!(
            decode(std::str::from_utf8(&corrupted).unwrap()),
            Err(CodeError::Typo)
        ));
        let b = unpack(&encode(&sample()).unwrap()).unwrap();
        let mut version = b.clone();
        version[4] = 4;
        assert!(matches!(
            decode(&rechecksum(version)),
            Err(CodeError::Version)
        ));
        let mut tag = b.clone();
        tag[70] = 99;
        assert!(matches!(decode(&rechecksum(tag)), Err(CodeError::Version)));
        let mut count = b.clone();
        count[69] = 9;
        assert!(matches!(
            decode(&rechecksum(count)),
            Err(CodeError::Characters)
        ));
        assert!(decode(&format!("{PREFIX}{}", "A".repeat(1025))).is_err());
        assert!(decode("friend:ABC").is_err());
    }
    #[test]
    fn duplicate_addresses_zero_ports_and_trailing_bytes_rejected() {
        let b = unpack(&encode(&sample()).unwrap()).unwrap();
        let mut zero = b.clone();
        zero[75] = 0;
        zero[76] = 0;
        assert!(matches!(
            decode(&rechecksum(zero)),
            Err(CodeError::Characters)
        ));
        // An extra copy of the existing IPv4 record must be rejected,
        // even when the checksum was recomputed after tampering.
        let mut duplicate = b.clone();
        let first_v4 = b[70..77].to_vec();
        duplicate[69] = 3;
        duplicate.splice(96..96, first_v4);
        assert!(matches!(
            decode(&rechecksum(duplicate)),
            Err(CodeError::Characters)
        ));
        let mut junk = b;
        junk.insert(junk.len() - CHECKSUM, 42);
        assert!(matches!(
            decode(&rechecksum(junk)),
            Err(CodeError::Characters)
        ));
    }
    #[test]
    fn rejects_noncanonical_padding_empty_route_and_truncation() {
        let original = encode(&sample()).unwrap();
        let extra = format!("{original}0");
        assert!(decode(&extra).is_err());
        let mut no_routes = sample();
        no_routes.endpoint = EndpointAddr::new(no_routes.endpoint.id);
        assert!(matches!(encode(&no_routes), Err(CodeError::Characters)));
        for i in 0..original.len() {
            // A prefix alone or a shortened base32 token must never parse.
            assert!(decode(&original[..i]).is_err());
        }
        for i in 0..256 {
            let bytes = (0..i).map(|j| (j * 19 + 7) as u8).collect::<Vec<_>>();
            let candidate = pack(&bytes);
            assert!(decode(&candidate).is_err());
        }
    }
}
