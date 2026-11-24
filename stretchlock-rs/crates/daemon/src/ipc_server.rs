//! Named-pipe IPC server: `\\.\pipe\stretchlock`.
//!
//! Protocol: length-prefixed (u32 little-endian) serde_json frames of
//! `stretchlock_core::ipc::Request` / `Response`. A single listener thread
//! accepts connections; the tray client count is one, so connections are
//! served sequentially on their own connection thread.

#![cfg(windows)]

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::sync::Arc;

use stretchlock_core::ipc::{Request, Response};

use crate::app::RequestHandler;

const PIPE_PATH: windows::core::PCWSTR = windows::core::w!(r"\\.\pipe\stretchlock");
const PIPE_BUFFER_SIZE: u32 = 4096;

pub fn spawn(handler: Arc<dyn RequestHandler>) {
    if let Err(err) = std::thread::Builder::new()
        .name("stretchlock-ipc".into())
        .spawn(move || listener_loop(handler))
    {
        log::warn!("Failed to spawn IPC listener thread: {}", err);
    }
}

fn create_pipe() -> windows::Win32::Foundation::HANDLE {
    unsafe {
        windows::Win32::System::Pipes::CreateNamedPipeW(
            PIPE_PATH,
            windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX,
            windows::Win32::System::Pipes::PIPE_TYPE_BYTE
                | windows::Win32::System::Pipes::PIPE_READMODE_BYTE
                | windows::Win32::System::Pipes::PIPE_WAIT
                | windows::Win32::System::Pipes::PIPE_REJECT_REMOTE_CLIENTS,
            windows::Win32::System::Pipes::PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            None,
        )
    }
}

/// Accept loop. A fresh instance is created *before* the previous one is
/// served, so the tray never sees the pipe missing between requests.
fn listener_loop(handler: Arc<dyn RequestHandler>) {
    let mut pending = create_pipe();
    loop {
        if pending.is_invalid() {
            log::warn!(
                "CreateNamedPipeW failed ({}); retrying in 1s",
                io::Error::last_os_error()
            );
            std::thread::sleep(std::time::Duration::from_secs(1));
            pending = create_pipe();
            continue;
        }
        let connected = unsafe { windows::Win32::System::Pipes::ConnectNamedPipe(pending, None) };
        if connected.is_err() {
            let err = io::Error::last_os_error();
            let code = err.raw_os_error();
            let already = code == Some(windows::Win32::Foundation::ERROR_PIPE_CONNECTED.0 as i32);
            let aborted =
                code == Some(windows::Win32::Foundation::ERROR_OPERATION_ABORTED.0 as i32);
            if !already {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(pending);
                }
                if aborted {
                    log::info!("IPC listener aborted");
                    return;
                }
                log::warn!("ConnectNamedPipe failed: {}", err);
                pending = create_pipe();
                continue;
            }
        }
        // Keep a spare instance listening while this connection is served.
        let spare = create_pipe();
        serve_connection(pending, handler.clone());
        pending = spare;
    }
}

fn serve_connection(handle: windows::Win32::Foundation::HANDLE, handler: Arc<dyn RequestHandler>) {
    // SAFETY: `handle` is a valid, connected named-pipe handle owned by this
    // function; it is consumed exactly once here.
    let mut pipe = unsafe { PipeStream::from_raw_handle(handle.0 as RawHandle) };
    let exit = loop {
        match read_frame(&mut pipe) {
            Ok(Some(payload)) => {
                let response = match serde_json::from_slice::<Request>(&payload) {
                    Ok(request) => {
                        let (response, exit) = handler.handle(request);
                        if exit {
                            break Some(response);
                        }
                        response
                    }
                    Err(err) => {
                        log::warn!("Malformed IPC request: {}", err);
                        Response::Err(format!("bad request: {}", err))
                    }
                };
                let reply = match serde_json::to_vec(&response) {
                    Ok(json) => {
                        let mut framed = (json.len() as u32).to_le_bytes().to_vec();
                        framed.extend_from_slice(&json);
                        framed
                    }
                    Err(err) => {
                        log::warn!("Failed to serialize IPC response: {}", err);
                        continue;
                    }
                };
                if let Err(err) = write_all(&mut pipe, &reply) {
                    log::warn!("IPC write failed: {}", err);
                    break None;
                }
            }
            Ok(None) => {
                log::debug!("IPC client disconnected");
                break None;
            }
            Err(err) => {
                log::warn!("IPC read failed: {}", err);
                break None;
            }
        }
    };
    if let Some(response) = exit {
        // Deliver the final response before tearing anything down.
        if let Ok(json) = serde_json::to_vec(&response) {
            let mut framed = (json.len() as u32).to_le_bytes().to_vec();
            framed.extend_from_slice(&json);
            let _ = write_all(&mut pipe, &framed);
        }
        let _ = pipe.flush();
        handler.shutdown();
        std::process::exit(0);
    }
}

struct PipeStream {
    inner: std::fs::File,
}

impl PipeStream {
    unsafe fn from_raw_handle(handle: RawHandle) -> PipeStream {
        PipeStream {
            inner: std::fs::File::from_raw_handle(handle),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        unsafe {
            windows::Win32::Storage::FileSystem::FlushFileBuffers(
                windows::Win32::Foundation::HANDLE(self.inner.as_raw_handle() as _),
            )
        }
        .map_err(|err| io::Error::other(err))
    }
}

/// Read one length-prefixed frame; `Ok(None)` on clean EOF.
fn read_frame(pipe: &mut PipeStream) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0u8; 4];
    let mut filled = 0;
    while filled < 4 {
        let read = read_once(pipe, &mut header[filled..])?;
        if read == 0 {
            if filled == 0 {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated frame header",
            ));
        }
        filled += read;
    }
    let len = u32::from_le_bytes(header) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let read = read_once(pipe, &mut payload[filled..])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated frame body",
            ));
        }
        filled += read;
    }
    Ok(Some(payload))
}

fn read_once(pipe: &mut PipeStream, buf: &mut [u8]) -> io::Result<usize> {
    use std::io::Read;
    pipe.inner.read(buf)
}

fn write_all(pipe: &mut PipeStream, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    pipe.inner.write_all(data)
}
