//! The stdio framing rules of lsp-framework, independent of process ownership.
use std::io;

use crate::jsonrpc::{Packet, ResponseError};
use std::io::{BufRead, Write};

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const CONTENT_TYPE: &str = "application/vscode-jsonrpc; charset=utf-8";

#[derive(Debug)]
pub enum ConnectionError {
    Io(io::Error),
    Framing(String),
    Message(ResponseError),
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Framing(error) => formatter.write_str(error),
            Self::Message(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ConnectionError {}
impl From<io::Error> for ConnectionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub struct Connection<R> {
    reader: R,
}

impl<R: BufRead> Connection<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }

    pub fn read_packet(&mut self) -> Result<Packet, ConnectionError> {
        let mut header_size = 0usize;
        let mut content_length = 0usize;
        let mut content_type = CONTENT_TYPE.to_owned();
        loop {
            let mut line = Vec::new();
            loop {
                let chunk = self.reader.fill_buf()?;
                if chunk.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "LSP header reached EOF",
                    )
                    .into());
                }
                let consumed = chunk
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(chunk.len(), |index| index + 1);
                header_size = header_size
                    .checked_add(consumed)
                    .ok_or_else(|| ConnectionError::Framing("LSP header overflow".into()))?;
                if header_size > MAX_HEADER_BYTES {
                    return Err(ConnectionError::Framing("LSP header exceeds 64 KiB".into()));
                }
                line.extend_from_slice(&chunk[..consumed]);
                let complete = chunk[consumed - 1] == b'\n';
                self.reader.consume(consumed);
                if complete {
                    break;
                }
            }
            if !line.ends_with(b"\r\n") {
                return Err(ConnectionError::Framing(
                    "Expected CRLF in LSP header".into(),
                ));
            }
            if line[..line.len() - 2].contains(&b'\r') {
                return Err(ConnectionError::Framing(
                    "Unexpected CR in LSP header".into(),
                ));
            }
            line.truncate(line.len() - 2);
            if line.is_empty() {
                break;
            }
            let Some(colon) = line.iter().position(|byte| *byte == b':') else {
                continue;
            };
            let field = ascii_trim(&line[..colon]);
            let value = ascii_trim(&line[colon + 1..]);
            if field.eq_ignore_ascii_case(b"Content-Length") {
                if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                    return Err(ConnectionError::Framing("Invalid Content-Length".into()));
                }
                content_length = 0;
                for digit in value {
                    content_length = content_length
                        .checked_mul(10)
                        .and_then(|length| length.checked_add((digit - b'0') as usize))
                        .ok_or_else(|| {
                            ConnectionError::Framing("Content-Length overflow".into())
                        })?;
                }
            } else if field.eq_ignore_ascii_case(b"Content-Type") {
                content_type = std::str::from_utf8(value)
                    .map_err(|_| ConnectionError::Framing("Invalid Content-Type".into()))?
                    .to_owned();
            }
        }
        if content_length > MAX_FRAME_BYTES {
            return Err(ConnectionError::Framing("LSP body exceeds 16 MiB".into()));
        }
        let mut body = vec![0; content_length];
        self.reader.read_exact(&mut body)?;
        // Consume the complete frame before rejecting its content type, as upstream
        // does, so a following valid frame remains aligned.
        if !content_type.starts_with("application/vscode-jsonrpc") {
            return Err(ConnectionError::Framing(
                "Unsupported LSP Content-Type".into(),
            ));
        }
        if let Some(index) = content_type.find("charset=") {
            let charset = content_type[index + "charset=".len()..]
                .split(';')
                .next()
                .unwrap_or_default()
                .trim_matches(|character: char| {
                    character.is_ascii_whitespace() || character == '\u{b}'
                });
            if charset != "utf-8" && charset != "utf8" {
                return Err(ConnectionError::Framing("Unsupported LSP charset".into()));
            }
        }
        Packet::parse(&body).map_err(ConnectionError::Message)
    }
}

fn ascii_trim(mut value: &[u8]) -> &[u8] {
    let whitespace = |byte: &u8| byte.is_ascii_whitespace() || *byte == 11;
    while value.first().is_some_and(whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

pub fn encode_packet(packet: &Packet) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(&packet.value()).map_err(io::Error::other)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "LSP body exceeds 16 MiB",
        ));
    }
    let mut frame = format!(
        "Content-Length: {}\r\nContent-Type: {CONTENT_TYPE}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn write_packet(writer: &mut impl Write, packet: &Packet) -> io::Result<()> {
    writer.write_all(&encode_packet(packet)?)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::{Message, Request};
    use std::io::Cursor;
    #[test]
    fn framing_preserves_utf8_byte_length_and_duplicate_header_semantics() {
        let packet = Packet::Single(Message::Request(Request {
            id: None,
            method: "😀".into(),
            params: None,
        }));
        let bytes = encode_packet(&packet).unwrap();
        assert_eq!(
            Connection::new(Cursor::new(bytes)).read_packet().unwrap(),
            packet
        );
        let body = serde_json::to_vec(&packet.value()).unwrap();
        let mut frame = format!(
            "Ignored\r\nContent-Length: 1\r\ncOnTeNt-LeNgTh : {} \t\r\n\r\n",
            body.len()
        )
        .into_bytes();
        frame.extend(body);
        assert_eq!(
            Connection::new(Cursor::new(frame)).read_packet().unwrap(),
            packet
        );
    }
    #[test]
    fn complete_invalid_frames_leave_next_message_readable() {
        let packet = Packet::Single(Message::Request(Request {
            id: None,
            method: "ok".into(),
            params: None,
        }));
        let mut bytes = b"Content-Length: 2\r\nContent-Type: text/plain\r\n\r\n{}".to_vec();
        bytes.extend(encode_packet(&packet).unwrap());
        let mut connection = Connection::new(Cursor::new(bytes));
        assert!(matches!(
            connection.read_packet(),
            Err(ConnectionError::Framing(_))
        ));
        assert_eq!(connection.read_packet().unwrap(), packet);
    }
    #[test]
    fn rejects_header_body_and_eof_errors() {
        for bytes in [
            b"Content-Length: -1\r\n\r\n".as_slice(),
            b"Content-Length: 2\n\n{}",
            b"Content-Length: 16777217\r\n\r\n",
            b"Content-Length: 999999999999999999999999999999\r\n\r\n",
            b"Content-Length: 2\r\n\r\n{",
        ] {
            assert!(Connection::new(Cursor::new(bytes)).read_packet().is_err());
        }
        assert!(
            Connection::new(Cursor::new(vec![b'x'; MAX_HEADER_BYTES + 1]))
                .read_packet()
                .is_err()
        );
    }
}
