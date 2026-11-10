use anyhow::{anyhow, bail, ensure, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use crypto_box::{aead::OsRng, PublicKey};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::Duration;
use ureq::http::{header::AUTHORIZATION, HeaderValue, Request};

const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'~');
const TRANSPORT_FAILURE: &str =
    "GitHub could not be reached. A write may have succeeded; rerun to inspect current state.";

pub(crate) fn validate_repository(repository: &str) -> Result<()> {
    let valid = |part: &str| {
        !part.is_empty()
            && !matches!(part, "." | "..")
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    };
    ensure!(
        repository
            .split_once('/')
            .is_some_and(|(owner, name)| valid(owner) && valid(name)),
        "Repository must be owner/name."
    );
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Status(pub(crate) u16);

impl fmt::Display for Status {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hint = match self.0 {
            401 => "Check the token.",
            403 => "Check token permissions, organization approval/SSO, and rate limits.",
            404 => "Check repository access and environment availability.",
            422 => "Check the setting and repository environment support.",
            _ => "Retry after checking GitHub availability.",
        };
        write!(out, "GitHub request failed (HTTP {}). {hint}", self.0)
    }
}

impl std::error::Error for Status {}

#[derive(Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) exists: bool,
    pub(crate) variables: BTreeMap<String, String>,
    pub(crate) secrets: BTreeSet<String>,
}

pub(crate) struct GitHubApi {
    pub(crate) root: String,
    base: String,
    authorization: HeaderValue,
    agent: ureq::Agent,
}

impl GitHubApi {
    pub(crate) fn new(repository: &str, token: &str) -> Result<Self> {
        Self::with_base(repository, token, "https://api.github.com")
    }

    fn with_base(repository: &str, token: &str, base: &str) -> Result<Self> {
        validate_repository(repository)?;
        ensure!(
            !token.trim().is_empty() && !token.chars().any(char::is_control),
            "A GitHub token is required."
        );
        let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| anyhow!("Invalid GitHub token."))?;
        authorization.set_sensitive(true);
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build()
            .into();
        Ok(Self {
            root: format!("/repos/{repository}"),
            base: base.to_owned(),
            authorization,
            agent,
        })
    }

    pub(crate) fn environment_path(&self, environment: &str) -> String {
        format!(
            "{}/environments/{}",
            self.root,
            utf8_percent_encode(environment, SEGMENT)
        )
    }

    pub(crate) fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        allow_missing: bool,
    ) -> Result<Option<Value>> {
        ensure!(
            path.strip_prefix(&self.root)
                .is_some_and(|suffix| suffix.starts_with('/')),
            "Invalid GitHub request path."
        );
        let mut request = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base))
            .header(AUTHORIZATION, self.authorization.clone())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if body.is_some() {
            request = request.header("Content-Type", "application/json");
        }
        let request = request
            .body(body.map(Value::to_string).unwrap_or_default())
            .map_err(|_| anyhow!("Invalid GitHub request."))?;
        let mut response = self
            .agent
            .run(request)
            .map_err(|_| anyhow!(TRANSPORT_FAILURE))?;
        let status = response.status().as_u16();
        if allow_missing && status == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            bail!(Status(status));
        }
        let bytes = response
            .body_mut()
            .read_to_vec()
            .map_err(|_| anyhow!(TRANSPORT_FAILURE))?;
        if bytes.is_empty() {
            return Ok(Some(json!({})));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| anyhow!("GitHub returned an unexpected response."))
    }

    pub(crate) fn list_values(
        &self,
        environment: &str,
        kind: &str,
    ) -> Result<BTreeMap<String, Option<String>>> {
        ensure!(
            matches!(kind, "variables" | "secrets"),
            "Invalid GitHub setting kind."
        );
        let mut values = BTreeMap::new();
        for page in 1.. {
            let path = format!(
                "{}/{kind}?per_page=100&page={page}",
                self.environment_path(environment)
            );
            let data = self.request("GET", &path, None, false)?.unwrap_or_default();
            let unexpected = || anyhow!("GitHub returned an unexpected response.");
            let items = data[kind].as_array().ok_or_else(unexpected)?;
            for item in items {
                let name = item["name"].as_str().ok_or_else(unexpected)?;
                let value = if kind == "variables" {
                    Some(item["value"].as_str().ok_or_else(unexpected)?.to_owned())
                } else {
                    None
                };
                ensure!(
                    values.insert(name.to_owned(), value).is_none(),
                    "GitHub returned a duplicate setting."
                );
            }
            let total = data["total_count"].as_u64().unwrap_or(values.len() as u64);
            if items.len() < 100 || values.len() as u64 >= total {
                break;
            }
        }
        Ok(values)
    }

    pub(crate) fn snapshot(&self, environment: &str) -> Result<Snapshot> {
        if self
            .request("GET", &self.environment_path(environment), None, true)?
            .is_none()
        {
            return Ok(Snapshot::default());
        }
        let variables = self
            .list_values(environment, "variables")?
            .into_iter()
            .filter_map(|(name, value)| Some((name, value?)))
            .collect();
        let secrets = self
            .list_values(environment, "secrets")?
            .into_keys()
            .collect();
        Ok(Snapshot {
            exists: true,
            variables,
            secrets,
        })
    }

    pub(crate) fn ensure_environment(
        &self,
        environment: &str,
        wait_timer: Option<u32>,
    ) -> Result<bool> {
        let path = self.environment_path(environment);
        if self.request("GET", &path, None, true)?.is_some() {
            return Ok(true);
        }
        let body = wait_timer.map_or_else(|| json!({}), |minutes| json!({"wait_timer": minutes}));
        self.request("PUT", &path, Some(&body), false)?;
        Ok(false)
    }

    pub(crate) fn write_variable(
        &self,
        environment: &str,
        name: &str,
        value: &str,
        exists: bool,
    ) -> Result<()> {
        let mut path = format!("{}/variables", self.environment_path(environment));
        let method = if exists {
            path.push('/');
            path.push_str(&utf8_percent_encode(name, SEGMENT).to_string());
            "PATCH"
        } else {
            "POST"
        };
        self.request(
            method,
            &path,
            Some(&json!({"name": name, "value": value})),
            false,
        )?;
        Ok(())
    }

    pub(crate) fn write_secret(&self, environment: &str, name: &str, value: &str) -> Result<()> {
        ensure!(
            value.len() <= 48 * 1024,
            "The secret exceeds GitHub's 48 KiB limit."
        );
        let path = format!("{}/secrets", self.environment_path(environment));
        let key = self
            .request("GET", &format!("{path}/public-key"), None, false)?
            .unwrap_or_default();
        let invalid = || anyhow!("GitHub returned an invalid encryption key.");
        let key_id = key["key_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(invalid)?;
        let public = STANDARD
            .decode(key["key"].as_str().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        let public = PublicKey::from_slice(&public).map_err(|_| invalid())?;
        let encrypted = public
            .seal(&mut OsRng, value.as_bytes())
            .map_err(|_| invalid())?;
        let name = utf8_percent_encode(name, SEGMENT);
        self.request(
            "PUT",
            &format!("{path}/{name}"),
            Some(&json!({"key_id": key_id, "encrypted_value": STANDARD.encode(encrypted)})),
            false,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_box::SecretKey;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn mock_request(listener: &TcpListener) -> (TcpStream, String, Vec<u8>) {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut headers = String::new();
        let mut length = 0;
        loop {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
            let normalized = line.to_ascii_lowercase();
            if let Some(value) = normalized.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
            headers.push_str(&line);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        (stream, headers, body)
    }

    fn mock_reply(mut stream: TcpStream, status: &str, headers: &str, body: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
        .unwrap();
    }

    #[test]
    fn github_secrets_use_anonymous_encryption() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let api = GitHubApi::with_base("owner/repo", "test-token", &base).unwrap();
        let server = thread::spawn(move || {
            let secret = SecretKey::generate(&mut OsRng);
            let (stream, headers, _) = mock_request(&listener);
            assert!(headers.starts_with(
                "GET /repos/owner/repo/environments/custom%2Fname/secrets/public-key "
            ));
            assert!(headers
                .to_ascii_lowercase()
                .contains("authorization: bearer test-token"));
            mock_reply(
                stream,
                "200 OK",
                "",
                &json!({"key_id":"test-id", "key": STANDARD.encode(secret.public_key().as_bytes())}).to_string(),
            );
            let (stream, headers, body) = mock_request(&listener);
            assert!(headers.starts_with(
                "PUT /repos/owner/repo/environments/custom%2Fname/secrets/SIGNING_PASSWORD "
            ));
            assert!(!String::from_utf8_lossy(&body).contains("test-password"));
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["key_id"], "test-id");
            let encrypted = STANDARD
                .decode(body["encrypted_value"].as_str().unwrap())
                .unwrap();
            assert_eq!(secret.unseal(&encrypted).unwrap(), b"test-password");
            mock_reply(stream, "204 No Content", "", "");
        });
        api.write_secret("custom/name", "SIGNING_PASSWORD", "test-password")
            .unwrap();
        server.join().unwrap();
    }

    #[test]
    fn existing_environment_is_not_replaced() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let api = GitHubApi::with_base("owner/repo", "test-token", &base).unwrap();
        let server = thread::spawn(move || {
            let (stream, headers, _) = mock_request(&listener);
            assert!(headers.starts_with("GET /repos/owner/repo/environments/delayed-docs "));
            mock_reply(
                stream,
                "200 OK",
                "",
                r#"{"protection_rules":[{"type":"wait_timer","wait_timer":30}]}"#,
            );
        });
        assert!(api.ensure_environment("delayed-docs", Some(15)).unwrap());
        server.join().unwrap();
    }

    #[test]
    fn github_errors_exclude_remote_response_and_disable_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let api = GitHubApi::with_base("owner/repo", "test-token", &base).unwrap();
        let redirect = format!("Location: {base}/outside\r\n");
        let server = thread::spawn(move || {
            let (stream, _, _) = mock_request(&listener);
            mock_reply(stream, "403 Forbidden", "", "private remote information");
            let (stream, _, _) = mock_request(&listener);
            mock_reply(stream, "302 Found", &redirect, "");
            // Any redirect follow-up would have no server response and could not return HTTP 302.
        });
        let path = api.environment_path("publish");
        let error = api.request("GET", &path, None, true).unwrap_err();
        assert!(error.to_string().contains("HTTP 403"));
        assert!(!error.to_string().contains("private remote information"));
        let error = api.request("GET", &path, None, true).unwrap_err();
        assert_eq!(error.downcast_ref::<Status>().unwrap().0, 302);
        server.join().unwrap();
    }
}
