//! Read-only Central Portal status and the upload receipt emitted by Vanniktech 0.37.0.
use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use regex::Regex;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum State {
    Failed,
    Published,
    Active,
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
    ensure!(
        Regex::new(r"\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\z")?
            .is_match(id),
        "Invalid Central deployment ID"
    );
    Ok(())
}

pub(crate) fn upload_id(line: &str) -> Result<Option<String>> {
    let Some(rest) = line.strip_prefix("Uploaded bundle to Central Portal as ") else {
        return Ok(None);
    };
    let Some((mode, id)) = rest.split_once(", deployment id: ") else {
        bail!("Unrecognized Central upload receipt; retain the upload journal");
    };
    ensure!(
        matches!(mode, "AUTOMATIC" | "USER_MANAGED"),
        "Unknown Central publishing mode"
    );
    validate_id(id)?;
    Ok(Some(id.into()))
}

pub(crate) fn classify(
    value: &Value,
    id: &str,
    group: &str,
    artifact: &str,
    version: &str,
) -> Result<State> {
    validate_id(id)?;
    ensure!(
        value["deploymentId"] == id
            && value["deploymentName"] == format!("{group}-{artifact}-{version}"),
        "Central deployment identity differs from the upload receipt"
    );
    if let Some(purls) = value.get("purls") {
        let purls = purls
            .as_array()
            .context("Malformed Central deployment coordinates")?;
        // FAILED deployments can have no validated components. If present, all
        // components must belong to the single selected publication.
        let expected = format!("pkg:maven/{group}/{artifact}@{version}");
        ensure!(
            purls.iter().all(|p| p.as_str() == Some(&expected)),
            "Central deployment contains different coordinates"
        );
    }
    match value["deploymentState"].as_str() {
        Some("FAILED") => Ok(State::Failed),
        Some("PUBLISHED") => Ok(State::Published),
        Some("PENDING" | "VALIDATING" | "VALIDATED" | "PUBLISHING") => Ok(State::Active),
        _ => bail!("Central returned an unknown deployment state; preserve the journal"),
    }
}

pub(crate) fn status(id: &str) -> Result<Value> {
    validate_id(id)?;
    let credential = |name: &str, gradle: &str| -> Result<String> {
        std::env::var(name)
            .or_else(|_| std::env::var(gradle))
            .ok()
            .filter(|s| !s.is_empty())
            .with_context(|| format!("Missing {name} for Central status verification"))
    };
    let username = credential(
        "MAVEN_CENTRAL_USERNAME",
        "ORG_GRADLE_PROJECT_mavenCentralUsername",
    )?;
    let password = credential(
        "MAVEN_CENTRAL_PASSWORD",
        "ORG_GRADLE_PROJECT_mavenCentralPassword",
    )?;
    let token = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .https_only(true)
        .max_redirects(0)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    // Never include the request, credentials or response body in errors or journals.
    let mut response = agent
        .post(format!(
            "https://central.sonatype.com/api/v1/publisher/status?id={id}"
        ))
        .header("Authorization", format!("Bearer {token}"))
        .send_empty()
        .map_err(|_| anyhow::anyhow!("Central status request failed; preserve the journal"))?;
    ensure!(
        response.status().as_u16() == 200,
        "Central status returned HTTP {}; preserve the journal",
        response.status().as_u16()
    );
    let bytes = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_vec()
        .map_err(|_| anyhow::anyhow!("Could not read Central status"))?;
    serde_json::from_slice(&bytes).context("Invalid Central status JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const ID: &str = "28570f16-da32-4c14-bd2e-c1acc0782365";

    #[test]
    fn receipt_requires_the_exact_plugin_upload_message() {
        assert_eq!(
            upload_id(&format!(
                "Uploaded bundle to Central Portal as AUTOMATIC, deployment id: {ID}"
            ))
            .unwrap(),
            Some(ID.into())
        );
        assert_eq!(
            upload_id(&format!(
                "Uploaded bundle to Central Portal as USER_MANAGED, deployment id: {ID}"
            ))
            .unwrap(),
            Some(ID.into())
        );
        for line in [
            format!("deployment id: {ID}"),
            format!("Failed for deployment id: {ID}"),
            String::from("BUILD FAILED"),
        ] {
            assert_eq!(upload_id(&line).unwrap(), None);
        }
        assert!(upload_id(
            "Uploaded bundle to Central Portal as AUTOMATIC, deployment id: invalid"
        )
        .is_err());
    }

    #[test]
    fn status_requires_identity_and_preserves_nonterminal_states() {
        for (state, expected) in [
            ("FAILED", State::Failed),
            ("PUBLISHED", State::Published),
            ("PENDING", State::Active),
            ("VALIDATING", State::Active),
            ("VALIDATED", State::Active),
            ("PUBLISHING", State::Active),
        ] {
            let value = json!({"deploymentId":ID,"deploymentName":"com.example-demo-1.0.0","deploymentState":state});
            assert_eq!(
                classify(&value, ID, "com.example", "demo", "1.0.0").unwrap(),
                expected
            );
        }
        for value in [
            json!({"deploymentId":"different","deploymentName":"com.example-demo-1.0.0","deploymentState":"FAILED"}),
            json!({"deploymentId":ID,"deploymentName":"other","deploymentState":"FAILED"}),
            json!({"deploymentId":ID,"deploymentName":"com.example-demo-1.0.0","deploymentState":"unknown"}),
            json!({"deploymentId":ID,"deploymentName":"com.example-demo-1.0.0","deploymentState":"FAILED","purls":["pkg:maven/com.example/other@1.0.0"]}),
        ] {
            assert!(classify(&value, ID, "com.example", "demo", "1.0.0").is_err());
        }
    }
}
