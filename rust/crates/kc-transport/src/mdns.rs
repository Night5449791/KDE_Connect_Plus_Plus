//! Best-effort mDNS discovery for `_kdeconnect._udp`.
//!
//! Upstream announces/discovers this service type. A full mDNS resolver is
//! unnecessary here: we send a PTR query to 224.0.0.251:5353 and, for any
//! response mentioning `_kdeconnect`, answer with a unicast UDP identity
//! packet to the responding address. Binding 5353 can fail (another daemon may
//! own it); that is non-fatal.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::interval;
use tracing::debug;

use crate::lan::HandshakeCtx;

fn mdns_target() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::new(224, 0, 0, 251), 5353))
}

/// PTR query for `_kdeconnect._udp.local.`
fn query_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in ["_kdeconnect", "_udp", "local"] {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&[0, 12, 0, 1]); // PTR, IN
    out
}

fn mentions_kdeconnect(body: &[u8]) -> bool {
    let needle = b"_kdeconnect";
    body.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

async fn recv_response(listener: Option<&UdpSocket>) -> Option<(Vec<u8>, SocketAddr)> {
    let listener = listener?;
    let mut buf = [0u8; 1500];
    let (len, source) = listener.recv_from(&mut buf).await.ok()?;
    Some((buf[..len].to_vec(), source))
}

pub(crate) async fn discovery_loop(ctx: Arc<HandshakeCtx>) {
    let Ok(sender) = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0))).await else {
        debug!("cannot create mDNS query socket");
        return;
    };
    let _ = sender.set_multicast_loop_v4(true);

    // This may be occupied by avahi/mdnsresponder; discovery is then disabled
    // but the rest of the LAN provider keeps working.
    let listener = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 5353)))
        .await
        .ok();
    if let Some(listener) = &listener {
        // Required on Linux for multicast responses to be delivered.
        let _ = listener.join_multicast_v4(Ipv4Addr::new(224, 0, 0, 251), Ipv4Addr::UNSPECIFIED);
    } else {
        debug!("port 5353 unavailable; mDNS discovery disabled");
    }

    let query = query_bytes();
    let mut ticker = interval(Duration::from_secs(5));
    let mut stop = ctx.stop_rx.clone();
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticker.tick() => {
                if let Err(e) = sender.send_to(&query, mdns_target()).await {
                    debug!(%e, "mDNS query failed");
                }
            }
            response = recv_response(listener.as_ref()) => {
                if let Some((body, source)) = response {
                    if mentions_kdeconnect(&body) {
                        ctx.send_identity_to(source.ip()).await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_query_has_expected_shape() {
        let query = query_bytes();
        assert_eq!(&query[..12], &[0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(query[12], 11); // length of "_kdeconnect"
        assert_eq!(&query[13..24], b"_kdeconnect");
        assert!(query.ends_with(&[0, 12, 0, 1]));
    }

    #[test]
    fn detects_service_name_case_insensitively() {
        assert!(mentions_kdeconnect(b"junk _KDECONNECT._udp.local more"));
        assert!(!mentions_kdeconnect(b"_somethingelse._udp.local"));
    }
}
