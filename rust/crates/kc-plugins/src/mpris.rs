//! MPRIS plugin (phone as state receiver): the desktop sends playback state,
//! the phone sends `kdeconnect.mpris.request` commands back.

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const MPRIS: &str = "kdeconnect.mpris";
const MPRIS_REQUEST: &str = "kdeconnect.mpris.request";

/// Actions accepted by the `action` field of an MPRIS request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MprisAction {
    PlayPause,
    Play,
    Pause,
    Stop,
    Previous,
    Next,
}

impl MprisAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlayPause => "PlayPause",
            Self::Play => "Play",
            Self::Pause => "Pause",
            Self::Stop => "Stop",
            Self::Previous => "Previous",
            Self::Next => "Next",
        }
    }
}

impl From<&str> for MprisAction {
    fn from(value: &str) -> Self {
        match value {
            "Play" => Self::Play,
            "Pause" => Self::Pause,
            "Stop" => Self::Stop,
            "Previous" => Self::Previous,
            "Next" => Self::Next,
            _ => Self::PlayPause,
        }
    }
}

impl From<String> for MprisAction {
    fn from(value: String) -> Self {
        Self::from(value.as_str())
    }
}

/// Playback state decoded from an incoming `kdeconnect.mpris` packet.
///
/// Album-art bytes, when `transferringAlbumArt` is set, remain in
/// `packet.payload` and are handed to the host together with the packet.
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub player: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub url: Option<String>,
    pub loop_status: Option<String>,
    pub shuffle: bool,
    pub volume: i32,
    pub length: i64,
    pub position: i64,
    pub rate: f64,
    pub is_playing: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_seek: bool,
    pub album_art_url: Option<String>,
    pub support_album_art_payload: bool,
    pub player_list: Vec<String>,
    pub fallback_player: Option<String>,
    pub transferring_album_art: bool,
}

impl Track {
    pub fn from_packet(packet: &NetworkPacket) -> Self {
        Self {
            player: packet.get_string("player"),
            title: packet.get_string("title"),
            artist: packet.get_string("artist"),
            album: packet.get_string("album"),
            url: packet.get_string("url"),
            loop_status: packet.get_string("loopStatus"),
            shuffle: packet.get_bool_or("shuffle", false),
            volume: packet.get_i64("volume").unwrap_or(0) as i32,
            length: packet.get_i64("length").unwrap_or(0),
            position: packet.get_i64("pos").unwrap_or(0),
            rate: packet.get_f64("rate").unwrap_or(1.0),
            is_playing: packet.get_bool_or("isPlaying", false),
            can_play: packet.get_bool_or("canPlay", false),
            can_pause: packet.get_bool_or("canPause", false),
            can_go_next: packet.get_bool_or("canGoNext", false),
            can_go_previous: packet.get_bool_or("canGoPrevious", false),
            can_seek: packet.get_bool_or("canSeek", false),
            album_art_url: packet.get_string("albumArtUrl"),
            support_album_art_payload: packet.get_bool_or("supportAlbumArtPayload", false),
            player_list: packet.get_string_list("playerList").unwrap_or_default(),
            fallback_player: packet.get_string("fallbackPlayer"),
            transferring_album_art: packet.get_bool_or("transferringAlbumArt", false),
        }
    }
}

fn request() -> NetworkPacket {
    NetworkPacket::new(MPRIS_REQUEST)
}

fn with_player(mut packet: NetworkPacket, player: Option<&str>) -> NetworkPacket {
    if let Some(player) = player {
        packet.set_str("player", player);
    }
    packet
}

/// Ask the peer for the list of available media players.
pub fn request_player_list() -> NetworkPacket {
    let mut packet = request();
    packet.set_bool("requestPlayerList", true);
    packet
}

/// Ask for the current track metadata of `player`.
pub fn request_now_playing(player: impl Into<String>) -> NetworkPacket {
    let mut packet = request();
    packet.set_str("player", player);
    packet.set_bool("requestNowPlaying", true);
    packet
}

/// Ask for the artwork at `url`; advertise that we accept a payload in reply.
pub fn request_album_art(player: impl Into<String>, url: impl Into<String>) -> NetworkPacket {
    let mut packet = request();
    packet.set_str("player", player);
    packet.set_str("albumArtUrl", url);
    packet.set_bool("supportAlbumArtPayload", true);
    packet.set_bool("requestAlbumArt", true);
    packet
}

/// Send a playback command to `player`.
pub fn action(player: impl Into<String>, action: impl Into<MprisAction>) -> NetworkPacket {
    let mut packet = request();
    packet.set_str("player", player);
    packet.set_str("action", action.into().as_str());
    packet
}

pub fn set_loop_status(status: impl Into<String>) -> NetworkPacket {
    let mut packet = request();
    packet.set_str("loopStatus", status);
    packet
}

pub fn set_loop_status_for_player(player: &str, status: impl Into<String>) -> NetworkPacket {
    with_player(set_loop_status(status), Some(player))
}

pub fn set_shuffle(shuffle: bool) -> NetworkPacket {
    let mut packet = request();
    packet.set_bool("shuffle", shuffle);
    packet
}

pub fn set_shuffle_for_player(player: &str, shuffle: bool) -> NetworkPacket {
    with_player(set_shuffle(shuffle), Some(player))
}

pub fn set_volume(volume: i32) -> NetworkPacket {
    let mut packet = request();
    packet.set_i64("volume", volume as i64);
    packet
}

pub fn set_volume_for_player(player: &str, volume: i32) -> NetworkPacket {
    with_player(set_volume(volume), Some(player))
}

/// Set absolute playback position in milliseconds.
pub fn set_position(pos_ms: i64) -> NetworkPacket {
    let mut packet = request();
    packet.set_i64("position", pos_ms);
    packet
}

pub fn set_position_for_player(player: &str, pos_ms: i64) -> NetworkPacket {
    with_player(set_position(pos_ms), Some(player))
}

/// Seek by `offset_ms` milliseconds (may be negative).
pub fn seek(offset_ms: i64) -> NetworkPacket {
    let mut packet = request();
    packet.set_i64("seek", offset_ms);
    packet
}

pub fn seek_for_player(player: &str, offset_ms: i64) -> NetworkPacket {
    with_player(seek(offset_ms), Some(player))
}

#[derive(Default, Debug)]
pub struct MprisPlugin;

impl Plugin for MprisPlugin {
    fn key(&self) -> &'static str {
        "MprisPlugin"
    }
    fn display_name(&self) -> &'static str {
        "MPRIS"
    }
    fn description(&self) -> &'static str {
        "Control media players and show their playback state"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[MPRIS]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[MPRIS_REQUEST]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        if packet.ty != MPRIS {
            return Ok(false);
        }
        let track = Track::from_packet(packet);
        tracing::debug!(device = %ctx.device_id, player = ?track.player, "MPRIS state received");
        ctx.platform.on_mpris(&ctx.device_id, packet);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};

    #[test]
    fn track_from_packet_reads_all_fields() {
        let mut packet = NetworkPacket::new(MPRIS);
        packet.set_str("player", "org.mpris.MediaPlayer2.vlc");
        packet.set_str("title", "Song");
        packet.set_str("artist", "Band");
        packet.set_str("album", "Record");
        packet.set_str("url", "file:///song.ogg");
        packet.set_str("loopStatus", "Playlist");
        packet.set_bool("shuffle", true);
        packet.set_i64("volume", 80);
        packet.set_i64("length", 200000);
        packet.set_i64("pos", 30000);
        packet.set_f64("rate", 1.25);
        packet.set_bool("isPlaying", true);
        packet.set_bool("canPlay", true);
        packet.set_bool("canPause", true);
        packet.set_bool("canGoNext", true);
        packet.set_bool("canGoPrevious", true);
        packet.set_bool("canSeek", true);
        packet.set_str("albumArtUrl", "file:///art.png");
        packet.set_bool("supportAlbumArtPayload", true);
        packet.set(
            "playerList",
            Value::Array(vec![Value::from("a"), Value::from("b")]),
        );
        packet.set_str("fallbackPlayer", "org.mpris.MediaPlayer2.null");
        packet.set_bool("transferringAlbumArt", true);
        packet.set_payload_bytes(vec![1, 2, 3]);

        let track = Track::from_packet(&packet);
        assert_eq!(track.player.as_deref(), Some("org.mpris.MediaPlayer2.vlc"));
        assert_eq!(track.title.as_deref(), Some("Song"));
        assert_eq!(track.artist.as_deref(), Some("Band"));
        assert_eq!(track.album.as_deref(), Some("Record"));
        assert_eq!(track.url.as_deref(), Some("file:///song.ogg"));
        assert_eq!(track.loop_status.as_deref(), Some("Playlist"));
        assert!(track.shuffle && track.is_playing && track.can_seek && track.can_go_previous);
        assert_eq!(track.volume, 80);
        assert_eq!(track.length, 200000);
        assert_eq!(track.position, 30000);
        assert_eq!(track.rate, 1.25);
        assert_eq!(track.album_art_url.as_deref(), Some("file:///art.png"));
        assert!(track.support_album_art_payload && track.transferring_album_art);
        assert_eq!(track.player_list, vec!["a", "b"]);
        assert_eq!(
            track.fallback_player.as_deref(),
            Some("org.mpris.MediaPlayer2.null")
        );
    }

    #[test]
    fn requests_roundtrip_on_the_wire() {
        let mut packets = vec![
            request_player_list(),
            request_now_playing("player-1"),
            request_album_art("player-1", "file:///art.png"),
            action("player-1", MprisAction::Next),
            set_loop_status("Track"),
            set_shuffle(true),
            set_volume(42),
            set_position(1234),
            seek(-500),
            seek_for_player("player-2", 700),
        ];
        for packet in &mut packets {
            assert_eq!(packet.ty, MPRIS_REQUEST);
        }
        let wire: Vec<_> = packets.iter().map(NetworkPacket::serialize).collect();
        let back: Vec<_> = wire
            .iter()
            .map(|w| NetworkPacket::deserialize(w).unwrap())
            .collect();

        assert_eq!(back[0].get_bool("requestPlayerList"), Some(true));
        assert_eq!(back[1].get_str("player"), Some("player-1"));
        assert_eq!(back[1].get_bool("requestNowPlaying"), Some(true));
        assert_eq!(back[2].get_bool("requestAlbumArt"), Some(true));
        assert_eq!(back[2].get_str("albumArtUrl"), Some("file:///art.png"));
        assert_eq!(back[2].get_bool("supportAlbumArtPayload"), Some(true));
        assert_eq!(back[3].get_str("action"), Some("Next"));
        assert_eq!(back[4].get_str("loopStatus"), Some("Track"));
        assert_eq!(back[5].get_bool("shuffle"), Some(true));
        assert_eq!(back[6].get_i64("volume"), Some(42));
        assert_eq!(back[7].get_i64("position"), Some(1234));
        assert_eq!(back[8].get_i64("seek"), Some(-500));
        assert_eq!(back[9].get_str("player"), Some("player-2"));
    }

    #[test]
    fn album_art_state_packet_can_carry_payload() {
        let mut packet = NetworkPacket::new(MPRIS);
        packet.set_bool("transferringAlbumArt", true);
        packet.set_str("albumArtUrl", "file:///art.png");
        packet.set_payload_bytes(vec![9, 8, 7]);
        packet.payload_transfer_info = Some(Map::from_iter([(
            "port".to_string(),
            Value::Number(1741.into()),
        )]));
        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        let track = Track::from_packet(&back);
        assert!(track.transferring_album_art);
        assert_eq!(track.album_art_url.as_deref(), Some("file:///art.png"));
        assert_eq!(back.payload_size, Some(3));
    }

    #[test]
    fn action_names_match_upstream() {
        assert_eq!(MprisAction::PlayPause.as_str(), "PlayPause");
        assert_eq!(MprisAction::Play.as_str(), "Play");
        assert_eq!(MprisAction::Pause.as_str(), "Pause");
        assert_eq!(MprisAction::Stop.as_str(), "Stop");
        assert_eq!(MprisAction::Previous.as_str(), "Previous");
        assert_eq!(MprisAction::Next.as_str(), "Next");

        // Both the enum and the upstream string form are accepted.
        assert_eq!(action("p", "Pause").get_str("action"), Some("Pause"));
        assert_eq!(
            action("p", MprisAction::Play).get_str("action"),
            Some("Play")
        );
    }
}
