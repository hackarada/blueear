//! Closed set of meeting apps Blue Ear can capture.
//!
//! Discriminants must stay in lockstep with Swift `BlueEarMeetingApp` in
//! `native/BlueEarAudio/.../MeetingApp.swift`.

use serde::{Deserialize, Serialize};

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingAppId {
    Teams = 0,
    Zoom = 1,
}

impl MeetingAppId {
    pub const ALL: [MeetingAppId; 2] = [MeetingAppId::Teams, MeetingAppId::Zoom];

    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// Parse the Swift/C ABI discriminant (e.g. status-callback detail).
    pub fn from_i32(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Teams),
            1 => Some(Self::Zoom),
            _ => None,
        }
    }

    /// Stable JSON / `source.json` wire name (`teams` | `zoom`).
    pub fn wire_id(self) -> &'static str {
        match self {
            Self::Teams => "teams",
            Self::Zoom => "zoom",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Teams => "Microsoft Teams",
            Self::Zoom => "Zoom",
        }
    }

    pub fn from_wire(id: &str) -> Option<Self> {
        match id {
            "teams" => Some(Self::Teams),
            "zoom" => Some(Self::Zoom),
            _ => None,
        }
    }
}

impl Default for MeetingAppId {
    fn default() -> Self {
        MeetingAppId::Teams
    }
}

impl std::fmt::Display for MeetingAppId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_id())
    }
}

impl std::str::FromStr for MeetingAppId {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_wire(s).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discriminants_match_swift_blue_ear_meeting_app() {
        assert_eq!(MeetingAppId::Teams as i32, 0);
        assert_eq!(MeetingAppId::Zoom as i32, 1);
        assert_eq!(MeetingAppId::from_i32(0), Some(MeetingAppId::Teams));
        assert_eq!(MeetingAppId::from_i32(1), Some(MeetingAppId::Zoom));
        assert_eq!(MeetingAppId::from_i32(99), None);
    }

    #[test]
    fn wire_ids_round_trip() {
        for app in MeetingAppId::ALL {
            assert_eq!(MeetingAppId::from_wire(app.wire_id()), Some(app));
        }
    }
}
