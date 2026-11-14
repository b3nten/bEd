//! Saved requests (`Collection`): what the panel edits and the workspace
//! keeps. `resolve` turns one into the engine's `Request`, with its query
//! parameters added to the URL and `{{variables}}` filled in.
use crate::{Method, Request};
use serde::{Deserialize, Serialize};

/// A workspace's requests and the variables they share.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Collection {
    pub requests: Vec<SavedRequest>,
    /// `{{name}}` in a request is replaced with the value of the enabled
    /// variable of that name.
    pub variables: Vec<Field>,
}

/// One request as written: unsent, variables and all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedRequest {
    /// Unique in its collection, never reused while it's there.
    pub id: u64,
    pub name: String,
    pub method: Method,
    pub url: String,
    /// Added to the URL's query, encoded.
    pub params: Vec<Field>,
    pub headers: Vec<Field>,
    pub body: String,
}

impl Default for SavedRequest {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            method: Method::Get,
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            body: String::new(),
        }
    }
}

/// A name and value that can be switched off without being deleted: a query
/// parameter, a header or a variable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Field {
    pub enabled: bool,
    pub name: String,
    pub value: String,
}

impl Default for Field {
    fn default() -> Self {
        Self {
            enabled: true,
            name: String::new(),
            value: String::new(),
        }
    }
}

impl Field {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            name: name.into(),
            value: value.into(),
        }
    }

    /// Enabled and named: it takes part in a request.
    fn active(&self) -> bool {
        self.enabled && !self.name.trim().is_empty()
    }
}

impl Collection {
    /// Add `request` at the end with a fresh id, which is returned.
    pub fn add(&mut self, mut request: SavedRequest) -> u64 {
        request.id = self.next_id();
        let id = request.id;
        self.requests.push(request);
        id
    }

    /// A copy of request `id`, just after it, named as a copy.
    pub fn duplicate(&mut self, id: u64) -> Option<u64> {
        let index = self.index(id)?;
        let mut copy = self.requests[index].clone();
        copy.id = self.next_id();
        copy.name = format!("{} copy", self.requests[index].label());
        let new = copy.id;
        self.requests.insert(index + 1, copy);
        Some(new)
    }

    /// Remove request `id`. Returns the one now in its place (the next, else
    /// the previous), to select instead.
    pub fn remove(&mut self, id: u64) -> Option<u64> {
        let index = self.index(id)?;
        self.requests.remove(index);
        self.requests
            .get(index)
            .or_else(|| index.checked_sub(1).and_then(|i| self.requests.get(i)))
            .map(|request| request.id)
    }

    pub fn get(&self, id: u64) -> Option<&SavedRequest> {
        self.requests.iter().find(|request| request.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut SavedRequest> {
        self.requests.iter_mut().find(|request| request.id == id)
    }

    /// Request `id` ready to send, or why it can't be.
    pub fn resolve(&self, id: u64) -> Result<Request, String> {
        self.get(id)
            .ok_or_else(|| "The request was deleted".to_owned())?
            .resolve(&self.variables)
    }

    fn index(&self, id: u64) -> Option<usize> {
        self.requests.iter().position(|request| request.id == id)
    }

    fn next_id(&self) -> u64 {
        self.requests.iter().map(|r| r.id).max().unwrap_or(0) + 1
    }
}

impl SavedRequest {
    /// What to call it: its name, else its URL, else "Untitled".
    pub fn label(&self) -> &str {
        [self.name.trim(), self.url.trim()]
            .into_iter()
            .find(|text| !text.is_empty())
            .unwrap_or("Untitled")
    }

    /// The request to send: `variables` filled in everywhere, enabled params
    /// added to the URL's query, enabled headers in order. An unknown
    /// variable is an error rather than sent as written.
    pub fn resolve(&self, variables: &[Field]) -> Result<Request, String> {
        let fill = |text: &str| substitute(text, variables);
        let mut params = Vec::new();
        for param in self.params.iter().filter(|p| p.active()) {
            params.push((fill(&param.name)?, fill(&param.value)?));
        }
        let mut headers = Vec::new();
        for header in self.headers.iter().filter(|h| h.active()) {
            headers.push((fill(&header.name)?, fill(&header.value)?));
        }
        Ok(Request {
            method: self.method,
            url: with_query(fill(&self.url)?.trim(), &params),
            headers,
            body: fill(&self.body)?.into_bytes(),
        })
    }
}

/// `text` with each `{{name}}` replaced by the value of the enabled variable
/// called `name` (spaces inside the braces are ignored; the last of several
/// with one name wins). Values are used as written, not filled in again.
pub fn substitute(text: &str, variables: &[Field]) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let Some(length) = rest[start + 2..].find("}}") else {
            break;
        };
        let name = rest[start + 2..start + 2 + length].trim();
        let value = variables
            .iter()
            .rev()
            .find(|v| v.enabled && v.name.trim() == name)
            .ok_or_else(|| format!("Unknown variable {{{{{name}}}}}"))?;
        out.push_str(&rest[..start]);
        out.push_str(&value.value);
        rest = &rest[start + 2 + length + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `url` with `params` added to its query (before any `#fragment`), each
/// name and value percent-encoded.
pub fn with_query(url: &str, params: &[(String, String)]) -> String {
    if params.is_empty() {
        return url.to_owned();
    }
    let (base, fragment) = match url.find('#') {
        Some(at) => url.split_at(at),
        None => (url, ""),
    };
    let mut out = base.to_owned();
    match base.find('?') {
        None => out.push('?'),
        Some(_) if base.ends_with('?') || base.ends_with('&') => {}
        Some(_) => out.push('&'),
    }
    for (i, (name, value)) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        encode(&mut out, name);
        out.push('=');
        encode(&mut out, value);
    }
    out.push_str(fragment);
    out
}

/// Percent-encode everything but RFC 3986's unreserved characters.
fn encode(out: &mut String, text: &str) {
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
}
