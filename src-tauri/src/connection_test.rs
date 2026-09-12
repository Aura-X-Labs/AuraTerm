//! Shared vocabulary for the New Session dialog's "Test" button.
//!
//! Each protocol runs its own probe — `ssh::probe`, `telnet`, `serial` — and
//! they all answer with the [`ConnectionTestReport`] defined here. This module
//! also owns the one step every network probe starts with: a TCP connect that
//! says *why* it failed, so the dialog can tell "nothing is listening" from "a
//! firewall is dropping it" instead of echoing an OS string.
//!
//! Nothing in here touches Tauri state. A test must never leave a session, a
//! saved connection or a trusted host key behind, and the easiest way to keep
//! that true is for the probes to have no handle on any of them.

use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::net::{TcpStream, ToSocketAddrs};

use crate::serial_params::{SerialFlowControl, SerialParity};

/// How long name resolution plus the TCP handshake may take, per attempt.
pub const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TestStatus {
    Success,
    Warning,
    Failure,
}

/// What the probe found. The frontend maps each one to a sentence; `detail`
/// carries the raw backend message for anyone who wants the specifics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TestCode {
    Ok,
    // Any network transport.
    ResolveFailed,
    Refused,
    Timeout,
    Unreachable,
    ConnectFailed,
    PeerClosed,
    // SSH.
    HandshakeFailed,
    HandshakeTimeout,
    HostKeyChanged,
    AuthFailed,
    AuthPartial,
    AuthInteractiveRequired,
    AuthInteractiveUntested,
    AuthMethodUnsupported,
    AuthTimeout,
    PasswordMissing,
    KeyInvalid,
    AgentUnavailable,
    // Serial.
    SerialPortBusy,
    SerialPortNotFound,
    SerialPermissionDenied,
    SerialParamsRejected,
    SerialOpenFailed,
    SerialOpenTimeout,
    Rfc2217Refused,
}

impl TestCode {
    /// The only place a status is derived; the frontend takes it as given.
    ///
    /// Warnings are the outcomes where the connection itself is fine but the
    /// test could not finish the job — a second factor, a keyboard-interactive
    /// prompt, a server that will not take line settings.
    pub fn status(self) -> TestStatus {
        match self {
            Self::Ok => TestStatus::Success,
            Self::AuthPartial
            | Self::AuthInteractiveRequired
            | Self::AuthInteractiveUntested
            | Self::PasswordMissing
            | Self::Rfc2217Refused => TestStatus::Warning,
            _ => TestStatus::Failure,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostKeyStatus {
    /// Matches the fingerprint already in `ssh_known_hosts.json`.
    Trusted,
    /// Never seen before. A real connection would record it; a test does not.
    New,
    /// Differs from the trusted fingerprint.
    Changed,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyReport {
    /// The known-hosts scope, `host:port`.
    pub label: String,
    pub status: HostKeyStatus,
    /// The fingerprint the server presented this time.
    pub fingerprint: String,
    /// The trusted fingerprint, only when it differs.
    pub expected_fingerprint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedHop {
    /// 1-based position in the jump host list.
    pub index: usize,
    /// `user@host:port`.
    pub label: String,
}

/// Line settings an RFC 2217 server reported. Only the fields it explicitly
/// confirmed are filled: echoing back what we asked for would claim a check
/// that never happened.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSerialParams {
    pub baud_rate: Option<u32>,
    pub data_bits: Option<u8>,
    pub stop_bits: Option<u8>,
    pub parity: Option<SerialParity>,
    pub flow_control: Option<SerialFlowControl>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTestReport {
    pub status: TestStatus,
    pub code: TestCode,
    /// Raw backend message, in English. Never contains a secret.
    pub detail: Option<String>,
    pub elapsed_ms: u64,
    /// Set only when the failure happened at a jump host.
    pub failed_hop: Option<FailedHop>,
    /// SSH: the hops after the one the chain stopped at — never reached, so
    /// never tested — as `user@host:port`, in chain order (the target last).
    pub untested_hops: Vec<String>,
    /// SSH: one entry per hop, in the order they were checked.
    pub host_keys: Vec<HostKeyReport>,
    /// SSH: what the server said it would accept, when authentication stopped.
    pub auth_methods: Vec<String>,
    /// RFC 2217: the server's current line settings.
    pub server_params: Option<ServerSerialParams>,
    /// RFC 2217: the server's SIGNATURE, when it sent one.
    pub signature: Option<String>,
}

impl ConnectionTestReport {
    pub fn new(code: TestCode, started: Instant) -> Self {
        Self {
            status: code.status(),
            code,
            detail: None,
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            failed_hop: None,
            untested_hops: Vec::new(),
            host_keys: Vec::new(),
            auth_methods: Vec::new(),
            server_params: None,
            signature: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_failed_hop(mut self, hop: Option<FailedHop>) -> Self {
        self.failed_hop = hop;
        self
    }

    /// Turn a failed probe step straight into the report for it.
    pub fn from_failure(failure: ProbeFailure, started: Instant) -> Self {
        Self::new(failure.code, started).with_detail(failure.detail)
    }
}

/// One probe step that did not work out: what it means, and the raw reason.
#[derive(Debug)]
pub struct ProbeFailure {
    pub code: TestCode,
    pub detail: String,
}

impl ProbeFailure {
    pub fn new(code: TestCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// `10s`, or `300ms` for the sub-second limits the tests use.
pub fn describe_duration(duration: Duration) -> String {
    if duration.subsec_millis() == 0 && duration.as_secs() > 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

/// Resolve `addr` and connect to it, all within one deadline.
///
/// `addr` is passed through in exactly the form the real connection path uses
/// — `"{host}:{port}"` for SSH, `(host, port)` for Telnet and network serial —
/// so the test resolves the same name the session would. Addresses are tried
/// in order; when all of them fail, the last error decides the code.
pub async fn tcp_connect<A: ToSocketAddrs>(
    addr: A,
    display: &str,
    timeout: Duration,
) -> Result<TcpStream, ProbeFailure> {
    let attempt = async {
        let addresses = tokio::net::lookup_host(addr).await.map_err(|error| {
            ProbeFailure::new(
                TestCode::ResolveFailed,
                format!("Cannot resolve \"{display}\": {error}"),
            )
        })?;

        let mut last_error = None;
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = Some(error),
            }
        }

        Err(match last_error {
            Some(error) => {
                ProbeFailure::new(classify_io_error(&error), format!("{display}: {error}"))
            }
            None => ProbeFailure::new(
                TestCode::ResolveFailed,
                format!("\"{display}\" resolved to no addresses"),
            ),
        })
    };

    match tokio::time::timeout(timeout, attempt).await {
        Ok(result) => result,
        Err(_) => Err(ProbeFailure::new(
            TestCode::Timeout,
            format!(
                "No answer from {display} within {}",
                describe_duration(timeout)
            ),
        )),
    }
}

/// Map a connect error onto the handful of outcomes that call for different
/// next steps. Everything else is a generic "could not connect".
pub fn classify_io_error(error: &std::io::Error) -> TestCode {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => TestCode::Refused,
        ErrorKind::TimedOut => TestCode::Timeout,
        ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => TestCode::Unreachable,
        _ => TestCode::ConnectFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    /// Every variant with the camelCase name the frontend's
    /// `CONNECTION_TEST_CODES` expects (`connectionTest.test.ts` checks the
    /// other direction by reading this file's enum).
    fn all_codes() -> Vec<(TestCode, &'static str)> {
        let codes = vec![
            (TestCode::Ok, "ok"),
            (TestCode::ResolveFailed, "resolveFailed"),
            (TestCode::Refused, "refused"),
            (TestCode::Timeout, "timeout"),
            (TestCode::Unreachable, "unreachable"),
            (TestCode::ConnectFailed, "connectFailed"),
            (TestCode::PeerClosed, "peerClosed"),
            (TestCode::HandshakeFailed, "handshakeFailed"),
            (TestCode::HandshakeTimeout, "handshakeTimeout"),
            (TestCode::HostKeyChanged, "hostKeyChanged"),
            (TestCode::AuthFailed, "authFailed"),
            (TestCode::AuthPartial, "authPartial"),
            (TestCode::AuthInteractiveRequired, "authInteractiveRequired"),
            (TestCode::AuthInteractiveUntested, "authInteractiveUntested"),
            (TestCode::AuthMethodUnsupported, "authMethodUnsupported"),
            (TestCode::AuthTimeout, "authTimeout"),
            (TestCode::PasswordMissing, "passwordMissing"),
            (TestCode::KeyInvalid, "keyInvalid"),
            (TestCode::AgentUnavailable, "agentUnavailable"),
            (TestCode::SerialPortBusy, "serialPortBusy"),
            (TestCode::SerialPortNotFound, "serialPortNotFound"),
            (TestCode::SerialPermissionDenied, "serialPermissionDenied"),
            (TestCode::SerialParamsRejected, "serialParamsRejected"),
            (TestCode::SerialOpenFailed, "serialOpenFailed"),
            (TestCode::SerialOpenTimeout, "serialOpenTimeout"),
            (TestCode::Rfc2217Refused, "rfc2217Refused"),
        ];
        // Exhaustive on purpose: a new variant fails to compile here until it
        // is added to the table above.
        for (code, _) in &codes {
            match code {
                TestCode::Ok
                | TestCode::ResolveFailed
                | TestCode::Refused
                | TestCode::Timeout
                | TestCode::Unreachable
                | TestCode::ConnectFailed
                | TestCode::PeerClosed
                | TestCode::HandshakeFailed
                | TestCode::HandshakeTimeout
                | TestCode::HostKeyChanged
                | TestCode::AuthFailed
                | TestCode::AuthPartial
                | TestCode::AuthInteractiveRequired
                | TestCode::AuthInteractiveUntested
                | TestCode::AuthMethodUnsupported
                | TestCode::AuthTimeout
                | TestCode::PasswordMissing
                | TestCode::KeyInvalid
                | TestCode::AgentUnavailable
                | TestCode::SerialPortBusy
                | TestCode::SerialPortNotFound
                | TestCode::SerialPermissionDenied
                | TestCode::SerialParamsRejected
                | TestCode::SerialOpenFailed
                | TestCode::SerialOpenTimeout
                | TestCode::Rfc2217Refused => {}
            }
        }
        codes
    }

    #[test]
    fn codes_serialize_to_the_names_the_frontend_knows() {
        for (code, name) in all_codes() {
            assert_eq!(
                serde_json::to_value(code).expect("serialize"),
                serde_json::Value::String(name.to_string()),
                "{code:?}",
            );
        }
    }

    #[test]
    fn only_ok_is_a_success_and_only_the_unfinished_outcomes_are_warnings() {
        let warnings = [
            TestCode::AuthPartial,
            TestCode::AuthInteractiveRequired,
            TestCode::AuthInteractiveUntested,
            TestCode::PasswordMissing,
            TestCode::Rfc2217Refused,
        ];
        for (code, _) in all_codes() {
            let expected = if code == TestCode::Ok {
                TestStatus::Success
            } else if warnings.contains(&code) {
                TestStatus::Warning
            } else {
                TestStatus::Failure
            };
            assert_eq!(code.status(), expected, "{code:?}");
        }
    }

    #[test]
    fn the_report_takes_its_status_from_the_code() {
        let started = Instant::now();
        assert_eq!(
            ConnectionTestReport::new(TestCode::Ok, started).status,
            TestStatus::Success
        );
        assert_eq!(
            ConnectionTestReport::new(TestCode::PasswordMissing, started).status,
            TestStatus::Warning,
        );
        let failed = ConnectionTestReport::from_failure(
            ProbeFailure::new(TestCode::Refused, "nobody home"),
            started,
        );
        assert_eq!(failed.status, TestStatus::Failure);
        assert_eq!(failed.code, TestCode::Refused);
        assert_eq!(failed.detail.as_deref(), Some("nobody home"));
        assert!(failed.host_keys.is_empty());
        assert!(failed.auth_methods.is_empty());
        assert!(failed.failed_hop.is_none());
        assert!(failed.server_params.is_none());
        assert!(failed.signature.is_none());
    }

    #[test]
    fn the_report_serializes_in_camel_case() {
        let mut report = ConnectionTestReport::new(TestCode::HostKeyChanged, Instant::now())
            .with_detail("key changed")
            .with_failed_hop(Some(FailedHop {
                index: 2,
                label: "jump@inner:22".into(),
            }));
        report.host_keys.push(HostKeyReport {
            label: "inner:22".into(),
            status: HostKeyStatus::Changed,
            fingerprint: "SHA256:new".into(),
            expected_fingerprint: Some("SHA256:old".into()),
        });
        report.auth_methods.push("publickey".into());
        report.server_params = Some(ServerSerialParams {
            baud_rate: Some(9600),
            parity: Some(SerialParity::Even),
            flow_control: Some(SerialFlowControl::Hardware),
            ..ServerSerialParams::default()
        });
        report.signature = Some("ser2net".into());

        let json = serde_json::to_value(&report).expect("serialize");
        assert_eq!(json["status"], "failure");
        assert_eq!(json["code"], "hostKeyChanged");
        assert_eq!(json["detail"], "key changed");
        assert!(json["elapsedMs"].is_u64());
        assert_eq!(json["failedHop"]["index"], 2);
        assert_eq!(json["failedHop"]["label"], "jump@inner:22");
        assert_eq!(json["hostKeys"][0]["label"], "inner:22");
        assert_eq!(json["hostKeys"][0]["status"], "changed");
        assert_eq!(json["hostKeys"][0]["fingerprint"], "SHA256:new");
        assert_eq!(json["hostKeys"][0]["expectedFingerprint"], "SHA256:old");
        assert_eq!(json["authMethods"][0], "publickey");
        assert_eq!(json["serverParams"]["baudRate"], 9600);
        assert_eq!(json["serverParams"]["parity"], "even");
        assert_eq!(json["serverParams"]["flowControl"], "hardware");
        assert!(json["serverParams"]["dataBits"].is_null());
        assert_eq!(json["signature"], "ser2net");
        // No snake_case leaks through to the frontend.
        let text = json.to_string();
        for snake in [
            "elapsed_ms",
            "failed_hop",
            "host_keys",
            "auth_methods",
            "server_params",
            "expected_fingerprint",
            "baud_rate",
            "flow_control",
        ] {
            assert!(!text.contains(snake), "{snake} in {text}");
        }
    }

    #[test]
    fn a_warning_report_serializes_as_warning() {
        let json = serde_json::to_value(ConnectionTestReport::new(
            TestCode::AuthPartial,
            Instant::now(),
        ))
        .expect("serialize");
        assert_eq!(json["status"], "warning");
        assert_eq!(json["code"], "authPartial");
        assert!(json["detail"].is_null());
        assert!(json["failedHop"].is_null());
        assert_eq!(json["hostKeys"], serde_json::json!([]));
        assert_eq!(json["authMethods"], serde_json::json!([]));
    }

    #[test]
    fn untested_hops_serialize_in_camel_case() {
        let mut report = ConnectionTestReport::new(TestCode::AuthFailed, Instant::now());
        assert_eq!(
            serde_json::to_value(&report).expect("serialize")["untestedHops"],
            serde_json::json!([])
        );
        report.untested_hops.push("ops@10.0.0.5:22".into());
        let json = serde_json::to_value(&report).expect("serialize");
        assert_eq!(json["untestedHops"], serde_json::json!(["ops@10.0.0.5:22"]));
        assert!(!json.to_string().contains("untested_hops"));
    }

    #[test]
    fn host_key_statuses_serialize_in_lower_case() {
        for (status, name) in [
            (HostKeyStatus::Trusted, "trusted"),
            (HostKeyStatus::New, "new"),
            (HostKeyStatus::Changed, "changed"),
        ] {
            assert_eq!(serde_json::to_value(status).expect("serialize"), name);
        }
    }

    #[test]
    fn io_errors_map_to_the_outcomes_that_need_different_next_steps() {
        let cases = [
            (ErrorKind::ConnectionRefused, TestCode::Refused),
            (ErrorKind::TimedOut, TestCode::Timeout),
            (ErrorKind::HostUnreachable, TestCode::Unreachable),
            (ErrorKind::NetworkUnreachable, TestCode::Unreachable),
            (ErrorKind::ConnectionReset, TestCode::ConnectFailed),
            (ErrorKind::PermissionDenied, TestCode::ConnectFailed),
            (ErrorKind::AddrNotAvailable, TestCode::ConnectFailed),
            (ErrorKind::Other, TestCode::ConnectFailed),
        ];
        for (kind, expected) in cases {
            assert_eq!(classify_io_error(&Error::from(kind)), expected, "{kind:?}");
        }
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(describe_duration(Duration::from_secs(10)), "10s");
        assert_eq!(describe_duration(Duration::from_millis(300)), "300ms");
        assert_eq!(describe_duration(Duration::from_millis(1500)), "1500ms");
        assert_eq!(describe_duration(Duration::ZERO), "0ms");
    }

    #[tokio::test]
    async fn connects_to_a_listening_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let accept = tokio::spawn(async move { listener.accept().await.map(|_| ()) });

        let stream = tcp_connect(("127.0.0.1", port), "local", Duration::from_secs(5))
            .await
            .expect("connect");
        assert_eq!(stream.peer_addr().expect("peer").port(), port);
        assert!(accept.await.expect("join").is_ok());
    }

    #[tokio::test]
    async fn accepts_the_host_colon_port_form_ssh_uses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let endpoint = format!("127.0.0.1:{port}");
        assert!(
            tcp_connect(endpoint.clone(), &endpoint, Duration::from_secs(5))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn a_closed_port_is_refused() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let endpoint = format!("127.0.0.1:{port}");
        let started = Instant::now();
        let failure = tcp_connect(("127.0.0.1", port), &endpoint, Duration::from_secs(8))
            .await
            .expect_err("nothing listens there");
        assert_eq!(failure.code, TestCode::Refused, "{}", failure.detail);
        assert!(failure.detail.starts_with(&endpoint), "{}", failure.detail);
        // Windows retries a refused loopback connect for about two seconds.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn an_unresolvable_name_fails_to_resolve() {
        // `.invalid` is reserved (RFC 6761) and never resolves.
        let failure = tcp_connect(
            ("auraterm-test.invalid", 22),
            "auraterm-test.invalid:22",
            Duration::from_secs(8),
        )
        .await
        .expect_err("must not resolve");
        assert_eq!(failure.code, TestCode::ResolveFailed, "{}", failure.detail);
        assert!(
            failure.detail.contains("auraterm-test.invalid"),
            "{}",
            failure.detail
        );
    }

    #[tokio::test]
    async fn gives_up_at_the_deadline_instead_of_hanging() {
        // A non-routable address: either the SYN goes nowhere (timeout) or the
        // stack says so at once (unreachable). What matters is the bound.
        let started = Instant::now();
        let failure = tcp_connect(
            ("10.255.255.1", 9),
            "10.255.255.1:9",
            Duration::from_millis(300),
        )
        .await
        .expect_err("must not connect");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "took {:?}",
            started.elapsed()
        );
        assert!(
            matches!(
                failure.code,
                TestCode::Timeout | TestCode::Unreachable | TestCode::ConnectFailed
            ),
            "{:?}: {}",
            failure.code,
            failure.detail,
        );
        if failure.code == TestCode::Timeout {
            assert_eq!(failure.detail, "No answer from 10.255.255.1:9 within 300ms");
        }
    }
}
