//! Modbus/TCP (Modbus Application Protocol v1.1b3; Modbus Messaging on TCP/IP
//! Implementation Guide v1.0b): the MBAP header and the register functions an
//! inverter or meter gateway uses.
//!
//! Parsing is total: every byte string yields a frame or an error, never a
//! panic, and every length the protocol bounds is checked against its bound.

use crate::EnergyError;

/// MBAP header length.
pub const MBAP_LEN: usize = 7;
/// Largest PDU: 253 bytes (256-byte serial ADU less address and CRC).
pub const MAX_PDU: usize = 253;
/// Largest register count a read may ask for (§6.3).
pub const MAX_READ_REGISTERS: u16 = 125;
/// Largest register count a write-multiple may carry (§6.12).
pub const MAX_WRITE_REGISTERS: u16 = 123;
const EXCEPTION_BIT: u8 = 0x80;

/// The function codes parsed here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Function {
    /// 0x03.
    ReadHoldingRegisters = 0x03,
    /// 0x04.
    ReadInputRegisters = 0x04,
    /// 0x06.
    WriteSingleRegister = 0x06,
    /// 0x10.
    WriteMultipleRegisters = 0x10,
}

impl TryFrom<u8> for Function {
    type Error = EnergyError;
    fn try_from(code: u8) -> Result<Self, EnergyError> {
        match code {
            0x03 => Ok(Self::ReadHoldingRegisters),
            0x04 => Ok(Self::ReadInputRegisters),
            0x06 => Ok(Self::WriteSingleRegister),
            0x10 => Ok(Self::WriteMultipleRegisters),
            other => Err(EnergyError::Unsupported(format!(
                "Modbus function 0x{other:02x}"
            ))),
        }
    }
}

/// The MBAP header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mbap {
    /// Pairs a response with its request.
    pub transaction: u16,
    /// Unit (slave) identifier.
    pub unit: u8,
}

/// A request PDU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Read `count` registers from `start` (holding or input).
    Read {
        /// Which table.
        function: Function,
        /// First register address.
        start: u16,
        /// How many, 1..=125.
        count: u16,
    },
    /// Write one register.
    WriteSingle {
        /// Address.
        address: u16,
        /// Value.
        value: u16,
    },
    /// Write consecutive registers.
    WriteMultiple {
        /// First address.
        start: u16,
        /// Values, 1..=123.
        values: Vec<u16>,
    },
}

/// A response PDU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// Registers read.
    Registers {
        /// Which table.
        function: Function,
        /// The values.
        values: Vec<u16>,
    },
    /// Echo of a single write.
    WroteSingle {
        /// Address.
        address: u16,
        /// Value.
        value: u16,
    },
    /// Acknowledgement of a multiple write.
    WroteMultiple {
        /// First address.
        start: u16,
        /// How many.
        count: u16,
    },
    /// The device refused: function code and exception code (§7).
    Exception {
        /// The function that failed.
        function: u8,
        /// 1 illegal function, 2 illegal address, 3 illegal value, 4 device failure, ...
        code: u8,
    },
}

fn u16_at(bytes: &[u8], at: usize) -> Result<u16, EnergyError> {
    bytes
        .get(at..at + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or(EnergyError::Truncated("Modbus"))
}

/// Splits an ADU into its header and PDU, checking the MBAP length field and
/// that the protocol identifier is 0 (Modbus).
///
/// # Errors
///
/// Truncated, a length field that disagrees with the bytes, a non-Modbus
/// protocol id, or a PDU over [`MAX_PDU`].
pub fn split_adu(adu: &[u8]) -> Result<(Mbap, &[u8]), EnergyError> {
    if adu.len() < MBAP_LEN + 1 {
        return Err(EnergyError::Truncated("MBAP"));
    }
    let transaction = u16_at(adu, 0)?;
    if u16_at(adu, 2)? != 0 {
        return Err(EnergyError::Malformed("MBAP protocol id is not 0".into()));
    }
    let length = usize::from(u16_at(adu, 4)?);
    // The length counts the unit id and the PDU.
    if length != adu.len() - 6 {
        return Err(EnergyError::Malformed(format!(
            "MBAP length {length}, {} bytes follow",
            adu.len() - 6
        )));
    }
    let pdu = &adu[MBAP_LEN..];
    if pdu.len() > MAX_PDU {
        return Err(EnergyError::Malformed(format!(
            "PDU of {} bytes",
            pdu.len()
        )));
    }
    Ok((
        Mbap {
            transaction,
            unit: adu[6],
        },
        pdu,
    ))
}

fn register_count(count: u16, max: u16) -> Result<u16, EnergyError> {
    if (1..=max).contains(&count) {
        Ok(count)
    } else {
        Err(EnergyError::Malformed(format!(
            "register count {count} outside 1..={max}"
        )))
    }
}

/// Parses a request PDU.
///
/// # Errors
///
/// An unsupported function, a wrong length, or a count outside its bound.
pub fn parse_request(pdu: &[u8]) -> Result<Request, EnergyError> {
    let function = Function::try_from(*pdu.first().ok_or(EnergyError::Truncated("PDU"))?)?;
    match function {
        Function::ReadHoldingRegisters | Function::ReadInputRegisters => {
            exact(pdu, 5)?;
            Ok(Request::Read {
                function,
                start: u16_at(pdu, 1)?,
                count: register_count(u16_at(pdu, 3)?, MAX_READ_REGISTERS)?,
            })
        }
        Function::WriteSingleRegister => {
            exact(pdu, 5)?;
            Ok(Request::WriteSingle {
                address: u16_at(pdu, 1)?,
                value: u16_at(pdu, 3)?,
            })
        }
        Function::WriteMultipleRegisters => {
            let count = register_count(u16_at(pdu, 3)?, MAX_WRITE_REGISTERS)?;
            let bytes = *pdu.get(5).ok_or(EnergyError::Truncated("PDU"))?;
            if usize::from(bytes) != 2 * usize::from(count) {
                return Err(EnergyError::Malformed(format!(
                    "byte count {bytes} for {count} registers"
                )));
            }
            exact(pdu, 6 + usize::from(bytes))?;
            Ok(Request::WriteMultiple {
                start: u16_at(pdu, 1)?,
                values: registers(&pdu[6..]),
            })
        }
    }
}

/// Parses a response PDU. `asked` is the request it answers, so a read
/// response of the wrong size is refused.
///
/// # Errors
///
/// A wrong length, a register count that does not match the request, or a
/// response to a different function.
pub fn parse_response(pdu: &[u8], asked: &Request) -> Result<Response, EnergyError> {
    let code = *pdu.first().ok_or(EnergyError::Truncated("PDU"))?;
    if code & EXCEPTION_BIT != 0 {
        exact(pdu, 2)?;
        return Ok(Response::Exception {
            function: code & !EXCEPTION_BIT,
            code: pdu[1],
        });
    }
    let function = Function::try_from(code)?;
    match (asked, function) {
        (
            Request::Read {
                function: f, count, ..
            },
            _,
        ) if *f == function => {
            let bytes = *pdu.get(1).ok_or(EnergyError::Truncated("PDU"))?;
            if usize::from(bytes) != 2 * usize::from(*count) {
                return Err(EnergyError::Malformed(format!(
                    "{bytes} bytes for {count} registers"
                )));
            }
            exact(pdu, 2 + usize::from(bytes))?;
            Ok(Response::Registers {
                function,
                values: registers(&pdu[2..]),
            })
        }
        (Request::WriteSingle { .. }, Function::WriteSingleRegister) => {
            exact(pdu, 5)?;
            Ok(Response::WroteSingle {
                address: u16_at(pdu, 1)?,
                value: u16_at(pdu, 3)?,
            })
        }
        (Request::WriteMultiple { .. }, Function::WriteMultipleRegisters) => {
            exact(pdu, 5)?;
            Ok(Response::WroteMultiple {
                start: u16_at(pdu, 1)?,
                count: u16_at(pdu, 3)?,
            })
        }
        _ => Err(EnergyError::Malformed(format!(
            "response 0x{code:02x} does not answer {asked:?}"
        ))),
    }
}

/// Encodes a request as a full ADU.
#[must_use]
pub fn encode_request(header: Mbap, request: &Request) -> Vec<u8> {
    let mut pdu = Vec::new();
    match request {
        Request::Read {
            function,
            start,
            count,
        } => {
            pdu.push(*function as u8);
            pdu.extend_from_slice(&start.to_be_bytes());
            pdu.extend_from_slice(&count.to_be_bytes());
        }
        Request::WriteSingle { address, value } => {
            pdu.push(Function::WriteSingleRegister as u8);
            pdu.extend_from_slice(&address.to_be_bytes());
            pdu.extend_from_slice(&value.to_be_bytes());
        }
        Request::WriteMultiple { start, values } => {
            pdu.push(Function::WriteMultipleRegisters as u8);
            pdu.extend_from_slice(&start.to_be_bytes());
            pdu.extend_from_slice(
                &u16::try_from(values.len())
                    .unwrap_or(u16::MAX)
                    .to_be_bytes(),
            );
            pdu.push(u8::try_from(values.len() * 2).unwrap_or(u8::MAX));
            for v in values {
                pdu.extend_from_slice(&v.to_be_bytes());
            }
        }
    }
    let length = u16::try_from(pdu.len() + 1).unwrap_or(u16::MAX);
    let mut adu = Vec::with_capacity(MBAP_LEN + pdu.len());
    adu.extend_from_slice(&header.transaction.to_be_bytes());
    adu.extend_from_slice(&0u16.to_be_bytes());
    adu.extend_from_slice(&length.to_be_bytes());
    adu.push(header.unit);
    adu.extend_from_slice(&pdu);
    adu
}

fn exact(pdu: &[u8], len: usize) -> Result<(), EnergyError> {
    if pdu.len() == len {
        Ok(())
    } else {
        Err(EnergyError::Malformed(format!(
            "PDU of {} bytes, expected {len}",
            pdu.len()
        )))
    }
}

fn registers(bytes: &[u8]) -> Vec<u16> {
    let (pairs, _) = bytes.as_chunks::<2>();
    pairs.iter().map(|b| u16::from_be_bytes(*b)).collect()
}
