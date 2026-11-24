use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum Request {
    GetStatus,
    SetEnabled(bool),
    ForceBreak,
    UpdateConfig(Box<Config>),
    StopService { passphrase: String },
    VerifyPassphrase { passphrase: String },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum Response {
    Status(Status),
    Ok,
    Denied,
    Err(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Status {
    pub enabled: bool,
    pub state: TimerState,
    pub remaining_seconds: u32,
    pub breaks_today: u32,
    pub streak: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TimerState {
    Idle,
    Working,
    Breaking,
    /// Break countdown finished; lock stays until the user resumes.
    AwaitingResume,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T>(value: T) -> serde_json::Value
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_value(&value).unwrap();
        let back: T = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, value);
        json
    }

    #[test]
    fn request_variants_round_trip() {
        assert_eq!(
            roundtrip(Request::GetStatus),
            serde_json::json!({"type": "GetStatus"})
        );
        assert_eq!(
            roundtrip(Request::SetEnabled(false)),
            serde_json::json!({"type": "SetEnabled", "data": false})
        );
        assert_eq!(
            roundtrip(Request::ForceBreak),
            serde_json::json!({"type": "ForceBreak"})
        );
        let config_json = roundtrip(Request::UpdateConfig(Box::new(Config::default())));
        assert_eq!(config_json["type"], "UpdateConfig");
        assert!(config_json["data"].is_object());
        assert_eq!(
            roundtrip(Request::StopService {
                passphrase: "pw".into()
            }),
            serde_json::json!({"type": "StopService", "data": {"passphrase": "pw"}})
        );
        assert_eq!(
            roundtrip(Request::VerifyPassphrase {
                passphrase: "pw".into()
            }),
            serde_json::json!({"type": "VerifyPassphrase", "data": {"passphrase": "pw"}})
        );
    }

    #[test]
    fn response_variants_round_trip() {
        assert_eq!(roundtrip(Response::Ok), serde_json::json!({"type": "Ok"}));
        assert_eq!(
            roundtrip(Response::Denied),
            serde_json::json!({"type": "Denied"})
        );
        assert_eq!(
            roundtrip(Response::Err("boom".into())),
            serde_json::json!({"type": "Err", "data": "boom"})
        );
        let status = Status {
            enabled: true,
            state: TimerState::Working,
            remaining_seconds: 120,
            breaks_today: 2,
            streak: 3,
        };
        let json = roundtrip(Response::Status(status));
        assert_eq!(json["data"]["state"], "working");
        assert_eq!(json["data"]["remaining_seconds"], 120);
    }

    #[test]
    fn timer_state_serializes_like_python_strings() {
        assert_eq!(
            serde_json::to_value(TimerState::Idle).unwrap(),
            "idle"
        );
        assert_eq!(
            serde_json::to_value(TimerState::Breaking).unwrap(),
            "breaking"
        );
    }
}
