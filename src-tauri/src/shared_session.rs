//! Protocol-independent seam used by Cloud Console to bridge existing tabs.

use crate::pty_broker::{PtyBroker, LOCAL_OWNER};
use crate::terminal_event_hub::{SubscriptionToken, TerminalEvent, TerminalEventHub};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionProtocol {
    Serial,
    Ssh,
    Telnet,
    Local,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TxPolicy {
    ReadOnly,
    ReadWrite,
    Temporary,
}

#[derive(Clone, Debug)]
pub struct SessionPolicy {
    pub tx: TxPolicy,
    pub tx_expires_at: Option<std::time::SystemTime>,
    pub rx_ring_bytes: usize,
}

impl SessionPolicy {
    pub fn allows_tx(&self, now: std::time::SystemTime) -> bool {
        match self.tx {
            TxPolicy::ReadOnly => false,
            TxPolicy::ReadWrite => true,
            TxPolicy::Temporary => self.tx_expires_at.is_some_and(|expiry| expiry > now),
        }
    }
}

#[async_trait]
pub trait SharedSessionPort: Send + Sync {
    async fn contains(&self, protocol: SessionProtocol, session_id: &str) -> bool;
    fn subscribe_rx(
        &self,
        session_id: &str,
        sink: Box<dyn FnMut(&TerminalEvent) + Send>,
    ) -> SubscriptionToken;
    fn unsubscribe_rx(&self, subscription: &SubscriptionToken);
    async fn write_tx(
        &self,
        protocol: SessionProtocol,
        session_id: &str,
        bytes: &[u8],
    ) -> Result<(), String>;
}

pub struct UnifiedSharedSessionPort {
    hub: Arc<TerminalEventHub>,
    serial: crate::serial::SerialState,
    ssh: crate::ssh::SshState,
    telnet: crate::telnet::TelnetState,
    local: Arc<PtyBroker>,
}

impl UnifiedSharedSessionPort {
    pub fn new(
        hub: Arc<TerminalEventHub>,
        serial: crate::serial::SerialState,
        ssh: crate::ssh::SshState,
        telnet: crate::telnet::TelnetState,
        local: Arc<PtyBroker>,
    ) -> Self {
        Self {
            hub,
            serial,
            ssh,
            telnet,
            local,
        }
    }
}

#[async_trait]
impl SharedSessionPort for UnifiedSharedSessionPort {
    async fn contains(&self, protocol: SessionProtocol, session_id: &str) -> bool {
        match protocol {
            SessionProtocol::Serial => self.serial.contains(session_id).await,
            SessionProtocol::Ssh => self.ssh.contains(session_id).await,
            SessionProtocol::Telnet => self.telnet.contains(session_id).await,
            SessionProtocol::Local => self.local.contains(session_id),
        }
    }

    fn subscribe_rx(
        &self,
        session_id: &str,
        mut sink: Box<dyn FnMut(&TerminalEvent) + Send>,
    ) -> SubscriptionToken {
        self.hub.subscribe(session_id, move |event| sink(event))
    }

    fn unsubscribe_rx(&self, subscription: &SubscriptionToken) {
        self.hub.unsubscribe(subscription);
    }

    async fn write_tx(
        &self,
        protocol: SessionProtocol,
        session_id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        match protocol {
            SessionProtocol::Serial => self.serial.write_bytes(session_id, bytes).await,
            SessionProtocol::Ssh => self.ssh.write_bytes(session_id, bytes).await,
            SessionProtocol::Telnet => self.telnet.write_bytes(session_id, bytes).await,
            SessionProtocol::Local => self
                .local
                .input(session_id, LOCAL_OWNER, bytes, LOCAL_OWNER)
                .map(|_| ()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_policy_expires_fail_closed() {
        let now = std::time::SystemTime::now();
        let expired = SessionPolicy {
            tx: TxPolicy::Temporary,
            tx_expires_at: Some(now - std::time::Duration::from_secs(1)),
            rx_ring_bytes: 1024,
        };
        assert!(!expired.allows_tx(now));
        assert!(!SessionPolicy {
            tx: TxPolicy::Temporary,
            tx_expires_at: None,
            rx_ring_bytes: 1
        }
        .allows_tx(now));
    }
}
