//! Adapter translated from pinned lsp-framework lsp/uri.{h,cpp}.
//! Its filesystem/path distinction is retained for Location vs LocationLink.
use std::{fmt, io, path::Path};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LspUri {
    scheme: String,
    authority: Option<String>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
impl LspUri {
    pub fn parse(input: &str) -> io::Result<Self> {
        let scheme_end = input
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'+'))
            .count();
        if scheme_end == 0 || input.as_bytes().get(scheme_end) != Some(&b':') {
            return Err(invalid("Invalid URI scheme"));
        }
        let scheme = input[..scheme_end].to_ascii_lowercase();
        let mut rest = &input[scheme_end + 1..];
        let authority = if let Some(after) = rest.strip_prefix("//") {
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            rest = &after[end..];
            if !rest.is_empty() && !rest.starts_with('/') {
                return Err(invalid("URI authority must precede an absolute path"));
            }
            Some(normalize_encoded_case(&after[..end]))
        } else {
            None
        };
        let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
        let path = Self::decode(&rest[..path_end])?;
        rest = &rest[path_end..];
        let query = if let Some(after) = rest.strip_prefix('?') {
            let end = after.find('#').unwrap_or(after.len());
            rest = &after[end..];
            Some(normalize_encoded_case(&after[..end]))
        } else {
            None
        };
        let fragment = rest.strip_prefix('#').map(normalize_encoded_case);
        Ok(Self {
            scheme,
            authority,
            path,
            query,
            fragment,
        })
    }
    pub fn file_uri_from_path(path: &str) -> io::Result<Self> {
        let absolute = std::path::absolute(Path::new(path))?;
        let native = absolute
            .to_str()
            .ok_or_else(|| invalid("Document path is not UTF-8"))?;
        #[cfg(windows)]
        let native = format!("/{native}");
        Ok(Self {
            scheme: "file".into(),
            authority: Some(String::new()),
            path: native.to_owned(),
            query: None,
            fragment: None,
        })
    }
    /// Linux target paths are interpreted on the SSH host, never on this host.
    pub fn file_uri_from_remote_path(path: &str) -> io::Result<Self> {
        if !path.starts_with('/') || path.contains('\0') {
            return Err(invalid(
                "Remote file URI requires an absolute UTF-8 target path",
            ));
        }
        Ok(Self {
            scheme: "file".into(),
            authority: Some(String::new()),
            path: path.to_owned(),
            query: None,
            fragment: None,
        })
    }
    pub fn fs_path(&self) -> String {
        #[cfg(windows)]
        return self.path.strip_prefix('/').unwrap_or(&self.path).to_owned();
        #[cfg(not(windows))]
        self.path.clone()
    }
    pub fn is_valid(&self) -> bool {
        !self.scheme.is_empty()
    }
    pub fn is_file_uri(&self) -> bool {
        self.scheme == "file"
    }
    pub fn has_authority(&self) -> bool {
        self.authority.is_some()
    }
    pub fn has_query(&self) -> bool {
        self.query.is_some()
    }
    pub fn has_fragment(&self) -> bool {
        self.fragment.is_some()
    }
    pub fn scheme(&self) -> &str {
        &self.scheme
    }
    pub fn authority(&self) -> &str {
        self.authority.as_deref().unwrap_or("")
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn query(&self) -> &str {
        self.query.as_deref().unwrap_or("")
    }
    pub fn fragment(&self) -> &str {
        self.fragment.as_deref().unwrap_or("")
    }
    pub fn encode(decoded: &str, exclude: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = String::new();
        for byte in decoded.bytes() {
            if exclude.as_bytes().contains(&byte)
                || byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'.' | b'-')
            {
                encoded.push(byte as char);
            } else {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 15) as usize] as char);
            }
        }
        encoded
    }
    pub fn decode(encoded: &str) -> io::Result<String> {
        let source = encoded.as_bytes();
        let mut decoded = Vec::new();
        let mut index = 0;
        while index < source.len() {
            if source[index] == b'%' && index + 2 < source.len() {
                let hex = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                let high =
                    hex(source[index + 1]).ok_or_else(|| invalid("Invalid percent encoding"))?;
                let low =
                    hex(source[index + 2]).ok_or_else(|| invalid("Invalid percent encoding"))?;
                decoded.push(high * 16 + low);
                index += 3;
            } else {
                decoded.push(source[index]);
                index += 1;
            }
        }
        String::from_utf8(decoded).map_err(|_| invalid("URI path is not UTF-8"))
    }
}
impl fmt::Display for LspUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.is_valid() {
            return Ok(());
        }
        write!(formatter, "{}:", self.scheme)?;
        if let Some(authority) = &self.authority {
            write!(formatter, "//{authority}")?;
        }
        write!(formatter, "{}", Self::encode(&self.path, "/"))?;
        if let Some(query) = &self.query {
            write!(formatter, "?{query}")?;
        }
        if let Some(fragment) = &self.fragment {
            write!(formatter, "#{fragment}")?;
        }
        Ok(())
    }
}
fn normalize_encoded_case(input: &str) -> String {
    let mut result = input.as_bytes().to_vec();
    let mut index = 0;
    while index + 2 < result.len() {
        if result[index] == b'%' {
            result[index + 1].make_ascii_uppercase();
            result[index + 2].make_ascii_uppercase();
            index += 3;
        } else {
            index += 1;
        }
    }
    String::from_utf8(result).expect("ASCII case normalization preserves UTF-8")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_byte_encoding_and_decoded_path_roundtrip() {
        let uri = LspUri::parse("FILE:///tmp/a%20b/%c3%a9%F0%9F%99%82~.rs?q=%ab#%cd").unwrap();
        assert_eq!(uri.path(), "/tmp/a b/é🙂~.rs");
        assert_eq!(uri.scheme(), "file");
        assert_eq!(
            uri.to_string(),
            "file:///tmp/a%20b/%C3%A9%F0%9F%99%82%7E.rs?q=%AB#%CD"
        );
        assert_eq!(LspUri::decode("x%A").unwrap(), "x%A");
        assert!(LspUri::decode("%FF").is_err());
        assert!(LspUri::parse("file:///bad%zz").is_err());
    }
    #[test]
    fn file_paths_become_absolute_and_authority_is_not_a_filesystem_prefix() {
        let uri = LspUri::file_uri_from_path("Cargo.toml").unwrap();
        assert!(uri.is_file_uri());
        assert!(uri.has_authority());
        assert!(Path::new(&uri.fs_path()).is_absolute());
        let remote = LspUri::parse("file://host/tmp/source.rs").unwrap();
        assert_eq!(remote.authority(), "host");
        assert_eq!(remote.path(), "/tmp/source.rs");
        assert!(LspUri::parse("file://host?query").is_err());
    }
    #[cfg(windows)]
    #[test]
    fn windows_fs_path_differs_from_link_path() {
        let uri = LspUri::parse("file:///C%3A/project/a.rs").unwrap();
        assert_eq!(uri.path(), "/C:/project/a.rs");
        assert_eq!(uri.fs_path(), "C:/project/a.rs");
    }
}
