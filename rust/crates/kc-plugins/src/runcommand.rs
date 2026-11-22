//! Run-command plugin: receives remote command definitions and output events,
//! and sends `kdeconnect.runcommand.request` packets back.

use std::collections::HashMap;

use kc_core::{Plugin, PluginContext, Result};
use kc_protocol::NetworkPacket;

const RUNCOMMAND: &str = "kdeconnect.runcommand";
const RUNCOMMAND_OUTPUT: &str = "kdeconnect.runcommand.output";
const RUNCOMMAND_REQUEST: &str = "kdeconnect.runcommand.request";

/// One command advertised by the peer; `commandList` is a JSON object of
/// key -> `{ name, command }`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteCommand {
    pub name: String,
    pub command: String,
}

/// Parsed `commandList` body of a `kdeconnect.runcommand` packet.
#[derive(Clone, Debug, Default)]
pub struct CommandList {
    pub commands: HashMap<String, RemoteCommand>,
    pub can_add_command: bool,
}

impl CommandList {
    pub fn from_packet(packet: &NetworkPacket) -> Self {
        let commands = packet
            .get_str("commandList")
            .and_then(|raw| serde_json::from_str::<HashMap<String, RemoteCommand>>(raw).ok())
            .unwrap_or_default();
        Self {
            commands,
            can_add_command: packet.get_bool_or("canAddCommand", false),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandStarted {
    pub id: i64,
    pub command: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOutputEvent {
    pub id: i64,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandFinished {
    pub id: i64,
    pub success: bool,
}

/// One of the three `kdeconnect.runcommand.output` event kinds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutput {
    Started(CommandStarted),
    Output(CommandOutputEvent),
    Finished(CommandFinished),
}

/// Distinguish output events by their marker key. `success` defaults to true.
pub fn parse_output(packet: &NetworkPacket) -> Option<CommandOutput> {
    let id = packet.get_i64("id").unwrap_or(0);
    if packet.get("commandStarted").is_some() {
        return Some(CommandOutput::Started(CommandStarted {
            id,
            command: packet.get_string("command").unwrap_or_default(),
        }));
    }
    if packet.get("commandOutput").is_some() {
        return Some(CommandOutput::Output(CommandOutputEvent {
            id,
            stdout: packet.get_string_list("stdout").unwrap_or_default(),
            stderr: packet.get_string_list("stderr").unwrap_or_default(),
        }));
    }
    if packet.get("commandFinished").is_some() {
        return Some(CommandOutput::Finished(CommandFinished {
            id,
            success: packet
                .get_bool("success")
                .or_else(|| packet.get_bool("commandFinished"))
                .unwrap_or(true),
        }));
    }
    None
}

/// Ask the peer for its command list.
pub fn request_commands() -> NetworkPacket {
    let mut packet = NetworkPacket::new(RUNCOMMAND_REQUEST);
    packet.set_bool("requestCommandList", true);
    packet
}

/// Execute the command registered under `key`.
pub fn run_command(key: impl Into<String>) -> NetworkPacket {
    let mut packet = NetworkPacket::new(RUNCOMMAND_REQUEST);
    packet.set_str("key", key);
    packet
}

/// Enter command-setup mode on the peer.
pub fn setup() -> NetworkPacket {
    let mut packet = NetworkPacket::new(RUNCOMMAND_REQUEST);
    packet.set_bool("setup", true);
    packet
}

/// Stop the running command on the peer.
pub fn stop() -> NetworkPacket {
    let mut packet = NetworkPacket::new(RUNCOMMAND_REQUEST);
    packet.set_bool("stop", true);
    packet
}

#[derive(Default, Debug)]
pub struct RunCommandPlugin;

impl Plugin for RunCommandPlugin {
    fn key(&self) -> &'static str {
        "RunCommandPlugin"
    }
    fn display_name(&self) -> &'static str {
        "Run commands"
    }
    fn description(&self) -> &'static str {
        "Run and manage commands on a paired device"
    }
    fn supported_packet_types(&self) -> &'static [&'static str] {
        &[RUNCOMMAND, RUNCOMMAND_OUTPUT]
    }
    fn outgoing_packet_types(&self) -> &'static [&'static str] {
        &[RUNCOMMAND_REQUEST]
    }

    fn on_packet(&self, ctx: &PluginContext, packet: &NetworkPacket) -> Result<bool> {
        match packet.ty.as_str() {
            RUNCOMMAND => {
                let list = CommandList::from_packet(packet);
                tracing::debug!(device = %ctx.device_id, commands = list.commands.len(), "command list received");
                ctx.platform.on_run_command(&ctx.device_id, packet);
                Ok(true)
            }
            RUNCOMMAND_OUTPUT => {
                if parse_output(packet).is_none() {
                    return Ok(false);
                }
                ctx.platform.on_run_command(&ctx.device_id, packet);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn command_list_is_parsed_from_json_string() {
        let json = r#"{"a":{"name":"List files","command":"ls -l"},"b":{"name":"Uptime","command":"uptime"}}"#;
        let mut packet = NetworkPacket::new(RUNCOMMAND);
        packet.set_str("commandList", json);
        packet.set_bool("canAddCommand", true);

        let back = NetworkPacket::deserialize(&packet.serialize()).unwrap();
        let list = CommandList::from_packet(&back);
        assert!(list.can_add_command);
        assert_eq!(list.commands.len(), 2);
        assert_eq!(
            list.commands.get("a"),
            Some(&RemoteCommand {
                name: "List files".into(),
                command: "ls -l".into()
            })
        );
        assert_eq!(list.commands["b"].command, "uptime");
    }

    #[test]
    fn malformed_command_list_yields_empty_map() {
        let mut packet = NetworkPacket::new(RUNCOMMAND);
        packet.set_str("commandList", "not json");
        assert!(CommandList::from_packet(&packet).commands.is_empty());
        assert!(CommandList::from_packet(&NetworkPacket::new(RUNCOMMAND))
            .commands
            .is_empty());
    }

    #[test]
    fn output_events_are_distinguished() {
        let mut started = NetworkPacket::new(RUNCOMMAND_OUTPUT);
        started.set("commandStarted", Value::Bool(true));
        started.set_i64("id", 1);
        started.set_str("command", "ls");
        assert_eq!(
            parse_output(&started),
            Some(CommandOutput::Started(CommandStarted {
                id: 1,
                command: "ls".into()
            }))
        );

        let mut output = NetworkPacket::new(RUNCOMMAND_OUTPUT);
        output.set("commandOutput", Value::Bool(true));
        output.set_i64("id", 2);
        output.set(
            "stdout",
            Value::Array(vec![Value::from("one"), Value::from("two")]),
        );
        output.set("stderr", Value::Array(vec![Value::from("err")]));
        assert_eq!(
            parse_output(&output),
            Some(CommandOutput::Output(CommandOutputEvent {
                id: 2,
                stdout: vec!["one".into(), "two".into()],
                stderr: vec!["err".into()],
            }))
        );

        let mut finished = NetworkPacket::new(RUNCOMMAND_OUTPUT);
        finished.set("commandFinished", Value::Bool(true));
        finished.set_i64("id", 3);
        assert_eq!(
            parse_output(&finished),
            Some(CommandOutput::Finished(CommandFinished {
                id: 3,
                success: true
            }))
        );

        let mut failed = NetworkPacket::new(RUNCOMMAND_OUTPUT);
        failed.set("commandFinished", Value::Bool(true));
        failed.set_i64("id", 4);
        failed.set_bool("success", false);
        assert_eq!(
            parse_output(&failed),
            Some(CommandOutput::Finished(CommandFinished {
                id: 4,
                success: false
            }))
        );

        assert_eq!(parse_output(&NetworkPacket::new(RUNCOMMAND_OUTPUT)), None);
    }

    #[test]
    fn request_constructors_roundtrip() {
        let packets = [request_commands(), run_command("a"), setup(), stop()];
        let back: Vec<_> = packets
            .iter()
            .map(|p| NetworkPacket::deserialize(&p.serialize()).unwrap())
            .collect();
        for packet in &back {
            assert_eq!(packet.ty, RUNCOMMAND_REQUEST);
        }
        assert_eq!(back[0].get_bool("requestCommandList"), Some(true));
        assert_eq!(back[1].get_str("key"), Some("a"));
        assert_eq!(back[2].get_bool("setup"), Some(true));
        assert_eq!(back[3].get_bool("stop"), Some(true));
    }

    #[test]
    fn remote_command_is_serde_roundtrippable() {
        let command = RemoteCommand {
            name: "Say hi".into(),
            command: "echo hi".into(),
        };
        let json = serde_json::to_string(&command).unwrap();
        let back: RemoteCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(back, command);
    }
}
