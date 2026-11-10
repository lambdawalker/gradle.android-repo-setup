use crate::github::GitHubApi;
use crate::vault::{
    confirm, expand_home, lock_file, lock_path, parent, private_dir, private_screen, prompt,
    random_bytes, read_regular_file, regular_file_exists, secret_prompt, use_saved, validate_alias,
    SetupConfig, SetupVault, SigningPasswords,
};
use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{Builder, TempPath};

const SECRET_NAMES: [&str; 4] = [
    "ANDROID_KEYSTORE_PASSWORD",
    "ANDROID_KEY_ALIAS",
    "ANDROID_KEY_PASSWORD",
    "ANDROID_KEYSTORE_BASE64",
];
const GITHUB_SECRET_LIMIT: usize = 48 * 1024;
const KEYTOOL_ERROR: &str = "keytool failed. Check the JDK, keystore, alias, and passwords.";

fn generate_password() -> Result<String> {
    let groups: [&[u8]; 4] = [
        b"abcdefghijklmnopqrstuvwxyz",
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        b"0123456789",
        b"!@#$%^&*()-_=+[]{}:,.?",
    ];
    let alphabet = groups.concat();
    let bound = 256 - 256 % alphabet.len();
    loop {
        let mut bytes = [0; 32];
        random_bytes(&mut bytes)?;
        let value: Vec<_> = bytes
            .into_iter()
            .filter(|byte| usize::from(*byte) < bound)
            .map(|byte| alphabet[usize::from(byte) % alphabet.len()])
            .take(25)
            .collect();
        if value.len() == 25
            && groups
                .iter()
                .all(|group| value.iter().any(|byte| group.contains(byte)))
        {
            return Ok(String::from_utf8(value)?);
        }
    }
}

fn read_password(label: &str, allow_generate: bool, saved: Option<&str>) -> Result<(String, bool)> {
    if let Some(saved) = saved.filter(|value| !value.is_empty()) {
        if use_saved(label)? {
            return Ok((saved.to_owned(), false));
        }
    }
    let hint = if allow_generate {
        " [Enter to generate]"
    } else {
        ""
    };
    loop {
        let value = secret_prompt(&format!("{label}{hint}"))?;
        if value.is_empty() && allow_generate {
            return Ok((generate_password()?, true));
        }
        if value.chars().count() < 6 || value.chars().any(|ch| ch <= '\u{1f}' || ch == '\u{7f}') {
            setup_message!("Enter at least six characters without control characters.");
        } else if value == secret_prompt(&format!("Confirm {}", label.to_ascii_lowercase()))? {
            return Ok((value, false));
        } else {
            setup_message!("Passwords differ. Try again.");
        }
    }
}

fn find_keytool() -> Result<PathBuf> {
    let executable = if cfg!(windows) {
        "keytool.exe"
    } else {
        "keytool"
    };
    let mut locations = Vec::new();
    if let Some(java) = std::env::var_os("JAVA_HOME").filter(|value| !value.is_empty()) {
        locations.push(expand_home(Path::new(&java))?.join("bin"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        locations.extend(std::env::split_paths(&path));
    }
    locations
        .into_iter()
        .map(|dir| dir.join(executable))
        .find(|path| {
            let Ok(metadata) = path.metadata() else {
                return false;
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return false;
                }
            }
            metadata.is_file()
        })
        .context("Install a JDK and add keytool to PATH.")
}

fn run_keytool(
    keytool: &Path,
    operation: &str,
    path: &Path,
    alias: &str,
    store_password: &str,
    key_password: &str,
    extra: &[&str],
) -> Result<()> {
    let mut child = Command::new(keytool)
        .arg(operation)
        .arg("-keystore")
        .arg(path)
        .args([
            "-storetype",
            "JKS",
            "-alias",
            alias,
            "-storepass:env",
            "DEMO_KEYTOOL_STORE_PASSWORD",
            "-keypass:env",
            "DEMO_KEYTOOL_KEY_PASSWORD",
        ])
        .args(extra)
        .env_remove("JAVA_TOOL_OPTIONS")
        .env_remove("_JAVA_OPTIONS")
        .env_remove("JDK_JAVA_OPTIONS")
        .env("DEMO_KEYTOOL_STORE_PASSWORD", store_password)
        .env("DEMO_KEYTOOL_KEY_PASSWORD", key_password)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| anyhow::anyhow!(KEYTOOL_ERROR))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => bail!(KEYTOOL_ERROR),
            Ok(None) if start.elapsed() < Duration::from_secs(60) => {
                thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                bail!(KEYTOOL_ERROR);
            }
        }
    }
}

fn prepare_keystore(
    keytool: &Path,
    path: &Path,
    alias: &str,
    store_password: &str,
    key_password: &str,
    reuse: bool,
) -> Result<()> {
    private_dir(parent(path)).context("Could not prepare the keystore directory.")?;
    let _lock = lock_file(
        &lock_path(path),
        "Another setup wizard is using this keystore. Close it before retrying.",
    )?;
    let exists = regular_file_exists(path)?;
    if reuse {
        ensure!(exists, "Choose a regular file path for the keystore.");
        // A certificate request proves that both passwords unlock the private key.
        return run_keytool(
            keytool,
            "-certreq",
            path,
            alias,
            store_password,
            key_password,
            &[],
        );
    }
    ensure!(!exists, "Keystore already exists at the target path.");
    let directory = Builder::new()
        .prefix(".signing-")
        .tempdir_in(parent(path))?;
    let temporary = directory.path().join("signing.jks");
    run_keytool(
        keytool,
        "-genkeypair",
        &temporary,
        alias,
        store_password,
        key_password,
        &[
            "-keyalg",
            "RSA",
            "-keysize",
            "3072",
            "-validity",
            "10950",
            "-dname",
            "CN=Android APK Signing",
            "-noprompt",
        ],
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .context("Could not reopen the generated keystore for flushing.")?
        .sync_all()
        .context("Could not flush the generated keystore.")?;
    TempPath::try_from_path(temporary)?
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .context("Could not install the signing keystore without replacing an existing key.")?;
    Ok(())
}

fn resolve_keystore(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let candidate = std::path::absolute(expand_home(candidate)?)?;
    regular_file_exists(&candidate)?;
    let mut resolved = PathBuf::new();
    for part in candidate.components() {
        match part {
            Component::ParentDir => {
                if resolved.parent().is_some() {
                    resolved.pop();
                }
            }
            Component::CurDir => {}
            _ => {
                resolved.push(part);
                if let Ok(canonical) = resolved.canonicalize() {
                    resolved = canonical;
                }
            }
        }
    }
    regular_file_exists(&resolved)?;
    let inside_project = resolved.starts_with(root)
        && (root.join(".git").symlink_metadata().is_ok()
            || root.join(".project/setup/config.yaml").is_file());
    ensure!(
        !inside_project
            && !resolved
                .ancestors()
                .any(|path| path.join(".git").symlink_metadata().is_ok()),
        "Keep the keystore outside the repository checkout."
    );
    Ok(resolved)
}

fn choose_keystore(
    root: &Path,
    config: &SetupConfig,
    vault: &mut SetupVault,
) -> Result<(PathBuf, String)> {
    let state_path = config.state_path();
    let default_path = state_path.with_file_name(format!(
        "{}-{}.jks",
        config.alias(),
        state_path
            .file_stem()
            .and_then(OsStr::to_str)
            .context("Vault filename must be UTF-8.")?
    ));
    let saved_path = vault
        .state
        .keystore
        .as_deref()
        .filter(|value| !value.is_empty());
    let default = saved_path.unwrap_or(
        default_path
            .to_str()
            .context("Keystore path must be UTF-8.")?,
    );
    let value = if saved_path.is_some() && use_saved("Keystore path")? {
        default.to_owned()
    } else {
        prompt("Keystore path", Some(default))?
    };
    let path = resolve_keystore(root, Path::new(&value))?;
    vault.state.keystore = Some(
        path.to_str()
            .context("Keystore path must be UTF-8.")?
            .to_owned(),
    );
    vault.save()?;

    let saved_alias = vault
        .state
        .alias
        .as_deref()
        .filter(|value| !value.is_empty());
    let default_alias = config.alias();
    let default = saved_alias.unwrap_or(&default_alias);
    let alias = if saved_alias.is_some() && use_saved("Signing key alias")? {
        default.to_owned()
    } else {
        prompt("Signing key alias", Some(default))?
    };
    validate_alias(&alias)?;
    vault.state.alias = Some(alias.clone());
    vault.save()?;
    Ok((path, alias))
}

fn confirm_backup(generated: &[(&str, &str)]) -> Result<bool> {
    if generated.is_empty() {
        return Ok(true);
    }
    if !confirm("Reveal generated passwords in this private terminal?")? {
        return Ok(false);
    }
    private_screen(|| {
        for (label, password) in generated {
            setup_message!("{label} {password}");
        }
        confirm("Have you saved these passwords in your password manager?")
    })
}

fn inspect(api: &GitHubApi, config: &SetupConfig) -> Result<(Option<Value>, BTreeSet<String>)> {
    let environment = api.request(
        "GET",
        &api.environment_path(&config.environment),
        None,
        true,
    )?;
    let existing = if environment.is_some() {
        api.list_values(&config.environment, "secrets")?
            .into_keys()
            .collect()
    } else {
        BTreeSet::new()
    };
    Ok((environment, existing))
}

fn ensure_branch_policy(
    api: &GitHubApi,
    config: &SetupConfig,
    environment: Option<&Value>,
    allow_create: bool,
) -> Result<()> {
    let path = api.environment_path(&config.environment);
    let policy = json!({"protected_branches": false, "custom_branch_policies": true});
    match environment {
        None if allow_create => {
            api.request(
                "PUT",
                &path,
                Some(&json!({"deployment_branch_policy": policy})),
                false,
            )?;
        }
        Some(environment) if environment["deployment_branch_policy"] == policy => {}
        _ => bail!(
            "Configure {} to allow only the {} branch in GitHub Settings.",
            config.environment,
            config.branch
        ),
    }
    let path = format!("{path}/deployment-branch-policies");
    let rules = api
        .request("GET", &format!("{path}?per_page=100"), None, false)?
        .context("GitHub did not return branch policies.")?;
    let entries = rules["branch_policies"]
        .as_array()
        .context("Invalid GitHub branch policies.")?;
    let total_count = rules
        .get("total_count")
        .map_or(Some(entries.len() as u64), Value::as_u64);
    if allow_create && entries.is_empty() && total_count == Some(0) {
        api.request(
            "POST",
            &path,
            Some(&json!({"name": config.branch, "type": "branch"})),
            false,
        )?;
    } else {
        ensure!(
            entries.len() == 1
                && total_count == Some(1)
                && entries[0]["name"] == config.branch
                && entries[0]["type"] == "branch",
            "Existing rules for {} must allow only the {} branch.",
            config.environment,
            config.branch
        );
    }
    Ok(())
}

fn upload(
    api: &GitHubApi,
    config: &SetupConfig,
    original: (Option<Value>, BTreeSet<String>),
    missing: &[&str],
    passwords: &SigningPasswords,
) -> Result<()> {
    let bytes = read_regular_file(Path::new(&passwords.keystore), GITHUB_SECRET_LIMIT / 4 * 3)
        .context("Could not read the signing keystore within GitHub's 48 KiB encoded limit.")?
        .context("Could not read the signing keystore.")?;
    let encoded = STANDARD.encode(&bytes);
    let values = [
        passwords.store.as_str(),
        passwords.alias.as_str(),
        passwords.key.as_deref().context("Missing key password.")?,
        encoded.as_str(),
    ];
    ensure!(
        values
            .iter()
            .all(|value| value.len() <= GITHUB_SECRET_LIMIT),
        "A signing secret exceeds GitHub's 48 KiB limit."
    );
    let current = inspect(api, config)?;
    ensure!(
        current == original,
        "The environment changed during setup. Rerun to review its current state."
    );
    ensure_branch_policy(api, config, current.0.as_ref(), true)?;
    for (name, value) in SECRET_NAMES.into_iter().zip(values) {
        if missing.contains(&name) {
            ensure!(
                !api.list_values(&config.environment, "secrets")?
                    .contains_key(name),
                "A signing secret was added concurrently. Rerun to review the state."
            );
            api.write_secret(&config.environment, name, value)?;
            setup_message!("Uploaded {name}.");
        }
    }
    setup_message!("APK signing environment prepared.");
    Ok(())
}

pub fn run(
    root: &Path,
    config: &SetupConfig,
    vault: &mut SetupVault,
    operations: &[&str],
) -> Result<bool> {
    ensure!(
        !operations.is_empty()
            && operations
                .iter()
                .all(|step| matches!(*step, "key" | "github")),
        "Choose signing keystore setup, GitHub setup, or both."
    );
    let local = operations.contains(&"key");
    let mut remote = None;
    let mut missing = Vec::new();
    setup_message!("Accepted answers are saved in the encrypted local vault as you go.");
    if operations.contains(&"github") {
        let saved = vault
            .state
            .github_token
            .as_deref()
            .filter(|value| !value.is_empty());
        let token = match saved {
            Some(token) if use_saved("GitHub token")? => token.to_owned(),
            _ => secret_prompt("GitHub token")?.trim().to_owned(),
        };
        let api = GitHubApi::new(&config.repository, &token)?;
        let state = inspect(&api, config)?;
        if vault.state.github_token.as_deref() != Some(&token) {
            vault.state.github_token = Some(token);
            vault.save()?;
        }
        missing.extend(
            SECRET_NAMES
                .into_iter()
                .filter(|name| !state.1.contains(*name)),
        );
        if missing.is_empty() {
            ensure_branch_policy(&api, config, state.0.as_ref(), false)?;
            setup_message!(
                "All APK signing secrets already exist. Existing values were preserved."
            );
            if !local {
                vault.state.finish("github");
                vault.save()?;
                return Ok(true);
            }
        }
        remote = Some((api, state));
    }

    let keytool = find_keytool()?;
    let (path, alias) = choose_keystore(root, config, vault)?;
    let reuse = regular_file_exists(&path)?;
    ensure!(
        reuse || local,
        "GitHub-only setup requires an existing keystore. Choose both operations to create one."
    );
    if remote
        .as_ref()
        .is_some_and(|(_, (_, existing))| SECRET_NAMES.iter().any(|name| existing.contains(*name)))
    {
        ensure!(
            reuse,
            "GitHub already has signing secrets. Use the original keystore."
        );
        if !missing.is_empty()
            && !confirm("Partial upload found. Are you using the original keystore and alias?")?
        {
            return Ok(false);
        }
    }
    let path_text = path.to_str().context("Keystore path must be UTF-8.")?;
    let saved = vault
        .state
        .signing_passwords
        .as_ref()
        .filter(|saved| saved.keystore == path_text && saved.alias == alias);
    let (store, new_store) = read_password(
        "Keystore password",
        !reuse,
        saved.map(|value| value.store.as_str()),
    )?;
    let saved_key = saved.and_then(|value| value.key.clone());
    vault.state.signing_passwords = Some(SigningPasswords {
        keystore: path_text.to_owned(),
        alias: alias.clone(),
        store,
        key: saved_key,
    });
    vault.save()?;
    let passwords = vault.state.signing_passwords.as_mut().unwrap();
    let (key, new_key) = read_password("Key password", !reuse, passwords.key.as_deref())?;
    passwords.key = Some(key);
    vault.save()?;
    setup_message!("Repository {}", config.repository);
    setup_message!("Keystore {}", path.display());
    setup_message!(
        "{}",
        if reuse {
            "Reuse existing keystore."
        } else {
            "Create a new keystore."
        }
    );
    if !missing.is_empty() {
        setup_message!(
            "Environment {} ({} only)",
            config.environment,
            config.branch
        );
        setup_message!("Upload missing secrets {}", missing.join(", "));
    }
    if !confirm("Apply these changes?")? {
        return Ok(false);
    }
    let passwords = vault.state.signing_passwords.as_ref().unwrap();
    let key = passwords.key.as_deref().unwrap();
    let mut generated = Vec::new();
    if new_store {
        generated.push(("Keystore password", passwords.store.as_str()));
    }
    if new_key {
        generated.push(("Key password", key));
    }
    if !confirm_backup(&generated)? {
        return Ok(false);
    }
    vault.state.start(operations);
    vault.save()?;
    let passwords = vault.state.signing_passwords.as_ref().unwrap();
    let key = passwords.key.as_deref().unwrap();
    prepare_keystore(&keytool, &path, &alias, &passwords.store, key, reuse)?;
    setup_message!(
        "Keystore ready at {}. Keep an offline backup.",
        path.display()
    );
    vault.state.finish("key");
    vault.save()?;
    if let Some((api, state)) = remote {
        if !missing.is_empty() {
            upload(
                &api,
                config,
                state,
                &missing,
                vault.state.signing_passwords.as_ref().unwrap(),
            )?;
        }
        vault.state.finish("github");
        vault.save()?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_passwords_meet_keytool_requirements() -> Result<()> {
        let password = generate_password()?;
        assert_eq!(password.len(), 25);
        assert!(password.bytes().any(|byte| byte.is_ascii_lowercase()));
        assert!(password.bytes().any(|byte| byte.is_ascii_uppercase()));
        assert!(password.bytes().any(|byte| byte.is_ascii_digit()));
        assert!(password.bytes().any(|byte| !byte.is_ascii_alphanumeric()));
        Ok(())
    }

    #[test]
    fn keystore_paths_reject_repository_checkouts() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let checkout = scratch.path().join("checkout");
        fs::create_dir_all(checkout.join(".git"))?;
        assert!(resolve_keystore(&checkout, &checkout.join("subdir/key.jks")).is_err());
        assert!(resolve_keystore(&checkout, &scratch.path().join("key.jks")).is_ok());
        assert!(
            resolve_keystore(scratch.path(), &scratch.path().join(".projects/key.jks")).is_ok()
        );
        Ok(())
    }

    #[test]
    fn keytool_creates_and_verifies_a_private_key_without_overwriting_it() -> Result<()> {
        let Ok(keytool) = find_keytool() else {
            return Ok(());
        };
        let scratch = tempfile::tempdir()?;
        let path = scratch.path().join("test.jks");
        prepare_keystore(
            &keytool,
            &path,
            "test",
            "fixture-store",
            "fixture-key",
            false,
        )?;
        prepare_keystore(
            &keytool,
            &path,
            "test",
            "fixture-store",
            "fixture-key",
            true,
        )?;
        assert!(prepare_keystore(
            &keytool,
            &path,
            "test",
            "fixture-store",
            "wrong-password",
            true
        )
        .is_err());
        let original = fs::read(&path)?;
        assert!(prepare_keystore(
            &keytool,
            &path,
            "test",
            "fixture-store",
            "fixture-key",
            false
        )
        .is_err());
        assert_eq!(fs::read(&path)?, original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(path.metadata()?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }
}
