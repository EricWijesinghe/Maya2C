//! A module's shape: what compiling it costs beyond its length (ADR-142).
//!
//! Cranelift's compile time is not a function of a module's byte length.
//! Measured on 2026-11-14 (`tests/gas_adversarial_calibration_tests.rs`), an
//! empty function costs tens of microseconds to compile in 4 bytes, a
//! declared local about a hundred nanoseconds in a fraction of a byte, and a
//! loop, a branch or a call microseconds in two or three bytes, with time per
//! operator growing as one function holds more of them. Priced by length
//! alone, a module shaped to stress the compiler compiled more than a
//! thousand times slower than its price.
//!
//! So a module is read once, linearly, before anything compiles it: this
//! scan counts what the compile pays for ([`ModuleShape`]), and refuses a
//! function past the per-function bounds, where compile time stops growing
//! linearly. It never runs the module or builds anything; it reads the
//! sections, the local declarations and the operators, skipping their
//! immediates.
//!
//! The scan accepts exactly the operators of the proposals the VM enables
//! (MVP, sign extension, saturating conversions, bulk memory, multi-value)
//! and refuses any other opcode, which the engine's validator refuses as
//! well. A module the scan refuses was never going to deploy.

use crate::error::{Result, VmError};

/// Most functions, imported and defined, one module may hold.
pub const MAX_FUNCTIONS: usize = 10_000;

/// Most locals one function may declare (wasmparser allows 50,000).
pub const MAX_LOCALS_PER_FUNCTION: usize = 1_024;

/// Most control operators (`block`, `loop`, `if`, `else`, `br`, `br_if`,
/// `br_table`, `return`) one function may hold. Past a few thousand in one
/// function, compile time per operator grows with their number: 40,000 `if`s
/// in one function compiled in 10.4 s (release), 260 µs each against 20 µs
/// each at 500.
pub const MAX_CONTROL_PER_FUNCTION: usize = 2_000;

/// What a module's compile pays for beyond its length (`crate::gas`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModuleShape {
    /// Functions, imported and defined.
    pub functions: u64,
    /// Locals declared, over every function.
    pub locals: u64,
    /// Control operators, `loop` included, over every function.
    pub controls: u64,
    /// `call` and `call_indirect` operators, over every function.
    pub calls: u64,
}

/// Reads `wasm`'s shape, refusing a module past the bounds above.
///
/// # Errors
///
/// [`VmError::SizeLimit`] past a bound, [`VmError::InvalidModule`] for bytes
/// that are not a module this VM's features can express.
pub fn scan(wasm: &[u8]) -> Result<ModuleShape> {
    let mut reader = Reader::new(wasm);
    if reader.take(8)? != b"\0asm\x01\0\0\0" {
        return Err(invalid("not a version 1 wasm module"));
    }
    let mut shape = ModuleShape::default();
    while !reader.done() {
        let id = reader.byte()?;
        let size = reader.u32()? as usize;
        let mut section = Reader::new(reader.take(size)?);
        match id {
            2 => imports(&mut section, &mut shape)?,
            3 => {
                let defined = section.u32()?;
                shape.functions = shape.functions.saturating_add(u64::from(defined));
            }
            10 => code(&mut section, &mut shape)?,
            _ => {}
        }
        if shape.functions > MAX_FUNCTIONS as u64 {
            return Err(too_many("functions", shape.functions, MAX_FUNCTIONS));
        }
    }
    Ok(shape)
}

fn invalid(what: &str) -> VmError {
    VmError::InvalidModule(format!("module shape: {what}"))
}

fn too_many(what: &'static str, actual: u64, limit: usize) -> VmError {
    VmError::SizeLimit {
        what,
        actual: usize::try_from(actual).unwrap_or(usize::MAX),
        limit,
    }
}

/// Counts imported functions; skips the other import kinds.
fn imports(section: &mut Reader<'_>, shape: &mut ModuleShape) -> Result<()> {
    for _ in 0..section.u32()? {
        section.name()?;
        section.name()?;
        match section.byte()? {
            0 => {
                section.u32()?;
                shape.functions = shape.functions.saturating_add(1);
            }
            1 => {
                section.byte()?;
                section.limits()?;
            }
            2 => section.limits()?,
            3 => {
                section.byte()?;
                section.byte()?;
            }
            _ => return Err(invalid("unknown import kind")),
        }
    }
    Ok(())
}

/// Every function body: its locals, then its operators.
fn code(section: &mut Reader<'_>, shape: &mut ModuleShape) -> Result<()> {
    for _ in 0..section.u32()? {
        let size = section.u32()? as usize;
        let mut body = Reader::new(section.take(size)?);
        let mut locals: u64 = 0;
        for _ in 0..body.u32()? {
            locals = locals.saturating_add(u64::from(body.u32()?));
            body.byte()?;
            if locals > MAX_LOCALS_PER_FUNCTION as u64 {
                return Err(too_many("function locals", locals, MAX_LOCALS_PER_FUNCTION));
            }
        }
        let (controls, calls) = operators(&mut body)?;
        if controls > MAX_CONTROL_PER_FUNCTION as u64 {
            return Err(too_many(
                "function control operators",
                controls,
                MAX_CONTROL_PER_FUNCTION,
            ));
        }
        shape.locals = shape.locals.saturating_add(locals);
        shape.controls = shape.controls.saturating_add(controls);
        shape.calls = shape.calls.saturating_add(calls);
    }
    Ok(())
}

/// `(control operators, calls)` in one body, skipping every immediate.
/// Calls compile linearly (5–6 µs each up to 10,000 in one function), so
/// they are priced but not bounded per function.
fn operators(body: &mut Reader<'_>) -> Result<(u64, u64)> {
    let (mut controls, mut calls) = (0u64, 0u64);
    while !body.done() {
        match body.byte()? {
            // unreachable, nop, end, drop, select, numeric and conversion
            // operators, sign extension: no immediate.
            0x00 | 0x01 | 0x0b | 0x1a | 0x1b | 0x45..=0xc4 => {}
            0x02..=0x04 => {
                body.block_type()?;
                controls += 1;
            }
            0x05 | 0x0f => controls += 1,
            0x0c | 0x0d => {
                body.u32()?;
                controls += 1;
            }
            0x10 => {
                body.u32()?;
                calls += 1;
            }
            // One control however many targets: a 200,000-target table
            // measured 17 ns a byte, far inside the byte price.
            0x0e => {
                let targets = body.u32()?;
                for _ in 0..=targets {
                    body.u32()?;
                }
                controls += 1;
            }
            0x11 => {
                body.u32()?;
                body.u32()?;
                calls += 1;
            }
            // local.get/set/tee, global.get/set, memory.size, memory.grow
            0x20..=0x24 | 0x3f | 0x40 => {
                body.u32()?;
            }
            0x28..=0x3e => body.mem_arg()?,
            0x41 | 0x42 => body.leb()?,
            0x43 => {
                body.take(4)?;
            }
            0x44 => {
                body.take(8)?;
            }
            0xfc => match body.u32()? {
                0..=7 => {}
                // memory.init, memory.copy, table.init, table.copy
                8 | 10 | 12 | 14 => {
                    body.u32()?;
                    body.u32()?;
                }
                // data.drop, memory.fill, elem.drop
                9 | 11 | 13 => {
                    body.u32()?;
                }
                _ => return Err(invalid("operator outside the enabled proposals")),
            },
            _ => return Err(invalid("operator outside the enabled proposals")),
        }
    }
    Ok((controls, calls))
}

/// A bounds-checked cursor over module bytes.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(len)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid("truncated"))?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Skips one LEB128 integer of at most ten bytes.
    fn leb(&mut self) -> Result<()> {
        for _ in 0..10 {
            if self.byte()? & 0x80 == 0 {
                return Ok(());
            }
        }
        Err(invalid("LEB128 integer too long"))
    }

    fn u32(&mut self) -> Result<u32> {
        let mut value: u64 = 0;
        for shift in (0..35).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return u32::try_from(value).map_err(|_| invalid("u32 out of range"));
            }
        }
        Err(invalid("u32 LEB128 too long"))
    }

    fn name(&mut self) -> Result<()> {
        let len = self.u32()? as usize;
        self.take(len).map(|_| ())
    }

    fn limits(&mut self) -> Result<()> {
        let flags = self.byte()?;
        self.u32()?;
        if flags & 1 == 1 {
            self.u32()?;
        }
        Ok(())
    }

    /// `0x40`, one value type, or a type index (s33, non-negative).
    fn block_type(&mut self) -> Result<()> {
        match self.bytes.get(self.at) {
            Some(0x40 | 0x6f..=0x7f) => self.byte().map(|_| ()),
            Some(_) => self.leb(),
            None => Err(invalid("truncated")),
        }
    }

    /// Alignment, then (with multi-memory's flag bit) a memory index, then
    /// the offset.
    fn mem_arg(&mut self) -> Result<()> {
        let align = self.u32()?;
        if align & 0x40 != 0 {
            self.u32()?;
        }
        self.leb()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wasm(text: &str) -> Vec<u8> {
        wat::parse_str(text).expect("valid WAT")
    }

    #[test]
    fn a_module_is_counted_by_what_compiling_it_costs() {
        let shape = scan(&wasm(
            r#"(module
                (import "env" "block_height" (func (result i64)))
                (memory 1)
                (func (param i32) (result i64) (local i64 i64) (local i32)
                  (block (loop (br_if 1 (local.get 0)) (br 0)))
                  (if (local.get 0) (then (nop)) (else (nop)))
                  (drop (i32.load offset=8 (i32.const 0)))
                  (memory.fill (i32.const 0) (i32.const 0) (i32.const 1))
                  (br_table 0 0 0 (i32.const 0))
                  (i64.const -1)))"#,
        ))
        .unwrap();
        assert_eq!(
            shape,
            ModuleShape {
                functions: 2,
                locals: 3,
                // block, loop, br_if, br, if, else, br_table
                controls: 7,
                calls: 0,
            }
        );
    }

    #[test]
    fn every_bound_is_refused_and_truncation_is_not_a_panic() {
        let locals = format!(
            "(module (func {}))",
            "(local i64)".repeat(MAX_LOCALS_PER_FUNCTION + 1)
        );
        assert!(matches!(
            scan(&wasm(&locals)),
            Err(VmError::SizeLimit {
                what: "function locals",
                ..
            })
        ));
        let control = format!(
            "(module (func {}))",
            "(block)".repeat(MAX_CONTROL_PER_FUNCTION + 1)
        );
        assert!(matches!(
            scan(&wasm(&control)),
            Err(VmError::SizeLimit {
                what: "function control operators",
                ..
            })
        ));
        let functions = format!("(module {})", "(func)".repeat(MAX_FUNCTIONS + 1));
        assert!(matches!(
            scan(&wasm(&functions)),
            Err(VmError::SizeLimit {
                what: "functions",
                ..
            })
        ));
        // Every truncation is an answer, never a panic or a read past the end.
        let good = wasm("(module (func (block (nop))))");
        assert!(scan(&good).is_ok());
        for len in 0..good.len() {
            let _ = scan(&good[..len]);
        }
        assert!(scan(&good[..good.len() - 1]).is_err());
        // A SIMD operator is not an enabled proposal.
        assert!(
            scan(&[b"\0asm\x01\0\0\0".as_slice(), &[10, 5, 1, 3, 0, 0xfd, 0x0c]].concat()).is_err()
        );
    }
}
