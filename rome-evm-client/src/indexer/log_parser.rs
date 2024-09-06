use {
    crate::error::{Result, RomeEvmError::LogParserError},
    ethers::{
        abi::{self, ParamType},
        types::{Address, Log, H256},
        utils::hex,
    },
    rome_evm::{EVENT_LOG, EXIT_REASON, H160, REVERT_ERROR, REVERT_PANIC},
    std::mem::size_of,
};

pub trait Parser {
    fn consume(&mut self, log: &String) -> Result<()> {
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
    fn advance(&mut self, item: Vec<u8>) -> Result<()>;
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
            let revert = decode_revert(Some(&self.return_value)).unwrap_or_default();
            log = format!("{log}, Revert \"{revert})\"");
        }

        log
    }
}

#[derive(Default, Debug)]
pub struct LogParser {
    pub events: Vec<Log>,
    pub exit_reason: Option<ExitReason>,
}

impl LogParser {
    pub fn new() -> Self {
        LogParser {
            events: vec![],
            exit_reason: None,
        }
    }

    pub fn parse(&mut self, logs: &Vec<String>) -> Result<()> {
        for log in logs {
            let mut event_parser = EventParser::default();
            let mut reason_parser = ExitReasonParser::default();
            event_parser.consume(log)?;
            reason_parser.consume(log)?;

            if event_parser.found() {
                self.events.push(event_parser.event)
            }
            if reason_parser.found() {
                self.exit_reason = Some(reason_parser.exit_reason)
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
    fn advance(&mut self, item: Vec<u8>) -> Result<()> {
        match self.state {
            EventState::Init => {
                if item == EVENT_LOG {
                    self.state = EventState::Address;
                }
            }
            EventState::Address => {
                if item.len() != size_of::<H160>() {
                    return Err(LogParserError(format!("event log: address expected")));
                };
                self.event.address = Address::from_slice(&item);
                self.state = EventState::TopicsLen;
            }
            EventState::TopicsLen => {
                if item.len() != size_of::<u8>() {
                    return Err(LogParserError(format!("event log: topics_len expected")));
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
                    return Err(LogParserError(format!("event log: topic expected")));
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
        match self.state {
            EventState::Topics | EventState::Data => true,
            _ => false,
        }
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
    fn advance(&mut self, item: Vec<u8>) -> Result<()> {
        match self.state {
            ExitReasonState::Init => {
                if item == EXIT_REASON {
                    self.state = ExitReasonState::Code;
                }
            }
            ExitReasonState::Code => {
                if item.len() != size_of::<u8>() {
                    return Err(LogParserError(format!("exit reason: code expected")));
                };
                self.exit_reason.code = item[0];
                self.state = ExitReasonState::ReasonLen;
            }
            ExitReasonState::ReasonLen => {
                if item.len() != size_of::<usize>() {
                    return Err(LogParserError(format!("exit reason: topics_len expected")));
                };
                self.reason_len = usize::from_le_bytes(item.as_slice().try_into().unwrap());
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
        match self.state {
            ExitReasonState::Reason | ExitReasonState::ReturnValue => true,
            _ => false,
        }
    }
}

pub fn decode_revert(return_value: Option<&Vec<u8>>) -> Option<String> {
    let return_value = if let Some(value) = return_value {
        value
    } else {
        return None;
    };

    let mes = if return_value.starts_with(REVERT_ERROR) {
        let return_value = &return_value[REVERT_ERROR.len()..];
        match abi::decode(&[ParamType::String], &return_value) {
            Ok(tokens) => {
                if let Some(token) = tokens.get(0) {
                    Some(token.clone().into_string().unwrap_or_default())
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!("error to decode revert message: {:?}", e);
                None
            }
        }
    } else if return_value.starts_with(REVERT_PANIC) {
        let return_value = &return_value[REVERT_PANIC.len()..];
        match abi::decode(&[ParamType::Uint(32)], &return_value) {
            Ok(tokens) => {
                if let Some(token) = tokens.get(0) {
                    let value = token.clone().into_uint().unwrap_or_default();
                    Some(format!("panic {}", value.to_string()))
                } else {
                    None
                }
            }
            Err(e) => {
                tracing::warn!("error to decode revert panic: {:?}", e);
                None
            }
        }
    } else {
        Some(hex::encode(&return_value))
    };

    mes
}
