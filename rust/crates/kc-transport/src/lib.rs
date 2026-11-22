//! Transport implementations for the Rust KDE Connect client.
//!
//! * [`LanLinkProvider`] — UDP identity broadcast on port 1716, TCP links with
//!   mutual TLS 1.2, streamed payloads on a second TLS connection and a
//!   best-effort mDNS query, mirroring upstream `LanLinkProvider`/`LanLink`.
//! * [`LoopbackLinkProvider`] — one self-connected link that echoes packets.
//!
//! All events are delivered through [`kc_core::LinkEvent`] on the unbounded
//! channel supplied to [`kc_core::LinkProvider::start`].

mod framing;
mod identity;
mod lan;
mod link;
mod loopback;
mod mdns;
mod net;

pub use lan::LanLinkProvider;
pub use link::{fetch_payload, send_payload, LanLink};
pub use loopback::{LoopbackLink, LoopbackLinkProvider};

pub use framing::{decode_packet_line, read_line_bounded};
pub use identity::{
    check_targets, identity_with_tcp_port, pinned_cert_der, targeted_identity_packet,
};
pub use net::{
    bind_tcp_listener, bind_udp_reuse, find_free_tcp_port, PAYLOAD_PORT_MIN, TCP_PORT_MAX,
    TCP_PORT_MIN, UDP_PORT,
};
