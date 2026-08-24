//! RFC 2217 — Telnet Com Port Control Option (option 44).
//!
//! This module is the *codec* only: it turns line parameters into
//! subnegotiation frames and turns the peer's replies back into values. It
//! performs no IO and knows nothing about sockets, so the whole negotiation can
//! be exercised in unit tests.
//!
//! The framing layer (IAC parsing, WILL/DO bookkeeping) stays in
//! [`crate::util::TelnetIacFilter`], which hands complete subnegotiation
//! payloads to [`ComPortState`]. The transport that owns a socket lives in
//! [`crate::serial_link`].

use crate::serial_params::{
    LineErrors, ModemLines, PurgeTarget, SerialFlowControl, SerialParams, SerialParamsConfirmed,
    SerialParity, SerialSignals,
};
use crate::util::{IAC, SB, SE};

/// Telnet option code for COM-PORT-OPTION.
pub const COM_PORT_OPTION: u8 = 44;

// Client -> access server commands.
pub const SIGNATURE: u8 = 0;
pub const SET_BAUDRATE: u8 = 1;
pub const SET_DATASIZE: u8 = 2;
pub const SET_PARITY: u8 = 3;
pub const SET_STOPSIZE: u8 = 4;
pub const SET_CONTROL: u8 = 5;
pub const NOTIFY_LINESTATE: u8 = 6;
pub const NOTIFY_MODEMSTATE: u8 = 7;
pub const SET_LINESTATE_MASK: u8 = 10;
pub const SET_MODEMSTATE_MASK: u8 = 11;
pub const PURGE_DATA: u8 = 12;

/// Access server -> client replies reuse the client command numbers plus 100.
const SERVER_OFFSET: u8 = 100;

// SET-CONTROL values (one byte that means five different things).
const CONTROL_REQUEST_OUTBOUND_FLOW: u8 = 0;
const CONTROL_FLOW_NONE: u8 = 1;
const CONTROL_FLOW_XON_XOFF: u8 = 2;
const CONTROL_FLOW_HARDWARE: u8 = 3;
const CONTROL_BREAK_ON: u8 = 5;
const CONTROL_BREAK_OFF: u8 = 6;
const CONTROL_DTR_ON: u8 = 8;
const CONTROL_DTR_OFF: u8 = 9;
const CONTROL_RTS_ON: u8 = 11;
const CONTROL_RTS_OFF: u8 = 12;

// PURGE-DATA targets.
const PURGE_RECEIVE: u8 = 1;
const PURGE_TRANSMIT: u8 = 2;
const PURGE_BOTH: u8 = 3;

/// LINESTATE bits worth subscribing to: the four that say something went wrong
/// on the wire. The three "register empty" bits fire constantly and carry no
/// information a terminal user can act on.
const LINESTATE_SUBSCRIPTION: u8 =
    LINESTATE_BREAK | LINESTATE_FRAMING | LINESTATE_PARITY | LINESTATE_OVERRUN;

/// MODEMSTATE bits worth subscribing to: the four level signals. The four delta
/// bits are derivable from consecutive levels.
const MODEMSTATE_SUBSCRIPTION: u8 =
    MODEMSTATE_CD | MODEMSTATE_RI | MODEMSTATE_DSR | MODEMSTATE_CTS;

// LINESTATE (UART line status register) bits worth surfacing.
const LINESTATE_BREAK: u8 = 16;
const LINESTATE_FRAMING: u8 = 8;
const LINESTATE_PARITY: u8 = 4;
const LINESTATE_OVERRUN: u8 = 2;

// MODEMSTATE (UART modem status register) bits.
const MODEMSTATE_CD: u8 = 128;
const MODEMSTATE_RI: u8 = 64;
const MODEMSTATE_DSR: u8 = 32;
const MODEMSTATE_CTS: u8 = 16;

/// Frame one COM-PORT-OPTION subnegotiation: `IAC SB 44 <command> <value> IAC SE`.
///
/// Every `0xFF` inside the value must be doubled, otherwise the peer reads it
/// as an `IAC` and the rest of the frame is parsed as commands. This is the
/// single easiest thing to get wrong in RFC 2217 — it is invisible at 115200
/// 8N1 (no baud rate encodes a `0xFF`) and then silently corrupts ZMODEM.
pub fn subnegotiation(command: u8, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 7);
    out.extend_from_slice(&[IAC, SB, COM_PORT_OPTION, command]);
    for &byte in value {
        out.push(byte);
        if byte == IAC {
            out.push(IAC);
        }
    }
    out.extend_from_slice(&[IAC, SE]);
    out
}

fn parity_code(parity: SerialParity) -> u8 {
    match parity {
        SerialParity::None => 1,
        SerialParity::Odd => 2,
        SerialParity::Even => 3,
    }
}

fn flow_control_code(flow_control: SerialFlowControl) -> u8 {
    match flow_control {
        SerialFlowControl::None => CONTROL_FLOW_NONE,
        SerialFlowControl::Software => CONTROL_FLOW_XON_XOFF,
        SerialFlowControl::Hardware => CONTROL_FLOW_HARDWARE,
    }
}

/// Negotiation state for one RFC 2217 session.
///
/// Tracks what we asked for, what the peer confirmed, and the line/modem status
/// it reported. `requested` and `effective` are deliberately separate: a device
/// server is free to clamp 115200 down to 9600, and showing the requested value
/// as if it had taken effect is how an afternoon disappears.
pub struct ComPortState {
    requested: SerialParams,
    effective: SerialParams,
    confirmed: SerialParamsConfirmed,
    adopt_server_params: bool,
    negotiated: bool,
    refused: bool,
    modem: ModemLines,
    signals: SerialSignals,
    line_errors: LineErrors,
    signature: Option<String>,
    dirty: bool,
}

impl ComPortState {
    pub fn new(requested: SerialParams, adopt_server_params: bool) -> Self {
        Self {
            requested,
            effective: requested,
            confirmed: SerialParamsConfirmed::default(),
            adopt_server_params,
            negotiated: false,
            refused: false,
            modem: ModemLines::default(),
            signals: SerialSignals::default(),
            line_errors: LineErrors::default(),
            signature: None,
            dirty: false,
        }
    }

    pub fn negotiated(&self) -> bool {
        self.negotiated
    }

    /// Whether the handshake has reached a verdict either way.
    pub fn settled(&self) -> bool {
        self.negotiated || self.refused
    }

    /// Stop waiting for an answer that is not coming.
    ///
    /// A peer that simply ignores `WILL COM-PORT-OPTION` — a raw TCP port, most
    /// often — never sends `DONT`, so silence has to be turned into a verdict
    /// on a timer or the UI can never say anything definite.
    pub fn give_up(&mut self) {
        if !self.negotiated && !self.refused {
            self.refused = true;
            self.dirty = true;
        }
    }

    pub fn requested(&self) -> SerialParams {
        self.requested
    }

    pub fn effective(&self) -> SerialParams {
        self.effective
    }

    pub fn confirmed(&self) -> SerialParamsConfirmed {
        self.confirmed
    }

    pub fn modem(&self) -> ModemLines {
        self.modem
    }

    pub fn signals(&self) -> SerialSignals {
        self.signals
    }

    pub fn line_errors(&self) -> LineErrors {
        self.line_errors
    }

    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    /// Consume the "something changed" flag so the reader loop only emits a
    /// status event when there is news.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// The peer accepted our `WILL COM-PORT-OPTION`; push the parameter block.
    pub fn on_do(&mut self) -> Vec<u8> {
        if self.negotiated {
            return Vec::new();
        }
        self.negotiated = true;
        self.refused = false;
        self.dirty = true;
        self.parameter_block()
    }

    /// The peer refused COM-PORT-OPTION. The session stays usable as a plain
    /// byte pipe; the caller surfaces that the parameters never took effect.
    pub fn on_dont(&mut self) {
        if !self.negotiated {
            self.refused = true;
            self.dirty = true;
        }
    }

    fn parameter_block(&self) -> Vec<u8> {
        let mut out = Vec::new();

        if self.adopt_server_params {
            // Value 0 means "report what you currently have" for every setter,
            // so we learn the server's configuration without clobbering it.
            // Shared console servers need this: otherwise the first person to
            // connect silently retunes the port for everyone already on it.
            out.extend_from_slice(&subnegotiation(SET_BAUDRATE, &0u32.to_be_bytes()));
            out.extend_from_slice(&subnegotiation(SET_DATASIZE, &[0]));
            out.extend_from_slice(&subnegotiation(SET_PARITY, &[0]));
            out.extend_from_slice(&subnegotiation(SET_STOPSIZE, &[0]));
            out.extend_from_slice(&subnegotiation(
                SET_CONTROL,
                &[CONTROL_REQUEST_OUTBOUND_FLOW],
            ));
        } else {
            out.extend_from_slice(&subnegotiation(
                SET_BAUDRATE,
                &self.requested.baud_rate.to_be_bytes(),
            ));
            out.extend_from_slice(&subnegotiation(SET_DATASIZE, &[self.requested.data_bits]));
            out.extend_from_slice(&subnegotiation(
                SET_PARITY,
                &[parity_code(self.requested.parity)],
            ));
            out.extend_from_slice(&subnegotiation(SET_STOPSIZE, &[self.requested.stop_bits]));
            out.extend_from_slice(&subnegotiation(
                SET_CONTROL,
                &[flow_control_code(self.requested.flow_control)],
            ));
            // Assert DTR so adapters that gate the line on it come alive. RTS is
            // left alone under hardware flow control, where the UART owns it.
            out.extend_from_slice(&subnegotiation(SET_CONTROL, &[CONTROL_DTR_ON]));
            if self.requested.flow_control != SerialFlowControl::Hardware {
                out.extend_from_slice(&subnegotiation(SET_CONTROL, &[CONTROL_RTS_ON]));
            }
        }

        // Subscribe to the bits a person can act on. Some firmware pushes
        // MODEMSTATE dozens of times a second, which is why the reader loop
        // coalesces status events before they reach the frontend.
        out.extend_from_slice(&subnegotiation(SET_LINESTATE_MASK, &[LINESTATE_SUBSCRIPTION]));
        out.extend_from_slice(&subnegotiation(SET_MODEMSTATE_MASK, &[MODEMSTATE_SUBSCRIPTION]));
        // Drop whatever the device server buffered before we showed up.
        out.extend_from_slice(&subnegotiation(PURGE_DATA, &[PURGE_BOTH]));
        out
    }

    fn confirm_flow(&mut self, flow: SerialFlowControl) {
        self.effective.flow_control = flow;
        self.confirmed.flow_control = true;
        self.dirty = true;
    }

    /// Record a DTR (`rts = None`) or RTS (`rts = Some(())`) acknowledgement.
    fn set_signal(&mut self, on: bool, rts: Option<()>) {
        let slot = if rts.is_some() {
            &mut self.signals.rts
        } else {
            &mut self.signals.dtr
        };
        if *slot != on {
            *slot = on;
            self.dirty = true;
        }
    }

    /// Retune the line on a live session.
    ///
    /// Only the parameters that actually changed are sent, and each one's
    /// confirmation is cleared so the UI shows "asked, not yet acknowledged"
    /// until the server answers.
    pub fn retune(&mut self, params: SerialParams) -> Vec<u8> {
        let previous = self.requested;
        self.requested = params;
        let mut out = Vec::new();

        if params.baud_rate != previous.baud_rate || !self.confirmed.baud_rate {
            self.confirmed.baud_rate = false;
            out.extend_from_slice(&subnegotiation(SET_BAUDRATE, &params.baud_rate.to_be_bytes()));
        }
        if params.data_bits != previous.data_bits || !self.confirmed.data_bits {
            self.confirmed.data_bits = false;
            out.extend_from_slice(&subnegotiation(SET_DATASIZE, &[params.data_bits]));
        }
        if params.parity != previous.parity || !self.confirmed.parity {
            self.confirmed.parity = false;
            out.extend_from_slice(&subnegotiation(SET_PARITY, &[parity_code(params.parity)]));
        }
        if params.stop_bits != previous.stop_bits || !self.confirmed.stop_bits {
            self.confirmed.stop_bits = false;
            out.extend_from_slice(&subnegotiation(SET_STOPSIZE, &[params.stop_bits]));
        }
        if params.flow_control != previous.flow_control || !self.confirmed.flow_control {
            self.confirmed.flow_control = false;
            out.extend_from_slice(&subnegotiation(
                SET_CONTROL,
                &[flow_control_code(params.flow_control)],
            ));
        }

        if !out.is_empty() {
            self.dirty = true;
        }
        out
    }

    /// Assert or release BREAK. Callers hold it asserted for a while, then
    /// release — the duration is the client's to time, not the server's.
    pub fn break_frame(assert: bool) -> Vec<u8> {
        subnegotiation(
            SET_CONTROL,
            &[if assert { CONTROL_BREAK_ON } else { CONTROL_BREAK_OFF }],
        )
    }

    pub fn dtr_frame(on: bool) -> Vec<u8> {
        subnegotiation(
            SET_CONTROL,
            &[if on { CONTROL_DTR_ON } else { CONTROL_DTR_OFF }],
        )
    }

    pub fn rts_frame(on: bool) -> Vec<u8> {
        subnegotiation(
            SET_CONTROL,
            &[if on { CONTROL_RTS_ON } else { CONTROL_RTS_OFF }],
        )
    }

    pub fn purge_frame(target: PurgeTarget) -> Vec<u8> {
        subnegotiation(
            PURGE_DATA,
            &[match target {
                PurgeTarget::Input => PURGE_RECEIVE,
                PurgeTarget::Output => PURGE_TRANSMIT,
                PurgeTarget::Both => PURGE_BOTH,
            }],
        )
    }

    /// Handle one complete subnegotiation payload: `[command, value...]`, with
    /// `IAC IAC` already collapsed by the framing layer.
    pub fn handle_subnegotiation(&mut self, payload: &[u8]) {
        let Some((&raw_command, value)) = payload.split_first() else {
            return;
        };

        // Replies should carry the +100 server offset. A few implementations
        // echo the bare client command instead; accept both rather than
        // silently ignoring a well-meaning peer.
        let command = if raw_command >= SERVER_OFFSET {
            raw_command - SERVER_OFFSET
        } else {
            raw_command
        };

        match command {
            SIGNATURE => {
                if !value.is_empty() {
                    self.signature = Some(String::from_utf8_lossy(value).into_owned());
                    self.dirty = true;
                }
            }
            SET_BAUDRATE => {
                if let Some(bytes) = value.get(..4) {
                    let baud = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    if baud > 0 {
                        self.effective.baud_rate = baud;
                        self.confirmed.baud_rate = true;
                        self.dirty = true;
                    }
                }
            }
            SET_DATASIZE => {
                if let Some(&bits) = value.first() {
                    if (5..=8).contains(&bits) {
                        self.effective.data_bits = bits;
                        self.confirmed.data_bits = true;
                        self.dirty = true;
                    }
                }
            }
            SET_PARITY => {
                // MARK (4) and SPACE (5) have no representation in AuraTerm's
                // parity type; leave the value unconfirmed rather than lie.
                let parity = match value.first() {
                    Some(1) => Some(SerialParity::None),
                    Some(2) => Some(SerialParity::Odd),
                    Some(3) => Some(SerialParity::Even),
                    _ => None,
                };
                if let Some(parity) = parity {
                    self.effective.parity = parity;
                    self.confirmed.parity = true;
                    self.dirty = true;
                }
            }
            SET_STOPSIZE => {
                // 3 means 1.5 stop bits, which AuraTerm does not offer.
                if let Some(bits @ (1 | 2)) = value.first().copied() {
                    self.effective.stop_bits = bits;
                    self.confirmed.stop_bits = true;
                    self.dirty = true;
                }
            }
            SET_CONTROL => {
                // Only the flow-control subset of SET-CONTROL describes a line
                // parameter; BREAK/DTR/RTS acknowledgements land here too and
                // must not be mistaken for one.
                match value.first().copied() {
                    Some(CONTROL_FLOW_NONE) => self.confirm_flow(SerialFlowControl::None),
                    Some(CONTROL_FLOW_XON_XOFF) => self.confirm_flow(SerialFlowControl::Software),
                    Some(CONTROL_FLOW_HARDWARE) => self.confirm_flow(SerialFlowControl::Hardware),
                    // The same command acknowledges the control lines, which is
                    // the only way a client learns their real state.
                    Some(CONTROL_DTR_ON) => self.set_signal(true, None),
                    Some(CONTROL_DTR_OFF) => self.set_signal(false, None),
                    Some(CONTROL_RTS_ON) => self.set_signal(true, Some(())),
                    Some(CONTROL_RTS_OFF) => self.set_signal(false, Some(())),
                    _ => {}
                }
            }
            NOTIFY_LINESTATE => {
                if let Some(&bits) = value.first() {
                    let errors = LineErrors {
                        break_detected: bits & LINESTATE_BREAK != 0,
                        framing: bits & LINESTATE_FRAMING != 0,
                        parity: bits & LINESTATE_PARITY != 0,
                        overrun: bits & LINESTATE_OVERRUN != 0,
                    };
                    if errors != self.line_errors {
                        self.line_errors = errors;
                        self.dirty = true;
                    }
                }
            }
            NOTIFY_MODEMSTATE => {
                if let Some(&bits) = value.first() {
                    let modem = ModemLines {
                        cts: bits & MODEMSTATE_CTS != 0,
                        dsr: bits & MODEMSTATE_DSR != 0,
                        cd: bits & MODEMSTATE_CD != 0,
                        ri: bits & MODEMSTATE_RI != 0,
                    };
                    if modem != self.modem {
                        self.modem = modem;
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial_params::{SerialParams, SerialSignals};

    fn params() -> SerialParams {
        SerialParams {
            baud_rate: 115200,
            data_bits: 8,
            stop_bits: 1,
            parity: SerialParity::None,
            flow_control: SerialFlowControl::None,
        }
    }

    #[test]
    fn frames_a_subnegotiation() {
        assert_eq!(
            subnegotiation(SET_DATASIZE, &[8]),
            vec![IAC, SB, COM_PORT_OPTION, SET_DATASIZE, 8, IAC, SE],
        );
    }

    #[test]
    fn doubles_ff_inside_the_payload() {
        // 255 baud encodes as 00 00 00 FF; the FF must go out twice or the peer
        // reads it as IAC and the frame falls apart. Missing this is invisible
        // at ordinary baud rates and then corrupts ZMODEM at random.
        assert_eq!(
            subnegotiation(SET_BAUDRATE, &255u32.to_be_bytes()),
            vec![
                IAC,
                SB,
                COM_PORT_OPTION,
                SET_BAUDRATE,
                0,
                0,
                0,
                0xFF,
                0xFF,
                IAC,
                SE,
            ],
        );
    }

    #[test]
    fn pushes_the_parameter_block_on_do() {
        let mut state = ComPortState::new(params(), false);
        let block = state.on_do();
        assert!(state.negotiated());

        // Baud rate first, then the rest of the line settings.
        let mut expected_head = subnegotiation(SET_BAUDRATE, &115200u32.to_be_bytes());
        expected_head.extend_from_slice(&subnegotiation(SET_DATASIZE, &[8]));
        expected_head.extend_from_slice(&subnegotiation(SET_PARITY, &[1]));
        expected_head.extend_from_slice(&subnegotiation(SET_STOPSIZE, &[1]));
        expected_head.extend_from_slice(&subnegotiation(SET_CONTROL, &[CONTROL_FLOW_NONE]));
        assert!(block.starts_with(&expected_head));

        // DTR and RTS asserted, the actionable notifications subscribed to,
        // stale buffers purged.
        assert!(windows_contains(&block, &subnegotiation(SET_CONTROL, &[CONTROL_DTR_ON])));
        assert!(windows_contains(&block, &subnegotiation(SET_CONTROL, &[CONTROL_RTS_ON])));
        assert!(windows_contains(
            &block,
            &subnegotiation(SET_LINESTATE_MASK, &[LINESTATE_SUBSCRIPTION]),
        ));
        assert!(windows_contains(
            &block,
            &subnegotiation(SET_MODEMSTATE_MASK, &[MODEMSTATE_SUBSCRIPTION]),
        ));
        assert!(windows_contains(&block, &subnegotiation(PURGE_DATA, &[PURGE_BOTH])));

        // A second DO must not replay the block.
        assert!(state.on_do().is_empty());
    }

    #[test]
    fn hardware_flow_control_leaves_rts_to_the_uart() {
        let mut requested = params();
        requested.flow_control = SerialFlowControl::Hardware;
        let block = ComPortState::new(requested, false).on_do();
        assert!(windows_contains(&block, &subnegotiation(SET_CONTROL, &[CONTROL_DTR_ON])));
        assert!(!windows_contains(&block, &subnegotiation(SET_CONTROL, &[CONTROL_RTS_ON])));
    }

    #[test]
    fn adopt_mode_queries_instead_of_setting() {
        let block = ComPortState::new(params(), true).on_do();
        assert!(windows_contains(&block, &subnegotiation(SET_BAUDRATE, &[0, 0, 0, 0])));
        // Never sends the requested 115200, so a shared port stays as configured.
        assert!(!windows_contains(
            &block,
            &subnegotiation(SET_BAUDRATE, &115200u32.to_be_bytes()),
        ));
    }

    #[test]
    fn records_a_server_clamping_the_baud_rate() {
        let mut state = ComPortState::new(params(), false);
        state.on_do();
        state.take_dirty();

        // Server answers 101 SET-BAUDRATE with 9600, not the 115200 we asked for.
        state.handle_subnegotiation(&[SET_BAUDRATE + SERVER_OFFSET, 0, 0, 0x25, 0x80]);

        assert_eq!(state.requested().baud_rate, 115200);
        assert_eq!(state.effective().baud_rate, 9600);
        assert!(state.confirmed().baud_rate);
        assert!(state.take_dirty());
    }

    #[test]
    fn accepts_replies_without_the_server_offset() {
        // Some implementations echo the bare client command number.
        let mut state = ComPortState::new(params(), false);
        state.handle_subnegotiation(&[SET_DATASIZE, 7]);
        assert_eq!(state.effective().data_bits, 7);
        assert!(state.confirmed().data_bits);
    }

    #[test]
    fn set_control_acknowledgements_track_dtr_and_rts() {
        // The parameter block asserts both; a server that drops RTS has to be
        // able to say so, or the UI's "toggle" has no honest starting point.
        let mut state = ComPortState::new(params(), false);
        assert_eq!(state.signals(), SerialSignals { dtr: true, rts: true });

        state.handle_subnegotiation(&[SET_CONTROL + SERVER_OFFSET, CONTROL_RTS_OFF]);
        assert_eq!(state.signals(), SerialSignals { dtr: true, rts: false });

        state.handle_subnegotiation(&[SET_CONTROL + SERVER_OFFSET, CONTROL_DTR_OFF]);
        assert_eq!(state.signals(), SerialSignals { dtr: false, rts: false });

        // A control acknowledgement must not be mistaken for a flow-control one.
        assert!(!state.confirmed().flow_control);
    }

    #[test]
    fn parses_modem_and_line_state_notifications() {
        let mut state = ComPortState::new(params(), false);

        state.handle_subnegotiation(&[
            NOTIFY_MODEMSTATE + SERVER_OFFSET,
            MODEMSTATE_CTS | MODEMSTATE_DSR | MODEMSTATE_CD,
        ]);
        assert_eq!(
            state.modem(),
            ModemLines { cts: true, dsr: true, cd: true, ri: false },
        );

        state.handle_subnegotiation(&[
            NOTIFY_LINESTATE + SERVER_OFFSET,
            LINESTATE_FRAMING | LINESTATE_PARITY,
        ]);
        assert!(state.line_errors().framing);
        assert!(state.line_errors().parity);
        assert!(!state.line_errors().overrun);
    }

    #[test]
    fn ignores_unrepresentable_parity_and_stop_bits() {
        let mut state = ComPortState::new(params(), false);
        // MARK parity and 1.5 stop bits have no AuraTerm representation; the
        // values must stay unconfirmed rather than be silently rewritten.
        state.handle_subnegotiation(&[SET_PARITY + SERVER_OFFSET, 4]);
        state.handle_subnegotiation(&[SET_STOPSIZE + SERVER_OFFSET, 3]);
        assert!(!state.confirmed().parity);
        assert!(!state.confirmed().stop_bits);
        assert_eq!(state.effective().parity, SerialParity::None);
        assert_eq!(state.effective().stop_bits, 1);
    }

    #[test]
    fn dont_settles_the_handshake_without_agreeing() {
        let mut state = ComPortState::new(params(), false);
        state.on_dont();
        assert!(state.settled());
        assert!(!state.negotiated());
    }

    #[test]
    fn silence_becomes_a_verdict_when_we_give_up() {
        // A raw TCP port never answers WILL COM-PORT-OPTION at all, so silence
        // has to be turned into a decision or the UI can never say anything.
        let mut state = ComPortState::new(params(), false);
        assert!(!state.settled());
        state.give_up();
        assert!(state.settled());
        assert!(!state.negotiated());
        assert!(state.take_dirty(), "the verdict has to reach the frontend");

        // Giving up after the peer already agreed must not undo the agreement.
        let mut agreed = ComPortState::new(params(), false);
        agreed.on_do();
        agreed.give_up();
        assert!(agreed.negotiated());
    }

    #[test]
    fn retune_sends_only_what_changed() {
        let mut state = ComPortState::new(params(), false);
        state.on_do();
        // Pretend the server confirmed everything it was sent.
        state.handle_subnegotiation(&[SET_BAUDRATE + SERVER_OFFSET, 0, 1, 0xC2, 0x00]);
        state.handle_subnegotiation(&[SET_DATASIZE + SERVER_OFFSET, 8]);
        state.handle_subnegotiation(&[SET_PARITY + SERVER_OFFSET, 1]);
        state.handle_subnegotiation(&[SET_STOPSIZE + SERVER_OFFSET, 1]);
        state.handle_subnegotiation(&[SET_CONTROL + SERVER_OFFSET, CONTROL_FLOW_NONE]);

        let mut next = params();
        next.baud_rate = 9600;
        let frames = state.retune(next);

        // Only the baud rate moved, so only the baud rate goes on the wire.
        assert_eq!(frames, subnegotiation(SET_BAUDRATE, &9600u32.to_be_bytes()));
        // …and it is no longer confirmed until the server says so again.
        assert!(!state.confirmed().baud_rate);
        assert!(state.confirmed().data_bits);
        assert_eq!(state.requested().baud_rate, 9600);
    }

    #[test]
    fn retune_resends_anything_never_confirmed() {
        // A server that stayed silent on data size gets asked again, even
        // though the value did not change.
        let mut state = ComPortState::new(params(), false);
        state.on_do();
        let frames = state.retune(params());
        assert!(windows_contains(&frames, &subnegotiation(SET_DATASIZE, &[8])));
    }

    #[test]
    fn control_frames_match_the_spec() {
        assert_eq!(
            ComPortState::break_frame(true),
            subnegotiation(SET_CONTROL, &[CONTROL_BREAK_ON]),
        );
        assert_eq!(
            ComPortState::break_frame(false),
            subnegotiation(SET_CONTROL, &[CONTROL_BREAK_OFF]),
        );
        assert_eq!(
            ComPortState::dtr_frame(false),
            subnegotiation(SET_CONTROL, &[CONTROL_DTR_OFF]),
        );
        assert_eq!(
            ComPortState::rts_frame(true),
            subnegotiation(SET_CONTROL, &[CONTROL_RTS_ON]),
        );
        assert_eq!(
            ComPortState::purge_frame(PurgeTarget::Input),
            subnegotiation(PURGE_DATA, &[PURGE_RECEIVE]),
        );
        assert_eq!(
            ComPortState::purge_frame(PurgeTarget::Both),
            subnegotiation(PURGE_DATA, &[PURGE_BOTH]),
        );
    }

    fn windows_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }
}
