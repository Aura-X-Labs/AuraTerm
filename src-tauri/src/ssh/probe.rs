//! Non-interactive SSH connection test for the New Session dialog.
//!
//! Runs the same transport, handshake and first authentication step a real
//! connection does, hop by hop through any jump hosts, and then disconnects.
//!
//! What it deliberately does not do, and why that holds by construction:
//!
//! - **No session.** The command takes no `SshState`, so there is nowhere to
//!   register one; no channel is ever opened on the target.
//! - **No writes to `ssh_known_hosts.json`.** The trusted list is read once and
//!   handed to [`run_probe`] as a plain map; the probe has no `AppHandle` to
//!   save with. A new host is reported, a changed one stops the test before any
//!   credential leaves the machine.
//! - **No prompts.** Keyboard-interactive is never started, so no MFA dialog can
//!   appear; for that auth type the probe only asks the server which methods it
//!   offers (`none` auth).
//! - **No hang.** Every stage has its own timeout, and the whole chain a total.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::{client, keys::PrivateKey, Disconnect, MethodKind};
use tauri::AppHandle;
use tokio::sync::Mutex;

use super::{forwarding, known_hosts, AgentAuthError, JumpHostConfig, SharedSshHandle};
use crate::connection_test::{
    self, describe_duration, ConnectionTestReport, FailedHop, HostKeyReport, HostKeyStatus,
    ProbeFailure, TestCode, TCP_CONNECT_TIMEOUT,
};

type ProbeSession = client::Handle<known_hosts::ClientHandler>;

/// How long a single `disconnect` may take before we just drop the handle.
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-stage time limits. A struct rather than constants so tests can shrink
/// them instead of waiting out the real values.
#[derive(Clone, Copy, Debug)]
pub(super) struct ProbeLimits {
    /// Reaching one hop: resolve + TCP connect for the first hop (the
    /// direct-tcpip open for later ones) *and* the SSH handshake after it,
    /// together. The transport step alone may use all of it.
    pub(super) tcp: Duration,
    /// Version exchange, key exchange and host key, per hop — further capped
    /// by whatever is left of `tcp`.
    pub(super) handshake: Duration,
    /// One authentication step, per hop. Generous because an agent may be
    /// waiting on a hardware key touch.
    pub(super) auth: Duration,
    /// The whole chain, as a backstop for long jump host lists.
    pub(super) total: Duration,
}

impl Default for ProbeLimits {
    fn default() -> Self {
        Self {
            tcp: TCP_CONNECT_TIMEOUT,
            handshake: Duration::from_secs(10),
            auth: Duration::from_secs(20),
            total: Duration::from_secs(60),
        }
    }
}

/// What a hop authenticates with. Lives only in memory for the length of the
/// test; never logged, never reported.
pub(super) enum Credential {
    Password(String),
    /// Password auth with nothing typed: the hop is checked for reachability
    /// and host key, and authentication is skipped.
    NoPassword,
    Key(Arc<PrivateKey>),
    Agent,
    /// Keyboard-interactive: answering it needs a person, so only the server's
    /// offered methods are read.
    KeyboardInteractive,
}

pub(super) struct Hop {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) user: String,
    pub(super) credential: Credential,
    /// 1-based jump host position; `None` for the target.
    pub(super) jump_index: Option<usize>,
}

impl Hop {
    fn endpoint(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    fn label(&self) -> String {
        format!("{}@{}:{}", self.user, self.host, self.port)
    }

    fn failed_hop(&self) -> Option<FailedHop> {
        self.jump_index.map(|index| FailedHop {
            index,
            label: self.label(),
        })
    }
}

#[derive(Debug)]
pub(super) enum PlanError {
    /// The request itself is malformed — an unknown auth type, a key-mode hop
    /// without a key. The dialog should never send one.
    Invalid(String),
    /// A private key did not decode. Reported before anything touches the
    /// network: a wrong passphrase is a local problem.
    Key {
        hop: Option<FailedHop>,
        detail: String,
    },
}

/// Why one hop's credential could not be prepared.
enum CredentialError {
    /// The private key did not decode (bad key, wrong or missing passphrase).
    Key(String),
    /// The request is malformed.
    Invalid(String),
}

/// Resolve one hop's auth type and secrets into a [`Credential`], decoding a
/// private key up front.
fn resolve_credential(
    auth_type: Option<String>,
    password: Option<String>,
    private_key: Option<String>,
    passphrase: Option<String>,
) -> Result<Credential, CredentialError> {
    // Same normalisation as the real connection path.
    let password = password.filter(|value| !value.is_empty());
    let private_key = private_key.filter(|value| !value.trim().is_empty());
    match super::effective_auth_method(auth_type, &password, &private_key).as_str() {
        "key" => {
            let key = private_key
                .ok_or_else(|| CredentialError::Invalid("Private key is required".to_string()))?;
            let decoded = super::decode_private_key(&key, passphrase.as_deref())
                .map_err(CredentialError::Key)?;
            Ok(Credential::Key(Arc::new(decoded)))
        }
        "password" => Ok(password.map_or(Credential::NoPassword, Credential::Password)),
        "agent" => Ok(Credential::Agent),
        "none" => Ok(Credential::KeyboardInteractive),
        other => Err(CredentialError::Invalid(format!(
            "Unsupported SSH auth type: {other}"
        ))),
    }
}

/// Expand "jump hosts 1..N, then the target" into the hops to probe, in order.
/// Every private key is decoded here, before any hop is contacted.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_plan(
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    private_key: Option<String>,
    passphrase: Option<String>,
    auth_type: Option<String>,
    jump_hosts: Vec<JumpHostConfig>,
) -> Result<Vec<Hop>, PlanError> {
    let mut hops = Vec::with_capacity(jump_hosts.len() + 1);

    for (offset, jump) in jump_hosts.into_iter().enumerate() {
        let index = offset + 1;
        let label = format!("{}@{}:{}", jump.user, jump.host, jump.port);
        let credential = resolve_credential(
            Some(jump.auth_type),
            jump.password,
            jump.private_key,
            jump.passphrase,
        )
        .map_err(|error| match error {
            CredentialError::Key(detail) => PlanError::Key {
                hop: Some(FailedHop {
                    index,
                    label: label.clone(),
                }),
                detail,
            },
            CredentialError::Invalid(message) => {
                PlanError::Invalid(format!("ProxyJump {label}: {message}"))
            }
        })?;
        hops.push(Hop {
            host: jump.host,
            port: jump.port,
            user: jump.user,
            credential,
            jump_index: Some(index),
        });
    }

    let credential =
        resolve_credential(auth_type, password, private_key, passphrase).map_err(|error| {
            match error {
                CredentialError::Key(detail) => PlanError::Key { hop: None, detail },
                CredentialError::Invalid(message) => PlanError::Invalid(message),
            }
        })?;
    hops.push(Hop {
        host,
        port,
        user,
        credential,
        jump_index: None,
    });

    Ok(hops)
}

/// Compare a presented fingerprint against the trusted list. Pure: reads the
/// map, never changes it.
pub(super) fn classify_host_key(
    trusted: &HashMap<String, String>,
    scope: &str,
    observed: &str,
) -> (HostKeyStatus, Option<String>) {
    match trusted.get(scope) {
        None => (HostKeyStatus::New, None),
        Some(expected) if expected == observed => (HostKeyStatus::Trusted, None),
        Some(expected) => (HostKeyStatus::Changed, Some(expected.clone())),
    }
}

/// The auth method the probe actually tried, for reading a rejection.
#[derive(Clone, Copy, Debug)]
pub(super) enum ProbeMethod {
    Password,
    PublicKey,
    /// `none` auth, used to list methods for keyboard-interactive.
    None,
}

/// Turn a server rejection into a verdict.
///
/// A rejection that still lists the method we used means "wrong credentials";
/// one that does not means the method itself is not on offer, and the
/// remaining list says whether a person could still get in interactively.
pub(super) fn classify_auth_failure(
    method: ProbeMethod,
    remaining: &[MethodKind],
    partial_success: bool,
) -> TestCode {
    if partial_success {
        return TestCode::AuthPartial;
    }
    match method {
        ProbeMethod::Password if remaining.contains(&MethodKind::Password) => TestCode::AuthFailed,
        ProbeMethod::Password if remaining.contains(&MethodKind::KeyboardInteractive) => {
            TestCode::AuthInteractiveRequired
        }
        ProbeMethod::PublicKey if remaining.contains(&MethodKind::PublicKey) => {
            TestCode::AuthFailed
        }
        ProbeMethod::None if remaining.contains(&MethodKind::KeyboardInteractive) => {
            TestCode::AuthInteractiveUntested
        }
        _ => TestCode::AuthMethodUnsupported,
    }
}

/// How the chain ended, before it is turned into a report.
pub(super) struct Outcome {
    code: TestCode,
    detail: Option<String>,
    failed_hop: Option<FailedHop>,
    untested_hops: Vec<String>,
    auth_methods: Vec<String>,
}

impl Outcome {
    fn ok() -> Self {
        Self {
            code: TestCode::Ok,
            detail: None,
            failed_hop: None,
            untested_hops: Vec::new(),
            auth_methods: Vec::new(),
        }
    }

    fn fail(code: TestCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: Some(detail.into()),
            failed_hop: None,
            untested_hops: Vec::new(),
            auth_methods: Vec::new(),
        }
    }

    fn rejected(code: TestCode, remaining: &[MethodKind]) -> Self {
        Self {
            code,
            detail: None,
            failed_hop: None,
            untested_hops: Vec::new(),
            auth_methods: remaining
                .iter()
                .map(|method| <&str>::from(method).to_string())
                .collect(),
        }
    }

    fn from_failure(failure: ProbeFailure) -> Self {
        Self::fail(failure.code, failure.detail)
    }

    /// Pin the outcome on `hops[index]`, the hop the chain stopped at: name it
    /// if it is a jump host, and list every hop after it as never tested.
    fn at(mut self, hops: &[Hop], index: usize) -> Self {
        self.failed_hop = hops.get(index).and_then(Hop::failed_hop);
        self.untested_hops = hops.iter().skip(index + 1).map(Hop::label).collect();
        self
    }

    fn into_report(self, host_keys: Vec<HostKeyReport>, started: Instant) -> ConnectionTestReport {
        let mut report =
            ConnectionTestReport::new(self.code, started).with_failed_hop(self.failed_hop);
        report.detail = self.detail;
        report.untested_hops = self.untested_hops;
        report.host_keys = host_keys;
        report.auth_methods = self.auth_methods;
        report
    }
}

/// State that has to survive the total timeout cancelling the chain: the
/// connections still to be closed, and the host keys already seen.
struct ProbeContext {
    handles: Vec<SharedSshHandle>,
    host_keys: Vec<HostKeyReport>,
}

/// Probe every hop in order, then close whatever was opened.
///
/// Free of Tauri so it can be driven directly from tests.
pub(super) async fn run_probe(
    trusted: &HashMap<String, String>,
    hops: Vec<Hop>,
    limits: ProbeLimits,
    started: Instant,
) -> ConnectionTestReport {
    let mut ctx = ProbeContext {
        handles: Vec::new(),
        host_keys: Vec::new(),
    };
    let outcome =
        match tokio::time::timeout(limits.total, probe_chain(trusted, &hops, limits, &mut ctx))
            .await
        {
            Ok(outcome) => outcome,
            // Every hop that passed is in `handles`, so the next one is the hop
            // that was still being probed when time ran out.
            Err(_) => Outcome::fail(
                TestCode::Timeout,
                format!(
                    "The test exceeded {} and was stopped",
                    describe_duration(limits.total)
                ),
            )
            .at(&hops, ctx.handles.len()),
        };
    close_chain(std::mem::take(&mut ctx.handles)).await;
    outcome.into_report(ctx.host_keys, started)
}

async fn probe_chain(
    trusted: &HashMap<String, String>,
    hops: &[Hop],
    limits: ProbeLimits,
    ctx: &mut ProbeContext,
) -> Outcome {
    for (index, hop) in hops.iter().enumerate() {
        match probe_hop(trusted, hop, limits, ctx).await {
            Ok(session) => ctx.handles.push(Arc::new(Mutex::new(session))),
            Err(outcome) => return outcome.at(hops, index),
        }
    }
    Outcome::ok()
}

/// Reach one hop, check its host key and authenticate. On any failure the hop's
/// own connection is closed before returning.
async fn probe_hop(
    trusted: &HashMap<String, String>,
    hop: &Hop,
    limits: ProbeLimits,
    ctx: &mut ProbeContext,
) -> Result<ProbeSession, Outcome> {
    let endpoint = hop.endpoint();
    // Reaching the hop — transport and handshake together — shares one budget,
    // so a slow connect cannot be followed by a full-length handshake wait.
    let reach_deadline = Instant::now() + limits.tcp;

    // 1. Transport: a TCP connection for the first hop, a direct-tcpip channel
    //    through the previous hop for every later one.
    let (session, observed) = match ctx.handles.last().cloned() {
        None => {
            // The same address form `russh::client::connect` resolves.
            let stream = connection_test::tcp_connect(endpoint.clone(), &endpoint, limits.tcp)
                .await
                .map_err(Outcome::from_failure)?;
            handshake(stream, &endpoint, limits, reach_deadline).await?
        }
        Some(previous) => {
            let channel = match tokio::time::timeout(
                limits.tcp,
                super::open_proxy_channel(previous, hop.host.clone(), hop.port),
            )
            .await
            {
                Ok(Ok(channel)) => channel,
                Ok(Err(error)) => return Err(Outcome::fail(TestCode::ConnectFailed, error)),
                Err(_) => {
                    return Err(Outcome::fail(
                        TestCode::Timeout,
                        format!(
                            "The previous hop did not open a channel to {endpoint} within {}",
                            describe_duration(limits.tcp),
                        ),
                    ))
                }
            };
            handshake(channel.into_stream(), &endpoint, limits, reach_deadline).await?
        }
    };

    // 2. Host key, read-only.
    let Some(observed) = observed else {
        close_session(&session).await;
        return Err(Outcome::fail(
            TestCode::HandshakeFailed,
            format!("SSH server {endpoint} did not present a host key"),
        ));
    };
    // Untrimmed, exactly as the real connection path scopes it.
    let scope = known_hosts::known_host_scope(&hop.host, hop.port);
    let (status, expected) = classify_host_key(trusted, &scope, &observed);
    ctx.host_keys.push(HostKeyReport {
        label: scope.clone(),
        status,
        fingerprint: observed,
        expected_fingerprint: expected,
    });
    if status == HostKeyStatus::Changed {
        // Stop before sending anything a man in the middle could collect.
        close_session(&session).await;
        return Err(Outcome::fail(
            TestCode::HostKeyChanged,
            format!("The host key for {scope} does not match the trusted one"),
        ));
    }

    // 3. Authentication, at most one attempt per method.
    let mut session = session;
    let verdict = match tokio::time::timeout(limits.auth, authenticate_hop(&mut session, hop)).await
    {
        Ok(verdict) => verdict,
        Err(_) => Err(Outcome::fail(
            TestCode::AuthTimeout,
            format!(
                "No answer to the authentication request within {}",
                describe_duration(limits.auth)
            ),
        )),
    };
    match verdict {
        Ok(()) => Ok(session),
        Err(outcome) => {
            close_session(&session).await;
            Err(outcome)
        }
    }
}

/// Run the SSH handshake over `stream`, within `limits.handshake` and never past
/// `reach_deadline`, the end of the hop's connect-plus-handshake budget.
async fn handshake<R>(
    stream: R,
    endpoint: &str,
    limits: ProbeLimits,
    reach_deadline: Instant,
) -> Result<(ProbeSession, Option<String>), Outcome>
where
    R: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let remaining = reach_deadline.saturating_duration_since(Instant::now());
    let (limit, timed_out) = if limits.handshake <= remaining {
        (
            limits.handshake,
            format!(
                "{endpoint} did not finish the SSH handshake within {}",
                describe_duration(limits.handshake)
            ),
        )
    } else {
        (
            remaining,
            format!(
                "{endpoint} did not finish connecting and the SSH handshake within {}",
                describe_duration(limits.tcp),
            ),
        )
    };

    // No expected fingerprint: the handshake accepts any key and records it,
    // and the comparison happens afterwards against the read-only map. Agent
    // forwarding off, so the handler refuses any agent channel the server opens.
    let attempt = known_hosts::connect_stream_with_expected_fingerprint(
        stream,
        None,
        forwarding::new_remote_forward_registry(),
        false,
    );
    match tokio::time::timeout(limit, attempt).await {
        Ok(Ok(connected)) => Ok(connected),
        Ok(Err((error, _))) => Err(Outcome::fail(TestCode::HandshakeFailed, error)),
        Err(_) => Err(Outcome::fail(TestCode::HandshakeTimeout, timed_out)),
    }
}

/// One non-interactive authentication attempt for `hop`.
async fn authenticate_hop(session: &mut ProbeSession, hop: &Hop) -> Result<(), Outcome> {
    let user = hop.user.clone();
    let (method, result) = match &hop.credential {
        Credential::NoPassword => {
            return Err(Outcome::fail(
                TestCode::PasswordMissing,
                "No password was entered, so authentication was not attempted",
            ))
        }
        Credential::Password(password) => (
            ProbeMethod::Password,
            session
                .authenticate_password(user, password.clone())
                .await
                .map_err(|error| format!("Password auth error: {error}")),
        ),
        Credential::Key(key) => (
            ProbeMethod::PublicKey,
            super::authenticate_with_decoded_key(session, user, key.clone()).await,
        ),
        Credential::Agent => match super::try_agent_identities(session, &hop.user).await {
            Ok(result) => (ProbeMethod::PublicKey, Ok(result)),
            Err(AgentAuthError::Unavailable(message)) => {
                return Err(Outcome::fail(TestCode::AgentUnavailable, message))
            }
            Err(AgentAuthError::Failed(message)) => {
                return Err(Outcome::fail(TestCode::AuthFailed, message))
            }
        },
        // Never `authenticate_keyboard_interactive_start`: that is the path that
        // ends in an MFA prompt. `none` auth just returns the offered methods.
        Credential::KeyboardInteractive => (
            ProbeMethod::None,
            session
                .authenticate_none(user)
                .await
                .map_err(|error| format!("Auth error: {error}")),
        ),
    };

    match result {
        Ok(client::AuthResult::Success) => Ok(()),
        Ok(client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        }) => {
            let code = classify_auth_failure(method, &remaining_methods, partial_success);
            Err(Outcome::rejected(code, &remaining_methods))
        }
        // Typically the server hanging up after too many attempts.
        Err(error) => Err(Outcome::fail(TestCode::AuthFailed, error)),
    }
}

/// Best-effort polite disconnect; the handle is dropped right after either way.
async fn close_session(session: &ProbeSession) {
    let _ = tokio::time::timeout(
        DISCONNECT_TIMEOUT,
        session.disconnect(Disconnect::ByApplication, "", "en"),
    )
    .await;
}

/// Disconnect from the innermost hop outwards, so each jump host sees its
/// forwarded channel close before its own connection goes.
async fn close_chain(handles: Vec<SharedSshHandle>) {
    for handle in handles.into_iter().rev() {
        let session = handle.lock().await;
        close_session(&session).await;
    }
}

/// Try the connection described by the New Session form without opening a
/// session, saving anything, or trusting a new host key.
///
/// Every expected outcome — refused, timed out, wrong password — comes back as
/// `Ok(report)`. `Err` is reserved for requests the dialog should never send
/// and for a trusted-hosts file that cannot be read.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ssh_test_connection(
    app: AppHandle,
    host: String,
    port: u16,
    user: String,
    password: Option<String>,
    private_key: Option<String>,
    passphrase: Option<String>,
    auth_type: Option<String>,
    jump_hosts: Option<Vec<JumpHostConfig>>,
) -> Result<ConnectionTestReport, String> {
    let started = Instant::now();
    // Read-only: the probe gets a copy of the map and never sees `app`.
    let trusted = known_hosts::load_known_hosts(&app).await?.hosts;
    let hops = match build_plan(
        host,
        port,
        user,
        password,
        private_key,
        passphrase,
        auth_type,
        jump_hosts.unwrap_or_default(),
    ) {
        Ok(hops) => hops,
        Err(PlanError::Invalid(message)) => return Err(message),
        Err(PlanError::Key { hop, detail }) => {
            return Ok(ConnectionTestReport::new(TestCode::KeyInvalid, started)
                .with_detail(detail)
                .with_failed_hop(hop))
        }
    };
    Ok(run_probe(&trusted, hops, ProbeLimits::default(), started).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection_test::TestStatus;
    use russh::keys::ssh_key::LineEnding;
    use russh::keys::{Algorithm, HashAlg};
    use russh::server::{self, Auth};
    use russh::{ChannelOpenFailure, MethodSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use tokio::io::AsyncWriteExt;

    // ── Pure pieces ─────────────────────────────────────────────────────

    fn trusted_map(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(scope, fp)| (scope.to_string(), fp.to_string()))
            .collect()
    }

    #[test]
    fn host_keys_are_new_trusted_or_changed() {
        let trusted = trusted_map(&[("10.0.0.5:22", "SHA256:known")]);
        let snapshot = trusted.clone();

        assert_eq!(
            classify_host_key(&trusted, "10.0.0.6:22", "SHA256:x"),
            (HostKeyStatus::New, None)
        );
        assert_eq!(
            classify_host_key(&trusted, "10.0.0.5:22", "SHA256:known"),
            (HostKeyStatus::Trusted, None),
        );
        assert_eq!(
            classify_host_key(&trusted, "10.0.0.5:22", "SHA256:other"),
            (HostKeyStatus::Changed, Some("SHA256:known".to_string())),
        );
        // Same host, different port: a different scope, so a new host.
        assert_eq!(
            classify_host_key(&trusted, "10.0.0.5:2222", "SHA256:known").0,
            HostKeyStatus::New
        );
        assert_eq!(
            trusted, snapshot,
            "classification must never touch the trusted list"
        );
    }

    #[test]
    fn auth_rejections_are_read_by_what_is_still_on_offer() {
        use MethodKind::{KeyboardInteractive as Ki, Password as Pw, PublicKey as Pk};
        let cases: &[(ProbeMethod, &[MethodKind], bool, TestCode)] = &[
            // A partial success beats everything: the credential was right.
            (ProbeMethod::Password, &[Pk], true, TestCode::AuthPartial),
            (ProbeMethod::PublicKey, &[], true, TestCode::AuthPartial),
            (ProbeMethod::None, &[Ki], true, TestCode::AuthPartial),
            // Password.
            (
                ProbeMethod::Password,
                &[Pw, Pk],
                false,
                TestCode::AuthFailed,
            ),
            (
                ProbeMethod::Password,
                &[Pw, Ki],
                false,
                TestCode::AuthFailed,
            ),
            (
                ProbeMethod::Password,
                &[Pk, Ki],
                false,
                TestCode::AuthInteractiveRequired,
            ),
            (
                ProbeMethod::Password,
                &[Pk],
                false,
                TestCode::AuthMethodUnsupported,
            ),
            (
                ProbeMethod::Password,
                &[],
                false,
                TestCode::AuthMethodUnsupported,
            ),
            // Public key (file or agent).
            (
                ProbeMethod::PublicKey,
                &[Pk, Pw],
                false,
                TestCode::AuthFailed,
            ),
            (
                ProbeMethod::PublicKey,
                &[Pw, Ki],
                false,
                TestCode::AuthMethodUnsupported,
            ),
            (
                ProbeMethod::PublicKey,
                &[],
                false,
                TestCode::AuthMethodUnsupported,
            ),
            // `none`, for keyboard-interactive.
            (
                ProbeMethod::None,
                &[Pw, Ki],
                false,
                TestCode::AuthInteractiveUntested,
            ),
            (
                ProbeMethod::None,
                &[Pw, Pk],
                false,
                TestCode::AuthMethodUnsupported,
            ),
        ];
        for (method, remaining, partial, expected) in cases {
            assert_eq!(
                classify_auth_failure(*method, remaining, *partial),
                *expected,
                "{method:?} remaining={remaining:?} partial={partial}",
            );
        }
    }

    fn plan(
        password: Option<&str>,
        private_key: Option<&str>,
        passphrase: Option<&str>,
        auth_type: Option<&str>,
        jump_hosts: Vec<JumpHostConfig>,
    ) -> Result<Vec<Hop>, PlanError> {
        build_plan(
            "10.0.0.5".to_string(),
            22,
            "ops".to_string(),
            password.map(str::to_string),
            private_key.map(str::to_string),
            passphrase.map(str::to_string),
            auth_type.map(str::to_string),
            jump_hosts,
        )
    }

    fn jump(host: &str, auth_type: &str) -> JumpHostConfig {
        JumpHostConfig {
            id: format!("j-{host}"),
            host: host.to_string(),
            port: 22,
            user: "jump".to_string(),
            auth_type: auth_type.to_string(),
            password: None,
            private_key: None,
            passphrase: None,
        }
    }

    fn client_key() -> PrivateKey {
        PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519).expect("key")
    }

    fn openssh(key: &PrivateKey) -> String {
        key.to_openssh(LineEnding::LF).expect("encode").to_string()
    }

    fn plan_ok(result: Result<Vec<Hop>, PlanError>) -> Vec<Hop> {
        match result {
            Ok(hops) => hops,
            Err(error) => panic!("expected a plan, got {error:?}"),
        }
    }

    #[test]
    fn the_plan_puts_jump_hosts_first_and_the_target_last() {
        let hops = plan_ok(plan(
            Some("pw"),
            None,
            None,
            Some("password"),
            vec![jump("a", "agent"), jump("b", "none")],
        ));
        assert_eq!(hops.len(), 3);
        assert_eq!((hops[0].host.as_str(), hops[0].jump_index), ("a", Some(1)));
        assert_eq!((hops[1].host.as_str(), hops[1].jump_index), ("b", Some(2)));
        assert_eq!(
            (hops[2].host.as_str(), hops[2].jump_index),
            ("10.0.0.5", None)
        );
        assert!(matches!(hops[0].credential, Credential::Agent));
        assert!(matches!(
            hops[1].credential,
            Credential::KeyboardInteractive
        ));
        assert!(matches!(hops[2].credential, Credential::Password(ref p) if p == "pw"));
        assert_eq!(
            hops[1].failed_hop(),
            Some(FailedHop {
                index: 2,
                label: "jump@b:22".to_string()
            })
        );
        assert_eq!(hops[2].failed_hop(), None);
    }

    #[test]
    fn an_empty_password_skips_authentication_instead_of_sending_it() {
        let hops = plan_ok(plan(Some(""), None, None, Some("password"), vec![]));
        assert!(matches!(hops[0].credential, Credential::NoPassword));
        let hops = plan_ok(plan(None, None, None, Some("password"), vec![]));
        assert!(matches!(hops[0].credential, Credential::NoPassword));
    }

    #[test]
    fn an_unnamed_auth_type_is_inferred_like_a_real_connection() {
        let key = openssh(&client_key());
        let hops = plan_ok(plan(Some("pw"), Some(&key), None, None, vec![]));
        assert!(
            matches!(hops[0].credential, Credential::Key(_)),
            "a key beats a password"
        );
        let hops = plan_ok(plan(Some("pw"), None, None, None, vec![]));
        assert!(matches!(hops[0].credential, Credential::Password(_)));
        let hops = plan_ok(plan(None, Some("   "), None, None, vec![]));
        assert!(
            matches!(hops[0].credential, Credential::KeyboardInteractive),
            "a blank key is no key"
        );
    }

    #[test]
    fn a_bad_private_key_fails_the_plan_before_any_connection() {
        // A listener that would record any contact.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("addr").port();

        let result = build_plan(
            "127.0.0.1".to_string(),
            port,
            "ops".to_string(),
            None,
            Some(
                "-----BEGIN OPENSSH PRIVATE KEY-----\nnot a key\n-----END OPENSSH PRIVATE KEY-----"
                    .to_string(),
            ),
            None,
            Some("key".to_string()),
            vec![],
        );
        match result {
            Err(PlanError::Key { hop, detail }) => {
                assert_eq!(hop, None, "the target is not a jump host");
                assert!(detail.starts_with("Private key parse error"), "{detail}");
            }
            Err(other) => panic!("expected a key error, got {other:?}"),
            Ok(_) => panic!("a garbage key must not plan"),
        }
        let accepted = listener.accept();
        assert!(
            matches!(&accepted, Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "the target was contacted: {accepted:?}",
        );
    }

    #[test]
    fn a_wrong_passphrase_is_a_key_error_on_the_hop_that_has_it() {
        let mut rng = russh::keys::key::safe_rng();
        let encrypted = openssh(&client_key().encrypt(&mut rng, b"right").expect("encrypt"));

        let mut second = jump("b", "key");
        second.private_key = Some(encrypted.clone());
        second.passphrase = Some("wrong".to_string());
        match plan(
            Some("pw"),
            None,
            None,
            Some("password"),
            vec![jump("a", "agent"), second],
        ) {
            Err(PlanError::Key { hop, .. }) => {
                assert_eq!(
                    hop,
                    Some(FailedHop {
                        index: 2,
                        label: "jump@b:22".to_string()
                    })
                );
            }
            Err(other) => panic!("expected a key error, got {other:?}"),
            Ok(_) => panic!("a wrong passphrase must not plan"),
        }

        // No passphrase at all is just as wrong.
        assert!(matches!(
            plan(None, Some(&encrypted), None, Some("key"), vec![]),
            Err(PlanError::Key { hop: None, .. }),
        ));
        // And the right one decodes.
        let hops = plan_ok(plan(
            None,
            Some(&encrypted),
            Some("right"),
            Some("key"),
            vec![],
        ));
        assert!(matches!(hops[0].credential, Credential::Key(_)));
    }

    #[test]
    fn malformed_requests_are_invalid_not_key_errors() {
        match plan(None, None, None, Some("kerberos"), vec![]) {
            Err(PlanError::Invalid(message)) => assert!(message.contains("kerberos"), "{message}"),
            Err(other) => panic!("expected invalid, got {other:?}"),
            Ok(_) => panic!("an unknown auth type must not plan"),
        }
        match plan(None, None, None, Some("key"), vec![]) {
            Err(PlanError::Invalid(message)) => assert_eq!(message, "Private key is required"),
            Err(other) => panic!("expected invalid, got {other:?}"),
            Ok(_) => panic!("key mode without a key must not plan"),
        }
        match plan(
            Some("pw"),
            None,
            None,
            Some("password"),
            vec![jump("bastion", "key")],
        ) {
            Err(PlanError::Invalid(message)) => {
                assert!(
                    message.starts_with("ProxyJump jump@bastion:22"),
                    "{message}"
                );
            }
            Err(other) => panic!("expected invalid, got {other:?}"),
            Ok(_) => panic!("a key-mode jump host without a key must not plan"),
        }
    }

    // ── Transport and handshake failures ────────────────────────────────

    fn quick_limits() -> ProbeLimits {
        ProbeLimits {
            tcp: Duration::from_secs(5),
            handshake: Duration::from_secs(5),
            auth: Duration::from_secs(5),
            total: Duration::from_secs(30),
        }
    }

    fn target(port: u16, credential: Credential) -> Vec<Hop> {
        vec![Hop {
            host: "127.0.0.1".to_string(),
            port,
            user: "ops".to_string(),
            credential,
            jump_index: None,
        }]
    }

    #[tokio::test]
    async fn a_closed_port_is_refused() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let report = run_probe(
            &HashMap::new(),
            target(port, Credential::NoPassword),
            quick_limits(),
            Instant::now(),
        )
        .await;
        assert_eq!(report.code, TestCode::Refused, "{:?}", report.detail);
        assert!(report.host_keys.is_empty());
        assert!(report.failed_hop.is_none());
    }

    #[tokio::test]
    async fn a_non_ssh_service_fails_the_handshake() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
                let _ = stream.shutdown().await;
            }
        });
        let report = run_probe(
            &HashMap::new(),
            target(port, Credential::NoPassword),
            quick_limits(),
            Instant::now(),
        )
        .await;
        assert_eq!(
            report.code,
            TestCode::HandshakeFailed,
            "{:?}",
            report.detail
        );
        assert_eq!(report.status, TestStatus::Failure);
        assert!(report.host_keys.is_empty());
    }

    #[tokio::test]
    async fn a_silent_server_times_out_the_handshake() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hold = tokio::spawn(async move {
            let accepted = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(accepted);
        });
        let limits = ProbeLimits {
            handshake: Duration::from_millis(300),
            ..quick_limits()
        };
        let started = Instant::now();
        let report = run_probe(
            &HashMap::new(),
            target(port, Credential::NoPassword),
            limits,
            started,
        )
        .await;
        assert_eq!(
            report.code,
            TestCode::HandshakeTimeout,
            "{:?}",
            report.detail
        );
        assert!(
            report.detail.as_deref().unwrap_or("").contains("300ms"),
            "{:?}",
            report.detail
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        hold.abort();
    }

    #[tokio::test]
    async fn the_total_limit_stops_a_probe_that_would_otherwise_wait() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hold = tokio::spawn(async move {
            let accepted = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(accepted);
        });
        let limits = ProbeLimits {
            total: Duration::from_millis(400),
            ..quick_limits()
        };
        let started = Instant::now();
        let report = run_probe(
            &HashMap::new(),
            target(port, Credential::NoPassword),
            limits,
            started,
        )
        .await;
        assert_eq!(report.code, TestCode::Timeout, "{:?}", report.detail);
        assert_eq!(
            report.detail.as_deref(),
            Some("The test exceeded 400ms and was stopped")
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        hold.abort();
    }

    #[tokio::test]
    async fn connect_and_handshake_share_one_budget_per_hop() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hold = tokio::spawn(async move {
            let accepted = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(accepted);
        });
        // A handshake limit longer than the reach budget: the budget wins.
        let limits = ProbeLimits {
            tcp: Duration::from_millis(300),
            handshake: Duration::from_secs(5),
            ..quick_limits()
        };
        let started = Instant::now();
        let report = run_probe(
            &HashMap::new(),
            target(port, Credential::NoPassword),
            limits,
            started,
        )
        .await;
        assert_eq!(
            report.code,
            TestCode::HandshakeTimeout,
            "{:?}",
            report.detail
        );
        assert_eq!(
            report.detail,
            Some(format!(
                "127.0.0.1:{port} did not finish connecting and the SSH handshake within 300ms"
            )),
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        hold.abort();
    }

    #[test]
    fn the_hops_after_the_one_that_stopped_the_chain_are_untested() {
        let hops = plan_ok(plan(
            Some("pw"),
            None,
            None,
            Some("password"),
            vec![jump("a", "agent"), jump("b", "none")],
        ));

        let at_first = Outcome::fail(TestCode::AuthFailed, "x").at(&hops, 0);
        assert_eq!(
            at_first.failed_hop,
            Some(FailedHop {
                index: 1,
                label: "jump@a:22".to_string()
            })
        );
        assert_eq!(
            at_first.untested_hops,
            vec!["jump@b:22".to_string(), "ops@10.0.0.5:22".to_string()]
        );

        let at_target = Outcome::fail(TestCode::AuthFailed, "x").at(&hops, 2);
        assert_eq!(at_target.failed_hop, None);
        assert!(at_target.untested_hops.is_empty());

        // Out of range (cannot happen, but must not panic).
        let past_end = Outcome::fail(TestCode::Timeout, "x").at(&hops, 3);
        assert_eq!(past_end.failed_hop, None);
        assert!(past_end.untested_hops.is_empty());
    }

    // ── Against a real SSH server, in process ───────────────────────────

    /// What the in-process server does.
    #[derive(Clone)]
    struct Behaviour {
        /// Methods the server advertises.
        methods: Vec<MethodKind>,
        /// The password it accepts, if any.
        password: Option<&'static str>,
        /// What a rejected password leaves on offer. OpenSSH keeps `password`
        /// listed after a wrong one; russh's default drops it.
        after_password_reject: Option<Vec<MethodKind>>,
        /// The client key it accepts, if any.
        accepted_key: Option<russh::keys::PublicKey>,
        /// Accept `none` auth outright.
        accept_none: bool,
        /// Sit on a password request this long before answering.
        password_delay: Option<Duration>,
    }

    impl Default for Behaviour {
        fn default() -> Self {
            Self {
                methods: vec![
                    MethodKind::Password,
                    MethodKind::PublicKey,
                    MethodKind::KeyboardInteractive,
                ],
                password: Some("hunter2"),
                after_password_reject: Some(vec![
                    MethodKind::Password,
                    MethodKind::PublicKey,
                    MethodKind::KeyboardInteractive,
                ]),
                accepted_key: None,
                accept_none: false,
                password_delay: None,
            }
        }
    }

    /// Everything a connection did, as the server saw it.
    #[derive(Clone, Default)]
    struct Journal {
        calls: Arc<StdMutex<Vec<&'static str>>>,
        accepted: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    impl Journal {
        fn record(&self, call: &'static str) {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(call);
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls
                .lock()
                .map(|calls| calls.clone())
                .unwrap_or_default()
        }

        fn count(&self, call: &str) -> usize {
            self.calls().iter().filter(|seen| **seen == call).count()
        }

        /// Wait until every accepted connection has ended.
        async fn all_closed(&self) -> bool {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if self.closed.load(Ordering::SeqCst) == self.accepted.load(Ordering::SeqCst) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            false
        }
    }

    struct TestHandler {
        behaviour: Behaviour,
        journal: Journal,
    }

    impl server::Handler for TestHandler {
        type Error = russh::Error;

        async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
            self.journal.record("none");
            Ok(if self.behaviour.accept_none {
                Auth::Accept
            } else {
                Auth::reject()
            })
        }

        async fn auth_password(
            &mut self,
            _user: &str,
            password: &str,
        ) -> Result<Auth, Self::Error> {
            self.journal.record("password");
            if let Some(delay) = self.behaviour.password_delay {
                tokio::time::sleep(delay).await;
            }
            if self.behaviour.password == Some(password) {
                return Ok(Auth::Accept);
            }
            Ok(Auth::Reject {
                proceed_with_methods: self
                    .behaviour
                    .after_password_reject
                    .as_ref()
                    .map(|methods| MethodSet::from(methods.as_slice())),
                partial_success: false,
            })
        }

        async fn auth_publickey(
            &mut self,
            _user: &str,
            public_key: &russh::keys::PublicKey,
        ) -> Result<Auth, Self::Error> {
            self.journal.record("publickey");
            let accepted = self
                .behaviour
                .accepted_key
                .as_ref()
                .is_some_and(|key| key.key_data() == public_key.key_data());
            Ok(if accepted {
                Auth::Accept
            } else {
                Auth::reject()
            })
        }

        async fn auth_keyboard_interactive<'a>(
            &'a mut self,
            _user: &str,
            _submethods: &str,
            _response: Option<server::Response<'a>>,
        ) -> Result<Auth, Self::Error> {
            self.journal.record("keyboard-interactive");
            Ok(Auth::reject())
        }

        async fn channel_open_session(
            &mut self,
            _channel: russh::Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            // The test must never get this far.
            self.journal.record("session-channel");
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            Ok(())
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: russh::Channel<server::Msg>,
            host_to_connect: &str,
            port_to_connect: u32,
            _originator_address: &str,
            _originator_port: u32,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            self.journal.record("direct-tcpip");
            let target = format!("{host_to_connect}:{port_to_connect}");
            match tokio::net::TcpStream::connect(target).await {
                Ok(mut upstream) => {
                    reply.accept().await;
                    tokio::spawn(async move {
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
                    });
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
            Ok(())
        }
    }

    struct TestServer {
        port: u16,
        fingerprint: String,
        journal: Journal,
    }

    impl TestServer {
        fn scope(&self) -> String {
            known_hosts::known_host_scope("127.0.0.1", self.port)
        }

        fn hop(&self, credential: Credential, jump_index: Option<usize>) -> Hop {
            Hop {
                host: "127.0.0.1".to_string(),
                port: self.port,
                user: "ops".to_string(),
                credential,
                jump_index,
            }
        }
    }

    async fn spawn_server(behaviour: Behaviour) -> TestServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let host_key = PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519)
            .expect("host key");
        let fingerprint = host_key
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();
        let config = Arc::new(server::Config {
            keys: vec![host_key],
            methods: MethodSet::from(behaviour.methods.as_slice()),
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        let journal = Journal::default();
        let shared = journal.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                shared.accepted.fetch_add(1, Ordering::SeqCst);
                let handler = TestHandler {
                    behaviour: behaviour.clone(),
                    journal: shared.clone(),
                };
                let config = config.clone();
                let journal = shared.clone();
                tokio::spawn(async move {
                    if let Ok(running) = server::run_stream(config, stream, handler).await {
                        let _ = running.await;
                    }
                    journal.closed.fetch_add(1, Ordering::SeqCst);
                });
            }
        });
        TestServer {
            port,
            fingerprint,
            journal,
        }
    }

    fn password(value: &str) -> Credential {
        Credential::Password(value.to_string())
    }

    #[tokio::test]
    async fn a_new_host_with_the_right_password_passes_and_nothing_is_trusted() {
        let server = spawn_server(Behaviour::default()).await;
        let trusted = HashMap::new();
        let report = run_probe(
            &trusted,
            vec![server.hop(password("hunter2"), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;

        assert_eq!(report.code, TestCode::Ok, "{:?}", report.detail);
        assert_eq!(report.status, TestStatus::Success);
        assert_eq!(report.host_keys.len(), 1);
        let key = &report.host_keys[0];
        assert_eq!(key.label, server.scope());
        assert_eq!(key.status, HostKeyStatus::New);
        assert_eq!(key.fingerprint, server.fingerprint);
        assert_eq!(key.expected_fingerprint, None);
        assert!(trusted.is_empty(), "a test must not trust a new host");

        assert_eq!(server.journal.calls(), vec!["password"]);
        assert!(
            server.journal.all_closed().await,
            "the connection must be closed afterwards"
        );
        assert_eq!(
            server.journal.count("session-channel"),
            0,
            "no session is ever opened"
        );
    }

    #[tokio::test]
    async fn a_known_host_with_the_same_key_is_trusted() {
        let server = spawn_server(Behaviour::default()).await;
        let trusted = trusted_map(&[(&server.scope(), &server.fingerprint)]);
        let report = run_probe(
            &trusted,
            vec![server.hop(password("hunter2"), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;
        assert_eq!(report.code, TestCode::Ok, "{:?}", report.detail);
        assert_eq!(report.host_keys[0].status, HostKeyStatus::Trusted);
        assert!(server.journal.all_closed().await);
    }

    #[tokio::test]
    async fn a_changed_host_key_stops_before_any_credential_is_sent() {
        let server = spawn_server(Behaviour::default()).await;
        let trusted = trusted_map(&[(&server.scope(), "SHA256:not-this-server")]);
        let report = run_probe(
            &trusted,
            vec![server.hop(password("hunter2"), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;

        assert_eq!(report.code, TestCode::HostKeyChanged, "{:?}", report.detail);
        assert_eq!(report.status, TestStatus::Failure);
        assert_eq!(report.host_keys.len(), 1);
        assert_eq!(report.host_keys[0].status, HostKeyStatus::Changed);
        assert_eq!(report.host_keys[0].fingerprint, server.fingerprint);
        assert_eq!(
            report.host_keys[0].expected_fingerprint.as_deref(),
            Some("SHA256:not-this-server")
        );
        assert!(server.journal.all_closed().await);
        assert!(
            server.journal.calls().is_empty(),
            "credentials reached the server: {:?}",
            server.journal.calls()
        );
        assert_eq!(
            trusted.get(&server.scope()).map(String::as_str),
            Some("SHA256:not-this-server")
        );
    }

    #[tokio::test]
    async fn a_wrong_password_is_tried_exactly_once() {
        let server = spawn_server(Behaviour::default()).await;
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(password("wrong"), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;

        assert_eq!(report.code, TestCode::AuthFailed, "{:?}", report.detail);
        assert!(
            report.auth_methods.contains(&"password".to_string()),
            "{:?}",
            report.auth_methods
        );
        assert!(server.journal.all_closed().await);
        assert_eq!(
            server.journal.count("password"),
            1,
            "{:?}",
            server.journal.calls()
        );
        assert_eq!(
            server.journal.count("keyboard-interactive"),
            0,
            "no fallback to an interactive prompt"
        );
    }

    #[tokio::test]
    async fn a_server_that_wants_keyboard_interactive_for_passwords_says_so() {
        let server = spawn_server(Behaviour {
            methods: vec![MethodKind::PublicKey, MethodKind::KeyboardInteractive],
            password: None,
            after_password_reject: None,
            ..Behaviour::default()
        })
        .await;
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(password("hunter2"), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;

        assert_eq!(
            report.code,
            TestCode::AuthInteractiveRequired,
            "{:?}",
            report.detail
        );
        assert_eq!(report.status, TestStatus::Warning);
        assert!(
            report
                .auth_methods
                .contains(&"keyboard-interactive".to_string()),
            "{:?}",
            report.auth_methods
        );
        assert!(server.journal.all_closed().await);
        assert_eq!(server.journal.count("keyboard-interactive"), 0);
    }

    #[tokio::test]
    async fn a_server_without_the_chosen_method_is_unsupported() {
        let server = spawn_server(Behaviour {
            methods: vec![MethodKind::PublicKey],
            password: None,
            after_password_reject: None,
            ..Behaviour::default()
        })
        .await;
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(password("hunter2"), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;
        assert_eq!(
            report.code,
            TestCode::AuthMethodUnsupported,
            "{:?}",
            report.detail
        );
        assert_eq!(report.auth_methods, vec!["publickey".to_string()]);
    }

    #[tokio::test]
    async fn keyboard_interactive_mode_never_starts_a_prompt() {
        let server = spawn_server(Behaviour::default()).await;
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(Credential::KeyboardInteractive, None)],
            quick_limits(),
            Instant::now(),
        )
        .await;

        assert_eq!(
            report.code,
            TestCode::AuthInteractiveUntested,
            "{:?}",
            report.detail
        );
        assert_eq!(report.status, TestStatus::Warning);
        assert_eq!(report.host_keys.len(), 1, "the host key is still checked");
        assert!(server.journal.all_closed().await);
        assert_eq!(server.journal.calls(), vec!["none"]);
        assert_eq!(server.journal.count("keyboard-interactive"), 0);
    }

    #[tokio::test]
    async fn keyboard_interactive_mode_passes_when_the_server_needs_no_credentials() {
        let server = spawn_server(Behaviour {
            accept_none: true,
            ..Behaviour::default()
        })
        .await;
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(Credential::KeyboardInteractive, None)],
            quick_limits(),
            Instant::now(),
        )
        .await;
        assert_eq!(report.code, TestCode::Ok, "{:?}", report.detail);
    }

    #[tokio::test]
    async fn no_password_checks_reach_and_host_key_but_sends_nothing() {
        let server = spawn_server(Behaviour::default()).await;
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(Credential::NoPassword, None)],
            quick_limits(),
            Instant::now(),
        )
        .await;

        assert_eq!(
            report.code,
            TestCode::PasswordMissing,
            "{:?}",
            report.detail
        );
        assert_eq!(report.status, TestStatus::Warning);
        assert_eq!(report.host_keys.len(), 1);
        assert_eq!(report.host_keys[0].status, HostKeyStatus::New);
        assert!(server.journal.all_closed().await);
        assert!(
            server.journal.calls().is_empty(),
            "{:?}",
            server.journal.calls()
        );
    }

    #[tokio::test]
    async fn an_accepted_private_key_passes_and_a_foreign_one_fails() {
        let key = client_key();
        let server = spawn_server(Behaviour {
            accepted_key: Some(key.public_key().clone()),
            ..Behaviour::default()
        })
        .await;

        let ok = run_probe(
            &HashMap::new(),
            vec![server.hop(Credential::Key(Arc::new(key)), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;
        assert_eq!(ok.code, TestCode::Ok, "{:?}", ok.detail);

        let rejected = run_probe(
            &HashMap::new(),
            vec![server.hop(Credential::Key(Arc::new(client_key())), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;
        assert_eq!(rejected.code, TestCode::AuthFailed, "{:?}", rejected.detail);
        assert!(
            rejected.auth_methods.contains(&"publickey".to_string()),
            "{:?}",
            rejected.auth_methods
        );
        assert!(server.journal.all_closed().await);
        assert_eq!(server.journal.count("session-channel"), 0);
    }

    #[tokio::test]
    async fn an_encrypted_key_is_decoded_from_the_plan_and_used() {
        let key = client_key();
        let mut rng = russh::keys::key::safe_rng();
        let encrypted = openssh(&key.encrypt(&mut rng, b"sesame").expect("encrypt"));
        let server = spawn_server(Behaviour {
            accepted_key: Some(key.public_key().clone()),
            ..Behaviour::default()
        })
        .await;

        let hops = plan_ok(build_plan(
            "127.0.0.1".to_string(),
            server.port,
            "ops".to_string(),
            None,
            Some(encrypted),
            Some("sesame".to_string()),
            Some("key".to_string()),
            vec![],
        ));
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;
        assert_eq!(report.code, TestCode::Ok, "{:?}", report.detail);
    }

    #[tokio::test]
    async fn a_slow_authentication_times_out() {
        let server = spawn_server(Behaviour {
            password_delay: Some(Duration::from_secs(3)),
            ..Behaviour::default()
        })
        .await;
        let limits = ProbeLimits {
            auth: Duration::from_millis(300),
            ..quick_limits()
        };
        let started = Instant::now();
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(password("hunter2"), None)],
            limits,
            started,
        )
        .await;
        assert_eq!(report.code, TestCode::AuthTimeout, "{:?}", report.detail);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "took {:?}",
            started.elapsed()
        );
    }

    // Partial success (a right password that is only one factor of two) is not
    // driven end to end here: russh 0.62's server resets `partial_success` to
    // false on every password/publickey/none rejection, so an in-process
    // server cannot send it. `auth_rejections_are_read_by_what_is_still_on_offer`
    // covers the verdict; the wire case is in the manual test plan.

    // ── Jump hosts ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_chain_through_a_jump_host_checks_both_and_closes_both() {
        let jump = spawn_server(Behaviour::default()).await;
        let target = spawn_server(Behaviour::default()).await;
        let hops = vec![
            jump.hop(password("hunter2"), Some(1)),
            target.hop(password("hunter2"), None),
        ];
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;

        assert_eq!(report.code, TestCode::Ok, "{:?}", report.detail);
        assert!(report.failed_hop.is_none());
        let labels: Vec<_> = report
            .host_keys
            .iter()
            .map(|key| key.label.clone())
            .collect();
        assert_eq!(labels, vec![jump.scope(), target.scope()]);
        assert_eq!(report.host_keys[1].fingerprint, target.fingerprint);

        assert_eq!(jump.journal.count("direct-tcpip"), 1);
        assert!(
            jump.journal.all_closed().await,
            "the jump host connection must be closed"
        );
        assert!(
            target.journal.all_closed().await,
            "the target connection must be closed"
        );
        assert_eq!(
            jump.journal.count("session-channel") + target.journal.count("session-channel"),
            0
        );
    }

    #[tokio::test]
    async fn a_failing_jump_host_is_named_and_the_target_is_never_contacted() {
        let jump = spawn_server(Behaviour::default()).await;
        let target = spawn_server(Behaviour::default()).await;
        let hops = vec![
            jump.hop(password("wrong"), Some(1)),
            target.hop(password("hunter2"), None),
        ];
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;

        assert_eq!(report.code, TestCode::AuthFailed, "{:?}", report.detail);
        assert_eq!(
            report.failed_hop,
            Some(FailedHop {
                index: 1,
                label: format!("ops@127.0.0.1:{}", jump.port)
            }),
        );
        assert_eq!(report.host_keys.len(), 1);
        assert!(jump.journal.all_closed().await);
        assert_eq!(target.journal.accepted.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_failing_target_behind_a_jump_host_is_not_blamed_on_the_jump() {
        let jump = spawn_server(Behaviour::default()).await;
        let target = spawn_server(Behaviour::default()).await;
        let hops = vec![
            jump.hop(password("hunter2"), Some(1)),
            target.hop(password("wrong"), None),
        ];
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;

        assert_eq!(report.code, TestCode::AuthFailed, "{:?}", report.detail);
        assert!(report.failed_hop.is_none());
        assert_eq!(report.host_keys.len(), 2);
        assert!(
            jump.journal.all_closed().await,
            "the jump host must be released after a target failure"
        );
        assert!(target.journal.all_closed().await);
    }

    #[tokio::test]
    async fn a_changed_key_on_the_jump_host_stops_the_chain_there() {
        let jump = spawn_server(Behaviour::default()).await;
        let target = spawn_server(Behaviour::default()).await;
        let trusted = trusted_map(&[(&jump.scope(), "SHA256:stale")]);
        let hops = vec![
            jump.hop(password("hunter2"), Some(1)),
            target.hop(password("hunter2"), None),
        ];
        let report = run_probe(&trusted, hops, quick_limits(), Instant::now()).await;

        assert_eq!(report.code, TestCode::HostKeyChanged, "{:?}", report.detail);
        assert_eq!(report.failed_hop.as_ref().map(|hop| hop.index), Some(1));
        assert!(jump.journal.all_closed().await);
        assert!(
            jump.journal.calls().is_empty(),
            "{:?}",
            jump.journal.calls()
        );
        assert_eq!(target.journal.accepted.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_jump_host_that_cannot_reach_the_target_reports_a_connect_failure() {
        let jump = spawn_server(Behaviour::default()).await;
        let closed_port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let hops = vec![
            jump.hop(password("hunter2"), Some(1)),
            Hop {
                host: "127.0.0.1".to_string(),
                port: closed_port,
                user: "ops".to_string(),
                credential: password("hunter2"),
                jump_index: None,
            },
        ];
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;
        assert_eq!(report.code, TestCode::ConnectFailed, "{:?}", report.detail);
        assert!(report.failed_hop.is_none());
        assert!(jump.journal.all_closed().await);
    }

    #[tokio::test]
    async fn a_failing_jump_host_lists_the_hops_it_kept_from_being_tested() {
        let jump = spawn_server(Behaviour::default()).await;
        let target = spawn_server(Behaviour::default()).await;
        let hops = vec![
            jump.hop(password("wrong"), Some(1)),
            target.hop(password("hunter2"), None),
        ];
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;

        assert_eq!(report.code, TestCode::AuthFailed, "{:?}", report.detail);
        assert_eq!(
            report.untested_hops,
            vec![format!("ops@127.0.0.1:{}", target.port)]
        );
        let json = serde_json::to_value(&report).expect("serialize");
        assert_eq!(
            json["untestedHops"][0],
            format!("ops@127.0.0.1:{}", target.port)
        );
    }

    #[tokio::test]
    async fn a_passing_chain_leaves_nothing_untested() {
        let jump = spawn_server(Behaviour::default()).await;
        let target = spawn_server(Behaviour::default()).await;
        let hops = vec![
            jump.hop(password("hunter2"), Some(1)),
            target.hop(password("hunter2"), None),
        ];
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;
        assert_eq!(report.code, TestCode::Ok, "{:?}", report.detail);
        assert!(report.untested_hops.is_empty());
    }

    #[tokio::test]
    async fn the_total_limit_names_the_jump_host_it_stopped_at() {
        // A jump host that accepts TCP and then says nothing.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let jump_port = listener.local_addr().expect("addr").port();
        let hold = tokio::spawn(async move {
            let accepted = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(accepted);
        });
        let hops = vec![
            Hop {
                host: "127.0.0.1".to_string(),
                port: jump_port,
                user: "jump".to_string(),
                credential: Credential::NoPassword,
                jump_index: Some(1),
            },
            Hop {
                host: "10.0.0.5".to_string(),
                port: 22,
                user: "ops".to_string(),
                credential: Credential::NoPassword,
                jump_index: None,
            },
        ];
        let limits = ProbeLimits {
            total: Duration::from_millis(400),
            ..quick_limits()
        };
        let report = run_probe(&HashMap::new(), hops, limits, Instant::now()).await;

        assert_eq!(report.code, TestCode::Timeout, "{:?}", report.detail);
        assert_eq!(
            report.failed_hop,
            Some(FailedHop {
                index: 1,
                label: format!("jump@127.0.0.1:{jump_port}")
            }),
        );
        assert_eq!(report.untested_hops, vec!["ops@10.0.0.5:22".to_string()]);
        hold.abort();
    }

    /// A TCP listener that accepts one client and never says a word.
    async fn spawn_silent_listener() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hold = tokio::spawn(async move {
            let accepted = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(15)).await;
            drop(accepted);
        });
        (port, hold)
    }

    fn silent_target(port: u16) -> Hop {
        Hop {
            host: "127.0.0.1".to_string(),
            port,
            user: "ops".to_string(),
            credential: password("hunter2"),
            jump_index: None,
        }
    }

    #[tokio::test]
    async fn a_silent_target_behind_a_jump_host_is_held_to_the_same_reach_budget() {
        // Channel open plus handshake share `limits.tcp` for hops reached
        // through a jump host too, not just for the first one.
        let jump = spawn_server(Behaviour::default()).await;
        let (target_port, hold) = spawn_silent_listener().await;
        let limits = ProbeLimits {
            tcp: Duration::from_millis(1500),
            handshake: Duration::from_secs(10),
            ..quick_limits()
        };
        let started = Instant::now();
        let hops = vec![
            jump.hop(password("hunter2"), Some(1)),
            silent_target(target_port),
        ];
        let report = run_probe(&HashMap::new(), hops, limits, started).await;

        assert_eq!(
            report.code,
            TestCode::HandshakeTimeout,
            "{:?}",
            report.detail
        );
        assert_eq!(
            report.detail,
            Some(format!("127.0.0.1:{target_port} did not finish connecting and the SSH handshake within 1500ms")),
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        // The target is not a jump host, and nothing comes after it.
        assert!(report.failed_hop.is_none());
        assert!(report.untested_hops.is_empty());
        assert_eq!(
            report.host_keys.len(),
            1,
            "only the jump host got as far as a host key"
        );
        assert_eq!(jump.journal.count("direct-tcpip"), 1);
        assert!(
            jump.journal.all_closed().await,
            "the jump host must be released"
        );
        hold.abort();
    }

    #[tokio::test]
    async fn the_total_limit_after_a_passing_jump_host_blames_the_target_and_closes_the_jump() {
        let jump = spawn_server(Behaviour::default()).await;
        let (target_port, hold) = spawn_silent_listener().await;
        let limits = ProbeLimits {
            total: Duration::from_secs(2),
            ..quick_limits()
        };
        let started = Instant::now();
        let hops = vec![
            jump.hop(password("hunter2"), Some(1)),
            silent_target(target_port),
        ];
        let report = run_probe(&HashMap::new(), hops, limits, started).await;

        assert_eq!(report.code, TestCode::Timeout, "{:?}", report.detail);
        assert_eq!(
            report.detail.as_deref(),
            Some("The test exceeded 2s and was stopped")
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        // The jump host passed, so the stop is pinned on the target.
        assert!(report.failed_hop.is_none(), "{:?}", report.failed_hop);
        assert!(
            report.untested_hops.is_empty(),
            "{:?}",
            report.untested_hops
        );
        assert_eq!(report.host_keys.len(), 1);
        assert_eq!(report.host_keys[0].label, jump.scope());
        assert!(
            jump.journal.all_closed().await,
            "a passed hop is still disconnected after the total limit"
        );
        assert_eq!(jump.journal.count("session-channel"), 0);
        hold.abort();
    }

    #[tokio::test]
    async fn a_jump_host_without_a_password_stops_the_chain_with_a_warning_there() {
        let jump = spawn_server(Behaviour::default()).await;
        let target = spawn_server(Behaviour::default()).await;
        let hops = vec![
            jump.hop(Credential::NoPassword, Some(1)),
            target.hop(password("hunter2"), None),
        ];
        let report = run_probe(&HashMap::new(), hops, quick_limits(), Instant::now()).await;

        assert_eq!(
            report.code,
            TestCode::PasswordMissing,
            "{:?}",
            report.detail
        );
        assert_eq!(report.status, TestStatus::Warning);
        assert_eq!(
            report.failed_hop,
            Some(FailedHop {
                index: 1,
                label: format!("ops@127.0.0.1:{}", jump.port)
            }),
        );
        assert_eq!(
            report.untested_hops,
            vec![format!("ops@127.0.0.1:{}", target.port)]
        );
        assert_eq!(report.host_keys.len(), 1);
        assert!(jump.journal.all_closed().await);
        assert!(
            jump.journal.calls().is_empty(),
            "{:?}",
            jump.journal.calls()
        );
        assert_eq!(
            target.journal.accepted.load(Ordering::SeqCst),
            0,
            "the target must not be contacted"
        );
    }

    #[tokio::test]
    async fn the_report_never_echoes_a_secret() {
        let server = spawn_server(Behaviour::default()).await;
        let report = run_probe(
            &HashMap::new(),
            vec![server.hop(password("correct-horse-battery"), None)],
            quick_limits(),
            Instant::now(),
        )
        .await;
        let json = serde_json::to_string(&report).expect("serialize");
        assert!(!json.contains("correct-horse-battery"), "{json}");
    }
}
