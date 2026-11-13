use crate::associations;
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
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{Builder, NamedTempFile, TempPath};

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

fn resolve_file_path(candidate: &Path) -> Result<PathBuf> {
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
    Ok(resolved)
}

fn resolve_keystore(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let resolved = resolve_file_path(candidate)?;
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

fn prepare_signing_files(
    keytool: &Path,
    config: &SetupConfig,
    passwords: &SigningPasswords,
    application_ids: &[String],
    reuse: bool,
) -> Result<()> {
    let application_ids = associations::validate_ids(application_ids)?;
    let output = associations::path(config);
    let keystore = Path::new(&passwords.keystore);
    let resolved_output = resolve_file_path(&output)?;
    let resolved_keystore = resolve_file_path(keystore)?;
    #[cfg(windows)]
    let same_path = resolved_output.to_string_lossy().to_lowercase()
        == resolved_keystore.to_string_lossy().to_lowercase();
    #[cfg(not(windows))]
    let same_path = resolved_output == resolved_keystore;
    ensure!(!same_path, "The public association file must not replace the signing keystore. Choose a different keystore path.");
    regular_file_exists(&output)?;
    private_dir(parent(&output))?;
    let directory = Builder::new()
        .prefix(".certificate-")
        .tempdir_in(parent(&output))?;
    let certificate = directory.path().join("signing.der");
    let key = passwords.key.as_deref().context("Missing key password")?;
    prepare_keystore(
        keytool,
        keystore,
        &passwords.alias,
        &passwords.store,
        key,
        reuse,
    )?;
    // Hash the certificate bytes, independent of keytool's display language and formatting.
    run_keytool(
        keytool,
        "-exportcert",
        keystore,
        &passwords.alias,
        &passwords.store,
        key,
        &[
            "-file",
            certificate
                .to_str()
                .context("Certificate path must be UTF-8.")?,
        ],
    )?;
    let bytes = read_regular_file(&certificate, 1024 * 1024)?
        .context("keytool did not export the signing certificate.")?;
    associations::write(&output, &application_ids, &bytes)
}

const LOCAL_PROPERTIES_BEGIN: &str = "# BEGIN ANDROID REPO SETUP SIGNING";
const LOCAL_PROPERTIES_END: &str = "# END ANDROID REPO SETUP SIGNING";
const LOCAL_PROPERTIES_KEYS: [&str; 4] = [
    "ANDROID_KEYSTORE_PATH",
    "ANDROID_KEYSTORE_PASSWORD",
    "ANDROID_KEY_ALIAS",
    "ANDROID_KEY_PASSWORD",
];

fn signing_property_line(line: &str) -> bool {
    let line = line.trim_start();
    LOCAL_PROPERTIES_KEYS.iter().any(|key| {
        line.strip_prefix(key).is_some_and(|rest| {
            rest.chars()
                .next()
                .is_some_and(|ch| ch == '=' || ch == ':' || ch.is_whitespace())
        })
    })
}

fn local_properties_path(value: &str) -> String {
    let (value, windows_path) = if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        (format!(r"\\{rest}"), true)
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        (rest.to_owned(), true)
    } else {
        let windows_drive = value.as_bytes().get(1) == Some(&b':')
            && value.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
        let unc = value.starts_with(r"\\");
        (value.to_owned(), windows_drive || unc)
    };
    if windows_path {
        value.replace('\\', "/")
    } else {
        value
    }
}

fn escape_property_value(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\u{000c}' => escaped.push_str("\\f"),
            ' ' | '=' | ':' | '#' | '!' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn local_properties_content(existing: &str, passwords: &SigningPasswords) -> Result<String> {
    let key = passwords.key.as_deref().context("Missing key password.")?;
    let normalized = existing.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = Vec::new();
    let mut managed = false;
    for line in normalized.lines() {
        if line == LOCAL_PROPERTIES_BEGIN {
            ensure!(!managed, "local.properties contains nested managed signing blocks.");
            managed = true;
            continue;
        }
        if line == LOCAL_PROPERTIES_END {
            ensure!(managed, "local.properties contains an unmatched managed signing block end.");
            managed = false;
            continue;
        }
        if !managed && !signing_property_line(line) {
            lines.push(line.to_owned());
        }
    }
    ensure!(
        !managed,
        "local.properties contains an incomplete managed signing block."
    );
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(LOCAL_PROPERTIES_BEGIN.into());
    lines.push(format!(
        "ANDROID_KEYSTORE_PATH={}",
        local_properties_path(&passwords.keystore)
    ));
    lines.push(format!(
        "ANDROID_KEYSTORE_PASSWORD={}",
        escape_property_value(&passwords.store)
    ));
    lines.push(format!(
        "ANDROID_KEY_ALIAS={}",
        escape_property_value(&passwords.alias)
    ));
    if key != passwords.store {
        lines.push(format!(
            "ANDROID_KEY_PASSWORD={}",
            escape_property_value(key)
        ));
    }
    lines.push(LOCAL_PROPERTIES_END.into());
    Ok(lines.join("\n") + "\n")
}

fn ensure_local_properties_ignored(root: &Path) -> Result<()> {
    if !root.join(".git").exists() {
        return Ok(());
    }
    let tracked = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", "local.properties"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Git is required to verify local.properties safety.")?;
    ensure!(
        !tracked.success(),
        "Refusing to write signing passwords to tracked local.properties."
    );
    ensure!(
        tracked.code() == Some(1),
        "Could not determine whether local.properties is tracked."
    );
    let ignored = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "-q", "--", "local.properties"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Git is required to verify local.properties safety.")?;
    ensure!(
        ignored.success(),
        "Add local.properties to .gitignore before storing signing passwords there."
    );
    Ok(())
}

fn write_local_properties(path: &Path, content: &str) -> Result<()> {
    match path.symlink_metadata() {
        Ok(metadata) => ensure!(
            metadata.is_file(),
            "local.properties must be a regular file, not a directory or symlink."
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .context("Could not replace local.properties.")?;
    Ok(())
}

pub fn configure_local_gradle_signing(root: &Path, vault: &SetupVault) -> Result<bool> {
    let passwords = vault
        .state
        .signing_passwords
        .as_ref()
        .context("No saved APK signing identity. Run APK signing key setup first.")?;
    ensure!(
        passwords.key.as_deref().is_some_and(|key| !key.is_empty()),
        "No saved key password. Run APK signing key setup first."
    );
    ensure!(
        regular_file_exists(Path::new(&passwords.keystore))?,
        "The saved APK signing keystore no longer exists. Run APK signing key setup again."
    );
    ensure_local_properties_ignored(root)?;
    let path = root.join("local.properties");
    let existing = match fs::read_to_string(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let content = local_properties_content(&existing, passwords)?;
    setup_message!(
        "Local Gradle signing will store the keystore path and signing passwords in plaintext at {}.",
        path.display()
    );
    setup_message!("Existing non-signing local.properties entries will be preserved.");
    if !confirm("Write Android signing credentials to ignored local.properties?")? {
        return Ok(false);
    }
    write_local_properties(&path, &content)?;
    setup_message!("Local Gradle signing configured in {}.", path.display());
    Ok(true)
}

fn choose_application_ids(root: &Path, saved: &[String]) -> Result<Vec<String>> {
    let defaults = associations::application_ids(root, saved)?;
    if !defaults.is_empty() {
        setup_message!("Android application IDs: {}", defaults.join(", "));
        if use_saved("Android application IDs")? {
            return Ok(defaults);
        }
    }
    loop {
        let joined = defaults.join(", ");
        let value = prompt(
            "Android application IDs (comma-separated installed package names)",
            (!joined.is_empty()).then_some(joined.as_str()),
        )?;
        let ids = value.split(',').map(str::to_owned).collect::<Vec<_>>();
        match associations::validate_ids(&ids) {
            Ok(ids) => return Ok(ids),
            Err(error) => setup_message!("{error}"),
        }
    }
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
                setup_message!("To create or refresh the public Android association file, select APK signing key and verify your existing local keystore.");
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
    let application_ids = choose_application_ids(root, &vault.state.android_application_ids)?;
    vault.state.android_application_ids = application_ids.clone();
    vault.save()?;
    setup_message!("Repository {}", config.repository);
    setup_message!("Keystore {}", path.display());
    setup_message!("Android application IDs: {}", application_ids.join(", "));
    setup_message!(
        "Write public signing associations to {}",
        associations::path(config).display()
    );
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
    prepare_signing_files(&keytool, config, passwords, &application_ids, reuse)?;
    setup_message!(
        "Keystore ready at {}. Keep an offline backup.",
        path.display()
    );
    setup_message!("Public Android associations: {}. Import this JSON in your backend's association manager for the intended environment.", associations::path(config).display());
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
    fn local_properties_preserve_other_values_and_replace_signing_entries() -> Result<()> {
        let passwords = SigningPasswords {
            keystore: r"C:\Users\David\.projects\demo\secrets\release key.jks".into(),
            alias: "release:key".into(),
            store: "store pass:=#!".into(),
            key: Some("different key".into()),
        };
        let existing = "sdk.dir=C:\\\\Android\\Sdk\nANDROID_KEYSTORE_PATH=old.jks\ncustom=value\n";
        let rendered = local_properties_content(existing, &passwords)?;
        assert!(rendered.contains("sdk.dir=C:\\\\Android\\Sdk\n"));
        assert!(rendered.contains("custom=value\n"));
        assert!(!rendered.contains("ANDROID_KEYSTORE_PATH=old.jks"));
        assert!(rendered.contains("ANDROID_KEYSTORE_PATH=C\\:\\\\Users\\\\David\\\\.projects\\\\demo\\\\secrets\\\\release\\ key.jks"));
        assert!(rendered.contains("ANDROID_KEYSTORE_PASSWORD=store\\ pass\\:\\=\\#\\!"));
        assert!(rendered.contains("ANDROID_KEY_ALIAS=release\\:key"));
        assert!(rendered.contains("ANDROID_KEY_PASSWORD=different\\ key"));
        assert_eq!(rendered.matches(LOCAL_PROPERTIES_BEGIN).count(), 1);
        assert_eq!(rendered.matches(LOCAL_PROPERTIES_END).count(), 1);
        Ok(())
    }

    #[test]
    fn local_properties_strip_windows_verbatim_prefixes() {
        assert_eq!(
            local_properties_path(r"\\?\C:\Users\David\.projects\auth\release.jks"),
            "C:/Users/David/.projects/auth/release.jks",
        );
        assert_eq!(
            local_properties_path(r"\\?\UNC\server\share\release.jks"),
            "//server/share/release.jks",
        );
        assert_eq!(
            local_properties_path(r"C:\Users\David\release.jks"),
            "C:/Users/David/release.jks",
        );
        assert_eq!(local_properties_path("/home/david/release.jks"), "/home/david/release.jks");
    }

    #[test]
    fn local_properties_omit_redundant_key_password_and_replace_managed_block() -> Result<()> {
        let passwords = SigningPasswords {
            keystore: "/tmp/release.jks".into(),
            alias: "release".into(),
            store: "same-password".into(),
            key: Some("same-password".into()),
        };
        let existing = format!(
            "sdk.dir=/sdk\n{LOCAL_PROPERTIES_BEGIN}\nANDROID_KEYSTORE_PATH=old\nANDROID_KEYSTORE_PASSWORD=old\nANDROID_KEY_ALIAS=old\nANDROID_KEY_PASSWORD=old\n{LOCAL_PROPERTIES_END}\n"
        );
        let rendered = local_properties_content(&existing, &passwords)?;
        assert!(rendered.starts_with("sdk.dir=/sdk\n\n"));
        assert!(rendered.contains("ANDROID_KEYSTORE_PATH=/tmp/release.jks"));
        assert!(rendered.contains("ANDROID_KEYSTORE_PASSWORD=same-password"));
        assert!(rendered.contains("ANDROID_KEY_ALIAS=release"));
        assert!(!rendered.contains("ANDROID_KEY_PASSWORD="));
        assert_eq!(rendered.matches(LOCAL_PROPERTIES_BEGIN).count(), 1);
        Ok(())
    }

    #[test]
    fn local_properties_reject_incomplete_managed_block() {
        let passwords = SigningPasswords {
            keystore: "/tmp/release.jks".into(),
            alias: "release".into(),
            store: "password".into(),
            key: Some("password".into()),
        };
        assert!(local_properties_content(
            &format!("{LOCAL_PROPERTIES_BEGIN}\nANDROID_KEYSTORE_PATH=old\n"),
            &passwords,
        )
        .is_err());
    }

    #[test]
    fn signing_rejects_export_keystore_collision_before_mutation() -> Result<()> {
        let keytool = find_keytool()?;
        let scratch = tempfile::tempdir()?;
        let config = SetupConfig::new(
            "owner/example".into(),
            "release".into(),
            "main".into(),
            Some(scratch.path().to_owned()),
            None,
            None,
        )?;
        let output = associations::path(&config);
        // The lexical paths differ but refer to the same file, even before it exists.
        fs::create_dir_all(parent(&output).join("nested"))?;
        let alias_path = parent(&output)
            .join("nested/..")
            .join(output.file_name().unwrap());
        let passwords = SigningPasswords {
            keystore: alias_path.to_str().unwrap().to_owned(),
            alias: "test".into(),
            store: "fixture-store".into(),
            key: Some("fixture-key".into()),
        };
        prepare_keystore(
            &keytool,
            &output,
            "test",
            "fixture-store",
            "fixture-key",
            false,
        )?;
        let original = fs::read(&output)?;
        let ids = vec!["com.example.app".into()];
        assert!(
            prepare_signing_files(&keytool, &config, &passwords, &ids, true).is_err(),
            "export accepted the selected keystore as its destination"
        );
        assert_eq!(fs::read(&output)?, original);
        fs::remove_file(&output)?;
        assert!(prepare_signing_files(&keytool, &config, &passwords, &ids, false).is_err());
        assert!(
            !output.exists(),
            "collision was detected only after creating a key"
        );
        Ok(())
    }

    #[test]
    fn signing_files_export_real_certificate_identity_on_create_and_reuse() -> Result<()> {
        let keytool = find_keytool()?;
        let scratch = tempfile::tempdir()?;
        let config = SetupConfig::new(
            "owner/example".into(),
            "release".into(),
            "main".into(),
            Some(scratch.path().to_owned()),
            None,
            None,
        )?;
        let mut passwords = SigningPasswords {
            keystore: scratch
                .path()
                .join("example.jks")
                .to_str()
                .unwrap()
                .to_owned(),
            alias: "test".into(),
            store: "fixture-store".into(),
            key: Some("fixture-key".into()),
        };
        let ids = vec!["com.example.app".into()];
        prepare_signing_files(&keytool, &config, &passwords, &ids, false)?;
        let output = crate::associations::path(&config);
        assert!(
            output.is_file(),
            "signing setup did not create the public association file"
        );
        let original_key = fs::read(&passwords.keystore)?;
        let original_export = fs::read(&output)?;
        let value: Value = serde_json::from_slice(&original_export)?;
        let fingerprint = value[0]["sha256CertFingerprints"][0].as_str().unwrap();
        let listing = Command::new(&keytool)
            .args([
                "-J-Duser.language=en",
                "-J-Duser.country=US",
                "-list",
                "-v",
                "-keystore",
                &passwords.keystore,
                "-alias",
                &passwords.alias,
                "-storepass:env",
                "ASSOCIATION_TEST_PASSWORD",
            ])
            .env("ASSOCIATION_TEST_PASSWORD", &passwords.store)
            .output()?;
        assert!(listing.status.success());
        assert!(
            String::from_utf8_lossy(&listing.stdout).contains(&format!("SHA256: {fingerprint}"))
        );
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["packageName"], "com.example.app");
        fs::remove_file(&output)?;
        prepare_signing_files(&keytool, &config, &passwords, &ids, true)?;
        assert_eq!(fs::read(&output)?, original_export);
        assert_eq!(fs::read(&passwords.keystore)?, original_key);
        passwords.store = "incorrect-password".into();
        assert!(prepare_signing_files(&keytool, &config, &passwords, &ids, true).is_err());
        assert_eq!(fs::read(&output)?, original_export);
        Ok(())
    }

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
