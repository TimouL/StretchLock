//! Named-pipe client for the StretchLock daemon.
//!
//! Protocol: u32 LE length prefix + serde_json payload, both directions.

use std::io::{Read, Write};

#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::time::Duration;

use stretchlock_core::ipc::{Request, Response};

#[cfg(windows)]
pub const PIPE_PATH: &str = r"\\.\pipe\stretchlock";

const MAX_FRAME: usize = 4 * 1024 * 1024;

#[cfg(windows)]
const MAX_ATTEMPTS: usize = 3;

#[cfg(windows)]
fn open_pipe() -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_PATH)
}

#[cfg(windows)]
fn connect() -> std::io::Result<std::fs::File> {
    // ERROR_PIPE_BUSY: the daemon's single pipe instance may still be
    // serving another request; retry briefly before giving up.
    for attempt in 0..MAX_ATTEMPTS {
        match open_pipe() {
            Ok(pipe) => return Ok(pipe),
            Err(err) if attempt + 1 < MAX_ATTEMPTS => {
                log::debug!("pipe connect attempt {} failed: {}", attempt + 1, err);
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(err) => return Err(err),
        }
    }
    unreachable!()
}

#[cfg(windows)]
pub fn daemon_running() -> bool {
    open_pipe().is_ok()
}

/// Path of the daemon executable: this exe itself, run with `--daemon`
/// (single-file dual-mode layout).
#[cfg(windows)]
pub fn daemon_exe() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()
}

/// Spawn the daemon if it is not already running. The child is detached
/// from the tray's console so it survives tray exit.
#[cfg(windows)]
pub fn ensure_daemon() -> Result<(), String> {
    if daemon_running() {
        return Ok(());
    }
    let path = daemon_exe()
        .ok_or_else(|| "cannot locate current executable".to_string())?;
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    Command::new(&path)
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map_err(|err| format!("failed to start {}: {err}", path.display()))?;
    log::info!("spawned daemon at {}", path.display());
    // Give the pipe a moment to come up so the caller's first request works.
    for _ in 0..20 {
        if daemon_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn read_frame(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .map_err(|err| format!("pipe closed while reading length: {err}"))?;
    let len = u32::from_le_bytes(len_buf);
    if len as usize > MAX_FRAME {
        return Err(format!("oversized response frame: {len} bytes"));
    }
    let mut body = vec![0u8; len as usize];
    reader
        .read_exact(&mut body)
        .map_err(|err| format!("pipe closed while reading body: {err}"))?;
    Ok(body)
}

fn write_frame(writer: &mut impl Write, payload: &[u8]) -> Result<(), String> {
    let len = (payload.len() as u32).to_le_bytes();
    writer
        .write_all(&len)
        .and_then(|_| writer.write_all(payload))
        .and_then(|_| writer.flush())
        .map_err(|err| format!("pipe write failed: {err}"))
}

/// Send one request and wait for the response. Each call opens a fresh
/// pipe connection, matching the daemon's single-request-per-connection
/// handling.
#[cfg(windows)]
pub fn request(req: Request) -> Result<Response, String> {
    let payload = serde_json::to_vec(&req).map_err(|err| format!("encode request: {err}"))?;
    let mut pipe = connect().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            "StretchLock service is not running".to_string()
        } else {
            format!("cannot open service pipe: {err}")
        }
    })?;
    write_frame(&mut pipe, &payload)?;
    let body = read_frame(&mut pipe)?;
    let response: Response =
        serde_json::from_slice(&body).map_err(|err| format!("decode response: {err}"))?;
    log::debug!("ipc response for {}: {response:?}", request_kind(&req));
    Ok(response)
}

#[cfg(windows)]
fn request_kind(req: &Request) -> &'static str {
    match req {
        Request::GetStatus => "GetStatus",
        Request::SetEnabled(_) => "SetEnabled",
        Request::ForceBreak => "ForceBreak",
        Request::UpdateConfig(_) => "UpdateConfig",
        Request::StopService { .. } => "StopService",
        Request::VerifyPassphrase { .. } => "VerifyPassphrase",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stretchlock_core::ipc::{Status, TimerState};
    use stretchlock_core::Config;

    fn frame_round_trip(payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        read_frame(&mut cursor).unwrap()
    }

    #[test]
    fn request_frames_round_trip() {
        let cases = [
            Request::GetStatus,
            Request::SetEnabled(false),
            Request::ForceBreak,
            Request::UpdateConfig(Box::new(Config::default())),
            Request::StopService {
                passphrase: "pw".into(),
            },
        ];
        for req in cases {
            let payload = serde_json::to_vec(&req).unwrap();
            let body = frame_round_trip(&payload);
            let decoded: Request = serde_json::from_slice(&body).unwrap();
            assert_eq!(decoded, req);
        }
    }

    #[test]
    fn status_frame_decodes_as_response() {
        let status = Status {
            enabled: true,
            state: TimerState::Working,
            remaining_seconds: 1499,
            breaks_today: 3,
            streak: 12,
        };
        let payload = serde_json::to_vec(&Response::Status(status)).unwrap();
        let mut framed = Vec::new();
        write_frame(&mut framed, &payload).unwrap();
        let body = read_frame(&mut std::io::Cursor::new(framed)).unwrap();
        let decoded: Response = serde_json::from_slice(&body).unwrap();
        match decoded {
            Response::Status(s) => {
                assert!(s.enabled);
                assert_eq!(s.state, TimerState::Working);
                assert_eq!(s.remaining_seconds, 1499);
                assert_eq!(s.breaks_today, 3);
                assert_eq!(s.streak, 12);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{}").unwrap();
        buf[0] = 0xFF;
        buf[3] = 0xFF;
        let err = read_frame(&mut std::io::Cursor::new(buf)).unwrap_err();
        assert!(err.contains("oversized"));
    }
}
