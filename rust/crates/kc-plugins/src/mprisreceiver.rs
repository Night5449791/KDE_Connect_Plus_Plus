//! MPRIS receiver plugin (phone as player): answers desktop requests with
//! `kdeconnect.mpris` state packets.

use kc_core::{OutgoingPacket, Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;
use serde_json::Value;

const MPRIS: &str = "kdeconnect.mpris";
const MPRIS_REQUEST: &str = "kdeconnect.mpris.request";

/// Fields of an incoming `kdeconnect.mpris.request`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MprisRequest {
    pub player: Option<String>,
    pub request_player_list: bool,
    pub request_now_playing: bool,
    pub request_album_art: bool,
    pub album_art_url: Option<String>,
    pub support_album_art_payload: bool,
    pub action: Option<String>,
    pub loop_status: Option<String>,
    pub shuffle: Option<bool>,
    pub volume: Option<i32>,
    pub position: Option<i64>,
    pub seek: Option<i64>,
}

impl MprisRequest {
    pub fn from_packet(packet: &NetworkPacket) -> Self {
        Self {
            player: packet.get_string("player"),
            request_player_list: packet.get_bool_or("requestPlayerList", false),
            request_now_playing: packet.get_bool_or("requestNowPlaying", false),
            request_album_art: packet.get_bool_or("requestAlbumArt", false),
            album_art_url: packet.get_string("albumArtUrl"),
            support_album_art_payload: packet.get_bool_or("supportAlbumArtPayload", false),
            action: packet.get_string("action"),
            loop_status: packet.get_string("loopStatus"),
            shuffle: packet.get_bool("shuffle"),
            volume: packet.get_i64("volume").map(|v| v as i32),
            position: packet.get_i64("position"),
            seek: packet.get_i64("seek"),
        }
    }
}

/// Build a `kdeconnect.mpris` now-playing state packet.
pub fn now_playing_packet(
    player: impl Into<String>,
    title: impl Into<String>,
    artist: impl Into<String>,
    album: impl Into<String>,
    is_playing: bool,
    volume: i32,
) -> NetworkPacket {
    let mut packet = NetworkPacket::new(MPRIS);
    packet.set_str("player", player);
    packet.set_str("title", title);
    packet.set_str("artist", artist);
    packet.set_str("album", album);
    packet.set_bool("isPlaying", is_playing);
    packet.set_i64("volume", volume as i64);
    packet
}

/// Advertise the players available on this device.
pub fn player_list_packet(players: &[String]) -> NetworkPacket {
    let mut packet = NetworkPacket::new(MPRIS);
    packet.set(
        "playerList",
        Value::Array(players.iter().cloned().map(Value::String).collect()),
    );
    packet
}

/// Send album artwork as the payload of a `kdeconnect.mpris` state packet.
pub fn album_art_packet(
    player: impl Into<String>,
    url: impl Into<String>,
    bytes: &[u8],
) -> NetworkPacket {
    let mut packet = NetworkPacket::new(MPRIS);
    packet.set_str("player", player);
    packet.set_str("albumArtUrl", url);
    packet.set_bool("transferringAlbumArt", true);
    packet.set_payload_bytes(bytes.to_vec());
    packet
}

#[derive(Default, Debug)]
pub struct MprisReceiverPlugin;

impl Plugin for MprisReceiverPlugin {
    fn key(&self) -> &'static str {
        "MprisReceiverPlugin"
    }
    fn display_name(&self) -> &'static str {
        "MPRIS receiver"
    }
    fn description(&self) -> &'static str {
        "Expose local media players to a remote device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[MPRIS_REQUEST]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[MPRIS]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        if packet.ty != MPRIS_REQUEST {
            return Ok(false);
        }

        let request = MprisRequest::from_packet(packet);
        if request.request_player_list {
            // The platform has no player list callback yet, so advertise an
            // empty list; a host may replace this via the public constructor.
            let reply = player_list_packet(&[]);
            let _ = ctx.sender.send(OutgoingPacket {
                device_id: ctx.device_id.clone(),
                packet: reply,
            });
        }
        if request.request_now_playing {
            // No host callback supplies a live track; reply with a default
            // state built from the requested player name.
            let reply = now_playing_packet(
                request.player.clone().unwrap_or_default(),
                "",
                "",
                "",
                false,
                0,
            );
            let _ = ctx.sender.send(OutgoingPacket {
                device_id: ctx.device_id.clone(),
                packet: reply,
            });
        }

        // Actions and setting changes have no local state to echo back; hand
        // them to the host so it can drive the actual media session.
        ctx.platform.on_mpris(&ctx.device_id, packet);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    #[test]
    fn request_from_packet_reads_every_field() {
        let mut packet = NetworkPacket::new(MPRIS_REQUEST);
        packet.set_str("player", "player-1");
        packet.set_bool("requestPlayerList", true);
        packet.set_bool("requestNowPlaying", true);
        packet.set_bool("requestAlbumArt", true);
        packet.set_str("albumArtUrl", "file:///art.png");
        packet.set_bool("supportAlbumArtPayload", true);
        packet.set_str("action", "PlayPause");
        packet.set_str("loopStatus", "Playlist");
        packet.set_bool("shuffle", true);
        packet.set_i64("volume", 55);
        packet.set_i64("position", 1000);
        packet.set_i64("seek", -250);

        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        let request = MprisRequest::from_packet(&back);
        assert_eq!(request.player.as_deref(), Some("player-1"));
        assert!(
            request.request_player_list && request.request_now_playing && request.request_album_art
        );
        assert_eq!(request.album_art_url.as_deref(), Some("file:///art.png"));
        assert!(request.support_album_art_payload);
        assert_eq!(request.action.as_deref(), Some("PlayPause"));
        assert_eq!(request.loop_status.as_deref(), Some("Playlist"));
        assert_eq!(request.shuffle, Some(true));
        assert_eq!(request.volume, Some(55));
        assert_eq!(request.position, Some(1000));
        assert_eq!(request.seek, Some(-250));
    }

    #[test]
    fn state_packet_constructors_roundtrip() {
        let now = now_playing_packet("player-1", "Title", "Artist", "Album", true, 70);
        let back = NetworkPacket::deserialize(&now.serialize()).unwrap();
        assert_eq!(back.ty, MPRIS);
        assert_eq!(back.get_str("player"), Some("player-1"));
        assert_eq!(back.get_str("title"), Some("Title"));
        assert_eq!(back.get_str("artist"), Some("Artist"));
        assert_eq!(back.get_str("album"), Some("Album"));
        assert_eq!(back.get_bool("isPlaying"), Some(true));
        assert_eq!(back.get_i64("volume"), Some(70));

        let list = player_list_packet(&["a".to_string(), "b".to_string()]);
        let back = NetworkPacket::deserialize(&list.serialize()).unwrap();
        assert_eq!(
            back.get_string_list("playerList"),
            Some(vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn album_art_packet_marks_transfer_and_carries_bytes() {
        let mut packet = album_art_packet("player-1", "file:///art.png", b"png");
        packet.payload_transfer_info = Some(Map::new());
        assert_eq!(packet.ty, MPRIS);
        assert_eq!(packet.get_bool("transferringAlbumArt"), Some(true));
        assert_eq!(packet.payload_size, Some(3));
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        assert_eq!(back.payload_size, Some(3));
        assert_eq!(back.get_str("albumArtUrl"), Some("file:///art.png"));
    }
}
