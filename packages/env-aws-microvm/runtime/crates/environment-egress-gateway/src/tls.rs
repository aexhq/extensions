use tokio::io::{AsyncRead, AsyncReadExt as _};

const MAX_CLIENT_HELLO_BYTES: usize = 64 * 1024;
const HANDSHAKE_CONTENT_TYPE: u8 = 22;
const CLIENT_HELLO_TYPE: u8 = 1;
const SERVER_NAME_EXTENSION: u16 = 0;
const ECH_EXTENSION: u16 = 0xfe0d;

/// Reads and validates one complete TLS ClientHello while preserving exact bytes for forwarding.
pub async fn read_client_hello<R: AsyncRead + Unpin>(
    reader: &mut R,
    deadline: tokio::time::Instant,
) -> Result<(Vec<u8>, String), TlsError> {
    let mut raw = Vec::new();
    let mut handshake = Vec::new();
    let wanted = loop {
        let mut header = [0_u8; 5];
        read_exact_timeout(reader, &mut header, deadline).await?;
        if header[0] != HANDSHAKE_CONTENT_TYPE {
            return Err(TlsError::NotClientHello);
        }
        let length = u16::from_be_bytes([header[3], header[4]]) as usize;
        if length == 0 || raw.len() + 5 + length > MAX_CLIENT_HELLO_BYTES {
            return Err(TlsError::TooLarge);
        }
        let mut record = vec![0_u8; length];
        read_exact_timeout(reader, &mut record, deadline).await?;
        raw.extend_from_slice(&header);
        raw.extend_from_slice(&record);
        handshake.extend_from_slice(&record);
        if handshake.len() >= 4 {
            if handshake[0] != CLIENT_HELLO_TYPE {
                return Err(TlsError::NotClientHello);
            }
            let wanted = 4
                + ((handshake[1] as usize) << 16)
                + ((handshake[2] as usize) << 8)
                + handshake[3] as usize;
            if wanted > MAX_CLIENT_HELLO_BYTES {
                return Err(TlsError::TooLarge);
            }
            if handshake.len() >= wanted {
                break wanted;
            }
        }
    };
    let sni = parse_client_hello(&handshake[..wanted])?;
    Ok((raw, sni))
}

async fn read_exact_timeout<R: AsyncRead + Unpin>(
    reader: &mut R,
    bytes: &mut [u8],
    deadline: tokio::time::Instant,
) -> Result<(), TlsError> {
    tokio::time::timeout_at(deadline, reader.read_exact(bytes))
        .await
        .map_err(|_| TlsError::Timeout)?
        .map(|_| ())
        .map_err(|_| TlsError::Truncated)
}

fn parse_client_hello(bytes: &[u8]) -> Result<String, TlsError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.u8()? != CLIENT_HELLO_TYPE {
        return Err(TlsError::NotClientHello);
    }
    let declared = cursor.u24()?;
    if declared != cursor.remaining() {
        return Err(TlsError::Truncated);
    }
    cursor.take(2 + 32)?; // legacy version + random
    let session = cursor.u8()? as usize;
    cursor.take(session)?;
    let ciphers = cursor.u16()? as usize;
    if ciphers == 0 || !ciphers.is_multiple_of(2) {
        return Err(TlsError::Malformed);
    }
    cursor.take(ciphers)?;
    let compression = cursor.u8()? as usize;
    cursor.take(compression)?;
    let extensions_len = cursor.u16()? as usize;
    let extensions = cursor.take(extensions_len)?;
    if cursor.remaining() != 0 {
        return Err(TlsError::Malformed);
    }
    let mut extensions = Cursor::new(extensions);
    let mut sni = None;
    while extensions.remaining() > 0 {
        let kind = extensions.u16()?;
        let data_len = extensions.u16()? as usize;
        let data = extensions.take(data_len)?;
        if kind == ECH_EXTENSION {
            return Err(TlsError::EchUnsupported);
        }
        if kind == SERVER_NAME_EXTENSION {
            if sni.is_some() {
                return Err(TlsError::Malformed);
            }
            sni = Some(parse_sni(data)?);
        }
    }
    sni.ok_or(TlsError::MissingSni)
}

fn parse_sni(bytes: &[u8]) -> Result<String, TlsError> {
    let mut cursor = Cursor::new(bytes);
    let list_len = cursor.u16()? as usize;
    let mut list = Cursor::new(cursor.take(list_len)?);
    if cursor.remaining() != 0 {
        return Err(TlsError::Malformed);
    }
    let mut host = None;
    while list.remaining() > 0 {
        let kind = list.u8()?;
        let len = list.u16()? as usize;
        let value = list.take(len)?;
        if kind == 0 {
            if host.is_some() {
                return Err(TlsError::Malformed);
            }
            let value = std::str::from_utf8(value).map_err(|_| TlsError::Malformed)?;
            host = Some(crate::policy::normalize_host(value)?);
        }
    }
    host.ok_or(TlsError::MissingSni)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], TlsError> {
        let end = self.position.checked_add(len).ok_or(TlsError::Malformed)?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or(TlsError::Truncated)?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, TlsError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, TlsError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u24(&mut self) -> Result<usize, TlsError> {
        let bytes = self.take(3)?;
        Ok(((bytes[0] as usize) << 16) | ((bytes[1] as usize) << 8) | bytes[2] as usize)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("TLS setup timed out")]
    Timeout,
    #[error("TLS ClientHello is truncated")]
    Truncated,
    #[error("TLS ClientHello is too large")]
    TooLarge,
    #[error("first TLS message is not a ClientHello")]
    NotClientHello,
    #[error("TLS ClientHello is malformed")]
    Malformed,
    #[error("TLS ClientHello has no visible SNI")]
    MissingSni,
    #[error("encrypted ClientHello is not supported")]
    EchUnsupported,
    #[error(transparent)]
    Policy(#[from] crate::policy::PolicyError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_hello(host: &str, extension_after_sni: Option<(u16, Vec<u8>)>) -> Vec<u8> {
        let mut sni = Vec::new();
        sni.extend_from_slice(&(host.len() as u16 + 3).to_be_bytes());
        sni.push(0);
        sni.extend_from_slice(&(host.len() as u16).to_be_bytes());
        sni.extend_from_slice(host.as_bytes());
        let mut extensions = Vec::new();
        extensions.extend_from_slice(&SERVER_NAME_EXTENSION.to_be_bytes());
        extensions.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&sni);
        if let Some((kind, value)) = extension_after_sni {
            extensions.extend_from_slice(&kind.to_be_bytes());
            extensions.extend_from_slice(&(value.len() as u16).to_be_bytes());
            extensions.extend_from_slice(&value);
        }
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend_from_slice(&[0; 32]);
        body.push(0);
        body.extend_from_slice(&2_u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(1);
        body.push(0);
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(&extensions);
        let mut handshake = vec![
            CLIENT_HELLO_TYPE,
            ((body.len() >> 16) & 0xff) as u8,
            ((body.len() >> 8) & 0xff) as u8,
            (body.len() & 0xff) as u8,
        ];
        handshake.extend_from_slice(&body);
        handshake
    }

    #[test]
    fn parser_extracts_visible_sni_and_rejects_ech() {
        assert_eq!(
            parse_client_hello(&client_hello("EXAMPLE.com", None)).unwrap(),
            "example.com"
        );
        assert!(matches!(
            parse_client_hello(&client_hello("example.com", Some((ECH_EXTENSION, vec![1])))),
            Err(TlsError::EchUnsupported)
        ));
    }
}
