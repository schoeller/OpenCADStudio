//! Length-framed transport over `interprocess::local_socket` streams.

use std::io::{Read, Write};

use serde::{de::DeserializeOwned, Serialize};

/// Maximum serialized message size accepted over the wire (64 MiB). Prevents
/// a malicious or buggy peer from exhausting host/runner memory.
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Errors that can occur during transport.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Encode(#[from] bincode::Error),
    #[error("empty message")]
    Empty,
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
}

/// Send a length-framed serialized message.
pub fn send<T: Serialize, W: Write + ?Sized>(
    stream: &mut W,
    msg: &T,
) -> Result<(), TransportError> {
    let bytes = bincode::serialize(msg)?;
    if bytes.len() > MAX_MESSAGE_SIZE {
        return Err(TransportError::TooLarge(bytes.len()));
    }
    let len = bytes.len() as u64;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

/// Receive a length-framed serialized message.
pub fn recv<T: DeserializeOwned, R: Read + ?Sized>(stream: &mut R) -> Result<T, TransportError> {
    let mut len_buf = [0u8; 8];
    stream.read_exact(&mut len_buf)?;
    let len_u64 = u64::from_le_bytes(len_buf);
    if len_u64 == 0 {
        return Err(TransportError::Empty);
    }
    if len_u64 > MAX_MESSAGE_SIZE as u64 {
        return Err(TransportError::TooLarge(len_u64 as usize));
    }
    let len = match usize::try_from(len_u64) {
        Ok(n) => n,
        Err(_) => return Err(TransportError::TooLarge(usize::MAX)),
    };
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(bincode::deserialize(&buf)?)
}
