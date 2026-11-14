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
//!
//! Multi-value is the one enabled proposal whose cost is not one operator's:
//! a block or a call carries as many values as its type names, and
//! wasmparser allows a thousand params and a thousand results. The PR #549
//! review measured 1,500 `block (type [1000]->[1000])` compiling in 8.2 s for
//! a price of 49.6 M fuel (166 ns a unit). So a function type may name at
//! most [`MAX_TYPE_ARITY`] values, and every typed block and every call is
//! priced by the values it carries ([`ModuleShape::arity`]).

use crate::error::{Result, VmError};

/// Most functions, imported and defined, one module may hold.
pub const MAX_FUNCTIONS: usize = 10_000;

/// Most locals one function may declare (wasmparser allows 50,000).
pub const MAX_LOCALS_PER_FUNCTION: usize = 1_024;

/// Most params and results, together, one function type may name
/// (wasmparser allows 1,000 of each). Contracts compiled from Rust use a
/// handful; the review found compile time per carried value growing with the
/// count well before a thousand.
pub const MAX_TYPE_ARITY: usize = 16;

/// Most `call` and `call_indirect` operators one function may hold. Calls
/// compiled linearly up to 10,000 in one function; the review measured
/// 20,000 slower per call.
pub const MAX_CALLS_PER_FUNCTION: usize = 10_000;

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
    /// Values carried, over every function: the params and results of each
    /// defined function's own type, of each `block` and `if` (one for a
    /// single-result block type), and of each call's callee type.
    pub arity: u64,
    /// Values carried by `loop`s, counted as for `arity`. Priced apart: a
    /// loop's params are fed again at every back edge, and their compile
    /// time grows with the loops in the function, not only with the values.
    pub loop_arity: u64,
}

/// Params plus results of each function type, and the type of each function,
/// imported then defined: what a typed block or a call carries.
#[derive(Default)]
struct Types {
    arity: Vec<u32>,
    functions: Vec<u32>,
}

impl Types {
    fn of_type(&self, index: u32) -> Result<u64> {
        self.arity
            .get(index as usize)
            .map(|a| u64::from(*a))
            .ok_or_else(|| invalid("type index out of range"))
    }

    fn of_function(&self, index: u32) -> Result<u64> {
        let ty = *self
            .functions
            .get(index as usize)
            .ok_or_else(|| invalid("function index out of range"))?;
        self.of_type(ty)
    }
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
    let mut types = Types::default();
    // Where the defined functions start in `types.functions`: each body's
    // own type is found from it.
    let mut first_defined = 0usize;
    while !reader.done() {
        let id = reader.byte()?;
        let size = reader.u32()? as usize;
        let mut section = Reader::new(reader.take(size)?);
        match id {
            1 => type_section(&mut section, &mut types)?,
            2 => imports(&mut section, &mut shape, &mut types)?,
            3 => {
                let defined = section.u32()?;
                shape.functions = shape.functions.saturating_add(u64::from(defined));
                if shape.functions > MAX_FUNCTIONS as u64 {
                    return Err(too_many("functions", shape.functions, MAX_FUNCTIONS));
                }
                first_defined = types.functions.len();
                for _ in 0..defined {
                    let ty = section.u32()?;
                    types.of_type(ty)?;
                    types.functions.push(ty);
                }
            }
            10 => code(&mut section, &mut shape, &types, first_defined)?,
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

/// Every function type's arity, refusing one past [`MAX_TYPE_ARITY`] and any
/// type form or value type outside the enabled proposals.
fn type_section(section: &mut Reader<'_>, types: &mut Types) -> Result<()> {
    for _ in 0..section.u32()? {
        if section.byte()? != 0x60 {
            return Err(invalid("type outside the enabled proposals"));
        }
        let mut arity: u64 = 0;
        // Params, then results.
        for _ in 0..2 {
            let count = section.u32()?;
            arity = arity.saturating_add(u64::from(count));
            if arity > MAX_TYPE_ARITY as u64 {
                return Err(too_many(
                    "function type params and results",
                    arity,
                    MAX_TYPE_ARITY,
                ));
            }
            for _ in 0..count {
                // f64, f32, i64, i32: SIMD and reference types are off.
                if !matches!(section.byte()?, 0x7c..=0x7f) {
                    return Err(invalid("value type outside the enabled proposals"));
                }
            }
        }
        // At most MAX_TYPE_ARITY, checked above.
        types.arity.push(u32::try_from(arity).unwrap_or(u32::MAX));
    }
    Ok(())
}

/// Counts imported functions and records their types; skips the other
/// import kinds.
fn imports(section: &mut Reader<'_>, shape: &mut ModuleShape, types: &mut Types) -> Result<()> {
    for _ in 0..section.u32()? {
        section.name()?;
        section.name()?;
        match section.byte()? {
            0 => {
                let ty = section.u32()?;
                types.of_type(ty)?;
                types.functions.push(ty);
                shape.functions = shape.functions.saturating_add(1);
                if shape.functions > MAX_FUNCTIONS as u64 {
                    return Err(too_many("functions", shape.functions, MAX_FUNCTIONS));
                }
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
fn code(
    section: &mut Reader<'_>,
    shape: &mut ModuleShape,
    types: &Types,
    first_defined: usize,
) -> Result<()> {
    for index in 0..section.u32()? {
        let own = first_defined
            .checked_add(index as usize)
            .and_then(|i| u32::try_from(i).ok())
            .ok_or_else(|| invalid("function index out of range"))?;
        shape.arity = shape.arity.saturating_add(types.of_function(own)?);
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
        let ops = operators(&mut body, types)?;
        if ops.controls > MAX_CONTROL_PER_FUNCTION as u64 {
            return Err(too_many(
                "function control operators",
                ops.controls,
                MAX_CONTROL_PER_FUNCTION,
            ));
        }
        if ops.calls > MAX_CALLS_PER_FUNCTION as u64 {
            return Err(too_many(
                "function calls",
                ops.calls,
                MAX_CALLS_PER_FUNCTION,
            ));
        }
        shape.locals = shape.locals.saturating_add(locals);
        shape.controls = shape.controls.saturating_add(ops.controls);
        shape.calls = shape.calls.saturating_add(ops.calls);
        shape.arity = shape.arity.saturating_add(ops.arity);
        shape.loop_arity = shape.loop_arity.saturating_add(ops.loop_arity);
    }
    Ok(())
}

/// What one body's operators count.
#[derive(Default)]
struct Operators {
    controls: u64,
    calls: u64,
    arity: u64,
    loop_arity: u64,
}

/// One body's control operators, calls and carried values, skipping every
/// other immediate.
fn operators(body: &mut Reader<'_>, types: &Types) -> Result<Operators> {
    let mut ops = Operators::default();
    while !body.done() {
        match body.byte()? {
            // unreachable, nop, end, drop, select, numeric and conversion
            // operators, sign extension: no immediate.
            0x00 | 0x01 | 0x0b | 0x1a | 0x1b | 0x45..=0xc4 => {}
            op @ 0x02..=0x04 => {
                let carried = match body.block_type()? {
                    BlockType::Empty => 0,
                    BlockType::Value => 1,
                    BlockType::Type(index) => types.of_type(index)?,
                };
                if op == 0x03 {
                    ops.loop_arity = ops.loop_arity.saturating_add(carried);
                } else {
                    ops.arity = ops.arity.saturating_add(carried);
                }
                ops.controls += 1;
            }
            0x05 | 0x0f => ops.controls += 1,
            0x0c | 0x0d => {
                body.u32()?;
                ops.controls += 1;
            }
            0x10 => {
                let callee = body.u32()?;
                ops.arity = ops.arity.saturating_add(types.of_function(callee)?);
                ops.calls += 1;
            }
            // One control however many targets: a 200,000-target table
            // measured 17 ns a byte, far inside the byte price.
            0x0e => {
                let targets = body.u32()?;
                for _ in 0..=targets {
                    body.u32()?;
                }
                ops.controls += 1;
            }
            0x11 => {
                let ty = body.u32()?;
                body.u32()?;
                ops.arity = ops.arity.saturating_add(types.of_type(ty)?);
                ops.calls += 1;
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
    Ok(ops)
}

/// What a `block`, `loop` or `if` carries.
enum BlockType {
    Empty,
    Value,
    Type(u32),
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

    /// `0x40`, one value type, or a type index (an s33 that must be
    /// non-negative: a first byte with the sign bit 0x40 clear, which reads
    /// the same as an unsigned LEB128).
    fn block_type(&mut self) -> Result<BlockType> {
        match self.bytes.get(self.at) {
            Some(0x40) => self.byte().map(|_| BlockType::Empty),
            Some(0x6f..=0x7f) => self.byte().map(|_| BlockType::Value),
            Some(byte) if byte & 0x40 == 0 => self.u32().map(BlockType::Type),
            Some(_) => Err(invalid("block type outside the enabled proposals")),
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
                // The defined function's own (i32) -> (i64).
                arity: 2,
                loop_arity: 0,
            }
        );
    }

    #[test]
    fn typed_blocks_and_calls_are_counted_by_the_values_they_carry() {
        let shape = scan(&wasm(
            r#"(module
                (type $t (func (param i32 i32) (result i32 i32 i32)))
                (import "env" "f" (func $f (type $t)))
                (table 1 funcref)
                (func (result i32)
                  (drop (drop (drop (call $f (i32.const 0) (i32.const 0)))))
                  (drop (drop (drop (call_indirect (type $t) (i32.const 0) (i32.const 0) (i32.const 0)))))
                  (block (result i32) (i32.const 0))
                  (drop)
                  (i32.const 0) (i32.const 0)
                  (block (type $t) (param i32 i32) (result i32 i32 i32) (i32.const 0))
                  (loop (param i32) (result i32))
                  (drop) (drop) (drop)
                  (i32.const 7)))"#,
        ))
        .unwrap();
        // own () -> i32: 1; call $f: 5; call_indirect $t: 5; block (result
        // i32): 1; block (type $t): 5.
        assert_eq!(shape.arity, 17);
        // loop (param i32) (result i32): priced apart.
        assert_eq!(shape.loop_arity, 2);
        assert_eq!(shape.calls, 2);
        assert_eq!(shape.controls, 3);
    }

    #[test]
    fn a_function_type_past_the_arity_bound_is_refused() {
        // The review's bomb: [1000] -> [1000]. Refused at the type, before
        // any body is read.
        let wide = |params: usize, results: usize| {
            wasm(&format!(
                "(module (type (func (param {}) (result {}))))",
                "i32 ".repeat(params),
                "i32 ".repeat(results)
            ))
        };
        let refused = |code: &[u8]| {
            matches!(
                scan(code),
                Err(VmError::SizeLimit {
                    what: "function type params and results",
                    ..
                })
            )
        };
        assert!(refused(&wide(1000, 1000)));
        assert!(refused(&wide(MAX_TYPE_ARITY, 1)));
        assert!(refused(&wide(1, MAX_TYPE_ARITY)));
        assert!(scan(&wide(MAX_TYPE_ARITY / 2, MAX_TYPE_ARITY / 2)).is_ok());
        assert!(scan(&wide(MAX_TYPE_ARITY, 0)).is_ok());
        // A reference type in a signature is a disabled proposal.
        assert!(scan(&wasm("(module (type (func (param externref))))")).is_err());
    }

    #[test]
    fn calls_past_the_per_function_bound_are_refused() {
        let calls = |n: usize| {
            wasm(&format!(
                "(module (func $g) (func {}))",
                "(call $g)".repeat(n)
            ))
        };
        assert!(scan(&calls(MAX_CALLS_PER_FUNCTION)).is_ok());
        assert!(matches!(
            scan(&calls(MAX_CALLS_PER_FUNCTION + 1)),
            Err(VmError::SizeLimit {
                what: "function calls",
                ..
            })
        ));
    }

    #[test]
    fn an_index_the_module_does_not_define_is_refused() {
        // A body whose block names type 5 in a module with one type: the
        // validator refuses it too, and the scan must not price it as zero.
        let mut code = wasm("(module (func (block)))");
        let at = code
            .windows(2)
            .rposition(|w| w == [0x02, 0x40])
            .expect("the block");
        code[at + 1] = 0x05;
        assert!(scan(&code).is_err());
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
