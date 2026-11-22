//! Pairing state machine (Rust counterpart of `PairingHandler.kt`).
//!
//! The `kdeconnect.pair` packet body contains `pair: Boolean` and, for
//! protocol v8, `timestamp: Long` (seconds since epoch). Both peers compute a
//! verification code from the sorted public keys plus the timestamp; the
//! first 8 uppercase hex characters of SHA-256 are shown in the UI.

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::packet::{NetworkPacket, PACKET_TYPE_PAIR};

/// Clock skew accepted for a v8 pairing request (30 minutes, same as upstream).
pub const MAX_TIME_DIFFERENCE: i64 = 1800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairState {
    NotPaired,
    Requested,
    RequestedByPeer,
    Paired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingEvent {
    /// A peer asked us to pair. The string is the 8-character verification key.
    PairingRequested { verification_key: String },
    /// The peer accepted our pairing request.
    PairingAccepted,
    /// Pairing was cancelled or reset.
    PairingCancelled,
    /// An incoming packet could not be processed.
    PairingFailed(String),
    /// A previously paired peer unpaired from its side.
    Unpaired,
}

#[derive(Debug, Error)]
pub enum PairingError {
    #[error("pairing request is missing a v8 timestamp")]
    MissingTimestamp,
    #[error("pairing timestamp differs from local time by more than 1800 seconds")]
    ClockSkew,
    #[error("peer public key is not known yet")]
    MissingPeerKey,
    #[error("unexpected packet in state {0:?}")]
    UnexpectedState(PairState),
}

/// Result of `PairingHandler::accepted()`/`requested()`.
#[derive(Clone, Debug)]
pub struct PairingResult {
    pub state: PairState,
    pub verification_key: Option<String>,
    pub timestamp: Option<i64>,
}

pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Deterministically sort two byte strings by unsigned lexicographic order and
/// concatenate them, matching Kotlin's `sortedConcat`.
pub fn sorted_concat(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    if a <= b {
        out.extend_from_slice(a);
        out.extend_from_slice(b);
    } else {
        out.extend_from_slice(b);
        out.extend_from_slice(a);
    }
    out
}

/// Compute the 8-character uppercase hex verification code.
pub fn verification_key(
    my_public_key: &[u8],
    peer_public_key: &[u8],
    timestamp: Option<i64>,
) -> String {
    let mut bytes = sorted_concat(my_public_key, peer_public_key);
    if let Some(ts) = timestamp {
        bytes.extend_from_slice(ts.to_string().as_bytes());
    }
    let digest = Sha256::digest(&bytes);
    hex::encode(digest)[..8].to_uppercase()
}

#[derive(Debug)]
pub struct PairingHandler {
    state: PairState,
    my_public_key: Vec<u8>,
    peer_public_key: Option<Vec<u8>>,
    timestamp: Option<i64>,
}

impl PairingHandler {
    pub fn new(my_public_key: Vec<u8>) -> Self {
        Self {
            state: PairState::NotPaired,
            my_public_key,
            peer_public_key: None,
            timestamp: None,
        }
    }

    /// Start from an already-paired state (device loaded from settings).
    pub fn paired(my_public_key: Vec<u8>, peer_public_key: Vec<u8>) -> Self {
        Self {
            state: PairState::Paired,
            my_public_key,
            peer_public_key: Some(peer_public_key),
            timestamp: None,
        }
    }

    pub fn state(&self) -> PairState {
        self.state
    }

    pub fn is_paired(&self) -> bool {
        self.state == PairState::Paired
    }

    pub fn set_peer_public_key(&mut self, key: Vec<u8>) {
        self.peer_public_key = Some(key);
    }

    fn make_pair_packet(&self, pair: bool, include_timestamp: bool) -> NetworkPacket {
        let mut packet = NetworkPacket::new(PACKET_TYPE_PAIR);
        packet.set_bool("pair", pair);
        if include_timestamp {
            packet.set_i64("timestamp", now_seconds());
        }
        packet
    }

    /// User tapped "request pairing": `NotPaired -> Requested`.
    pub fn request_pairing(&mut self) -> Result<NetworkPacket, PairingError> {
        if self.state == PairState::Paired {
            return Err(PairingError::UnexpectedState(self.state));
        }
        self.state = PairState::Requested;
        self.timestamp = Some(now_seconds());
        Ok(self.make_pair_packet(true, true))
    }

    /// User accepted an incoming request: `RequestedByPeer -> Paired`.
    pub fn accept_pairing(&mut self) -> Result<PairingResult, PairingError> {
        if self.state != PairState::RequestedByPeer {
            return Err(PairingError::UnexpectedState(self.state));
        }
        let key = self.verification_key()?;
        self.state = PairState::Paired;
        Ok(PairingResult {
            state: self.state,
            verification_key: Some(key),
            timestamp: self.timestamp,
        })
    }

    /// Cancel an ongoing request or unpair: emits `pair=false`.
    pub fn cancel_pairing(&mut self) -> NetworkPacket {
        self.state = PairState::NotPaired;
        self.timestamp = None;
        self.make_pair_packet(false, false)
    }

    /// Handle an incoming `kdeconnect.pair` packet.
    pub fn on_packet(&mut self, packet: &NetworkPacket) -> Result<PairingEvent, PairingError> {
        if packet.ty != PACKET_TYPE_PAIR {
            return Err(PairingError::UnexpectedState(self.state));
        }
        let pair = packet.get_bool("pair").unwrap_or(false);
        if !pair {
            return match self.state {
                PairState::Requested | PairState::RequestedByPeer => {
                    self.state = PairState::NotPaired;
                    Ok(PairingEvent::PairingCancelled)
                }
                PairState::Paired => {
                    self.state = PairState::NotPaired;
                    Ok(PairingEvent::Unpaired)
                }
                PairState::NotPaired => Ok(PairingEvent::PairingCancelled),
            };
        }

        if self.state == PairState::Requested {
            // The peer accepted our request.
            self.state = PairState::Paired;
            return Ok(PairingEvent::PairingAccepted);
        }

        // A fresh request (or an "accept" we did not expect): treat as a new
        // request, like upstream does, after clearing any previous pairing.
        if self.state == PairState::Paired {
            self.state = PairState::NotPaired;
        }

        let timestamp = packet
            .get_i64("timestamp")
            .ok_or(PairingError::MissingTimestamp)?;
        let now = now_seconds();
        if (now - timestamp).abs() > MAX_TIME_DIFFERENCE {
            return Err(PairingError::ClockSkew);
        }
        self.timestamp = Some(timestamp);
        self.state = PairState::RequestedByPeer;

        let key = self.verification_key()?;
        Ok(PairingEvent::PairingRequested {
            verification_key: key,
        })
    }

    fn verification_key(&self) -> Result<String, PairingError> {
        let peer = self
            .peer_public_key
            .as_ref()
            .ok_or(PairingError::MissingPeerKey)?;
        Ok(verification_key(&self.my_public_key, peer, self.timestamp))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler() -> PairingHandler {
        PairingHandler::new(vec![1, 2, 3])
    }

    #[test]
    fn request_accept_flow() {
        let mut a = handler();
        let mut b = PairingHandler::new(vec![9, 8, 7]);
        a.set_peer_public_key(vec![9, 8, 7]);
        b.set_peer_public_key(vec![1, 2, 3]);

        let request = a.request_pairing().unwrap();
        assert!(request.get_bool("pair").unwrap());
        assert!(request.get_i64("timestamp").is_some());
        let event = b.on_packet(&request).unwrap();
        match event {
            PairingEvent::PairingRequested { verification_key } => {
                assert_eq!(verification_key.len(), 8)
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let accept = b.accept_pairing().unwrap();
        assert_eq!(accept.state, PairState::Paired);

        let mut accept_packet = NetworkPacket::new(PACKET_TYPE_PAIR);
        accept_packet.set_bool("pair", true);
        assert_eq!(
            a.on_packet(&accept_packet).unwrap(),
            PairingEvent::PairingAccepted
        );
        assert!(a.is_paired());
    }

    #[test]
    fn clock_skew_is_rejected() {
        let mut h = handler();
        h.set_peer_public_key(vec![9]);
        let mut p = NetworkPacket::new(PACKET_TYPE_PAIR);
        p.set_bool("pair", true);
        p.set_i64("timestamp", now_seconds() - 2000);
        assert!(matches!(h.on_packet(&p), Err(PairingError::ClockSkew)));
    }

    #[test]
    fn unpair_event() {
        let mut h = PairingHandler::paired(vec![1], vec![2]);
        let mut p = NetworkPacket::new(PACKET_TYPE_PAIR);
        p.set_bool("pair", false);
        assert_eq!(h.on_packet(&p).unwrap(), PairingEvent::Unpaired);
        assert_eq!(h.state(), PairState::NotPaired);
    }

    #[test]
    fn verification_key_is_deterministic() {
        let a = verification_key(&[1, 2], &[3, 4], Some(1234));
        let b = verification_key(&[3, 4], &[1, 2], Some(1234));
        assert_eq!(a, b);
        assert_ne!(a, verification_key(&[1, 2], &[3, 4], Some(1235)));
    }
}
