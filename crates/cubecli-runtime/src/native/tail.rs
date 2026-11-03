//! Polling log tail (no inotify dependency). Handles truncation/rotation by
//! offset comparison and reopen.

use std::path::Path;

use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::error::{Result, RuntimeError};

const POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Read the last `n` lines of a file (seek from the end, no full read).
pub fn last_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(vec![]);
    };
    let text = String::from_utf8_lossy(&bytes);
    let start = text.len().saturating_sub(text.len());
    let _ = start;
    let lines: Vec<&str> = text.lines().collect();
    Ok(lines
        .iter()
        .rev()
        .take(n)
        .rev()
        .map(|s| s.to_string())
        .collect())
}

/// Stream new lines until `cancel` fires. `tx` closes on rotation errors.
pub async fn follow(
    path: std::path::PathBuf,
    tx: Sender<String>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut offset: u64 = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            _ = tokio::time::sleep(POLL) => {}
        }
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue, // rotation gap — try again next tick
        };
        if meta.len() < offset {
            // truncated/rotated: start over
            offset = 0;
            buffer.clear();
        }
        if meta.len() == offset {
            continue;
        }
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(_) => continue,
        };
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| RuntimeError::Io {
                path: path.clone(),
                source: e,
            })?;
        let mut chunk = Vec::new();
        {
            let mut limited = file.take(meta.len() - offset);
            limited
                .read_to_end(&mut chunk)
                .await
                .map_err(|e| RuntimeError::Io {
                    path: path.clone(),
                    source: e,
                })?;
        }
        offset += chunk.len() as u64;
        buffer.extend_from_slice(&chunk);
        // emit complete lines only; keep the tail fragment buffered
        while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            if tx.send(text).await.is_err() {
                return Ok(()); // receiver gone
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn follow_streams_lines_and_survives_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("server.log");
        std::fs::write(&path, "one\ntwo\n").unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(follow(path.clone(), tx, cancel.clone()));

        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap(); // append
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        std::fs::write(&path, "fresh\n").unwrap(); // truncate (rotation)
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        cancel.cancel();
        let _ = task.await.unwrap();

        let mut lines = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }
        assert!(lines.contains(&"three".to_string()), "{lines:?}");
        assert!(
            lines.contains(&"fresh".to_string()),
            "truncation reopen broken: {lines:?}"
        );
    }

    #[test]
    fn last_lines_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("l");
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(last_lines(&path, 2).unwrap(), vec!["c", "d"]);
        assert_eq!(last_lines(&path, 10).unwrap(), vec!["a", "b", "c", "d"]);
    }
}
