use super::*;

#[test]
fn lists_current_signing_keys_with_primary_fingerprint_and_dates() {
    let fingerprint = "A".repeat(40);
    let listing = format!(
        "sec:u:3072:1:123:1735689600:1767225600:::::scSC:\n\
         fpr:::::::::{fingerprint}:\n\
         uid:u::::::::Maven Publisher <test@example.invalid>:\n\
         uid:u::::::::Second identity:\n\
         sec:e:3072:1:456:0:0:::::scSC:\n\
         fpr:::::::::{}:\n\
         uid:u::::::::Expired key:\n",
        "B".repeat(40)
    );
    let keys = parse_signing_keys(&listing);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].fingerprint, fingerprint);
    assert_eq!(keys[0].identity, "Maven Publisher <test@example.invalid>");
    assert_eq!(keys[0].created, "2025-01-01");
    assert_eq!(keys[0].expires, "2026-01-01");
}

#[test]
fn creates_and_verifies_a_key_without_reusing_an_agents_cached_unlock() {
    // Isolate GNUPGHOME in a subprocess so parallel unit tests retain their environments.
    if let Some(home) = std::env::var_os("ANDROID_SETUP_GPG_TEST_HOME") {
        let home = Path::new(&home);
        let defaults = home.join(".projects").join(".default");
        fs::create_dir_all(&defaults).unwrap();
        fs::write(
            defaults.join("maven-central.json"),
            r#"{"Name":"Example Publisher","Email":"publisher@example.invalid","KeySize":"2048","Expires":"0"}"#,
        ).unwrap();
        let passphrase = "a disposable passphrase";
        let mut answers = [
            ("C", false),
            ("", false),
            ("", false),
            ("", false),
            (passphrase, true),
            (passphrase, true),
            ("y", false),
        ]
        .into_iter();
        let key = pick_signing_key("project", home, &mut |_, masked| {
            let (answer, expected_masked) = answers.next().expect("unexpected prompt");
            assert_eq!(masked, expected_masked);
            Ok(Some(answer.to_owned()))
        })
        .unwrap()
        .unwrap();
        assert!(answers.next().is_none());
        assert!(is_fingerprint(&key.fingerprint));
        assert!(validate_signing_key(&key.armor).is_ok());
        assert!(key.passphrase == passphrase);
        let listing = run_gpg(
            "list keys",
            &["--with-colons", "--list-secret-keys"],
            b"",
            None,
            Some(home),
        )
        .unwrap();
        let keys = parse_signing_keys(std::str::from_utf8(&listing).unwrap());
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0].identity,
            "Example Publisher (project) <publisher@example.invalid>"
        );
        assert_eq!(keys[0].fingerprint, key.fingerprint);

        let wrong_passphrase = "a different passphrase";
        let error = export_verified_key(&key.fingerprint, wrong_passphrase, home)
            .unwrap_err()
            .to_string();
        assert!(error.contains("rejected the passphrase"), "{error}");
        let error =
            generate_key(&keys[0].identity, passphrase, 2048, "0", false, home).unwrap_err();
        assert!(error.is::<DuplicateKeyIdentity>(), "{error}");

        let diagnostics = fs::read_to_string(defaults.join("gpg-errors.log")).unwrap();
        for value in [
            passphrase,
            wrong_passphrase,
            &keys[0].identity,
            &key.fingerprint,
        ] {
            assert!(!diagnostics.contains(value));
        }
        assert!(diagnostics.contains("gpg_status_codes"));
        return;
    }

    if Command::new("gpg").arg("--version").output().is_err() {
        eprintln!("Skipping disposable keyring check because GnuPG is unavailable.");
        return;
    }
    let fixture = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixListener;
        let probe = fixture.path().join("agent-socket-test");
        if let Err(error) = UnixListener::bind(probe) {
            if matches!(
                error.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
            ) {
                eprintln!("Skipping disposable keyring check because the environment disallows Unix-domain sockets.");
                return;
            }
            panic!("Could not check for Unix-domain socket support: {error}");
        }
    }
    let keyring = fixture.path().join("gnupg");
    fs::create_dir(&keyring).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&keyring, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "gpg::tests::creates_and_verifies_a_key_without_reusing_an_agents_cached_unlock",
            "--nocapture",
        ])
        .env("ANDROID_SETUP_GPG_TEST_HOME", fixture.path())
        .env("GNUPGHOME", &keyring)
        .output()
        .unwrap();
    let _ = Command::new("gpgconf")
        .arg("--homedir")
        .arg(&keyring)
        .args(["--kill", "all"])
        .output();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
}

#[test]
fn records_only_fixed_diagnostics_and_detects_numeric_duplicate_errors() {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;

    let home = tempfile::tempdir().unwrap();
    let error = gpg_error(
        "create key",
        Ok(Output {
            status: std::process::ExitStatus::from_raw(2 << 8),
            stdout: b"[GNUPG:] FAILURE key_generate 33554736\n".to_vec(),
            stderr: b"private identity, key and path".to_vec(),
        }),
        Some(home.path()),
    );
    assert!(error.is::<DuplicateKeyIdentity>());
    assert!(error.to_string().contains("already exists"));
    let log = fs::read_to_string(home.path().join(".projects/.default/gpg-errors.log")).unwrap();
    assert!(!log.contains("private identity, key and path"));
    let record: Value = serde_json::from_str(&log).unwrap();
    assert_eq!(record["operation"], "create key");
    assert_eq!(record["gpg_status_codes"], json!([33554736]));
}
