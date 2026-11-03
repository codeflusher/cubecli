//! Hand-rolled Source RCON client (the protocol is one page long).
//!
//! Includes the multi-packet workaround for responses over 4096 bytes: a
//! trailing sentinel command marks the true end of the response stream.
//! Used for stop, save-flush, console, and as the status authority —
//! identically for the native and compose backends.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::error::{Result, RuntimeError};

const SERVERDATA_AUTH: i32 = 3;
const SERVERDATA_EXECCOMMAND: i32 = 2;
const SERVERDATA_AUTH_RESPONSE: i32 = 2;
const SERVERDATA_RESPONSE_VALUE: i32 = 0;
const SENTINEL_ID: i32 = 0x0C0FFEE;
const SENTINEL_PAYLOAD: &str = "__cubecli_end__";
const MAX_RESPONSE: usize = 1024 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub struct RconClient {
    stream: TcpStream,
}

impl std::fmt::Debug for RconClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RconClient").finish_non_exhaustive()
    }
}

/// Connect + authenticate. Fails with a clear error when the server isn't
/// listening yet (used as the liveness authority by `status`).
pub async fn connect(addr: std::net::SocketAddr, password: &str) -> Result<RconClient> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|e| RuntimeError::Rcon(format!("cannot reach {addr}: {e}")))?;
    let mut client = RconClient { stream };
    client.authenticate(password).await?;
    Ok(client)
}

impl RconClient {
    async fn authenticate(&mut self, password: &str) -> Result<()> {
        self.send(SERVERDATA_AUTH, 1, password).await?;
        // The server answers with an empty RESPONSE_VALUE, then AUTH_RESPONSE.
        // AUTH_RESPONSE carries our id on success, -1 on bad password.
        let deadline = tokio::time::Instant::now() + READ_TIMEOUT;
        loop {
            let (ptype, id, _payload) = tokio::time::timeout_at(deadline, self.read_packet())
                .await
                .map_err(|_| RuntimeError::Rcon("timeout waiting for auth response".into()))??;
            if ptype == SERVERDATA_AUTH_RESPONSE {
                if id == 1 {
                    return Ok(());
                }
                return Err(RuntimeError::Rcon(
                    "authentication failed (wrong rcon.password?)".into(),
                ));
            }
            // otherwise: the empty SERVERDATA_RESPONSE_VALUE preamble — keep reading
        }
    }

    /// Run a command, returning the concatenated response text.
    ///
    /// The multi-packet workaround reads the FIRST response fragment before
    /// sending the trailing sentinel: vanilla's RCON server closes the
    /// connection when two packets land in the same read, so a
    /// send-command + send-sentinel pipeline corrupts the stream (reproduced
    /// against 26.x — the command even executed, the reply was just lost).
    pub async fn command(&mut self, command: &str) -> Result<String> {
        self.send(SERVERDATA_EXECCOMMAND, 7, command).await?;

        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + READ_TIMEOUT;
        self.next_response(deadline, &mut out).await?;
        self.send(SERVERDATA_EXECCOMMAND, SENTINEL_ID, SENTINEL_PAYLOAD)
            .await?;
        while !self.next_response(deadline, &mut out).await? {}

        Ok(String::from_utf8_lossy(&out)
            .trim_end_matches('\n')
            .to_string())
    }

    /// Read one response packet into `out`; true when it is the sentinel.
    async fn next_response(
        &mut self,
        deadline: tokio::time::Instant,
        out: &mut Vec<u8>,
    ) -> Result<bool> {
        let (ptype, id, payload) = tokio::time::timeout_at(deadline, self.read_packet())
            .await
            .map_err(|_| RuntimeError::Rcon("timeout waiting for response".into()))??;
        if id == SENTINEL_ID {
            return Ok(true);
        }
        if ptype == SERVERDATA_RESPONSE_VALUE {
            if out.len() + payload.len() > MAX_RESPONSE {
                return Err(RuntimeError::Rcon("response exceeds 1 MiB cap".into()));
            }
            out.extend_from_slice(&payload);
        }
        Ok(false)
    }

    async fn send(&mut self, ptype: i32, id: i32, payload: &str) -> Result<()> {
        let length = (4 + 4 + payload.len() + 2) as i32;
        let mut packet = Vec::with_capacity((length as usize) + 4);
        packet.extend_from_slice(&length.to_le_bytes());
        packet.extend_from_slice(&id.to_le_bytes());
        packet.extend_from_slice(&ptype.to_le_bytes());
        packet.extend_from_slice(payload.as_bytes());
        packet.extend_from_slice(&[0, 0]);
        self.stream
            .write_all(&packet)
            .await
            .map_err(|e| RuntimeError::Rcon(format!("write failed: {e}")))?;
        self.stream
            .flush()
            .await
            .map_err(|e| RuntimeError::Rcon(format!("flush failed: {e}")))
    }

    /// Read one packet: length, id, type, payload.
    async fn read_packet(&mut self) -> Result<(i32, i32, Vec<u8>)> {
        let mut length_buf = [0u8; 4];
        self.stream
            .read_exact(&mut length_buf)
            .await
            .map_err(|e| RuntimeError::Rcon(format!("connection closed: {e}")))?;
        let length = i32::from_le_bytes(length_buf);
        if length < 10 || length as usize > MAX_RESPONSE {
            return Err(RuntimeError::Rcon(format!(
                "implausible packet length {length}"
            )));
        }
        let mut rest = vec![0u8; length as usize];
        self.stream
            .read_exact(&mut rest)
            .await
            .map_err(|e| RuntimeError::Rcon(format!("read failed: {e}")))?;
        let id = i32::from_le_bytes(rest[0..4].try_into().unwrap());
        let ptype = i32::from_le_bytes(rest[4..8].try_into().unwrap());
        let payload = rest[8..rest.len() - 2].to_vec();
        Ok((ptype, id, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny fake RCON server speaking the protocol — golden-path byte check.
    #[tokio::test]
    async fn handshake_and_command_roundtrip() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // auth request
            let (ptype, id, payload) = read_one(&mut sock).await;
            assert_eq!((ptype, id), (SERVERDATA_AUTH, 1));
            assert_eq!(payload, b"secret");
            // empty RESPONSE_VALUE then AUTH_RESPONSE
            write_one(&mut sock, SERVERDATA_RESPONSE_VALUE, 1, b"").await;
            write_one(&mut sock, SERVERDATA_AUTH_RESPONSE, 1, b"").await;
            // command
            let (ptype, id, payload) = read_one(&mut sock).await;
            assert_eq!((ptype, id), (SERVERDATA_EXECCOMMAND, 7));
            assert_eq!(payload, b"say hi");
            write_one(&mut sock, SERVERDATA_RESPONSE_VALUE, 7, b" Said hi.").await;
            let (_, sid, spayload) = read_one(&mut sock).await;
            assert_eq!(
                (sid, spayload.as_slice()),
                (SENTINEL_ID, SENTINEL_PAYLOAD.as_bytes())
            );
            write_one(&mut sock, SERVERDATA_RESPONSE_VALUE, SENTINEL_ID, b"").await;
        });

        let mut client = connect(addr, "secret").await.unwrap();
        let reply = client.command("say hi").await.unwrap();
        // vanilla keeps a leading space in RCON responses
        assert_eq!(reply, " Said hi.");
    }

    #[tokio::test]
    async fn bad_password_is_rejected() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_one(&mut sock).await;
            write_one(&mut sock, SERVERDATA_RESPONSE_VALUE, 1, b"").await;
            write_one(&mut sock, SERVERDATA_AUTH_RESPONSE, -1, b"").await;
        });
        let err = connect(addr, "wrong").await.unwrap_err();
        assert!(err.to_string().contains("authentication failed"));
    }

    /// Large answers arrive as several RESPONSE_VALUE fragments, possibly all
    /// before the sentinel is even sent — they must concatenate in order.
    #[tokio::test]
    async fn multi_fragment_responses_concatenate() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = read_one(&mut sock).await;
            write_one(&mut sock, SERVERDATA_AUTH_RESPONSE, 1, b"").await;
            let (_, id, _) = read_one(&mut sock).await;
            assert_eq!(id, 7);
            // both fragments go out before the client sends the sentinel
            write_one(&mut sock, SERVERDATA_RESPONSE_VALUE, 7, b"part1,").await;
            write_one(&mut sock, SERVERDATA_RESPONSE_VALUE, 7, b"part2").await;
            let (_, sid, _) = read_one(&mut sock).await;
            assert_eq!(sid, SENTINEL_ID);
            write_one(&mut sock, SERVERDATA_RESPONSE_VALUE, SENTINEL_ID, b"").await;
        });

        let mut client = connect(addr, "secret").await.unwrap();
        let reply = client.command("big").await.unwrap();
        assert_eq!(reply, "part1,part2");
    }

    async fn read_one(sock: &mut TcpStream) -> (i32, i32, Vec<u8>) {
        let mut len = [0u8; 4];
        sock.read_exact(&mut len).await.unwrap();
        let mut rest = vec![0u8; i32::from_le_bytes(len) as usize];
        sock.read_exact(&mut rest).await.unwrap();
        let id = i32::from_le_bytes(rest[0..4].try_into().unwrap());
        let ptype = i32::from_le_bytes(rest[4..8].try_into().unwrap());
        (ptype, id, rest[8..rest.len() - 2].to_vec())
    }

    async fn write_one(sock: &mut TcpStream, ptype: i32, id: i32, payload: &[u8]) {
        let length = (4 + 4 + payload.len() + 2) as i32;
        sock.write_all(&length.to_le_bytes()).await.unwrap();
        sock.write_all(&id.to_le_bytes()).await.unwrap();
        sock.write_all(&ptype.to_le_bytes()).await.unwrap();
        sock.write_all(payload).await.unwrap();
        sock.write_all(&[0, 0]).await.unwrap();
    }
}
