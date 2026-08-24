//! Vocabulary shared by every serial transport: line parameters, modem/line
//! status, and the transport discriminator.
//!
//! This lives in its own module so the RFC 2217 codec (`rfc2217.rs`) and the
//! transport implementations (`serial_link.rs`) can both depend on it without
//! depending on each other.

use serde::{Deserialize, Serialize};
use serialport::{DataBits, FlowControl, Parity, StopBits};

/// How a serial session reaches the UART.
///
/// `Local` is a port on this machine; the other two go over TCP to a device
/// server. `Rfc2217` negotiates the Telnet Com Port Control Option so line
/// parameters actually take effect; `RawTcp` is a bare byte pipe with no
/// framing at all (and therefore no parameter control).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SerialTransport {
    #[default]
    Local,
    Rfc2217,
    RawTcp,
}

impl SerialTransport {
    /// Parse the frontend's discriminator. `None` means a pre-0.3.4 caller that
    /// only ever meant a local port.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("local") {
            "local" => Ok(Self::Local),
            "rfc2217" => Ok(Self::Rfc2217),
            "raw-tcp" => Ok(Self::RawTcp),
            other => Err(format!("Unsupported serial transport: {other}")),
        }
    }

    pub fn is_network(self) -> bool {
        !matches!(self, Self::Local)
    }
}

/// Which direction's buffered bytes to discard.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PurgeTarget {
    /// Bytes the device sent that we have not read yet.
    Input,
    /// Bytes queued for the device that have not gone out yet.
    Output,
    #[default]
    Both,
}

impl PurgeTarget {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "input" => Ok(Self::Input),
            "output" => Ok(Self::Output),
            "both" => Ok(Self::Both),
            other => Err(format!("Unsupported purge target: {other}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialParity {
    None,
    Odd,
    Even,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialFlowControl {
    None,
    Hardware,
    Software,
}

/// The five line parameters, in the same shape the frontend sends them.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialParams {
    pub baud_rate: u32,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub parity: SerialParity,
    pub flow_control: SerialFlowControl,
}

impl Default for SerialParams {
    fn default() -> Self {
        Self {
            baud_rate: 9600,
            data_bits: 8,
            stop_bits: 1,
            parity: SerialParity::None,
            flow_control: SerialFlowControl::None,
        }
    }
}

impl SerialParams {
    /// Build from the raw values a Tauri command receives, validating each.
    pub fn from_wire(
        baud_rate: u32,
        data_bits: u8,
        stop_bits: u8,
        parity: &str,
        flow_control: &str,
    ) -> Result<Self, String> {
        if baud_rate == 0 {
            return Err("Baud rate must be greater than zero".to_string());
        }
        if !(5..=8).contains(&data_bits) {
            return Err(format!("Unsupported data bits: {data_bits}"));
        }
        if !(1..=2).contains(&stop_bits) {
            return Err(format!("Unsupported stop bits: {stop_bits}"));
        }
        Ok(Self {
            baud_rate,
            data_bits,
            stop_bits,
            parity: match parity {
                "none" => SerialParity::None,
                "odd" => SerialParity::Odd,
                "even" => SerialParity::Even,
                other => return Err(format!("Unsupported parity: {other}")),
            },
            flow_control: match flow_control {
                "none" => SerialFlowControl::None,
                "hardware" => SerialFlowControl::Hardware,
                "software" => SerialFlowControl::Software,
                other => return Err(format!("Unsupported flow control: {other}")),
            },
        })
    }

    pub fn data_bits_enum(&self) -> Result<DataBits, String> {
        match self.data_bits {
            5 => Ok(DataBits::Five),
            6 => Ok(DataBits::Six),
            7 => Ok(DataBits::Seven),
            8 => Ok(DataBits::Eight),
            other => Err(format!("Unsupported data bits: {other}")),
        }
    }

    pub fn stop_bits_enum(&self) -> Result<StopBits, String> {
        match self.stop_bits {
            1 => Ok(StopBits::One),
            2 => Ok(StopBits::Two),
            other => Err(format!("Unsupported stop bits: {other}")),
        }
    }

    pub fn parity_enum(&self) -> Parity {
        match self.parity {
            SerialParity::None => Parity::None,
            SerialParity::Odd => Parity::Odd,
            SerialParity::Even => Parity::Even,
        }
    }

    pub fn flow_control_enum(&self) -> FlowControl {
        match self.flow_control {
            SerialFlowControl::None => FlowControl::None,
            SerialFlowControl::Hardware => FlowControl::Hardware,
            SerialFlowControl::Software => FlowControl::Software,
        }
    }
}

/// Which line parameters the peer has explicitly confirmed.
///
/// Without this the UI cannot tell "the server agreed to 115200" from "nobody
/// ever answered, so we are still showing what you asked for".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialParamsConfirmed {
    pub baud_rate: bool,
    pub data_bits: bool,
    pub stop_bits: bool,
    pub parity: bool,
    pub flow_control: bool,
}

/// Modem status register bits (CTS/DSR/CD/RI), as reported by the peer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModemLines {
    pub cts: bool,
    pub dsr: bool,
    pub cd: bool,
    pub ri: bool,
}

/// The two output control lines, as last driven by this session.
///
/// They are outputs, so a local UART cannot read them back — what is recorded
/// here is what we set. RFC 2217 servers do acknowledge them, and those
/// acknowledgements overwrite this. Both start asserted, which is what a port
/// opens with and what the RFC 2217 parameter block requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialSignals {
    pub dtr: bool,
    pub rts: bool,
}

impl Default for SerialSignals {
    fn default() -> Self {
        Self { dtr: true, rts: true }
    }
}

/// Line status register error bits. A framing or parity error is usually the
/// most direct evidence that the baud rate is wrong.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineErrors {
    pub break_detected: bool,
    pub framing: bool,
    pub parity: bool,
    pub overrun: bool,
}

/// Snapshot pushed to the frontend as `serial-status:<id>`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialStatus {
    pub id: String,
    pub transport: SerialTransport,
    /// True once the peer answered `IAC DO COM-PORT-OPTION`.
    pub rfc2217_negotiated: bool,
    /// True once the handshake has a verdict — agreed, refused, or timed out.
    ///
    /// Separate from `rfc2217_negotiated` because "not agreed yet" and "will
    /// never agree" call for very different words, and a reconnect passes
    /// through the first on its way to the second.
    pub negotiation_settled: bool,
    /// True once Telnet BINARY is in effect in both directions. Without it the
    /// stream is 7-bit NVT and CR has to be stuffed, which breaks ZMODEM.
    pub binary_negotiated: bool,
    pub requested: SerialParams,
    pub effective: SerialParams,
    pub confirmed: SerialParamsConfirmed,
    pub modem: ModemLines,
    pub signals: SerialSignals,
    pub line_errors: LineErrors,
    /// Whether this session can retune the line, send BREAK and drive DTR/RTS.
    /// False for raw TCP, and for RFC 2217 until the option is agreed.
    pub controllable: bool,
    /// Peer's RFC 2217 SIGNATURE string, when it sent one.
    pub signature: Option<String>,
}

impl SerialStatus {
    pub fn local(id: &str, params: SerialParams) -> Self {
        Self {
            id: id.to_string(),
            transport: SerialTransport::Local,
            rfc2217_negotiated: false,
            negotiation_settled: true,
            binary_negotiated: true,
            requested: params,
            effective: params,
            // A local port either opened with these settings or failed to open,
            // so every parameter is confirmed by construction.
            confirmed: SerialParamsConfirmed {
                baud_rate: true,
                data_bits: true,
                stop_bits: true,
                parity: true,
                flow_control: true,
            },
            modem: ModemLines::default(),
            signals: SerialSignals::default(),
            line_errors: LineErrors::default(),
            controllable: true,
            signature: None,
        }
    }
}
