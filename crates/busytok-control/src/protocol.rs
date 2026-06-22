//! Shared length-prefixed JSON frame helpers for control IPC.

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const HELLO: &str = "busytok-hello";
pub const HELLO_ACK: &str = "busytok-ok";

const MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

pub async fn read_frame<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> Result<String> {
    let mut len_buf = [0u8; 4];
    reader
        .read_exact(&mut len_buf)
        .await
        .context("reading frame length")?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        anyhow::bail!("frame too large: {len} bytes");
    }
    buf.resize(len, 0);
    reader.read_exact(buf).await.context("reading frame body")?;
    String::from_utf8(buf.clone()).context("frame is not valid UTF-8")
}

pub async fn write_frame<W: AsyncWriteExt + Unpin>(writer: &mut W, payload: &str) -> Result<()> {
    let len = payload.len() as u32;
    writer
        .write_all(&len.to_be_bytes())
        .await
        .context("writing frame length")?;
    writer
        .write_all(payload.as_bytes())
        .await
        .context("writing frame body")?;
    writer.flush().await.context("flushing frame")?;
    Ok(())
}
