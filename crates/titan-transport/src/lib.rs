//! Bounded control/media records for the plain local v2 transport.
use anyhow::{Result, bail, ensure};
use rand::RngCore;
use std::path::PathBuf;
use titan_protocol::{CONTROL_LIMIT, Control};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}
pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        })
        .join("titancam")
}
fn json_depth(v: &serde_json::Value, n: usize) -> bool {
    if n > 16 {
        return false;
    }
    match v {
        serde_json::Value::Array(a) => a.len() <= 256 && a.iter().all(|v| json_depth(v, n + 1)),
        serde_json::Value::Object(o) => o.len() <= 256 && o.values().all(|v| json_depth(v, n + 1)),
        _ => true,
    }
}
pub async fn read_control<R: AsyncRead + Unpin>(r: &mut R) -> Result<Control> {
    let n = r.read_u32().await? as usize;
    ensure!(n > 0 && n <= CONTROL_LIMIT, "control record exceeds limit");
    let mut b = vec![0; n];
    r.read_exact(&mut b).await?;
    let c: Control = serde_json::from_slice(&b)?;
    ensure!(
        c.version == 1 && c.kind.len() <= 64 && c.session_id.len() <= 32 && json_depth(&c.body, 0),
        "invalid control envelope"
    );
    Ok(c)
}
pub async fn write_control<W: AsyncWrite + Unpin>(w: &mut W, c: &Control) -> Result<()> {
    let b = serde_json::to_vec(c)?;
    ensure!(b.len() <= CONTROL_LIMIT, "control record exceeds limit");
    w.write_u32(b.len() as u32).await?;
    w.write_all(&b).await?;
    w.flush().await?;
    Ok(())
}
pub async fn read_media<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<u8>> {
    let n = r.read_u32().await? as usize;
    if !(65..=titan_protocol::VIDEO_LIMIT + 64).contains(&n) {
        bail!("invalid media record length")
    }
    let mut h = [0; 64];
    r.read_exact(&mut h).await?;
    let header = titan_protocol::MediaHeader::parse_with_payload(&h, n - 64)?;
    ensure!(
        header.count == 1
            && header.index == 0
            && header.offset == 0
            && header.unit_len as usize == n - 64,
        "invalid USB media framing"
    );
    let mut b = Vec::with_capacity(n);
    b.extend(h);
    b.resize(n, 0);
    r.read_exact(&mut b[64..]).await?;
    Ok(b)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn partial_records_and_hostile_length() {
        let (mut a, mut b) = tokio::io::duplex(128);
        let task = tokio::spawn(async move {
            a.write_all(&[0, 0]).await.unwrap();
            a.write_all(&[0, 1, b'{']).await.unwrap()
        });
        assert!(read_control(&mut b).await.is_err());
        task.await.unwrap();
        let (mut a, mut b) = tokio::io::duplex(16);
        a.write_u32(u32::MAX).await.unwrap();
        assert!(read_control(&mut b).await.is_err());
    }
}
