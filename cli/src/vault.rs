use anyhow::{bail, ensure, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD, Engine};
use crossterm::{
    cursor::{MoveTo, MoveToColumn},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use crypto_secretbox::{
    aead::{AeadInPlace, KeyInit},
    Nonce, Tag, XSalsa20Poly1305,
};
use fs2::FileExt;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;
use unicode_casefold::UnicodeCaseFold;

const MAX_VAULT: usize = 1024 * 1024;
const DEFAULT_OPS: u32 = 3;
const DEFAULT_MEM: u32 = 256 * 1024 * 1024;
const INTERACTIVE_OPS: u32 = 2;
const INTERACTIVE_MEM: u32 = 64 * 1024 * 1024;
const MAX_OPS: u32 = 4;
const MAX_MEM: u32 = 1024 * 1024 * 1024;
const UNLOCK_ERROR: &str =
    "Could not unlock the setup vault. Check the password or restore your backup.";

pub struct SetupConfig {
    pub repository: String,
    pub environment: String,
    pub branch: String,
    pub state_dir: PathBuf,
    pub vault_path: Option<PathBuf>,
    pub key_alias: Option<String>,
}

impl SetupConfig {
    pub fn new(
        repository: String,
        environment: String,
        branch: String,
        state_dir: Option<PathBuf>,
        vault_path: Option<PathBuf>,
        key_alias: Option<String>,
    ) -> Result<Self> {
        crate::github::validate_repository(&repository)?;
        ensure!(
            !environment.trim().is_empty() && !branch.trim().is_empty(),
            "Environment and branch cannot be blank."
        );
        ensure!(
            !branch.contains(['*', '?', '[']),
            "Supply an exact branch name without wildcard characters."
        );
        if let Some(alias) = &key_alias {
            validate_alias(alias)?;
        }
        if let Some(path) = &vault_path {
            ensure!(
                path.file_name()
                    .is_some_and(|name| !name.to_string_lossy().ends_with(".lock"))
                    && !path.as_os_str().to_string_lossy().ends_with("/.")
                    && !path.as_os_str().to_string_lossy().ends_with("\\."),
                "Vault path must be a valid file path not ending in .lock."
            );
        }
        let state_dir = match state_dir {
            Some(path) => expand_home(&path)?,
            None => home()?.join(".projects"),
        };
        Ok(Self {
            repository,
            environment,
            branch,
            state_dir,
            vault_path: vault_path.map(|path| expand_home(&path)).transpose()?,
            key_alias,
        })
    }

    pub fn project_name(&self) -> String {
        self.repository
            .split_once('/')
            .unwrap()
            .1
            .to_ascii_lowercase()
    }

    pub fn alias(&self) -> String {
        if let Some(alias) = &self.key_alias {
            return alias.clone();
        }
        let name = self.project_name();
        let alias = name.trim_start_matches(|ch: char| !ch.is_ascii_alphanumeric());
        if alias.is_empty() {
            "signing".to_owned()
        } else {
            alias.chars().take(64).collect()
        }
    }

    pub fn project_secrets_dir(&self) -> PathBuf {
        self.state_dir.join(self.project_name()).join("secrets")
    }

    pub fn state_path(&self) -> PathBuf {
        if let Some(path) = &self.vault_path {
            return path.clone();
        }
        let identity = Sha256::digest(format!(
            "{}:{}",
            self.repository.case_fold().collect::<String>(),
            self.environment.case_fold().collect::<String>()
        ));
        self.project_secrets_dir()
            .join(format!("setup-{}.vault", &format!("{identity:x}")[..16]))
    }
}

pub fn validate_alias(alias: &str) -> Result<()> {
    ensure!(
        alias.len() <= 64
            && alias.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
            && alias.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
        "Use a key alias with 1–64 letters, digits, dots, underscores or hyphens, starting with a letter or digit."
    );
    Ok(())
}

fn home() -> Result<PathBuf> {
    std::env::home_dir().context("Could not determine the home directory.")
}

pub fn expand_home(path: &Path) -> Result<PathBuf> {
    if let Ok(suffix) = path.strip_prefix("~") {
        return Ok(home()?.join(suffix));
    }
    Ok(path.to_path_buf())
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

pub fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

pub fn regular_file_exists(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            ensure!(metadata.is_file(), "Choose a regular file path.");
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn file_options() -> OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        options
    }
    #[cfg(not(unix))]
    OpenOptions::new()
}

pub fn read_regular_file(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    if !regular_file_exists(path)? {
        return Ok(None);
    }
    let file = file_options().read(true).open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= limit as u64,
        "File must be regular and within size limits."
    );
    let mut data = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut data)?;
    ensure!(data.len() <= limit, "File exceeds the size limit.");
    Ok(Some(data))
}

pub fn lock_file(path: &Path, contention: &str) -> Result<File> {
    regular_file_exists(path)?;
    let file = file_options()
        .read(true)
        .write(true)
        .create(true)
        .open(path)?;
    ensure!(file.metadata()?.is_file(), "Lock must be a regular file.");
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(file),
        Err(error) if error.kind() == fs2::lock_contended_error().kind() => bail!("{contention}"),
        Err(error) => Err(error.into()),
    }
}

pub fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

pub struct VaultLock {
    _file: File,
}

pub fn lock(config: &SetupConfig, migrate: bool) -> Result<VaultLock> {
    let path = config.state_path();
    private_dir(parent(&path))?;
    let contention =
        "Another setup wizard is running for this repository. Close it before retrying.";
    let file = lock_file(&lock_path(&path), contention)?;
    if migrate && config.vault_path.is_none() && !regular_file_exists(&path)? {
        let legacy = config
            .state_dir
            .join(config.repository.to_ascii_lowercase().replace('/', "--"))
            .join("secrets")
            .join(path.file_name().unwrap());
        if legacy
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file())
        {
            let _legacy_lock = lock_file(&lock_path(&legacy), contention)?;
            fs::rename(&legacy, &path)
                .context("Could not move the existing vault to the project secrets folder.")?;
        }
    }
    Ok(VaultLock { _file: file })
}

fn present_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn present_passwords<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SigningPasswords>, D::Error> {
    SigningPasswords::deserialize(deserializer).map(Some)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultState {
    pub pending: Vec<String>,
    pub complete: Vec<String>,
    #[serde(
        default,
        deserialize_with = "present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub keystore: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub alias: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub github_token: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_passwords",
        skip_serializing_if = "Option::is_none"
    )]
    pub signing_passwords: Option<SigningPasswords>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub publishing_values: BTreeMap<String, BTreeMap<String, String>>,
}

impl VaultState {
    fn validate(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for step in self.pending.iter().chain(&self.complete) {
            ensure!(
                matches!(step.as_str(), "key" | "github") && seen.insert(step),
                "Invalid setup progress."
            );
        }
        Ok(())
    }

    pub fn start(&mut self, steps: &[&str]) {
        for step in steps {
            self.complete.retain(|entry| entry != step);
            if !self.pending.iter().any(|entry| entry == step) {
                self.pending.push((*step).to_owned());
            }
        }
    }

    pub fn finish(&mut self, step: &str) {
        self.pending.retain(|entry| entry != step);
        if !self.complete.iter().any(|entry| entry == step) {
            self.complete.push(step.to_owned());
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SigningPasswords {
    pub keystore: String,
    pub alias: String,
    pub store: String,
    #[serde(
        default,
        deserialize_with = "present_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub key: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u8,
    salt: String,
    ciphertext: String,
    #[serde(default = "interactive_ops")]
    opslimit: u32,
    #[serde(default = "interactive_mem")]
    memlimit: u32,
}

fn interactive_ops() -> u32 {
    INTERACTIVE_OPS
}

fn interactive_mem() -> u32 {
    INTERACTIVE_MEM
}

pub struct SetupVault {
    path: PathBuf,
    salt: [u8; 16],
    cipher: XSalsa20Poly1305,
    opslimit: u32,
    memlimit: u32,
    pub state: VaultState,
}

fn cipher(password: &str, salt: &[u8; 16], ops: u32, mem: u32) -> Result<XSalsa20Poly1305> {
    ensure!((1..=MAX_OPS).contains(&ops) && (8192..=MAX_MEM).contains(&mem));
    // libsodium expresses its memory limit in bytes, whereas Argon2 uses KiB.
    let params =
        Params::new(mem / 1024, ops, 1, Some(32)).map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))?;
    let mut key = crypto_secretbox::Key::default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))?;
    Ok(XSalsa20Poly1305::new(&key))
}

pub fn random_bytes(buffer: &mut [u8]) -> Result<()> {
    getrandom::fill(buffer).map_err(|_| anyhow::anyhow!("Could not obtain secure random bytes."))
}

impl SetupVault {
    pub fn open(path: &Path, password: &str) -> Result<Self> {
        Self::read(path, password).map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))
    }

    fn read(path: &Path, password: &str) -> Result<Self> {
        let bytes = read_regular_file(path, MAX_VAULT)?.context(UNLOCK_ERROR)?;
        let envelope: Envelope = serde_json::from_slice(&bytes)?;
        ensure!(envelope.version == 1, "Unsupported vault version.");
        let salt: [u8; 16] = STANDARD
            .decode(envelope.salt)?
            .try_into()
            .map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))?;
        let ciphertext = STANDARD.decode(envelope.ciphertext)?;
        ensure!(ciphertext.len() >= 40, "Incomplete ciphertext.");
        let cipher = cipher(password, &salt, envelope.opslimit, envelope.memlimit)?;
        let mut plaintext = ciphertext[40..].to_vec();
        cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&ciphertext[..24]),
                b"",
                &mut plaintext,
                Tag::from_slice(&ciphertext[24..40]),
            )
            .map_err(|_| anyhow::anyhow!(UNLOCK_ERROR))?;
        let state: VaultState = serde_json::from_slice(&plaintext)?;
        state.validate()?;
        Ok(Self {
            path: path.to_path_buf(),
            salt,
            cipher,
            opslimit: envelope.opslimit,
            memlimit: envelope.memlimit,
            state,
        })
    }

    fn create(path: &Path, password: &str) -> Result<Self> {
        Self::create_with_limits(path, password, DEFAULT_OPS, DEFAULT_MEM)
    }

    fn create_with_limits(
        path: &Path,
        password: &str,
        opslimit: u32,
        memlimit: u32,
    ) -> Result<Self> {
        let mut salt = [0; 16];
        random_bytes(&mut salt)?;
        let cipher = cipher(password, &salt, opslimit, memlimit)?;
        Ok(Self {
            path: path.to_path_buf(),
            salt,
            cipher,
            opslimit,
            memlimit,
            state: VaultState::default(),
        })
    }

    pub fn save(&self) -> Result<()> {
        self.state.validate()?;
        regular_file_exists(&self.path)?;
        let mut encrypted = serde_json::to_vec(&self.state)?;
        let mut nonce = Nonce::default();
        random_bytes(&mut nonce)?;
        let tag = self
            .cipher
            .encrypt_in_place_detached(&nonce, b"", &mut encrypted)
            .map_err(|_| anyhow::anyhow!("Could not encrypt the setup vault."))?;
        let envelope = Envelope {
            version: 1,
            salt: STANDARD.encode(self.salt),
            ciphertext: STANDARD.encode([nonce.as_slice(), tag.as_slice(), &encrypted].concat()),
            opslimit: self.opslimit,
            memlimit: self.memlimit,
        };
        let mut data = serde_json::to_vec(&envelope)?;
        data.push(b'\n');
        ensure!(
            data.len() <= MAX_VAULT,
            "Vault file exceeds the size limit."
        );
        private_dir(parent(&self.path))?;
        let mut file = NamedTempFile::new_in(parent(&self.path))
            .context("Could not create a temporary setup vault.")?;
        file.write_all(&data)
            .context("Could not write the temporary setup vault.")?;
        file.as_file()
            .sync_all()
            .context("Could not flush the temporary setup vault.")?;
        regular_file_exists(&self.path)?;
        file.persist(&self.path)
            .map_err(|error| error.error)
            .context("Could not replace the setup vault.")?;
        Ok(())
    }

    fn formatted(&self, reveal: bool) -> Result<String> {
        let mut value = serde_json::to_value(&self.state)?;
        if !reveal {
            for field in ["github_token", "signing_passwords"] {
                if let Some(secret) = value.get_mut(field) {
                    *secret = json!("[hidden]");
                }
            }
            if let Some(Value::Object(environments)) = value.get_mut("publishing_values") {
                for entries in environments.values_mut().filter_map(Value::as_object_mut) {
                    for secret in entries.values_mut() {
                        *secret = json!("[hidden]");
                    }
                }
            }
        }
        Ok(serde_json::to_string_pretty(&value)?)
    }
}

pub fn interactive() -> Result<()> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal() && io::stderr().is_terminal(),
        "Run this wizard in an interactive terminal without redirection."
    );
    Ok(())
}

struct RawTerminal {
    editing: bool,
}

impl RawTerminal {
    fn enable(editing: bool) -> Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self { editing };
        if editing {
            execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture)?;
        }
        Ok(guard)
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        if self.editing {
            let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        }
        let _ = terminal::disable_raw_mode();
    }
}

fn render_line(text: &str) -> Result<()> {
    execute!(
        io::stdout(),
        MoveToColumn(0),
        Clear(ClearType::CurrentLine),
        Print(text)
    )?;
    Ok(())
}

fn pressed_key(event: Event) -> Result<Option<KeyEvent>> {
    if let Event::Key(key) = event {
        if key.kind == KeyEventKind::Release {
            return Ok(None);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'q'))
        {
            bail!("Setup cancelled.");
        }
        return Ok(Some(key));
    }
    Ok(None)
}

fn timed_prompt(label: &str, default: &str) -> Result<String> {
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
    println!("{label} [{default}]");
    let _terminal = RawTerminal::enable(true)?;
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    let mut touched = false;
    let mut input = Vec::new();
    let mut cursor: usize = 0;
    let mut displayed_seconds = 0;
    let answer = loop {
        let event = if touched {
            let width = terminal::size()
                .ok()
                .map(|(cols, _)| cols as usize)
                .filter(|cols| *cols > 4)
                .unwrap_or(80)
                - 3;
            let start = (cursor + 1).saturating_sub(width);
            let line: String = input.iter().skip(start).take(width).collect();
            render_line(&format!("> {line}"))?;
            execute!(io::stdout(), MoveToColumn((2 + cursor - start) as u16))?;
            event::read()?
        } else {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break default.to_owned();
            }
            let seconds = remaining.as_millis().div_ceil(1000);
            if seconds != displayed_seconds {
                render_line(&format!(
                    "Default in {seconds}s. Enter to accept or type to pause."
                ))?;
                displayed_seconds = seconds;
            }
            if !event::poll(remaining.min(Duration::from_millis(200)))? {
                continue;
            }
            event::read()?
        };
        if matches!(&event, Event::Mouse(mouse) if matches!(mouse.kind, MouseEventKind::Down(_))) {
            touched = true;
            continue;
        }
        if let Event::Paste(pasted) = event {
            let pasted: Vec<_> = pasted.chars().filter(|ch| !ch.is_control()).collect();
            let len = pasted.len();
            input.splice(cursor..cursor, pasted);
            cursor += len;
            touched = true;
            continue;
        }
        let Some(key) = pressed_key(event)? else {
            continue;
        };
        touched = true;
        match key.code {
            KeyCode::Enter => {
                let input: String = input.into_iter().collect();
                let input = input.trim();
                break if input.is_empty() { default } else { input }.to_owned();
            }
            KeyCode::Esc => {
                render_line("Cancelled.\r\n")?;
                bail!("Setup cancelled.");
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                input.insert(cursor, ch);
                cursor += 1;
            }
            KeyCode::Backspace if cursor > 0 => {
                cursor -= 1;
                input.remove(cursor);
            }
            KeyCode::Delete if cursor < input.len() => {
                input.remove(cursor);
            }
            KeyCode::Left => cursor = cursor.saturating_sub(1),
            KeyCode::Right => cursor = (cursor + 1).min(input.len()),
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = input.len(),
            _ => {}
        }
    };
    render_line(&format!("> {answer}\r\n"))?;
    Ok(answer)
}

pub fn prompt(label: &str, default: Option<&str>) -> Result<String> {
    if let Some(default) = default {
        return timed_prompt(label, default);
    }
    print!("{label} ");
    io::stdout().flush()?;
    let mut answer = String::new();
    ensure!(io::stdin().read_line(&mut answer)? > 0, "Input closed.");
    Ok(answer.trim().to_owned())
}

pub fn secret_prompt(label: &str) -> Result<String> {
    Ok(rpassword::prompt_password(format!("{label} "))?)
}

pub fn confirm(label: &str) -> Result<bool> {
    println!("{label}");
    println!("Space toggles, Enter applies, Esc cancels.");
    let _terminal = RawTerminal::enable(false)?;
    let mut acknowledged = false;
    let confirmed = loop {
        let marker = if acknowledged { 'x' } else { ' ' };
        render_line(&format!("[{marker}] I understand and want to continue."))?;
        let Some(key) = pressed_key(event::read()?)? else {
            continue;
        };
        match key.code {
            KeyCode::Char(' ') if key.kind == KeyEventKind::Press => acknowledged = !acknowledged,
            KeyCode::Enter if acknowledged => break true,
            KeyCode::Esc => break false,
            _ => {}
        }
    };
    render_line(if confirmed {
        "Confirmed.\r\n"
    } else {
        "Cancelled.\r\n"
    })?;
    Ok(confirmed)
}

pub fn use_saved(label: &str) -> Result<bool> {
    loop {
        match prompt(
            &format!("{label} has a saved value. [1] Use vault value [2] Set a new value"),
            Some("1"),
        )?
        .as_str()
        {
            "1" => return Ok(true),
            "2" => return Ok(false),
            _ => println!("Enter 1 or 2."),
        }
    }
}

pub fn private_screen<T>(display: impl FnOnce() -> Result<T>) -> Result<T> {
    execute!(io::stdout(), EnterAlternateScreen, MoveTo(0, 0))?;
    let result = display();
    execute!(io::stdout(), Clear(ClearType::All), LeaveAlternateScreen)?;
    result
}

pub fn unlock_interactive(config: &SetupConfig, replace: bool) -> Result<SetupVault> {
    let path = config.state_path();
    if !replace && regular_file_exists(&path)? {
        return SetupVault::open(&path, &secret_prompt("Unlock local setup vault")?);
    }

    println!("Choose a vault password to encrypt saved setup answers on this device.");
    loop {
        let password = secret_prompt("Create vault password (at least 12 characters)")?;
        if password.chars().count() >= 12 && password == secret_prompt("Confirm vault password")? {
            let vault = SetupVault::create(&path, &password)?;
            vault.save()?;
            return Ok(vault);
        }
        println!("Use at least 12 characters and enter the same password twice.");
    }
}

pub fn view(config: &SetupConfig) -> Result<bool> {
    interactive()?;
    ensure!(
        regular_file_exists(&config.state_path())?,
        "There is no local setup vault to view. Run a setup step to create it."
    );
    let _lock = lock(config, false)?;
    let vault = loop {
        match SetupVault::open(
            &config.state_path(),
            &secret_prompt("Unlock local setup vault to view it")?,
        ) {
            Ok(vault) => break vault,
            Err(error) => {
                println!("{error}");
                if !confirm("Try again?")? {
                    return Ok(false);
                }
            }
        }
    };
    println!("Saved setup vault · read-only");
    println!("{}", vault.formatted(false)?);
    println!("Revealing displays passwords, tokens, and saved keys.");
    if confirm("I am in a private terminal. Reveal vault secrets?")? {
        private_screen(|| {
            println!("{}", vault.formatted(true)?);
            prompt("Press Enter to hide secrets and close.", None)?;
            Ok(())
        })?;
    }
    Ok(true)
}

pub fn remake(config: &SetupConfig) -> Result<bool> {
    interactive()?;
    let _lock = lock(config, true)?;
    println!("Remaking the local vault clears its saved credentials and setup progress.");
    println!("If the vault holds your only copy of a password you still need, recover it with View vault first.");
    if !confirm("Replace the local vault?")? {
        return Ok(false);
    }
    unlock_interactive(config, true)?;
    println!("Created a new encrypted setup vault.");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_and_opens_a_python_vault_then_preserves_saved_answers() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let config = SetupConfig::new(
            "Owner/Sample".into(),
            "demo-apk-release".into(),
            "main".into(),
            Some(scratch.path().to_owned()),
            None,
            None,
        )?;
        let path = config.state_path();
        let legacy = scratch
            .path()
            .join("owner--sample")
            .join("secrets")
            .join(path.file_name().unwrap());
        private_dir(parent(&legacy))?;
        fs::write(
            &legacy,
            include_bytes!("../tests/fixtures/python-setup.vault"),
        )?;
        let _lock = lock(&config, true)?;
        assert!(!legacy.exists());
        assert!(lock(&config, true).is_err());
        let mut vault = SetupVault::open(&path, "Synthetic test password")?;
        assert_eq!(vault.state.pending, ["github"]);
        assert_eq!(vault.state.complete, ["key"]);
        let masked = vault.formatted(false)?;
        assert!(masked.contains("PORTAL_TOKEN"));
        assert!(!masked.contains("fixture-"));
        assert!(vault.formatted(true)?.contains("fixture-portal-token"));
        vault.state.finish("github");
        vault.save()?;
        let saved = SetupVault::open(&path, "Synthetic test password")?;
        assert!(saved.state.pending.is_empty());
        assert_eq!(saved.state.complete, ["key", "github"]);
        assert_eq!(saved.state.github_token.as_deref(), Some("fixture-token"));
        assert!(SetupVault::open(&path, "wrong password").is_err());
        Ok(())
    }

    #[test]
    fn rejects_invalid_state_and_symlink_replacement() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let path = scratch.path().join("setup.vault");
        let mut vault = SetupVault::create_with_limits(&path, "Fixture password", 1, 8192)?;
        vault.state.start(&["key"]);
        vault.state.complete.push("key".into());
        assert!(vault.save().is_err());
        assert!(!path.exists());
        #[cfg(unix)]
        {
            vault.state.complete.clear();
            let target = scratch.path().join("target");
            fs::write(&target, "unchanged")?;
            std::os::unix::fs::symlink(&target, &path)?;
            assert!(vault.save().is_err());
            assert!(SetupVault::open(&path, "Fixture password").is_err());
            assert_eq!(fs::read_to_string(&target)?, "unchanged");
        }
        Ok(())
    }
}
