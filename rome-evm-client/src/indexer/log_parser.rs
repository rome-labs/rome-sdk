use crate::error::{ProgramResult, RomeEvmError::LogParserError};
use ethers::{
    abi::{self, ParamType},
    types::{Address, Log, H256, U256},
    utils::hex,
};
use rome_evm::{
    EVENT_LOG, EXIT_REASON, GAS_RECIPIENT, GAS_VALUE, H160, REVERT_ERROR, REVERT_PANIC,
};
use std::mem::size_of;

pub trait Parser {
    fn consume(&mut self, log: &str) -> ProgramResult<()> {
        if log.starts_with("Program data: ") {
            let (_, log) = log.split_at("Program data: ".len());
            let iter = log.split_whitespace();
            for encode in iter {
                let decode = base64::decode(encode)?;
                self.advance(decode)?
            }
        }
        Ok(())
    }
    fn advance(&mut self, item: Vec<u8>) -> ProgramResult<()>;
    fn found(&self) -> bool;
}

#[derive(PartialEq)]
pub enum ExitReasonState {
    Init,
    Code,
    ReasonLen,
    Reason,
    ReturnValue,
}
#[derive(PartialEq)]
pub enum EventState {
    Init,
    Address,
    TopicsLen,
    Topics,
    Data,
}

#[derive(PartialEq)]
pub enum GasReportState {
    Init,
    GasValue,
    GasRecipient,
    GasValueFound,
    GasRecipientFound,
}

#[derive(Default, Debug)]
pub struct ExitReason {
    pub code: u8,
    pub reason: String,
    pub return_value: Vec<u8>,
}

impl ExitReason {
    pub fn log(&self) -> String {
        let mut log = format!("EVM exit_code {}, reason {}", self.code, self.reason);
        if self.reason.starts_with("Revert") {
            let revert = decode_revert(&self.return_value).unwrap_or_default();
            log = format!("{log}, Revert \"{revert})\"");
        }

        log
    }
}

#[derive(Default, Debug)]
pub struct LogParser {
    pub events: Vec<Log>,
    pub exit_reason: Option<ExitReason>,
    pub gas_value: Option<U256>,
    pub gas_recipient: Option<Address>,
}

impl LogParser {
    pub fn new() -> Self {
        LogParser {
            events: vec![],
            exit_reason: None,
            gas_value: None,
            gas_recipient: None,
        }
    }

    pub fn parse(&mut self, logs: &Vec<String>) -> ProgramResult<()> {
        for log in logs {
            let mut event_parser = EventParser::default();
            let mut reason_parser = ExitReasonParser::default();
            let mut gas_value_parser = GasValueParser::default();
            let mut gas_recipient_parser = GasRecipientParser::default();

            event_parser.consume(log)?;
            reason_parser.consume(log)?;
            gas_value_parser.consume(log)?;
            gas_recipient_parser.consume(log)?;

            if event_parser.found() {
                self.events.push(event_parser.event)
            }
            if reason_parser.found() {
                self.exit_reason = Some(reason_parser.exit_reason)
            }
            if gas_value_parser.found() {
                self.gas_value = Some(gas_value_parser.gas_value)
            }
            if gas_recipient_parser.found() {
                self.gas_recipient = gas_recipient_parser.recipient
            }
        }

        Ok(())
    }
}

pub struct EventParser {
    pub state: EventState,
    pub topics_len: usize,
    pub event: Log,
}

impl Default for EventParser {
    fn default() -> Self {
        Self {
            state: EventState::Init,
            topics_len: 0,
            event: Log::default(),
        }
    }
}

impl Parser for EventParser {
    fn advance(&mut self, item: Vec<u8>) -> ProgramResult<()> {
        match self.state {
            EventState::Init => {
                if item == EVENT_LOG {
                    self.state = EventState::Address;
                }
            }
            EventState::Address => {
                if item.len() != size_of::<H160>() {
                    return Err(LogParserError("event log: address expected".to_string()));
                };
                self.event.address = Address::from_slice(&item);
                self.state = EventState::TopicsLen;
            }
            EventState::TopicsLen => {
                if item.len() != size_of::<u8>() {
                    return Err(LogParserError("event log: topics_len expected".to_string()));
                };
                self.topics_len = item[0] as usize;
                if self.topics_len > 0 {
                    self.state = EventState::Topics;
                } else {
                    self.state = EventState::Data;
                }
            }
            EventState::Topics => {
                if item.len() != size_of::<rome_evm::H256>() {
                    return Err(LogParserError("event log: topic expected".to_string()));
                };
                self.event.topics.push(H256::from_slice(&item));
                if self.event.topics.len() == self.topics_len {
                    self.state = EventState::Data
                }
            }
            EventState::Data => {
                self.event.data = item.into();
            }
        }

        Ok(())
    }

    fn found(&self) -> bool {
        matches!(self.state, EventState::Topics | EventState::Data)
    }
}

pub struct ExitReasonParser {
    pub state: ExitReasonState,
    pub reason_len: usize,
    pub exit_reason: ExitReason,
}

impl Default for ExitReasonParser {
    fn default() -> Self {
        Self {
            state: ExitReasonState::Init,
            reason_len: 0,
            exit_reason: ExitReason::default(),
        }
    }
}

impl Parser for ExitReasonParser {
    fn advance(&mut self, item: Vec<u8>) -> ProgramResult<()> {
        match self.state {
            ExitReasonState::Init => {
                if item == EXIT_REASON {
                    self.state = ExitReasonState::Code;
                }
            }
            ExitReasonState::Code => {
                if item.len() != size_of::<u8>() {
                    return Err(LogParserError("exit reason: code expected".to_string()));
                };
                self.exit_reason.code = item[0];
                self.state = ExitReasonState::ReasonLen;
            }
            ExitReasonState::ReasonLen => {
                if item.len() != size_of::<usize>() {
                    return Err(LogParserError(
                        "exit reason: topics_len expected".to_string(),
                    ));
                };
                self.reason_len = usize::from_le_bytes(item.as_slice().try_into().expect("here"));
                self.state = ExitReasonState::Reason;
            }
            ExitReasonState::Reason => {
                self.exit_reason.reason = String::from_utf8(item).map_err(|e| {
                    LogParserError(format!("exit reason: msg convert error {:?}", e))
                })?;

                self.state = ExitReasonState::ReturnValue;
            }
            ExitReasonState::ReturnValue => {
                self.exit_reason.return_value = item;
            }
        }

        Ok(())
    }

    fn found(&self) -> bool {
        matches!(
            self.state,
            ExitReasonState::Reason | ExitReasonState::ReturnValue
        )
    }
}

pub fn decode_revert(return_value: &[u8]) -> Option<String> {
    if return_value.len() < 4 {
        tracing::warn!("error to decode revert message: return value too short");
        return None;
    }

    let return_reason = &return_value[0..4];
    let return_value = &return_value[4..];

    match return_reason {
        REVERT_ERROR => match abi::decode(&[ParamType::String], return_value) {
            Ok(tokens) => tokens
                .first()
                .map(|token| token.clone().into_string().unwrap_or_default()),
            Err(e) => {
                tracing::warn!("error to decode revert message: {:?}", e);
                None
            }
        },
        REVERT_PANIC => match abi::decode(&[ParamType::Uint(32)], return_value) {
            Ok(tokens) => {
                if let Some(token) = tokens.first() {
                    let value = token.clone().into_uint().unwrap_or_default();
                    Some(format!("panic {}", value))
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!("error to decode revert panic: {:?}", e);
                None
            }
        },
        _ => Some(hex::encode(return_value)),
    }
}

pub struct GasValueParser {
    pub state: GasReportState,
    pub gas_value: U256,
}

impl Default for GasValueParser {
    fn default() -> Self {
        Self {
            state: GasReportState::Init,
            gas_value: U256::zero(),
        }
    }
}

impl Parser for GasValueParser {
    fn advance(&mut self, item: Vec<u8>) -> ProgramResult<()> {
        match self.state {
            GasReportState::Init => {
                if item == GAS_VALUE {
                    self.state = GasReportState::GasValue;
                }
            }
            GasReportState::GasValue => {
                if item.len() != 32 {
                    return Err(LogParserError(format!("Gas value: U256 expected")));
                };
                self.gas_value = U256::from_big_endian(item.as_slice());
                self.state = GasReportState::GasValueFound;
            }
            _ => {}
        }

        Ok(())
    }

    fn found(&self) -> bool {
        self.state == GasReportState::GasValueFound
    }
}

pub struct GasRecipientParser {
    pub state: GasReportState,
    pub recipient: Option<Address>,
}

impl Default for GasRecipientParser {
    fn default() -> Self {
        Self {
            state: GasReportState::Init,
            recipient: None,
        }
    }
}

impl Parser for GasRecipientParser {
    fn advance(&mut self, item: Vec<u8>) -> ProgramResult<()> {
        match self.state {
            GasReportState::Init => {
                if item == GAS_RECIPIENT {
                    self.state = GasReportState::GasRecipient;
                }
            }
            GasReportState::GasRecipient => {
                if item.len() != 20 {
                    return Err(LogParserError(format!("Gas recipient: H160 expected")));
                };
                self.recipient = Some(Address::from_slice(item.as_slice()));
                self.state = GasReportState::GasRecipientFound;
            }
            _ => {}
        }

        Ok(())
    }

    fn found(&self) -> bool {
        self.state == GasReportState::GasRecipientFound
    }
}
