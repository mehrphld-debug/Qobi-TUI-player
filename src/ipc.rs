use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::QobiError;

/// Newline-delimited JSON protocol between a secondary `qobi` invocation
/// and the primary instance (Chunk-Map §0, decision D2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum IpcMessage {
    Ping,
    EnqueueFiles { paths: Vec<PathBuf> },
    EnqueueDir { path: PathBuf },
}

/// Outcome of probing the instance socket.
pub enum Role {
    /// This process owns the socket and must serve it.
    Primary(tokio::net::UnixListener),
    /// Another live instance owns the socket; caller must [`send_message`].
    Secondary,
}

/// Base state directory: `~/.config/qobi`.
pub fn qobi_dir() -> Result<PathBuf, QobiError> {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".config").join("qobi"))
        .ok_or_else(|| QobiError::Ipc("$HOME is not set".to_string()))
}

pub fn socket_path() -> Result<PathBuf, QobiError> {
    qobi_dir().map(|d| d.join("qobi.sock"))
}

/// Bind the socket or detect a live primary.
/// A leftover socket file with nobody listening is reclaimed (stale takeover).
pub async fn resolve_role(sock: &Path) -> Result<Role, QobiError> {
    if let Some(parent) = sock.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    match tokio::net::UnixListener::bind(sock) {
        Ok(listener) => Ok(Role::Primary(listener)),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if is_live(sock).await {
                Ok(Role::Secondary)
            } else {
                // Stale file: nobody listening. Reclaim and retry once.
                let _ = tokio::fs::remove_file(sock).await;
                tokio::net::UnixListener::bind(sock)
                    .map(Role::Primary)
                    .map_err(QobiError::Io)
            }
        }
        Err(e) => Err(QobiError::Io(e)),
    }
}

/// True when something answers on the socket. Sends a `Ping` (rather than a
/// bare connect) so the primary's accept loop never sees a spurious EOF.
async fn is_live(sock: &Path) -> bool {
    let Ok(mut stream) = tokio::net::UnixStream::connect(sock).await else {
        return false;
    };
    stream.write_all(b"{\"op\":\"ping\"}\n").await.is_ok()
}

/// Send one message to the primary instance.
pub async fn send_message(sock: &Path, msg: &IpcMessage) -> Result<(), QobiError> {
    let mut stream = tokio::net::UnixStream::connect(sock)
        .await
        .map_err(|_| QobiError::StaleInstance(sock.to_string_lossy().into_owned()))?;
    let mut line = serde_json::to_string(msg).map_err(|e| QobiError::Ipc(e.to_string()))?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    Ok(())
}

/// Read a single message; `Ok(None)` means a corrupt line (log + continue, never crash).
pub async fn read_message<S>(stream: &mut S) -> Result<Option<IpcMessage>, QobiError>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut line = String::new();
    let n = BufReader::new(stream).read_line(&mut line).await?;
    if n == 0 {
        return Ok(None);
    }
    match serde_json::from_str(line.trim()) {
        Ok(msg) => Ok(Some(msg)),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_sock(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qobi-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        dir.join("qobi.sock")
    }

    #[tokio::test]
    async fn secondary_enqueue_reaches_primary() {
        let sock = tmp_sock("enqueue");
        let Role::Primary(listener) = resolve_role(&sock).await.expect("primary binds") else {
            panic!("expected primary");
        };
        let msg = IpcMessage::EnqueueFiles {
            paths: vec![PathBuf::from("/music/a.mp3")],
        };
        send_message(&sock, &msg).await.expect("send");
        let (stream, _) = listener.accept().await.expect("accept");
        let mut stream = stream;
        let got = read_message(&mut stream).await.expect("read");
        assert_eq!(got, Some(msg));
        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn second_resolver_sees_live_primary() {
        let sock = tmp_sock("live");
        let _primary = resolve_role(&sock).await.expect("primary binds");
        let ends_up_secondary =
            matches!(resolve_role(&sock).await.expect("probe"), Role::Secondary);
        assert!(ends_up_secondary, "live socket must report Secondary");
        drop(_primary);
        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn stale_file_is_reclaimed() {
        let sock = tmp_sock("stale");
        // A plain file squats the socket path: bind fails, connect fails → stale.
        std::fs::write(&sock, b"squat").expect("squat file");
        let reclaimed = matches!(
            resolve_role(&sock).await.expect("reclaim"),
            Role::Primary(_)
        );
        assert!(reclaimed, "stale file must be reclaimed as Primary");
        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn corrupt_line_yields_none_not_error() {
        let sock = tmp_sock("corrupt");
        let Role::Primary(listener) = resolve_role(&sock).await.expect("primary binds") else {
            panic!("expected primary");
        };
        let mut raw = tokio::net::UnixStream::connect(&sock)
            .await
            .expect("connect");
        raw.write_all(b"{{{not json\n").await.expect("write");
        let (stream, _) = listener.accept().await.expect("accept");
        let mut stream = stream;
        let got = read_message(&mut stream).await.expect("read must not err");
        assert_eq!(got, None);
        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn unreachable_socket_maps_to_stale_error() {
        let sock = tmp_sock("dead");
        let err = send_message(&sock, &IpcMessage::Ping)
            .await
            .expect_err("must fail");
        assert!(matches!(err, QobiError::StaleInstance(_)));
    }
}
