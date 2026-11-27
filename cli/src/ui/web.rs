//! Local browser renderer for the existing setup worker. All setup decisions stay in Rust.
use super::{Answer, Client, KeyChoice, Menu, Question, Update, CLIENT, PRIVATE_WORKER};
use crate::{
    discovery, publishing,
    gpg::{self, SigningKey},
    setup, vault,
};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const HEADER_LIMIT: usize = 16 * 1024;
const BODY_LIMIT: usize = 128 * 1024;
const LOG_LIMIT: usize = 512 * 1024;
const IDLE_LIMIT: Duration = Duration::from_secs(20 * 60);
const BOOTSTRAP_LIMIT: Duration = Duration::from_secs(10 * 60);

struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

struct Pending {
    id: u64,
    question: Question,
    reply: Sender<Answer>,
    keys: Vec<SigningKey>,
    key_status: Option<String>,
}

struct Session {
    bearer: String,
    csrf: String,
    last_seen: Instant,
    locked: bool,
    authenticated: bool,
}

#[derive(Clone, Copy)]
enum WorkerAction {
    Setup,
    ViewVault,
    RemakeVault,
}

fn regular_directory(root: &Path, relative: &Path) -> bool {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if !fs::symlink_metadata(&current)
            .is_ok_and(|metadata| metadata.file_type().is_dir())
        {
            return false;
        }
    }
    true
}

// Only show regular build outputs under the module's validated project directory.
// A bounded walk avoids following symlinks or traversing large build trees.
fn build_artifacts(root: &Path, relative: &Path, extension: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut directories = vec![(relative.to_path_buf(), 0usize)];
    let mut inspected = 0usize;
    while let Some((directory, depth)) = directories.pop() {
        if !regular_directory(root, &directory) {
            continue;
        }
        let Ok(entries) = fs::read_dir(root.join(&directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            inspected += 1;
            if inspected > 512 {
                break;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let path = directory.join(entry.file_name());
            if kind.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some(extension)
                && files.len() < 64
            {
                files.push(path.to_string_lossy().replace('\\', "/"));
            } else if kind.is_dir() && depth < 4 {
                directories.push((path, depth + 1));
            }
        }
        if inspected > 512 || files.len() >= 64 {
            break;
        }
    }
    files.sort();
    files
}

fn module_artifacts(root: &Path, id: &str, module: &discovery::Module) -> Vec<Value> {
    let (folder, extension, label) = match &module.kind {
        discovery::Kind::Application => ("build/outputs/apk", "apk", "APK"),
        discovery::Kind::Library => ("build/outputs/aar", "aar", "AAR"),
        discovery::Kind::JvmLibrary => ("build/libs", "jar", "JAR"),
        discovery::Kind::Other => return Vec::new(),
    };
    let output = if module.directory == "." {
        PathBuf::from(folder)
    } else {
        Path::new(module.directory.as_str()).join(folder)
    };
    let files = build_artifacts(root, &output, extension);
    let mut artifacts = if files.is_empty() {
        vec![json!({"path":output.to_string_lossy().replace('\\', "/"),
            "status":"not built","kind":extension,
            "description":format!("No local {label} build output found for {}. Build the module to populate this location.", module.path)})]
    } else {
        files.into_iter().map(|path| json!({"path":path,"status":"present","kind":extension,
            "description":format!("Local {label} build output for {}", module.path)})).collect()
    };
    let mut workflows = match &module.kind {
        discovery::Kind::Application => vec![format!("build-signed-{id}-apk.yml")],
        discovery::Kind::Library | discovery::Kind::JvmLibrary => vec![
            format!("publish-{id}.yml"),
            format!("verify-release-tooling-{id}.yml"),
            format!("reconcile-failed-jitpack-{id}.yml"),
        ],
        discovery::Kind::Other => Vec::new(),
    };
    if module.screenshots && !matches!(&module.kind, discovery::Kind::JvmLibrary) {
        workflows.push(format!("screenshots-{id}.yml"));
    }
    if !regular_directory(root, Path::new(".github/workflows")) {
        return artifacts;
    }
    for name in workflows {
        let path = format!(".github/workflows/{name}");
        if fs::symlink_metadata(root.join(&path))
            .is_ok_and(|metadata| metadata.file_type().is_file())
        {
            artifacts.push(json!({"path":path,"status":"present","kind":"workflow",
                "description":format!("GitHub Actions workflow for {}", module.path)}));
        }
    }
    artifacts
}

struct Server {
    root: PathBuf,
    sender: Sender<Update>,
    updates: Receiver<Update>,
    worker: Option<JoinHandle<Result<()>>>,
    launch_token: Option<String>,
    recovery_code: String,
    created_vault: bool,
    created_at: Instant,
    session: Option<Session>,
    pending: Option<Pending>,
    next_id: u64,
    revision: u64,
    phase: &'static str,
    status: &'static str,
    operation: &'static str,
    notes: String,
    last_chunk: String,
    closing: bool,
    closed_at: Option<Instant>,
    closed_seen: bool,
}

impl Server {
    fn new(
        root: PathBuf,
        sender: Sender<Update>,
        updates: Receiver<Update>,
        token: String,
        recovery_code: String,
    ) -> Self {
        Self {
            root,
            sender,
            updates,
            worker: None,
            launch_token: Some(token),
            recovery_code,
            created_vault: false,
            created_at: Instant::now(),
            session: None,
            pending: None,
            next_id: 0,
            revision: 0,
            phase: "ready",
            status: "idle",
            operation: "setup",
            notes: String::new(),
            last_chunk: String::new(),
            closing: false,
            closed_at: None,
            closed_seen: false,
        }
    }

    fn append_chunk(&mut self, chunk: &str) {
        let additional = chunk.strip_prefix(&self.last_chunk).unwrap_or(chunk);
        if !additional.is_empty() {
            self.notes.push_str(additional);
            // Keep bounded progress, preserving the most recent complete UTF-8 text.
            if self.notes.len() > LOG_LIMIT {
                let mut start = self.notes.len() - LOG_LIMIT;
                while !self.notes.is_char_boundary(start) {
                    start += 1;
                }
                self.notes = format!("[Earlier output omitted]\n{}", &self.notes[start..]);
            }
            self.revision += 1;
        }
        self.last_chunk.clear();
        self.last_chunk.push_str(chunk);
    }

    fn drain_updates(&mut self) {
        loop {
            match self.updates.try_recv() {
                Ok(Update::Question(question, notes, reply)) => {
                    self.append_chunk(&notes);
                    self.last_chunk.clear();
                    if self.closing {
                        // A declined confirmation may lead to another question.
                        // Keep canceling until the worker has actually stopped.
                        let _ = reply.send(Answer::Cancel);
                        continue;
                    }
                    if let Some(previous) = self.pending.take() {
                        let _ = previous.reply.send(Answer::Cancel);
                    }
                    self.next_id += 1;
                    let id = self.next_id;
                    if matches!(question, Question::Key) {
                        let sender = self.sender.clone();
                        thread::spawn(move || {
                            let result = gpg::list_signing_keys(None);
                            let _ = sender.send(Update::Keys(id, result));
                        });
                    }
                    self.pending = Some(Pending {
                        id,
                        question,
                        reply,
                        keys: Vec::new(),
                        key_status: None,
                    });
                    self.revision += 1;
                }
                Ok(Update::Notes(notes)) => self.append_chunk(&notes),
                Ok(Update::Phase(phase)) => {
                    self.phase = phase;
                    self.revision += 1;
                }
                Ok(Update::Finished(notes, success)) => {
                    self.append_chunk(&notes);
                    self.last_chunk.clear();
                    self.pending.take();
                    if !self.closing {
                        self.status = if success { "finished" } else { "error" };
                    }
                    if success {
                        self.phase = "complete";
                    }
                    self.revision += 1;
                }
                Ok(Update::Keys(id, result)) => {
                    if let Some(pending) = self.pending.as_mut().filter(|p| p.id == id) {
                        match result {
                            Ok(keys) => pending.keys = keys,
                            Err(_) => pending.key_status = Some(
                                "Could not load local signing keys. Create a new key or retry setup."
                                    .into(),
                            ),
                        }
                        self.revision += 1;
                    }
                }
                Ok(Update::Clipboard(..)) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn start(&mut self) -> std::result::Result<(), (u16, &'static str)> {
        self.start_worker(WorkerAction::Setup)
    }

    fn vault_action(&mut self, action: &str) -> std::result::Result<(), (u16, &'static str)> {
        let action = match action {
            "view-vault" => WorkerAction::ViewVault,
            "remake-vault" => WorkerAction::RemakeVault,
            _ => return Err((400, "Unknown vault action.")),
        };
        self.start_worker(action)
    }

    fn start_worker(
        &mut self,
        action: WorkerAction,
    ) -> std::result::Result<(), (u16, &'static str)> {
        if self.closing || self.pending.is_some() || self.status == "running" {
            return Err((409, "Setup is already active."));
        }
        if let Some(worker) = self.worker.take() {
            if !worker.is_finished() {
                self.worker = Some(worker);
                return Err((409, "Previous setup is still finishing."));
            }
            let _ = worker.join();
        }
        let sender = self.sender.clone();
        let root = self.root.clone();
        let created_vault = self.created_vault;
        if matches!(action, WorkerAction::Setup) {
            self.created_vault = false;
        }
        self.notes.clear();
        self.last_chunk.clear();
        self.status = "running";
        self.operation = match action {
            WorkerAction::Setup => "setup",
            WorkerAction::ViewVault => "view-vault",
            WorkerAction::RemakeVault => "remake-vault",
        };
        self.phase = if matches!(action, WorkerAction::Setup) {
            "modules"
        } else {
            "vault"
        };
        self.revision += 1;
        self.worker = Some(thread::spawn(move || {
            PRIVATE_WORKER.with(|private| private.set(true));
            CLIENT.with(|client| {
                *client.borrow_mut() = Some(Client {
                    sink: sender.clone(),
                    notes: String::new(),
                    web: true,
                });
            });
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match action {
                    WorkerAction::Setup => setup::run(&root, created_vault),
                    WorkerAction::ViewVault => {
                        if !vault::view(&setup::signing_config(&root)?)? {
                            super::message("Vault view canceled; no changes were made.".into());
                        }
                        Ok(())
                    }
                    WorkerAction::RemakeVault => {
                        if !vault::remake(&setup::signing_config(&root)?)? {
                            super::message("Vault replacement canceled; the vault was not changed.".into());
                        }
                        Ok(())
                    }
                }
            }))
            .unwrap_or_else(|_| {
                Err(anyhow!(
                    "Wizard operation stopped unexpectedly. Rerun to inspect saved progress."
                ))
            });
            let notes = CLIENT.with(|client| {
                client
                    .borrow_mut()
                    .take()
                    .map(|client| client.notes)
                    .unwrap_or_default()
            });
            let detail = match &result {
                Ok(()) => notes,
                Err(error) => format!("{notes}\n{error:#}"),
            };
            let _ = sender.send(Update::Finished(detail, result.is_ok()));
            result
        }));
        Ok(())
    }

    fn close(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.status = "closing";
        if let Some(pending) = self.pending.take() {
            let _ = pending.reply.send(Answer::Cancel);
        }
        self.revision += 1;
    }

    fn settle_close(&mut self) {
        if self.closing
            && self.closed_at.is_none()
            && self.worker.as_ref().is_none_or(JoinHandle::is_finished)
        {
            self.drain_updates();
            self.status = "closed";
            self.phase = "complete";
            self.closed_at = Some(Instant::now());
            self.revision += 1;
        }
    }

    fn can_exit(&self) -> bool {
        self.closed_at
            .is_some_and(|at| self.closed_seen || at.elapsed() >= Duration::from_secs(10))
    }

    fn lock_if_idle(&mut self) {
        if let Some(session) = &mut self.session {
            if !session.locked && !self.closing && session.last_seen.elapsed() >= IDLE_LIMIT {
                session.locked = true;
                self.revision += 1;
            }
        }
    }

    fn unlock(&mut self, input: &Value) -> std::result::Result<(), (u16, &'static str)> {
        let session = self.session.as_mut().ok_or((401, "No local session."))?;
        if !session.locked {
            return Err((409, "The session is already unlocked."));
        }
        let credential = input
            .get("credential")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .ok_or((422, "Enter the vault passphrase."))?;
        if session.authenticated && constant_time_eq(credential, &self.recovery_code) {
            session.locked = false;
            session.last_seen = Instant::now();
            self.revision += 1;
            return Ok(());
        }
        let config = setup::signing_config(&self.root)
            .map_err(|_| (500, "Could not load the setup vault configuration."))?;
        if !session.authenticated {
            // Migrate a legacy vault before deciding whether this is creation
            // or unlock. No worker can hold this lock before first login.
            let _migration = vault::lock(&config, true)
                .map_err(|_| (409, "Could not prepare the setup vault for login."))?;
        }
        let path = config.state_path();
        let exists = vault::regular_file_exists(&path)
            .map_err(|_| (500, "Could not inspect the setup vault."))?;
        if exists {
            // Open only to verify the passphrase, then discard decrypted data.
            if vault::SetupVault::open(&path, credential).is_err() {
                return Err((401, "Could not unlock the setup vault."));
            }
        } else {
            let confirmation = input
                .get("confirmation")
                .and_then(Value::as_str)
                .ok_or((422, "Confirm the new vault passphrase."))?;
            if credential.chars().count() < 12 {
                return Err((422, "Use at least 12 characters for the vault passphrase."));
            }
            if !constant_time_eq(credential, confirmation) {
                return Err((422, "Vault passphrases differ."));
            }
            vault::create_browser_vault(&config, credential)
                .map_err(|_| {
                    (
                        409,
                        "Could not create the setup vault. Refresh to check whether it now exists.",
                    )
                })?;
            self.created_vault = true;
        }
        session.locked = false;
        session.authenticated = true;
        session.last_seen = Instant::now();
        self.revision += 1;
        Ok(())
    }

    fn snapshot(&self) -> Value {
        if self.session.as_ref().is_some_and(|session| session.locked) {
            let status = if self.closing { self.status } else { "locked" };
            let vault_mode = setup::signing_config(&self.root)
                .ok()
                .and_then(|config| vault::browser_vault_exists(&config).ok())
                .map_or("create", |exists| if exists { "unlock" } else { "create" });
            return json!({"revision":self.revision,"phase":self.phase,"status":status,
                "vaultMode":vault_mode,"initial":self.session.as_ref().is_some_and(|session| !session.authenticated),
                "prompt":null,"notes":[],"context":{},"artifacts":[],"moduleArtifacts":{},"actions":[]});
        }
        let repository = fs::read_to_string(self.root.join(".project/setup/config.yaml"))
            .ok()
            .and_then(|text| serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text).ok())
            .and_then(|config| config["repository"].as_str().map(str::to_owned));
        let discovered = discovery::load(&self.root).ok().flatten().unwrap_or_default();
        let modules = discovered
            .iter()
            .map(|(name, module)| {
                json!({"name":name,"path":module.path,"kind":module.kind,
                    "selected":module.selected,"directory":module.directory})
            })
            .collect::<Vec<_>>();
        let module_artifacts = discovered
            .iter()
            .map(|(name, module)| (name.clone(), module_artifacts(&self.root, name, module)))
            .collect::<BTreeMap<_, _>>();
        let publishing = if self.root.join(".project/setup/repositories.yml").is_file() {
            match publishing::settings_catalog(&self.root) {
                Ok(destinations) => json!({"registryPresent":true,"destinations":destinations}),
                Err(error) => json!({"registryPresent":true,"destinations":[],"error":error.to_string()}),
            }
        } else {
            json!({"registryPresent":false,"destinations":[]})
        };
        let vault_exists = setup::signing_config(&self.root)
            .ok()
            .and_then(|config| vault::regular_file_exists(&config.state_path()).ok())
            .unwrap_or(false);
        let artifacts = [
            ".project/setup/config.yaml",
            ".project/setup/repositories.yml",
            ".project/setup/workflow-generation.json",
            ".project/setup/modules.properties",
            ".github/workflows",
        ]
        .into_iter()
        .filter(|path| self.root.join(path).exists())
        .map(|path| json!({"path":path,"status":"present","description":"Local repository artifact"}))
        .collect::<Vec<_>>();
        let lines = self.notes.lines().map(str::to_owned).collect::<Vec<_>>();
        json!({
            "revision":self.revision,"phase":self.phase,"status":self.status,
            "operation":self.operation,
            "prompt":self.pending.as_ref().map(Pending::public),
            "notes":lines,
            "context":{"repository":repository,"root":self.root,"modules":modules,"publishing":publishing,"vaultExists":vault_exists},
            "artifacts":artifacts,
            "moduleArtifacts":module_artifacts,
            "actions":[{"value":"setup","label":"Set up repository","description":"Discover modules and review the setup operations"}],
        })
    }

    fn answer(&mut self, input: &Value) -> std::result::Result<(), (u16, String)> {
        if self.closing {
            return Err((409, "Setup is stopping. Wait for cleanup.".into()));
        }
        let id = input
            .get("id")
            .and_then(Value::as_u64)
            .ok_or((400, "Missing prompt ID.".into()))?;
        let pending = self
            .pending
            .as_ref()
            .ok_or((409, "There is no current question.".into()))?;
        if pending.id != id {
            return Err((
                409,
                "This question has changed. Refresh the current step.".into(),
            ));
        }
        let answer = if input.get("cancel") == Some(&Value::Bool(true)) {
            Answer::Cancel
        } else {
            let value = input.get("value").ok_or((400, "Missing answer.".into()))?;
            pending.parse_answer(value)?
        };
        let answer = match answer {
            Answer::Menu(Menu::Setup(selected)) => {
                if let Question::Menu { operations, .. } = &pending.question {
                    let module_id = input
                        .get("module")
                        .and_then(Value::as_str)
                        .ok_or((422, "Choose a module tab before running actions.".into()))?;
                    let modules = discovery::load(&self.root)
                        .map_err(|error| (422, format!("Could not load module selection: {error}")))?
                        .ok_or((422, "Discover modules before choosing actions.".into()))?;
                    let module = modules.get(module_id)
                        .filter(|module| module.selected)
                        .ok_or((422, "Choose an included module tab.".into()))?;
                    if selected.iter().any(|&index| !operation_applies_to_module(&operations[index].id, &module.kind)) {
                        return Err((422, "An action is not available for the chosen module.".into()));
                    }
                    Answer::Menu(Menu::ScopedSetup { module_id: module_id.into(), selected })
                } else {
                    Answer::Menu(Menu::Setup(selected))
                }
            }
            other => other,
        };
        let pending = self.pending.take().expect("validated pending question");
        pending
            .reply
            .send(answer)
            .map_err(|_| (409, "Setup has stopped.".into()))?;
        self.revision += 1;
        Ok(())
    }

    fn authorize(&mut self, req: &Request) -> bool {
        let Some(session) = &mut self.session else {
            return false;
        };
        let expected = format!("Bearer {}", session.bearer);
        if !constant_time_eq(req.header("authorization").unwrap_or(""), &expected) {
            return false;
        }
        true
    }

    fn handle(&mut self, req: Request, host: &str) -> Response {
        self.lock_if_idle();
        if req.header("host") != Some(host) || req.path.contains('?') {
            return Response::json_error(403, "Invalid local host or request path.");
        }
        let origin = format!("http://{host}");
        if req.header("origin").is_some_and(|value| value != origin) {
            return Response::json_error(403, "Cross-origin requests are blocked.");
        }
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/") => Response::asset(
                "text/html; charset=utf-8",
                include_bytes!("web_assets/index.html"),
            ),
            ("GET", "/app.css") => Response::asset(
                "text/css; charset=utf-8",
                include_bytes!("web_assets/app.css"),
            ),
            ("GET", "/app.js") => Response::asset(
                "text/javascript; charset=utf-8",
                include_bytes!("web_assets/app.js"),
            ),
            ("GET", "/favicon.svg") => {
                Response::asset("image/svg+xml", include_bytes!("web_assets/favicon.svg"))
            }
            ("POST", path) if path.starts_with("/api/") => {
                if req.header("origin") != Some(origin.as_str()) {
                    return Response::json_error(403, "A matching local Origin is required.");
                }
                if !req
                    .header("content-type")
                    .is_some_and(|value| value.starts_with("application/json"))
                {
                    return Response::json_error(415, "Send JSON for this operation.");
                }
                let Ok(input) = serde_json::from_slice::<Value>(&req.body) else {
                    return Response::json_error(400, "Invalid JSON body.");
                };
                if path == "/api/session" {
                    let token = input.get("token").and_then(Value::as_str).unwrap_or("");
                    if self.session.is_some()
                        || !self
                            .launch_token
                            .as_deref()
                            .is_some_and(|saved| constant_time_eq(saved, token))
                        || self.created_at.elapsed() >= BOOTSTRAP_LIMIT
                    {
                        return Response::json_error(
                            401,
                            "One-time wizard handoff is invalid or expired.",
                        );
                    }
                    let (bearer, csrf) = match (random_secret(), random_secret()) {
                        (Ok(bearer), Ok(csrf)) => (bearer, csrf),
                        _ => return Response::json_error(500, "Could not create local session."),
                    };
                    self.launch_token = None;
                    self.session = Some(Session {
                        bearer: bearer.clone(),
                        csrf: csrf.clone(),
                        last_seen: Instant::now(),
                        locked: true,
                        authenticated: false,
                    });
                    return Response::json(200, json!({"session":bearer,"csrf":csrf}));
                }
                if !self.authorize(&req) {
                    return Response::json_error(
                        401,
                        "The wizard session is unavailable or expired.",
                    );
                }
                if !self.session.as_ref().is_some_and(|session| {
                    constant_time_eq(req.header("x-csrf-token").unwrap_or(""), &session.csrf)
                }) {
                    return Response::json_error(403, "Missing or invalid action token.");
                }
                if path == "/api/unlock" {
                    return match self.unlock(&input) {
                        Ok(()) => Response::json(200, self.snapshot()),
                        Err((code, message)) => Response::json_error(code, message),
                    };
                }
                if path == "/api/lock" {
                    if let Some(session) = &mut self.session {
                        session.locked = true;
                        self.revision += 1;
                    }
                    return Response::json(200, self.snapshot());
                }
                if self.session.as_ref().is_some_and(|session| session.locked)
                    && path != "/api/close"
                {
                    return Response::json_error(423, "Unlock the local session to continue.");
                }
                match path {
                    "/api/start"
                        if input.get("action").and_then(Value::as_str) == Some("setup") =>
                    {
                        match self.start() {
                            Ok(()) => {
                                if let Some(session) = &mut self.session {
                                    session.last_seen = Instant::now();
                                }
                                Response::json(200, self.snapshot())
                            }
                            Err((code, message)) => Response::json_error(code, message),
                        }
                    }
                    "/api/answer" => match self.answer(&input) {
                        Ok(()) => {
                            if let Some(session) = &mut self.session {
                                session.last_seen = Instant::now();
                            }
                            Response::json(200, self.snapshot())
                        }
                        Err((code, message)) => Response::json_error(code, &message),
                    },
                    "/api/vault-action" => match self.vault_action(
                        input.get("action").and_then(Value::as_str).unwrap_or(""),
                    ) {
                        Ok(()) => {
                            if let Some(session) = &mut self.session {
                                session.last_seen = Instant::now();
                            }
                            Response::json(200, self.snapshot())
                        }
                        Err((code, message)) => Response::json_error(code, message),
                    },
                    "/api/activity" => {
                        if let Some(session) = &mut self.session {
                            session.last_seen = Instant::now();
                        }
                        Response {
                            status: 204,
                            content_type: "application/json",
                            body: Vec::new(),
                        }
                    }
                    "/api/close" => {
                        self.close();
                        Response::json(202, json!({"status":"closing"}))
                    }
                    "/api/start" => Response::json_error(400, "Unknown setup action."),
                    _ => Response::json_error(404, "Unknown local operation."),
                }
            }
            ("GET", "/api/state") => {
                if !self.authorize(&req) {
                    Response::json_error(401, "The wizard session is unavailable or expired.")
                } else {
                    let snapshot = self.snapshot();
                    if self.status == "closed" {
                        self.closed_seen = true;
                    }
                    Response::json(200, snapshot)
                }
            }
            _ => Response::json_error(404, "Unknown local path."),
        }
    }
}

fn operation_applies_to_module(id: &str, kind: &discovery::Kind) -> bool {
    match id {
        "key" | "local" | "github" => *kind == discovery::Kind::Application,
        "publishing" => kind.is_library(),
        "workflows" => true,
        _ => false,
    }
}

impl Pending {
    fn public(&self) -> Value {
        let id = self.id;
        match &self.question {
            Question::Text {
                label,
                default,
                secret,
            } => json!({
                "id":id,"kind":"text","label":label,"secret":secret,
                "default":if *secret { None } else { default.as_deref() },
                "countdownSeconds":if !secret && default.is_some() { Some(20) } else { None },
            }),
            Question::Confirm(label) => json!({"id":id,"kind":"confirm","label":label}),
            Question::Menu {
                operations,
                first_run,
            } => json!({
                "id":id,"kind":"menu","label":"Choose setup operations","firstRun":first_run,
                "options":operations.iter().enumerate().map(|(index,operation)| json!({
                    "value":index.to_string(),"id":operation.id,"label":operation.label,
                    "description":operation.description,"selected":first_run,
                })).collect::<Vec<_>>(),
            }),
            Question::Modules(entries) => json!({
                "id":id,"kind":"modules","label":"Select libraries and apps",
                "options":entries.iter().enumerate().map(|(index,entry)| json!({
                    "value":index.to_string(),"label":entry.label,"description":entry.description,
                    "selected":entry.selected,"id":entry.id,"path":entry.path,
                    "kind":entry.kind,"directory":entry.directory,
                })).collect::<Vec<_>>(),
            }),
            Question::Saved(label) => json!({
                "id":id,"kind":"saved","label":label,"countdownSeconds":20,
                "options":[
                    {"value":"reuse","label":"Use vault value"},
                    {"value":"new","label":"Set new value"},
                ],
            }),
            Question::Key => {
                let mut options = vec![
                    json!({"value":"create","label":"Create new key","description":"Create a local Maven Central signing key"}),
                ];
                options.extend(self.keys.iter().map(|key| json!({
                    "value":key.fingerprint,"label":key.identity,
                    "description":format!("{} · Created {} · Expires {}",key.fingerprint,key.created,key.expires),
                })));
                json!({"id":id,"kind":"key","label":"Maven Central signing key",
                    "options":options,"message":self.key_status})
            }
            Question::Form { title, fields, .. } => json!({
                "id":id,"kind":"form","label":title,
                "fields":fields.iter().enumerate().map(|(index,field)| json!({
                    "name":index.to_string(),"label":field.label,"help":field.hint,
                    "secret":field.secret,"value":if field.secret { "" } else { &field.value },
                })).collect::<Vec<_>>(),
            }),
        }
    }

    fn parse_answer(&self, value: &Value) -> std::result::Result<Answer, (u16, String)> {
        fn invalid(message: &str) -> (u16, String) {
            (422, message.into())
        }
        match &self.question {
            Question::Text { secret, .. } => {
                let text = value
                    .as_str()
                    .ok_or_else(|| invalid("Enter a text value."))?;
                let max = if *secret { 48 * 1024 } else { 8 * 1024 };
                if text.len() > max
                    || text
                        .chars()
                        .any(|ch| ch == '\0' || ch == '\r' || ch == '\n')
                {
                    return Err(invalid(
                        "Enter a single-line value within the allowed size.",
                    ));
                }
                Ok(Answer::Text(if *secret {
                    text.to_owned()
                } else {
                    text.trim().to_owned()
                }))
            }
            Question::Confirm(_) => value
                .as_bool()
                .map(Answer::Confirm)
                .ok_or_else(|| invalid("Confirm or decline explicitly.")),
            Question::Menu { operations, .. } => {
                if let Some(action) = value.as_str() {
                    return match action {
                        "view-vault" => Ok(Answer::Menu(Menu::ViewVault)),
                        "remake-vault" => Ok(Answer::Menu(Menu::RemakeVault)),
                        "exit" => Ok(Answer::Menu(Menu::Exit)),
                        _ => Err(invalid("Unknown setup action.")),
                    };
                }
                let indices = selected_indices(value, operations.len())?;
                Ok(Answer::Menu(Menu::Setup(indices)))
            }
            Question::Modules(entries) => {
                let indices = selected_indices(value, entries.len())?;
                Ok(Answer::Menu(Menu::Setup(indices)))
            }
            Question::Saved(_) => match value.as_str() {
                Some("reuse" | "new") => Ok(Answer::Text(value.as_str().unwrap().into())),
                _ => Err(invalid("Choose whether to use the saved value.")),
            },
            Question::Key => match value.as_str() {
                Some("create") => Ok(Answer::Key(KeyChoice::Create)),
                Some(fingerprint) => self
                    .keys
                    .iter()
                    .find(|key| key.fingerprint == fingerprint)
                    .cloned()
                    .map(|key| Answer::Key(KeyChoice::Existing(key)))
                    .ok_or_else(|| invalid("Choose a loaded key or create a new one.")),
                None => Err(invalid("Choose a signing key.")),
            },
            Question::Form {
                fields, validate, ..
            } => {
                let values = value
                    .as_array()
                    .ok_or_else(|| invalid("Send the form fields."))?;
                if values.len() != fields.len() {
                    return Err(invalid("The form has changed. Refresh and try again."));
                }
                let mut checked = Vec::with_capacity(values.len());
                for (value, field) in values.iter().zip(fields) {
                    let text = value
                        .as_str()
                        .ok_or_else(|| invalid("Enter text in every form field."))?;
                    let max = if field.secret { 48 * 1024 } else { 8 * 1024 };
                    if text.len() > max
                        || text
                            .chars()
                            .any(|ch| ch == '\0' || ch == '\r' || ch == '\n')
                    {
                        return Err(invalid(
                            "Form values must be single-line and within size limits.",
                        ));
                    }
                    checked.push(text.to_owned());
                }
                validate(&checked).map_err(|error| (422, error.to_string()))?;
                Ok(Answer::Fields(checked))
            }
        }
    }
}

fn selected_indices(value: &Value, count: usize) -> std::result::Result<Vec<usize>, (u16, String)> {
    let choices = value
        .as_array()
        .ok_or((422, "Select the listed items.".into()))?;
    let mut seen = BTreeSet::new();
    let mut selected = Vec::new();
    for choice in choices {
        let index = choice
            .as_u64()
            .and_then(|index| usize::try_from(index).ok())
            .ok_or((422, "Select the listed items.".into()))?;
        if index >= count || !seen.insert(index) {
            return Err((
                422,
                "Selection contains an unknown or duplicate item.".into(),
            ));
        }
        selected.push(index);
    }
    if selected.is_empty() {
        return Err((422, "Select at least one item to continue.".into()));
    }
    Ok(selected)
}

fn constant_time_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut differences = 0u8;
    for (a, b) in left.bytes().zip(right.bytes()) {
        differences |= a ^ b;
    }
    differences == 0
}

fn random_secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    vault::random_bytes(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Response {
    fn asset(content_type: &'static str, body: &'static [u8]) -> Self {
        Self {
            status: 200,
            content_type,
            body: body.to_vec(),
        }
    }
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
        }
    }
    fn json_error(status: u16, message: &str) -> Self {
        Self::json(status, json!({"error":message}))
    }
    fn write(&self, stream: &mut TcpStream) -> io::Result<()> {
        let reason = match self.status {
            200 => "OK",
            202 => "Accepted",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            409 => "Conflict",
            413 => "Payload Too Large",
            415 => "Unsupported Media Type",
            422 => "Unprocessable Content",
            423 => "Locked",
            500 => "Internal Server Error",
            _ => "Error",
        };
        write!(stream,
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n\r\n",
            self.status, reason, self.content_type, self.body.len())?;
        stream.write_all(&self.body)?;
        Ok(())
    }
}

fn read_request(stream: &mut TcpStream) -> std::result::Result<Request, (u16, &'static str)> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() >= HEADER_LIMIT {
            return Err((413, "Request headers are too large."));
        }
        let mut chunk = [0u8; 4096];
        let count = stream
            .read(&mut chunk)
            .map_err(|_| (400, "Could not read local request."))?;
        if count == 0 {
            return Err((400, "Incomplete local request."));
        }
        bytes.extend_from_slice(&chunk[..count]);
    };
    if header_end > HEADER_LIMIT {
        return Err((413, "Request headers are too large."));
    }
    let header_text =
        std::str::from_utf8(&bytes[..header_end]).map_err(|_| (400, "Invalid request headers."))?;
    let mut lines = header_text[..header_text.len() - 4].split("\r\n");
    let start = lines.next().ok_or((400, "Missing request line."))?;
    let parts = start.split(' ').collect::<Vec<_>>();
    if parts.len() != 3
        || parts[2] != "HTTP/1.1"
        || !parts[1].starts_with('/')
        || parts[1].contains(['\r', '\n', '\0'])
    {
        return Err((400, "Invalid local request line."));
    }
    let method = parts[0].to_owned();
    let path = parts[1].to_owned();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or((400, "Invalid request header."))?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || value.contains(['\0', '\r', '\n'])
        {
            return Err((400, "Invalid request header."));
        }
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().into())
            .is_some()
        {
            return Err((400, "Repeated request headers are not supported."));
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err((400, "Encoded request bodies are not supported."));
    }
    let length = match headers.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| (400, "Invalid request size."))?,
        None if method == "POST" => return Err((400, "Missing request size.")),
        None => 0,
    };
    if length > BODY_LIMIT {
        return Err((413, "Request body is too large."));
    }
    if bytes.len() > header_end + length {
        return Err((400, "Pipelined requests are not supported."));
    }
    while bytes.len() < header_end + length {
        let mut chunk = [0u8; 4096];
        let count = stream
            .read(&mut chunk)
            .map_err(|_| (400, "Could not read local request."))?;
        if count == 0 || bytes.len() + count > header_end + length {
            return Err((400, "Incomplete or unexpected request body."));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(Request {
        method,
        path,
        headers,
        body: bytes[header_end..].to_vec(),
    })
}

fn serve_connection(server: &mut Server, mut stream: TcpStream, host: &str) -> io::Result<()> {
    // Winsock accept inherits the listener's nonblocking mode. The listener
    // polls for connections, but request reads and response writes are blocking.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let response = match read_request(&mut stream) {
        Ok(request) => server.handle(request, host),
        Err((code, message)) => {
            // Only static parser messages are logged; no headers or body data.
            eprintln!("Local wizard rejected a request ({code}): {message}");
            Response::json_error(code, message)
        }
    };
    response.write(&mut stream)
}

fn open_browser(url: &str) -> Result<()> {
    #[cfg(windows)]
    let mut process = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(target_os = "macos")]
    let mut process = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let mut process = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    process
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    process.spawn().context("Could not open browser")?;
    Ok(())
}

pub(crate) fn run(root: &Path, no_open: bool) -> Result<()> {
    super::protect_worker_panics();
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("Could not bind local wizard")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let host = format!("127.0.0.1:{}", address.port());
    let token = random_secret()?;
    let recovery_code = random_secret()?;
    let url = format!("http://{host}/#token={token}");
    let (sender, updates) = mpsc::channel();
    let mut server = Server::new(
        root.to_path_buf(),
        sender,
        updates,
        token,
        recovery_code.clone(),
    );
    println!("Local setup wizard: {url}");
    println!("Session recovery code (if idle-locked): {recovery_code}");
    println!(
        "This link works once. Keep the terminal open until setup finishes, then close the wizard."
    );
    if !no_open {
        if open_browser(&url).is_err() {
            eprintln!("Could not open the browser automatically. Open the local link shown above.");
        }
    }
    loop {
        server.drain_updates();
        server.settle_close();
        server.lock_if_idle();
        if server.can_exit() {
            break;
        }
        if server.session.is_none() && server.created_at.elapsed() >= BOOTSTRAP_LIMIT {
            server.close();
            continue;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if let Err(error) = serve_connection(&mut server, stream, &host) {
                    eprintln!("Local wizard connection error: {:?}", error.kind());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(40));
            }
            Err(error) => {
                server.close();
                eprintln!("Local wizard listener needs attention: {error}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    if let Some(worker) = server.worker.take() {
        let _result = worker
            .join()
            .map_err(|_| anyhow!("Setup worker stopped unexpectedly."))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "127.0.0.1:47635";

    fn server() -> Server {
        let (sender, updates) = mpsc::channel();
        Server::new(
            PathBuf::from("/nonexistent-browser-wizard-test"),
            sender,
            updates,
            "launch-secret".into(),
            "recovery-secret".into(),
        )
    }

    #[test]
    fn failed_setup_keeps_the_phase_that_needs_attention() {
        let mut server = server();
        server.sender.send(Update::Phase("workflows")).unwrap();
        server
            .sender
            .send(Update::Finished("Invalid workflow configuration".into(), false))
            .unwrap();
        server.drain_updates();

        assert_eq!(server.status, "error");
        assert_eq!(server.snapshot()["phase"], "workflows");
        assert!(server.snapshot()["notes"]
            .to_string()
            .contains("Invalid workflow configuration"));
    }

    fn request(method: &str, path: &str, body: Value, auth: Option<(&str, &str)>) -> Request {
        let mut headers = BTreeMap::from([
            ("host".into(), HOST.into()),
            ("content-type".into(), "application/json".into()),
            ("origin".into(), format!("http://{HOST}")),
        ]);
        if let Some((bearer, csrf)) = auth {
            headers.insert("authorization".into(), format!("Bearer {bearer}"));
            headers.insert("x-csrf-token".into(), csrf.into());
        }
        Request {
            method: method.into(),
            path: path.into(),
            headers,
            body: body.to_string().into_bytes(),
        }
    }

    fn login_unverified(server: &mut Server) -> (String, String) {
        let response = server.handle(
            request(
                "POST",
                "/api/session",
                json!({"token":"launch-secret"}),
                None,
            ),
            HOST,
        );
        assert_eq!(response.status, 200);
        let value: Value = serde_json::from_slice(&response.body).unwrap();
        (
            value["session"].as_str().unwrap().into(),
            value["csrf"].as_str().unwrap().into(),
        )
    }

    fn login(server: &mut Server) -> (String, String) {
        let credentials = login_unverified(server);
        // Most existing tests exercise operations after the vault gate. The
        // handshake itself is covered by the dedicated vault tests below.
        let session = server.session.as_mut().unwrap();
        session.locked = false;
        session.authenticated = true;
        credentials
    }

    #[test]
    fn nonblocking_listener_serves_a_delayed_browser_request() -> Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let client = thread::spawn(move || -> io::Result<String> {
            let mut stream = TcpStream::connect(address)?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            thread::sleep(Duration::from_millis(50));
            stream.write_all(
                format!("GET /favicon.svg HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes(),
            )?;
            let mut response = String::new();
            stream.read_to_string(&mut response)?;
            Ok(response)
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let (stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => return Err(error.into()),
            }
        };
        let mut server = server();
        serve_connection(&mut server, stream, &address.to_string())?;
        let response = client.join().map_err(|_| anyhow!("Browser test client stopped"))??;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("<svg"), "{response}");
        Ok(())
    }

    #[test]
    fn startup_requires_vault_creation_and_passphrase_on_return() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".project/setup")).unwrap();
        let suffix = root.path().file_name().unwrap().to_string_lossy();
        let repository = format!("testing/cli{suffix}");
        fs::write(root.path().join(discovery::CONFIG), format!("repository: {repository}\n")).unwrap();
        let config = setup::signing_config(root.path()).unwrap();
        let (sender, updates) = mpsc::channel();
        let mut server = Server::new(root.path().to_path_buf(), sender, updates,
            "launch-secret".into(), "recovery-secret".into());
        let (bearer, csrf) = login_unverified(&mut server);
        let state = server.snapshot();
        assert_eq!(state["status"], "locked");
        assert_eq!(state["vaultMode"], "create");
        assert_eq!(state["initial"], true);
        let send = |server: &mut Server, path: &str, value| server.handle(
            request("POST", path, value, Some((&bearer, &csrf))), HOST).status;
        assert_eq!(send(&mut server, "/api/start", json!({"action":"setup"})), 423);
        assert_eq!(send(&mut server, "/api/unlock", json!({"credential":"too short","confirmation":"too short"})), 422);
        assert_eq!(send(&mut server, "/api/unlock", json!({"credential":"a long passphrase","confirmation":"different"})), 422);
        assert_eq!(send(&mut server, "/api/unlock", json!({"credential":"recovery-secret"})), 422);
        assert_eq!(send(&mut server, "/api/unlock", json!({"credential":"a long passphrase","confirmation":"a long passphrase"})), 200);
        assert!(config.state_path().is_file());
        assert_eq!(send(&mut server, "/api/lock", json!({})), 200);
        assert_eq!(server.snapshot()["status"], "locked");
        assert_eq!(server.snapshot()["initial"], false);
        assert_eq!(send(&mut server, "/api/unlock", json!({"credential":"incorrect passphrase"})), 401);
        assert_eq!(send(&mut server, "/api/unlock", json!({"credential":"a long passphrase"})), 200);
        let (sender, updates) = mpsc::channel();
        let mut reopened = Server::new(root.path().to_path_buf(), sender, updates,
            "another-link".into(), "another-recovery".into());
        let response = reopened.handle(request("POST", "/api/session", json!({"token":"another-link"}), None), HOST);
        let identity: Value = serde_json::from_slice(&response.body).unwrap();
        let new_bearer = identity["session"].as_str().unwrap();
        let new_csrf = identity["csrf"].as_str().unwrap();
        assert_eq!(reopened.snapshot()["vaultMode"], "unlock");
        assert_eq!(reopened.handle(request("POST", "/api/unlock", json!({"credential":"another-recovery"}), Some((new_bearer,new_csrf))), HOST).status, 401);
        assert_eq!(reopened.handle(request("POST", "/api/unlock", json!({"credential":"a long passphrase"}), Some((new_bearer,new_csrf))), HOST).status, 200);
        fs::remove_dir_all(config.state_dir.join(config.project_name())).unwrap();
    }

    #[test]
    fn menu_options_expose_stable_ids_without_changing_answer_indices() {
        let (reply, _) = mpsc::channel();
        let pending = Pending {
            id: 7,
            question: Question::Menu {
                operations: vec![
                    crate::ui::OperationChoice {
                        id: "publishing".into(),
                        label: "Publishing environments".into(),
                        description: "Review credentials".into(),
                    },
                    crate::ui::OperationChoice {
                        id: "workflows".into(),
                        label: "GitHub workflows".into(),
                        description: "Generate files".into(),
                    },
                ],
                first_run: true,
            },
            reply,
            keys: Vec::new(),
            key_status: None,
        };
        let options = pending.public()["options"].clone();
        assert_eq!(options[0]["value"], "0");
        assert_eq!(options[0]["id"], "publishing");
        assert_eq!(options[1]["value"], "1");
        assert_eq!(options[1]["id"], "workflows");
        assert!(matches!(
            pending.parse_answer(&json!([0, 1])).unwrap(),
            Answer::Menu(Menu::Setup(indices)) if indices == vec![0, 1]
        ));
    }

    #[test]
    fn menu_answer_is_validated_against_the_active_module() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".project/setup")).unwrap();
        fs::write(
            root.path().join(discovery::CONFIG),
            "repository: example/project\nandroid_modules:\n  app:\n    path: :app\n    directory: app\n    kind: application\n  library:\n    path: :library\n    directory: library\n    kind: library\n  excluded:\n    path: :excluded\n    directory: excluded\n    kind: library\n    selected: false\n",
        ).unwrap();
        let (sender, updates) = mpsc::channel();
        let mut server = Server::new(root.path().to_path_buf(), sender, updates,
            "launch-secret".into(), "recovery-secret".into());
        let (bearer, csrf) = login(&mut server);
        let (reply, received) = mpsc::channel();
        server.pending = Some(Pending {
            id: 1,
            question: Question::Menu {
                operations: vec![
                    crate::ui::OperationChoice { id: "key".into(), label: "APK signing key".into(), description: String::new() },
                    crate::ui::OperationChoice { id: "publishing".into(), label: "Publishing".into(), description: String::new() },
                    crate::ui::OperationChoice { id: "workflows".into(), label: "Workflows".into(), description: String::new() },
                ],
                first_run: true,
            },
            reply,
            keys: Vec::new(),
            key_status: None,
        });
        for (module, choices) in [("app", json!([1])), ("library", json!([0])),
            ("excluded", json!([2])), ("unknown", json!([2]))] {
            let response = server.handle(request("POST", "/api/answer",
                json!({"id":1,"module":module,"value":choices}), Some((&bearer, &csrf))), HOST);
            assert_eq!(response.status, 422, "{module}");
            assert!(received.try_recv().is_err());
            assert_eq!(server.pending.as_ref().unwrap().id, 1);
        }
        let response = server.handle(request("POST", "/api/answer",
            json!({"id":1,"module":"app","value":[0,2]}), Some((&bearer, &csrf))), HOST);
        assert_eq!(response.status, 200);
        assert!(matches!(received.recv().unwrap(),
            Answer::Menu(Menu::ScopedSetup { module_id, selected })
                if module_id == "app" && selected == vec![0, 2]));
    }

    #[test]
    fn discovered_modules_are_structured_before_the_selection_is_saved() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".project/setup")).unwrap();
        fs::write(
            root.path().join(discovery::CONFIG),
            "repository: example/project\ntooling_ref: v0.1.28\nmodules: {}\n",
        )
        .unwrap();
        let (sender, updates) = mpsc::channel();
        let mut server = Server::new(
            root.path().to_path_buf(),
            sender,
            updates,
            "launch-secret".into(),
            "recovery-secret".into(),
        );
        let (reply, _) = mpsc::channel();
        server.pending = Some(Pending {
            id: 1,
            question: Question::Modules(vec![
                crate::ui::ModuleChoice {
                    id: "libs--vision".into(),
                    path: ":libs:vision".into(),
                    kind: discovery::Kind::Library,
                    directory: "components/vision".into(),
                    label: ":libs:vision — Android library".into(),
                    description: "components/vision · libs--vision".into(),
                    selected: true,
                },
                crate::ui::ModuleChoice {
                    id: "demo".into(),
                    path: ":demo".into(),
                    kind: discovery::Kind::Application,
                    directory: "mobile".into(),
                    label: ":demo — App".into(),
                    description: "mobile · demo".into(),
                    selected: false,
                },
            ]),
            reply,
            keys: Vec::new(),
            key_status: None,
        });
        let unsaved = server.snapshot();
        assert_eq!(unsaved["context"]["modules"], json!([]));
        let options = &unsaved["prompt"]["options"];
        assert_eq!(options[0]["id"], "libs--vision");
        assert_eq!(options[0]["path"], ":libs:vision");
        assert_eq!(options[0]["kind"], "library");
        assert_eq!(options[0]["directory"], "components/vision");
        assert_eq!(options[0]["selected"], true);
        assert_eq!(options[1]["id"], "demo");
        assert_eq!(options[1]["kind"], "application");
        assert_eq!(options[1]["selected"], false);
        assert!(matches!(
            server.pending.as_ref().unwrap().parse_answer(&json!([0])).unwrap(),
            Answer::Menu(Menu::Setup(indices)) if indices == vec![0]
        ));

        let saved = discovery::Modules::from([
            (
                "libs--vision".into(),
                discovery::Module {
                    path: ":libs:vision".into(),
                    directory: "components/vision".into(),
                    kind: discovery::Kind::Library,
                    selected: true,
                    namespace: None,
                    application_ids: vec![],
                    screenshots: false,
                },
            ),
            (
                "demo".into(),
                discovery::Module {
                    path: ":demo".into(),
                    directory: "mobile".into(),
                    kind: discovery::Kind::Application,
                    selected: false,
                    namespace: None,
                    application_ids: vec![],
                    screenshots: false,
                },
            ),
        ]);
        discovery::save(root.path(), &saved).unwrap();
        let context = server.snapshot()["context"]["modules"].clone();
        assert_eq!(context[0]["name"], "demo");
        assert_eq!(context[0]["selected"], false);
        assert_eq!(context[1]["name"], "libs--vision");
        assert_eq!(context[1]["kind"], "library");
        assert_eq!(context[1]["selected"], true);
    }

    #[test]
    fn module_artifacts_follow_module_kind_and_existing_build_outputs() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(".project/setup")).unwrap();
        fs::write(
            root.path().join(discovery::CONFIG),
            "repository: example/project\nandroid_modules:\n  app:\n    path: :app\n    directory: app\n    kind: application\n  library:\n    path: :library\n    directory: library\n    kind: library\n  tools:\n    path: :tools\n    directory: tools\n    kind: jvm-library\n",
        ).unwrap();
        fs::create_dir_all(root.path().join("app/build/outputs/apk/debug")).unwrap();
        fs::write(root.path().join("app/build/outputs/apk/debug/app-debug.apk"), b"apk").unwrap();
        fs::create_dir_all(root.path().join("tools/build/libs")).unwrap();
        fs::write(root.path().join("tools/build/libs/tools.jar"), b"jar").unwrap();
        fs::create_dir_all(root.path().join(".github/workflows")).unwrap();
        fs::write(
            root.path().join(".github/workflows/build-signed-app-apk.yml"),
            "name: signed APK\n",
        ).unwrap();
        let (sender, updates) = mpsc::channel();
        let server = Server::new(root.path().to_path_buf(), sender, updates, "token".into(), "recovery".into());
        let artifacts = server.snapshot()["moduleArtifacts"].clone();
        assert!(artifacts["app"].as_array().unwrap().iter().any(|artifact|
            artifact["path"] == "app/build/outputs/apk/debug/app-debug.apk"
                && artifact["status"] == "present" && artifact["kind"] == "apk"));
        assert!(artifacts["app"].as_array().unwrap().iter().any(|artifact|
            artifact["path"] == ".github/workflows/build-signed-app-apk.yml"));
        assert!(artifacts["library"].as_array().unwrap().iter().any(|artifact|
            artifact["path"] == "library/build/outputs/aar"
                && artifact["status"] == "not built" && artifact["kind"] == "aar"));
        assert!(artifacts["tools"].as_array().unwrap().iter().any(|artifact|
            artifact["path"] == "tools/build/libs/tools.jar"
                && artifact["status"] == "present" && artifact["kind"] == "jar"));
        assert!(!artifacts["app"].as_array().unwrap().iter().any(|artifact|
            artifact["path"] == "tools/build/libs/tools.jar"));

        fs::create_dir_all(root.path().join("library/build/outputs/aar")).unwrap();
        fs::write(
            root.path().join("library/build/outputs/aar/library-release.aar"),
            b"aar",
        ).unwrap();
        let built = server.snapshot()["moduleArtifacts"]["library"].clone();
        assert!(built.as_array().unwrap().iter().any(|artifact|
            artifact["path"] == "library/build/outputs/aar/library-release.aar"
                && artifact["status"] == "present"));
        assert!(!built.as_array().unwrap().iter().any(|artifact|
            artifact["status"] == "not built"));
    }

    #[test]
    fn one_time_handoff_and_bound_authorization() {
        let mut server = server();
        let mut forged = request(
            "POST",
            "/api/session",
            json!({"token":"launch-secret"}),
            None,
        );
        forged.headers.insert("host".into(), "evil.test".into());
        assert_eq!(server.handle(forged, HOST).status, 403);
        let mut cross_origin = request(
            "POST",
            "/api/session",
            json!({"token":"launch-secret"}),
            None,
        );
        cross_origin
            .headers
            .insert("origin".into(), "http://evil.test".into());
        assert_eq!(server.handle(cross_origin, HOST).status, 403);
        let (bearer, csrf) = login(&mut server);
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/session",
                        json!({"token":"launch-secret"}),
                        None
                    ),
                    HOST
                )
                .status,
            401
        );
        assert_eq!(
            server
                .handle(request("GET", "/api/state", json!({}), None), HOST)
                .status,
            401
        );
        let mut no_csrf = request("POST", "/api/activity", json!({}), Some((&bearer, &csrf)));
        no_csrf.headers.remove("x-csrf-token");
        assert_eq!(server.handle(no_csrf, HOST).status, 403);
        let mut wrong_origin = request("POST", "/api/activity", json!({}), Some((&bearer, &csrf)));
        wrong_origin
            .headers
            .insert("origin".into(), "http://evil.test".into());
        assert_eq!(server.handle(wrong_origin, HOST).status, 403);
        assert_eq!(
            server
                .handle(
                    request("POST", "/api/activity", json!({}), Some((&bearer, &csrf))),
                    HOST
                )
                .status,
            204
        );
    }

    #[test]
    fn vault_actions_require_an_unlocked_idle_session_and_valid_action() {
        let mut server = server();
        let (bearer, csrf) = login(&mut server);
        let send = |server: &mut Server, action: &str| {
            server.handle(
                request(
                    "POST",
                    "/api/vault-action",
                    json!({"action":action}),
                    Some((&bearer, &csrf)),
                ),
                HOST,
            )
        };
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/vault-action",
                        json!({"action":"view-vault"}),
                        None,
                    ),
                    HOST,
                )
                .status,
            401
        );
        let mut missing_csrf = request(
            "POST",
            "/api/vault-action",
            json!({"action":"view-vault"}),
            Some((&bearer, &csrf)),
        );
        missing_csrf.headers.remove("x-csrf-token");
        assert_eq!(server.handle(missing_csrf, HOST).status, 403);
        server.session.as_mut().unwrap().locked = true;
        assert_eq!(send(&mut server, "view-vault").status, 423);
        assert!(server.snapshot().get("operation").is_none());
        {
            let session = server.session.as_mut().unwrap();
            session.locked = false;
            session.last_seen = Instant::now() - IDLE_LIMIT - Duration::from_secs(1);
        }
        assert_eq!(send(&mut server, "view-vault").status, 423);
        {
            let session = server.session.as_mut().unwrap();
            session.locked = false;
            session.last_seen = Instant::now();
        }
        assert_eq!(send(&mut server, "invalid").status, 400);

        let (reply, _) = mpsc::channel();
        server.pending = Some(Pending {
            id: 1,
            question: Question::Confirm("Continue setup?".into()),
            reply,
            keys: Vec::new(),
            key_status: None,
        });
        assert_eq!(send(&mut server, "remake-vault").status, 409);
        server.pending = None;
        server.status = "running";
        assert_eq!(send(&mut server, "view-vault").status, 409);
        server.status = "idle";

        server.created_vault = true;
        let response = send(&mut server, "view-vault");
        assert_eq!(response.status, 200);
        assert_eq!(server.phase, "vault");
        assert_eq!(server.snapshot()["operation"], "view-vault");
        assert!(server.created_vault);
        let worker = server.worker.take().unwrap();
        assert!(worker.join().unwrap().is_err()); // The test root has no setup config.
        server.drain_updates();
        assert_eq!(server.status, "error");
        assert_eq!(send(&mut server, "remake-vault").status, 200);
        assert_eq!(server.snapshot()["operation"], "remake-vault");
        server.worker.take().unwrap().join().unwrap().unwrap_err();
    }

    #[test]
    fn stale_prompt_cannot_mutate_new_question_and_secret_defaults_are_hidden() {
        let mut server = server();
        let (bearer, csrf) = login(&mut server);
        let (reply, received) = mpsc::channel();
        server.pending = Some(Pending {
            id: 4,
            question: Question::Text {
                label: "Unlock vault".into(),
                default: Some("never-send-this".into()),
                secret: true,
            },
            reply,
            keys: Vec::new(),
            key_status: None,
        });
        let snapshot = server.snapshot().to_string();
        assert!(!snapshot.contains("never-send-this"));
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/answer",
                        json!({"id":3,"value":"wrong"}),
                        Some((&bearer, &csrf))
                    ),
                    HOST
                )
                .status,
            409
        );
        assert!(received.try_recv().is_err());
        assert_eq!(server.pending.as_ref().unwrap().id, 4);
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/answer",
                        json!({"id":4,"value":"entered-value"}),
                        Some((&bearer, &csrf))
                    ),
                    HOST
                )
                .status,
            200
        );
        assert!(
            matches!(received.recv().unwrap(), Answer::Text(value) if value == "entered-value")
        );
    }

    #[test]
    fn idle_lock_keeps_pending_work_and_requires_unlock() {
        let mut server = server();
        let (bearer, csrf) = login(&mut server);
        let (reply, received) = mpsc::channel();
        server.pending = Some(Pending {
            id: 1,
            question: Question::Confirm("Apply changes?".into()),
            reply,
            keys: Vec::new(),
            key_status: None,
        });
        server.session.as_mut().unwrap().last_seen =
            Instant::now() - Duration::from_secs(19 * 60);
        let before = server.handle(
            request("GET", "/api/state", json!({}), Some((&bearer, &csrf))), HOST);
        let before: Value = serde_json::from_slice(&before.body).unwrap();
        assert_ne!(before["status"], "locked");
        server.session.as_mut().unwrap().last_seen =
            Instant::now() - IDLE_LIMIT - Duration::from_secs(1);
        let response = server.handle(
            request("GET", "/api/state", json!({}), Some((&bearer, &csrf))),
            HOST,
        );
        let state: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(state["status"], "locked");
        assert!(state["prompt"].is_null());
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/answer",
                        json!({"id":1,"value":true}),
                        Some((&bearer, &csrf))
                    ),
                    HOST
                )
                .status,
            423
        );
        assert!(received.try_recv().is_err());
        assert_ne!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/unlock",
                        json!({"credential":"invalid"}),
                        Some((&bearer, &csrf))
                    ),
                    HOST
                )
                .status,
            200
        );
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/unlock",
                        json!({"credential":"recovery-secret"}),
                        Some((&bearer, &csrf))
                    ),
                    HOST
                )
                .status,
            200
        );
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/answer",
                        json!({"id":1,"value":true}),
                        Some((&bearer, &csrf))
                    ),
                    HOST
                )
                .status,
            200
        );
        assert!(matches!(received.recv().unwrap(), Answer::Confirm(true)));
    }

    #[test]
    fn close_retains_confirmation_until_cleanup_and_observation() {
        let mut server = server();
        let (bearer, csrf) = login(&mut server);
        let (reply, received) = mpsc::channel();
        server.pending = Some(Pending {
            id: 1,
            question: Question::Confirm("Apply changes?".into()),
            reply,
            keys: Vec::new(),
            key_status: None,
        });
        let response = server.handle(
            request("POST", "/api/close", json!({}), Some((&bearer, &csrf))),
            HOST,
        );
        assert_eq!(response.status, 202);
        assert_eq!(server.status, "closing");
        assert!(matches!(received.recv().unwrap(), Answer::Cancel));
        assert_eq!(
            server
                .handle(
                    request(
                        "POST",
                        "/api/answer",
                        json!({"id":1,"value":true}),
                        Some((&bearer, &csrf))
                    ),
                    HOST
                )
                .status,
            409
        );
        let (second_reply, second_received) = mpsc::channel();
        server
            .sender
            .send(Update::Question(
                Question::Text {
                    label: "Next prompt".into(),
                    default: None,
                    secret: true,
                },
                String::new(),
                second_reply,
            ))
            .unwrap();
        server.drain_updates();
        assert!(matches!(second_received.recv().unwrap(), Answer::Cancel));
        assert!(server.pending.is_none());
        server
            .sender
            .send(Update::Finished("Worker finished".into(), true))
            .unwrap();
        server.drain_updates();
        assert_eq!(server.status, "closing");
        server.settle_close();
        assert_eq!(server.status, "closed");
        assert!(!server.can_exit());
        let response = server.handle(
            request("GET", "/api/state", json!({}), Some((&bearer, &csrf))),
            HOST,
        );
        let state: Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(state["status"], "closed");
        assert!(server.can_exit());
    }
}
