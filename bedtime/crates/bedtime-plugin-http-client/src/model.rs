//! Saved request drafts and their conversion into a request ready to send.
//! Drafts can be incomplete; HTTP validation happens only at the Run boundary.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Request {
    pub id: u64,
    #[serde(default = "new_request_name")]
    pub name: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub params: Vec<KeyValue>,
    #[serde(default)]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub auth: Auth,
}

impl Request {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            name: new_request_name(),
            method: default_method(),
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            body: String::new(),
            auth: Auth::None,
        }
    }
}

fn new_request_name() -> String {
    "New request".into()
}

fn default_method() -> String {
    "GET".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct KeyValue {
    pub enabled: bool,
    pub name: String,
    pub value: String,
}

impl Default for KeyValue {
    fn default() -> Self {
        Self {
            enabled: true,
            name: String::new(),
            value: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Auth {
    #[default]
    None,
    Bearer {
        #[serde(default)]
        token: String,
    },
    Basic {
        #[serde(default)]
        username: String,
        #[serde(default)]
        password: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SavedWorkspace {
    pub version: u32,
    pub requests: Vec<Request>,
    pub selected: Option<u64>,
}

impl Default for SavedWorkspace {
    fn default() -> Self {
        Self {
            version: 1,
            requests: Vec::new(),
            selected: None,
        }
    }
}

/// Validate persistence relationships without rejecting unfinished drafts.
pub(crate) fn load(state: Option<&Value>) -> Result<SavedWorkspace, String> {
    let saved: SavedWorkspace = match state {
        None | Some(Value::Null) => return Ok(SavedWorkspace::default()),
        Some(state) => serde_json::from_value(state.clone())
            .map_err(|error| format!("Could not load saved HTTP requests: {error}"))?,
    };
    if saved.version != 1 {
        return Err(format!(
            "Saved HTTP requests use unsupported version {} (expected 1).",
            saved.version
        ));
    }
    let mut ids = HashSet::with_capacity(saved.requests.len());
    for request in &saved.requests {
        if request.id == 0 {
            return Err("Saved HTTP request IDs must be nonzero.".into());
        }
        if !ids.insert(request.id) {
            return Err(format!(
                "Saved HTTP requests contain duplicate ID {}.",
                request.id
            ));
        }
    }
    if let Some(selected) = saved.selected
        && !ids.contains(&selected)
    {
        return Err(format!(
            "Selected HTTP request {selected} does not exist in the saved workspace."
        ));
    }
    Ok(saved)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub(crate) fn resolve(request: &Request) -> Result<ResolvedRequest, String> {
    reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|_| format!("Invalid HTTP method '{}'.", request.method))?;
    let mut url = reqwest::Url::parse(&request.url)
        .map_err(|error| format!("Invalid request URL: {error}."))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Request URL must be an absolute HTTP or HTTPS URL with a host.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "Request URLs cannot include credentials. Use the auth controls instead.".into(),
        );
    }
    for (index, row) in request.params.iter().enumerate() {
        if !row.enabled || (row.name.is_empty() && row.value.is_empty()) {
            continue;
        }
        if row.name.is_empty() {
            return Err(format!(
                "Query parameter row {} has a value but no name.",
                index + 1
            ));
        }
        url.query_pairs_mut().append_pair(&row.name, &row.value);
    }

    let mut headers = Vec::with_capacity(request.headers.len() + 1);
    for (index, row) in request.headers.iter().enumerate() {
        if !row.enabled || (row.name.is_empty() && row.value.is_empty()) {
            continue;
        }
        if row.name.is_empty() {
            return Err(format!("Header row {} has a value but no name.", index + 1));
        }
        reqwest::header::HeaderName::from_bytes(row.name.as_bytes())
            .map_err(|_| format!("Invalid HTTP header name '{}'.", row.name))?;
        reqwest::header::HeaderValue::from_str(&row.value).map_err(|_| {
            format!(
                "Header '{}' contains a value that cannot be sent in HTTP.",
                row.name
            )
        })?;
        if request.auth != Auth::None && row.name.eq_ignore_ascii_case("authorization") {
            return Err("The Authorization header conflicts with the auth controls. Disable the header or choose no auth.".into());
        }
        headers.push((row.name.clone(), row.value.clone()));
    }
    let authorization = match &request.auth {
        Auth::None => None,
        Auth::Bearer { token } => Some(format!("Bearer {token}")),
        Auth::Basic { username, password } => {
            if username.contains(':') {
                return Err("Basic auth usernames cannot contain a colon.".into());
            }
            Some(format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{password}"))
            ))
        }
    };
    if let Some(authorization) = authorization {
        reqwest::header::HeaderValue::from_str(&authorization).map_err(|_| {
            "Authentication contains a value that cannot be sent in HTTP.".to_owned()
        })?;
        headers.push(("Authorization".into(), authorization));
    }
    Ok(ResolvedRequest {
        method: request.method.clone(),
        url: url.into(),
        headers,
        body: request.body.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(name: &str, value: &str) -> KeyValue {
        KeyValue {
            enabled: true,
            name: name.into(),
            value: value.into(),
        }
    }

    fn request() -> Request {
        let mut request = Request::new(1);
        request.url = "https://example.com/items".into();
        request
    }

    #[test]
    fn workspace_round_trip_preserves_drafts_rows_auth_and_selection() {
        let mut draft = request();
        draft.name = "Create item".into();
        draft.method = "POST".into();
        draft.url = "unfinished URL".into();
        draft.params = vec![row("tag", "one"), row("tag", "two")];
        draft.headers = vec![KeyValue {
            enabled: false,
            ..row("X-Token", "literal {{value}}")
        }];
        draft.auth = Auth::Basic {
            username: "user".into(),
            password: "secret".into(),
        };
        draft.body = "  {\"literal\":\"{{name}}\"}\r\n".into();
        let saved = SavedWorkspace {
            version: 1,
            requests: vec![draft],
            selected: Some(1),
        };
        let state = serde_json::to_value(&saved).unwrap();
        assert_eq!(state["requests"][0]["auth"]["type"], "basic");
        assert_eq!(load(Some(&state)).unwrap(), saved);
        assert_eq!(load(None).unwrap(), SavedWorkspace::default());
        assert_eq!(load(Some(&Value::Null)).unwrap(), SavedWorkspace::default());
    }

    #[test]
    fn persistence_defaults_keep_incomplete_drafts_editable() {
        let state = json!({"requests": [{"id": 7, "headers": [{}], "auth": {"type":"bearer"}}]});
        let loaded = load(Some(&state)).unwrap();
        let draft = &loaded.requests[0];
        assert_eq!(draft.id, 7);
        assert_eq!(draft.name, "New request");
        assert_eq!(draft.method, "GET");
        assert!(draft.url.is_empty());
        assert_eq!(draft.headers, [KeyValue::default()]);
        assert_eq!(
            draft.auth,
            Auth::Bearer {
                token: String::new()
            }
        );
        assert!(resolve(draft).is_err());
    }

    #[test]
    fn rejects_corrupt_workspace_relationships_and_unknown_versions() {
        for (state, expected) in [
            (json!({"version": 2}), "unsupported version 2"),
            (json!({"requests": [{"id": 0}]}), "nonzero"),
            (
                json!({"requests": [{"id": 3}, {"id": 3}]}),
                "duplicate ID 3",
            ),
            (
                json!({"requests": [{"id": 3}], "selected": 4}),
                "does not exist",
            ),
            (json!({"requests": [{}]}), "missing field `id`"),
        ] {
            assert!(load(Some(&state)).unwrap_err().contains(expected));
        }
    }

    #[test]
    fn query_rows_append_literal_encoded_pairs_preserving_existing_duplicates() {
        let mut draft = request();
        draft.url = "https://example.com/items?tag=original&tag=%2F#details".into();
        draft.params = vec![
            row("tag", "space & plus+"),
            row("empty", ""),
            KeyValue::default(),
            KeyValue {
                enabled: false,
                ..row("ignored", "value")
            },
        ];
        let resolved = resolve(&draft).unwrap();
        assert_eq!(
            resolved.url,
            "https://example.com/items?tag=original&tag=%2F&tag=space+%26+plus%2B&empty=#details"
        );
        let url = reqwest::Url::parse(&resolved.url).unwrap();
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            [
                ("tag".into(), "original".into()),
                ("tag".into(), "/".into()),
                ("tag".into(), "space & plus+".into()),
                ("empty".into(), "".into()),
            ]
        );
    }

    #[test]
    fn preserves_duplicate_headers_and_raw_body_without_substitution() {
        let mut draft = request();
        draft.method = "POST".into();
        draft.headers = vec![
            row("X-Tag", "one"),
            row("X-Tag", "two"),
            KeyValue::default(),
            KeyValue {
                enabled: false,
                ..row("invalid header", "ignored\r\nvalue")
            },
        ];
        draft.body = "  {{literal}}\r\n\0\n".into();
        let resolved = resolve(&draft).unwrap();
        assert_eq!(
            resolved.headers,
            [
                ("X-Tag".into(), "one".into()),
                ("X-Tag".into(), "two".into())
            ]
        );
        assert_eq!(resolved.body, draft.body);
    }

    #[test]
    fn auth_generates_headers_and_rejects_explicit_authorization_conflicts() {
        let mut draft = request();
        draft.auth = Auth::Bearer {
            token: "literal-token".into(),
        };
        assert_eq!(
            resolve(&draft).unwrap().headers,
            [("Authorization".into(), "Bearer literal-token".into())]
        );
        draft.auth = Auth::Basic {
            username: "Aladdin".into(),
            password: "open sesame".into(),
        };
        assert_eq!(
            resolve(&draft).unwrap().headers,
            [(
                "Authorization".into(),
                "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==".into()
            )]
        );
        draft.headers.push(row("aUtHoRiZaTiOn", "custom"));
        assert!(resolve(&draft).unwrap_err().contains("conflicts"));
        draft.headers[0].enabled = false;
        assert_eq!(resolve(&draft).unwrap().headers.len(), 1);
        draft.headers[0].enabled = true;
        draft.auth = Auth::None;
        assert_eq!(
            resolve(&draft).unwrap().headers,
            [("aUtHoRiZaTiOn".into(), "custom".into())]
        );
    }

    #[test]
    fn rejects_invalid_network_inputs_only_when_resolving() {
        let mut draft = request();
        draft.method = "invalid method".into();
        assert!(resolve(&draft).unwrap_err().contains("Invalid HTTP method"));
        draft.method = "GET".into();
        for url in [
            "file:///tmp/body",
            "http://user:secret@example.com",
            "http://:secret@example.com",
        ] {
            draft.url = url.into();
            assert!(resolve(&draft).is_err());
        }
        draft.url = "https://example.com".into();
        draft.params = vec![row("", "value")];
        assert!(
            resolve(&draft)
                .unwrap_err()
                .contains("Query parameter row 1")
        );
        draft.params.clear();
        draft.headers = vec![row("", "value")];
        assert!(resolve(&draft).unwrap_err().contains("Header row 1"));
        draft.headers = vec![row("bad header", "value")];
        assert!(
            resolve(&draft)
                .unwrap_err()
                .contains("Invalid HTTP header name")
        );
        draft.headers = vec![row("X-Token", "token\r\nInjected: value")];
        assert!(
            resolve(&draft)
                .unwrap_err()
                .contains("cannot be sent in HTTP")
        );
        draft.headers.clear();
        draft.auth = Auth::Bearer {
            token: "token\nInjected: value".into(),
        };
        assert!(resolve(&draft).unwrap_err().contains("Authentication"));
        draft.auth = Auth::Basic {
            username: "user:extra".into(),
            password: "secret".into(),
        };
        assert!(
            resolve(&draft)
                .unwrap_err()
                .contains("cannot contain a colon")
        );
    }
}
