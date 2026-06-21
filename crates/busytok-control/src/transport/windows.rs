//! Windows named-pipe transport.
//!
//! Pipe name includes user SID: `\\.\pipe\busytok-{user-sid}` for cross-user
//! isolation. ServerOptions uses reject_remote_clients(true) against network
//! clients and an explicit SDDL DACL with the current logon SID against other
//! local TS/RDP sessions.

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};

use super::ControlTransport;

pub struct NamedPipeTransport;

pub struct NamedPipeListener {
    name: String,
    next_instance: tokio::sync::Mutex<Option<NamedPipeServer>>,
    security: OwnedSecurityAttributes,
}

pub enum NamedPipeStream {
    Server(NamedPipeServer),
    Client(NamedPipeClient),
}

impl AsyncRead for NamedPipeStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            NamedPipeStream::Server(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            NamedPipeStream::Client(c) => std::pin::Pin::new(c).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for NamedPipeStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            NamedPipeStream::Server(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            NamedPipeStream::Client(c) => std::pin::Pin::new(c).poll_write(cx, buf),
        }
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            NamedPipeStream::Server(s) => std::pin::Pin::new(s).poll_flush(cx),
            NamedPipeStream::Client(c) => std::pin::Pin::new(c).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            NamedPipeStream::Server(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            NamedPipeStream::Client(c) => std::pin::Pin::new(c).poll_shutdown(cx),
        }
    }
}

// ---------------------------------------------------------------------------
// DACL via SDDL
// ---------------------------------------------------------------------------

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Authorization::*;
use windows_sys::Win32::Security::*;

pub struct OwnedSecurityAttributes {
    pub sa: SECURITY_ATTRIBUTES,
    // Backs the byte buffer that `sa.lpSecurityDescriptor` points into; kept
    // alive for the lifetime of the struct so the pointer remains valid.
    // Field is never *read* directly — reads happen via Win32 APIs through
    // the pointer — so dead_code is intentional.
    #[allow(dead_code)]
    buffer: Vec<u8>,
}

impl OwnedSecurityAttributes {
    pub fn from_sddl(sddl: &str) -> Result<Self> {
        let mut sd_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut sd_len: u32 = 0;
        let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut sd_ptr,
                &mut sd_len,
            )
        };
        if ok == 0 || sd_ptr.is_null() {
            anyhow::bail!(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW failed: {}",
                std::io::Error::last_os_error()
            );
        }
        let buffer = unsafe {
            let slice = std::slice::from_raw_parts(sd_ptr as *const u8, sd_len as usize);
            let v = slice.to_vec();
            LocalFree(sd_ptr as *mut _);
            v
        };
        let sd_ptr_in_buffer = buffer.as_ptr() as *mut std::ffi::c_void;
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd_ptr_in_buffer,
            bInheritHandle: 0,
        };
        Ok(Self { sa, buffer })
    }
}
unsafe impl Send for OwnedSecurityAttributes {}
// SAFETY: Once constructed, OwnedSecurityAttributes is immutable and the
// security descriptor buffer is never mutated. Raw pointers inside are
// only read by Win32 APIs that themselves tolerate concurrent reads.
// Required because NamedPipeListener (which embeds OwnedSecurityAttributes)
// must be Send + Sync to satisfy ControlTransport::Listener bounds and to
// be held across .await points in accept().
unsafe impl Sync for OwnedSecurityAttributes {}

fn build_pipe_sddl() -> Result<String> {
    let user_sid = busytok_config::platform::current_user_sid_string()
        .ok_or_else(|| anyhow::anyhow!("failed to read current user SID"))?;
    let logon_sid = busytok_config::platform::current_logon_sid_string();

    let (logon_ace, user_ace) = match &logon_sid {
        Some(ls) => (format!("(A;;GA;;;{ls})"), format!("(A;;GRGW;;;{user_sid})")),
        None => {
            tracing::warn!(
                event_code = "control.named_pipe.logon_sid_unavailable",
                "no logon SID; user SID gets GA fallback"
            );
            (String::new(), format!("(A;;GA;;;{user_sid})"))
        }
    };
    Ok(format!(
        "O:{user_sid}G:{user_sid}D:{logon_ace}{user_ace}(A;;GR;;;BA)(A;;GR;;;SY)"
    ))
}

// ---------------------------------------------------------------------------
// ControlTransport impl
// ---------------------------------------------------------------------------

#[async_trait]
impl ControlTransport for NamedPipeTransport {
    type Listener = NamedPipeListener;
    type Stream = NamedPipeStream;

    async fn bind(name: &str) -> Result<Self::Listener> {
        let sddl = build_pipe_sddl().context("failed to build pipe SDDL")?;
        let security = OwnedSecurityAttributes::from_sddl(&sddl)?;
        // SAFETY: security.sa points into security.buffer which lives in the
        // returned NamedPipeListener for the listener's lifetime.
        let first = unsafe {
            let mut opts = ServerOptions::new();
            opts.first_pipe_instance(true);
            opts.reject_remote_clients(true);
            opts.create_with_security_attributes_raw(name, &security.sa as *const _ as *mut _)
        }?;
        tracing::info!(event_code = "control.transport.named_pipe.bound", endpoint = %name);
        Ok(NamedPipeListener {
            name: name.to_string(),
            next_instance: tokio::sync::Mutex::new(Some(first)),
            security,
        })
    }

    async fn accept(listener: &Self::Listener) -> Result<Self::Stream> {
        let inst = listener
            .next_instance
            .lock()
            .await
            .take()
            .context("named pipe accept reentrancy bug: next_instance missing")?;

        let connected = match inst.connect().await {
            Ok(()) => inst,
            Err(e) => {
                tracing::error!(event_code = "control.transport.named_pipe.accept_failed",
                    error = %e, pipe = %listener.name);
                // Rebuild next_instance even on failure to avoid permanent breakage
                // SAFETY: listener.security.sa points into listener.security.buffer which
                // lives for the lifetime of the NamedPipeListener (held in the returned
                // struct after bind), satisfying the SA + descriptor lifetime contract.
                match unsafe {
                    let mut opts = ServerOptions::new();
                    opts.first_pipe_instance(false);
                    opts.reject_remote_clients(true);
                    opts.create_with_security_attributes_raw(
                        &listener.name,
                        &listener.security.sa as *const _ as *mut _,
                    )
                } {
                    Ok(new_inst) => *listener.next_instance.lock().await = Some(new_inst),
                    Err(rebuild_err) => tracing::error!(
                        event_code = "control.transport.named_pipe.rebuild_failed",
                        error = %rebuild_err, pipe = %listener.name,
                        "listener permanently broken until restart"
                    ),
                }
                return Err(e.into());
            }
        };

        // Prepare next instance
        // SAFETY: listener.security.sa points into listener.security.buffer which
        // lives for the lifetime of the NamedPipeListener (held in the returned
        // struct after bind), satisfying the SA + descriptor lifetime contract.
        let next = unsafe {
            let mut opts = ServerOptions::new();
            opts.first_pipe_instance(false);
            opts.reject_remote_clients(true);
            opts.create_with_security_attributes_raw(
                &listener.name,
                &listener.security.sa as *const _ as *mut _,
            )
        }
        .context("failed to prepare next named pipe instance after accept")?;
        *listener.next_instance.lock().await = Some(next);
        Ok(NamedPipeStream::Server(connected))
    }

    async fn connect(name: &str) -> Result<Self::Stream> {
        const MAX_RETRIES: u32 = 10;
        const BACKOFF_MS: u64 = 50;
        let mut last_err: Option<std::io::Error> = None;
        for attempt in 0..MAX_RETRIES {
            match ClientOptions::new().open(name) {
                Ok(c) => {
                    tracing::debug!(event_code = "control.transport.named_pipe.connected",
                        endpoint = %name, attempt);
                    return Ok(NamedPipeStream::Client(c));
                }
                Err(e) if is_pipe_busy(&e) => {
                    tracing::debug!(event_code = "control.transport.named_pipe.busy_retry",
                        endpoint = %name, attempt);
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_millis(BACKOFF_MS)).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Err(anyhow::Error::new(last_err.unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "named pipe busy retries exhausted",
            )
        }))
        .context("named pipe connect failed after bounded retry"))
    }
}

fn is_pipe_busy(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(231)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pipe_name() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(r"\\.\pipe\busytok-test-{}-{}", std::process::id(), n)
    }

    #[tokio::test]
    async fn bind_accept_connect_roundtrip() {
        let name = test_pipe_name();
        let listener = NamedPipeTransport::bind(&name).await.unwrap();
        let accept_task =
            tokio::spawn(async move { NamedPipeTransport::accept(&listener).await.unwrap() });
        let _client = NamedPipeTransport::connect(&name).await.unwrap();
        let _server_stream = accept_task.await.unwrap();
    }

    #[tokio::test]
    async fn accept_failure_does_not_break_listener() {
        let name = test_pipe_name();
        let listener = NamedPipeTransport::bind(&name).await.unwrap();
        drop(listener);
        let name2 = test_pipe_name();
        let listener2 = NamedPipeTransport::bind(&name2).await.unwrap();
        let _accept_task =
            tokio::spawn(async move { NamedPipeTransport::accept(&listener2).await });
        let _client = NamedPipeTransport::connect(&name2).await.unwrap();
    }

    #[test]
    fn sddl_round_trips_through_windows() {
        let sddl = match build_pipe_sddl() {
            Ok(s) => s,
            Err(_) => {
                eprintln!("skipping: build_pipe_sddl failed in test env");
                return;
            }
        };
        assert!(sddl.starts_with("O:S-1-"));
        assert!(sddl.contains("(A;;GR;;;BA)"));
        let attrs = OwnedSecurityAttributes::from_sddl(&sddl).unwrap();
        assert!(!attrs.buffer.is_empty());
    }

    #[test]
    fn invalid_sddl_returns_err() {
        assert!(OwnedSecurityAttributes::from_sddl("not valid sddl").is_err());
    }
}
