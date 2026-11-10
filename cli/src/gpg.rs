use anyhow::{anyhow, bail, ensure, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RSA_SIZES: [u16; 3] = [2048, 3072, 4096];
const LOOPBACK: [&str; 4] = ["--pinentry-mode", "loopback", "--passphrase-fd", "0"];
const SECRET_LIMIT: usize = 48 * 1024;

pub(crate) struct VerifiedKey {
    pub fingerprint: String,
    pub armor: String,
    pub passphrase: String,
}

pub(crate) struct SigningKey {
    pub fingerprint: String,
    pub identity: String,
    pub created: String,
    pub expires: String,
}

struct KeyDefaults {
    identity: String,
    bits: u16,
    expires: String,
}

struct DuplicateKeyIdentity(String);

impl fmt::Display for DuplicateKeyIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for DuplicateKeyIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl std::error::Error for DuplicateKeyIdentity {}

fn run_command(command: &mut Command, data: &[u8], timeout: Duration) -> io::Result<Output> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(data));
        let output = scope.spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        });
        let errors = scope.spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(result
                        .err()
                        .unwrap_or_else(|| io::Error::from(io::ErrorKind::TimedOut)));
                }
            }
        };
        let written = writer.join().unwrap();
        if status.success() {
            written?;
        }
        Ok(Output {
            status,
            stdout: output.join().unwrap()?,
            stderr: errors.join().unwrap()?,
        })
    })
}

fn log_error(home: &Path, record: Value) -> io::Result<()> {
    let directory = home.join(".projects").join(".default");
    fs::create_dir_all(&directory)?;
    let path = directory.join("gpg-errors.log");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut log = options.open(path)?;
    if !log.metadata()?.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    log.write_all(format!("{record}\n").as_bytes())
}

fn gpg_error(operation: &str, output: io::Result<Output>, home: Option<&Path>) -> anyhow::Error {
    let stderr = output.as_ref().map_or_else(
        |_| String::new(),
        |output| String::from_utf8_lossy(&output.stderr).to_ascii_lowercase(),
    );
    let mut codes = Vec::new();
    if let Ok(output) = &output {
        let pattern =
            Regex::new(r"(?m)^\[GNUPG:\] (?:ERROR|FAILURE) [a-zA-Z0-9_-]+ ([0-9]{1,10})\r?$")
                .unwrap();
        for stream in [&output.stdout, &output.stderr] {
            codes.extend(
                pattern
                    .captures_iter(&String::from_utf8_lossy(stream))
                    .filter_map(|capture| capture[1].parse::<u32>().ok()),
            );
        }
    }
    codes.sort_unstable();
    codes.dedup();
    let duplicate = operation == "create key"
        && (codes.iter().any(|code| code & 0xffff == 304) || stderr.contains("already exists"));
    let contains = |markers: &[&str]| markers.iter().any(|marker| stderr.contains(marker));
    let reason = match output.as_ref().err().map(io::Error::kind) {
        Some(io::ErrorKind::TimedOut) => "GnuPG timed out.",
        Some(io::ErrorKind::NotFound) => {
            "GnuPG was not found. Install it and make sure gpg is on PATH."
        }
        _ if contains(&[
            "no agent running",
            "failed to start gpg-agent",
            "can't connect to the agent",
            "can't connect to the gpg-agent",
        ]) =>
        {
            "GnuPG could not start or connect to gpg-agent."
        }
        _ if duplicate => "A key with this identity already exists.",
        _ if contains(&["bad passphrase", "bad_passphrase"])
            || codes.iter().any(|code| code & 0xffff == 11) =>
        {
            "GnuPG rejected the passphrase."
        }
        _ if contains(&["permission denied", "access denied"]) => {
            "GnuPG could not access a required file or keyring."
        }
        _ if contains(&["no secret key", "no_seckey"]) => "GnuPG could not access the private key.",
        _ if stderr.contains("keyserver") => "GnuPG could not complete the keyserver request.",
        _ => "GnuPG returned an unrecognized error.",
    };
    let exit_code = output.as_ref().ok().and_then(|output| output.status.code());
    let mut message = format!("Could not {operation}. {reason}");
    if let Some(code) = exit_code {
        message.push_str(&format!(" Exit code: {code}."));
    }
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| {
            let seconds = time.as_secs();
            let date = key_date(&seconds.to_string(), "1970-01-01");
            let hour = seconds / 3600 % 24;
            let minute = seconds / 60 % 60;
            let second = seconds % 60;
            format!("{date}T{hour:02}:{minute:02}:{second:02}+00:00")
        });
    let record = json!({
        "time": time,
        "operation": operation,
        "reason": reason,
        "exit_code": exit_code,
        "os_error": output.err().and_then(|error| error.raw_os_error()),
        "gpg_status_codes": codes,
    });
    if home.is_some_and(|home| log_error(home, record).is_ok()) {
        message.push_str(" See ~/.projects/.default/gpg-errors.log.");
    } else {
        message.push_str(" The diagnostic log could not be written.");
    }
    if duplicate {
        DuplicateKeyIdentity(message).into()
    } else {
        anyhow!(message)
    }
}

fn gpg_tool(name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        // Git for Windows ships an MSYS GPG that interprets Windows keyring
        // paths differently. Use both programs from the native installation.
        for root in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(PathBuf::from)
            .filter(|root| root.is_absolute())
        {
            let bin = root.join("GnuPG").join("bin");
            if bin.join("gpg.exe").is_file() && bin.join("gpgconf.exe").is_file() {
                return bin.join(format!("{name}.exe"));
            }
        }
    }
    PathBuf::from(name)
}

fn run_gpg(
    operation: &str,
    args: &[&str],
    data: &[u8],
    keyring: Option<&Path>,
    home: Option<&Path>,
) -> Result<Vec<u8>> {
    let mut command = Command::new(gpg_tool("gpg"));
    if let Some(keyring) = keyring {
        // The verification agent must use the fresh keyring's configuration.
        command
            .arg("--no-options")
            .arg("--homedir")
            .arg(keyring)
            .current_dir(keyring);
    }
    command.args(["--batch", "--no-tty"]);
    if !args.contains(&"--status-fd") {
        command.args(["--status-fd", "2"]);
    }
    match run_command(command.args(args), data, Duration::from_secs(120)) {
        Ok(output) if output.status.success() => Ok(output.stdout),
        output => Err(gpg_error(operation, output, home)),
    }
}

fn is_fingerprint(value: &str) -> bool {
    (40..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
}

fn key_date(value: &str, missing: &str) -> String {
    if value.is_empty() || value == "0" {
        return missing.to_owned();
    }
    if let Ok(seconds) = value.parse::<u64>() {
        if seconds <= 253_402_300_799 {
            // Convert days since 1970 to the Gregorian date using 400-year cycles.
            let days = seconds / 86400 + 719468;
            let era = days / 146097;
            let day_of_era = days % 146097;
            let year_of_era =
                (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
            let day_of_year =
                day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
            let month_from_march = (5 * day_of_year + 2) / 153;
            let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
            let month = (month_from_march + 2) % 12 + 1;
            let year = era * 400 + year_of_era + u64::from(month <= 2);
            return format!("{year:04}-{month:02}-{day:02}");
        }
    } else if let Some(capture) =
        Regex::new(r"\A([0-9]{4})([0-9]{2})([0-9]{2})T(?:[01][0-9]|2[0-3])[0-5][0-9][0-5][0-9]\z")
            .unwrap()
            .captures(value)
    {
        let year = capture[1].parse::<u16>().unwrap();
        let month = capture[2].parse::<usize>().unwrap();
        let day = capture[3].parse::<u8>().unwrap();
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let month_days = [
            0,
            31,
            28 + u8::from(leap),
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        if year > 0 && day > 0 && day <= *month_days.get(month).unwrap_or(&0) {
            return format!("{}-{}-{}", &capture[1], &capture[2], &capture[3]);
        }
    }
    "Unknown".to_owned()
}

fn display_identity(identity: &str) -> String {
    // GnuPG escapes colons and control bytes as \xNN in its machine-readable output.
    let mut decoded = Vec::new();
    let mut bytes = identity.as_bytes();
    while let Some((&byte, rest)) = bytes.split_first() {
        if bytes.starts_with(b"\\x") {
            if let Some(hex) = bytes
                .get(2..4)
                .and_then(|digits| std::str::from_utf8(digits).ok())
            {
                if let Ok(decoded_byte) = u8::from_str_radix(hex, 16) {
                    decoded.push(decoded_byte);
                    bytes = &bytes[4..];
                    continue;
                }
            }
        }
        decoded.push(byte);
        bytes = rest;
    }
    String::from_utf8_lossy(&decoded)
        .chars()
        .filter(|character| !character.is_control())
        .collect()
}

pub(crate) fn parse_signing_keys(listing: &str) -> Vec<SigningKey> {
    let mut keys = Vec::new();
    let mut primary = None;
    for line in listing.lines() {
        let fields: Vec<_> = line.split(':').collect();
        let valid = !matches!(fields.get(1).copied(), Some("r" | "e" | "d" | "i"));
        match fields.as_slice() {
            ["sec", _, _, _, _, created, expires, _, _, _, _, capabilities, ..] => {
                primary = (valid && capabilities.contains('S')).then_some(("", *created, *expires));
            }
            ["sec" | "ssb", ..] => primary = None,
            ["fpr", _, _, _, _, _, _, _, _, fingerprint, ..] if is_fingerprint(fingerprint) => {
                if let Some((slot, _, _)) = primary.as_mut() {
                    if slot.is_empty() {
                        *slot = fingerprint;
                    }
                }
            }
            ["uid", _, _, _, _, _, _, _, _, identity, ..] if valid => {
                if let Some((fingerprint, created, expires)) = primary.take() {
                    if !fingerprint.is_empty() {
                        keys.push(SigningKey {
                            fingerprint: fingerprint.to_owned(),
                            identity: display_identity(identity),
                            created: key_date(created, "Unknown"),
                            expires: key_date(expires, "No expiry"),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    keys
}

pub(crate) fn validate_signing_key(value: &str) -> Result<&str> {
    let value = value.trim();
    let body = value.strip_prefix("-----BEGIN PGP PRIVATE KEY BLOCK-----");
    ensure!(
        body.is_some_and(|body| body.starts_with('\n') || body.starts_with("\r\n"))
            && value.ends_with("-----END PGP PRIVATE KEY BLOCK-----"),
        "Use an ASCII-armored OpenPGP private key exported from GnuPG."
    );
    ensure!(
        value.len() <= SECRET_LIMIT,
        "The signing key exceeds GitHub's 48 KiB secret limit."
    );
    Ok(value)
}

fn validate_expiration(expiration: &str) -> Result<()> {
    ensure!(
        Regex::new(r"\A(?:0|[1-9][0-9]*[dwmy]?|[0-9]{4}-[0-9]{2}-[0-9]{2})\z")?
            .is_match(expiration),
        "Maven key expiration must be 0, a duration such as 2y, or YYYY-MM-DD."
    );
    Ok(())
}

fn load_key_defaults(home: &Path, project: &str) -> Result<KeyDefaults> {
    let mut defaults = KeyDefaults {
        identity: String::new(),
        bits: 3072,
        expires: "2y".into(),
    };
    let source = match fs::read_to_string(
        home.join(".projects")
            .join(".default")
            .join("maven-central.json"),
    ) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(defaults),
        Err(_) => {
            bail!("Could not read Maven key defaults from ~/.projects/.default/maven-central.json.")
        }
    };
    let value: Value =
        serde_json::from_str(source.trim_start_matches('\u{feff}')).map_err(|_| {
            anyhow!(
                "Could not read Maven key defaults from ~/.projects/.default/maven-central.json."
            )
        })?;
    let fields = value
        .as_object()
        .context("Maven key defaults must be a JSON object.")?;
    let string = |key: &str| fields.get(key).map_or(Some(""), Value::as_str);
    let name = string("Name")
        .context("Maven key defaults Name and Email must be strings.")?
        .trim();
    let email = string("Email")
        .context("Maven key defaults Name and Email must be strings.")?
        .trim();
    if !name.is_empty() && !email.is_empty() {
        defaults.identity = if project.is_empty() {
            format!("{name} <{email}>")
        } else {
            format!("{name} ({project}) <{email}>")
        };
    }
    if let Some(size) = fields.get("KeySize") {
        defaults.bits = size
            .as_u64()
            .and_then(|size| u16::try_from(size).ok())
            .or_else(|| size.as_str()?.parse().ok())
            .unwrap_or_default();
    }
    ensure!(
        fields
            .get("KeyType")
            .is_none_or(|kind| kind.as_str() == Some("rsa"))
            && RSA_SIZES.contains(&defaults.bits),
        "Maven key defaults must select RSA with 2048, 3072, or 4096 bits."
    );
    if let Some(expiration) = fields.get("Expires") {
        let expiration = expiration
            .as_str()
            .context("Maven key expiration must be 0, a duration such as 2y, or YYYY-MM-DD.")?;
        validate_expiration(expiration)?;
        defaults.expires = expiration.to_owned();
    }
    Ok(defaults)
}

fn passphrase_input(passphrase: &str) -> Result<Vec<u8>> {
    ensure!(
        !passphrase.contains(['\r', '\n', '\0']),
        "The OpenPGP passphrase must fit on one line."
    );
    let mut bytes = passphrase.as_bytes().to_vec();
    bytes.push(b'\n');
    Ok(bytes)
}

fn generate_key(
    identity: &str,
    passphrase: &str,
    bits: u16,
    expires: &str,
    allow_duplicate: bool,
    home: &Path,
) -> Result<String> {
    ensure!(
        Regex::new(r"\A[^<>\r\n\x00]+ <[^<>\s@\x00]+@[^<>\s@\x00]+>\z")?.is_match(identity),
        "Enter an OpenPGP identity in the form Name <email@example.com>."
    );
    ensure!(
        RSA_SIZES.contains(&bits),
        "Choose an RSA key size of 2048, 3072, or 4096 bits."
    );
    validate_expiration(expires)?;
    ensure!(
        passphrase.chars().count() >= 12,
        "Use at least 12 characters for a new key's passphrase."
    );
    let mut arguments = vec!["--status-fd", "1"];
    arguments.extend(LOOPBACK);
    if allow_duplicate {
        arguments.push("--yes");
    }
    let algorithm = format!("rsa{bits}");
    arguments.extend([
        "--quick-generate-key",
        "--",
        identity,
        &algorithm,
        "sign",
        expires,
    ]);
    let output = run_gpg(
        "create key",
        &arguments,
        &passphrase_input(passphrase)?,
        None,
        Some(home),
    )?;
    for line in String::from_utf8_lossy(&output).lines() {
        if let ["[GNUPG:]", "KEY_CREATED", "P" | "B" | "S", fingerprint] =
            line.split_whitespace().collect::<Vec<_>>().as_slice()
        {
            if is_fingerprint(fingerprint) {
                return Ok((*fingerprint).to_owned());
            }
        }
    }
    bail!("GnuPG did not report the fingerprint of the generated key.")
}

fn export_verified_key(fingerprint: &str, passphrase: &str, home: &Path) -> Result<String> {
    ensure!(is_fingerprint(fingerprint), "Invalid OpenPGP fingerprint.");
    let secret = passphrase_input(passphrase)?;
    let mut arguments = LOOPBACK.to_vec();
    arguments.extend(["--armor", "--export-secret-keys", fingerprint]);
    let armor = run_gpg("export private key", &arguments, &secret, None, Some(home))?;
    let armor = validate_signing_key(
        std::str::from_utf8(&armor)
            .map_err(|_| anyhow!("GnuPG did not export a UTF-8 armored private key."))?,
    )?;

    // A new keyring and agent prevent a cached unlock from masking a wrong passphrase.
    let keyring = tempfile::Builder::new()
        .prefix("android-setup-gpg-")
        .tempdir()
        .map_err(|_| anyhow!("Could not create a temporary GnuPG verification keyring."))?;
    let check = (|| -> Result<()> {
        run_gpg(
            "import into test keyring",
            &["--import"],
            armor.as_bytes(),
            Some(keyring.path()),
            Some(home),
        )?;
        let challenge = keyring.path().join("challenge");
        fs::write(&challenge, b"android-repo-setup signing check")
            .map_err(|_| anyhow!("Could not write the GnuPG signing check."))?;
        let mut sign = LOOPBACK.to_vec();
        sign.extend([
            "--local-user",
            fingerprint,
            "--output",
            "-",
            "--detach-sign",
            "challenge",
        ]);
        run_gpg(
            "test signing",
            &sign,
            &secret,
            Some(keyring.path()),
            Some(home),
        )?;
        Ok(())
    })();
    let _ = run_command(
        Command::new(gpg_tool("gpgconf"))
            .arg("--homedir")
            .arg(keyring.path())
            .args(["--kill", "all"]),
        b"",
        Duration::from_secs(10),
    );
    check?;
    Ok(armor.to_owned())
}

pub(crate) fn publish_public_key(fingerprint: &str) -> Result<()> {
    ensure!(is_fingerprint(fingerprint), "Invalid OpenPGP fingerprint.");
    let home = std::env::home_dir();
    run_gpg(
        "publish public key",
        &[
            "--keyserver",
            "hkps://keyserver.ubuntu.com",
            "--send-keys",
            fingerprint,
        ],
        b"",
        None,
        home.as_deref(),
    )?;
    Ok(())
}

type Prompt<'a> = dyn FnMut(&str, bool) -> Result<Option<String>> + 'a;

fn confirm(prompt: &mut Prompt<'_>, message: &str) -> Result<bool> {
    Ok(prompt(message, false)?
        .is_some_and(|answer| matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")))
}

fn ask_with_default(prompt: &mut Prompt<'_>, label: &str, default: &str) -> Result<Option<String>> {
    let label = if default.is_empty() {
        label.to_owned()
    } else {
        format!("{label} [{}]", display_identity(default))
    };
    Ok(prompt(&label, false)?.map(|answer| {
        let answer = answer.trim();
        if answer.is_empty() {
            default.to_owned()
        } else {
            answer.to_owned()
        }
    }))
}

fn verify_choice(fingerprint: &str, passphrase: String, home: &Path) -> Result<VerifiedKey> {
    let armor = export_verified_key(fingerprint, &passphrase, home)?;
    Ok(VerifiedKey {
        fingerprint: fingerprint.to_owned(),
        armor,
        passphrase,
    })
}

fn create_signing_key(
    project: &str,
    home: &Path,
    prompt: &mut Prompt<'_>,
) -> Result<Option<VerifiedKey>> {
    let defaults = load_key_defaults(home, project)?;
    eprintln!("The OpenPGP identity is public if the key is published.");
    let Some(identity) = ask_with_default(
        prompt,
        "OpenPGP identity (Name <email@example.com>)",
        &defaults.identity,
    )?
    else {
        return Ok(None);
    };
    let Some(bits) = ask_with_default(
        prompt,
        "RSA key size (2048, 3072, or 4096)",
        &defaults.bits.to_string(),
    )?
    else {
        return Ok(None);
    };
    let bits = bits
        .parse()
        .context("Choose an RSA key size of 2048, 3072, or 4096 bits.")?;
    let Some(expires) = ask_with_default(
        prompt,
        "Expiration (duration, YYYY-MM-DD, or 0)",
        &defaults.expires,
    )?
    else {
        return Ok(None);
    };
    let Some(passphrase) = prompt("New OpenPGP passphrase (at least 12 characters)", true)? else {
        return Ok(None);
    };
    let Some(confirmation) = prompt("Confirm OpenPGP passphrase", true)? else {
        return Ok(None);
    };
    ensure!(passphrase == confirmation, "The passphrases do not match.");
    if !confirm(prompt, "Create this key in the local GnuPG keyring? [y/N]")? {
        return Ok(None);
    }
    let fingerprint = match generate_key(&identity, &passphrase, bits, &expires, false, home) {
        Err(error) if error.is::<DuplicateKeyIdentity>() => {
            eprintln!("{error}");
            if !confirm(
                prompt,
                "Create another key with this identity and keep the existing key? [y/N]",
            )? {
                return Ok(None);
            }
            generate_key(&identity, &passphrase, bits, &expires, true, home)?
        }
        result => result?,
    };
    let mut passphrase = passphrase;
    loop {
        match verify_choice(&fingerprint, passphrase, home) {
            Ok(key) => return Ok(Some(key)),
            Err(error) => eprintln!("{error} The generated key remains in the local keyring."),
        }
        if !confirm(prompt, "Retry verification of the generated key? [y/N]")? {
            return Ok(None);
        }
        let Some(retry) = prompt("OpenPGP passphrase", true)? else {
            return Ok(None);
        };
        passphrase = retry;
    }
}

pub(crate) fn pick_signing_key(
    project: &str,
    home: &Path,
    prompt: &mut impl FnMut(&str, bool) -> Result<Option<String>>,
) -> Result<Option<VerifiedKey>> {
    loop {
        eprintln!("Maven signing key\n  C. Create a local OpenPGP key");
        let keys = match run_gpg(
            "list keys",
            &["--with-colons", "--list-secret-keys"],
            b"",
            None,
            Some(home),
        ) {
            Ok(output) => parse_signing_keys(&String::from_utf8_lossy(&output)),
            Err(error) => {
                eprintln!("{error}");
                Vec::new()
            }
        };
        for (index, key) in keys.iter().enumerate() {
            eprintln!(
                "  {}. {}\n     Created {}   Expires {}\n     {}",
                index + 1,
                key.identity,
                key.created,
                key.expires,
                key.fingerprint
            );
        }
        let Some(answer) = prompt("Choose C, a key number, or Enter to go back", false)? else {
            return Ok(None);
        };
        let answer = answer.trim();
        if answer.is_empty() {
            return Ok(None);
        }
        let result = if answer.eq_ignore_ascii_case("c") {
            create_signing_key(project, home, prompt)
        } else if let Some(key) = answer
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| keys.get(index))
        {
            let Some(passphrase) =
                prompt("OpenPGP passphrase (blank for an unprotected key)", true)?
            else {
                return Ok(None);
            };
            verify_choice(&key.fingerprint, passphrase, home).map(Some)
        } else {
            eprintln!("Choose C, a listed key number, or Enter.");
            continue;
        };
        match result {
            Ok(result) => return Ok(result),
            Err(error) => eprintln!("{error}"),
        }
    }
}

#[cfg(test)]
#[path = "../tests/support/gpg_port.rs"]
mod tests;
