//! Shared helpers of the bridge and its command-line tools.

use std::fs;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};

/// The only port: what the patched app connects to.
pub const DEFAULT_PORT: u16 = 7892;

/// A line longer than this is not UCI; the connection is dropped instead of buffering without end.
pub const MAX_LINE: u64 = 1 << 20;

/// `TCP_PORT` from the environment, else 7892.
pub fn port() -> u16 {
    std::env::var("TCP_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Where `token.txt` lives: `TOKEN_FILE`, else the current folder.
pub fn token_path() -> PathBuf {
    std::env::var_os("TOKEN_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new("token.txt").to_path_buf())
}

pub fn read_token(path: &Path) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_string())
}

/// Reads one line (without `\r\n`), at most `MAX_LINE` bytes. `Ok(None)` is end of stream.
pub fn read_line<R: BufRead>(reader: &mut R) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let n = reader.by_ref().take(MAX_LINE).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && n as u64 >= MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}
