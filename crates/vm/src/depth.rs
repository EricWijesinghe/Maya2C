//! A call-depth bound that is the same on every validator (re-freeze R-6).
//!
//! Without it, the only thing that stops recursion is wasmtime's native
//! stack check (`max_wasm_stack`). Where that fires is a byte count over
//! native frames, and frame sizes belong to the binary: its target triple,
//! its ABI and the host frames between guest frames. Measured on one CPU, the
//! deepest completing recursion of one shape was 10,915 in a Windows release
//! build and 10,919 in a Linux one (ADR-142 amendment 6), so a call recursing
//! to a depth between the two applied on one validator and failed on the
//! other: two state roots for one block.
//!
//! So every module is rewritten before it compiles, as the stack limiter in
//! parity's `wasm-instrument` does for Substrate's contracts pallet: a
//! mutable `i32` global counts the *slots* every live guest frame may hold,
//! and a function whose entry would pass [`DEPTH_LIMIT_SLOTS`] traps with
//! `unreachable` before its body runs. The count is a function of the wasm
//! alone, so every binary traps at the same call.
//!
//! A frame's slots are [`FRAME_SLOTS`] plus its params, its declared locals
//! and the most operand-stack values its body can hold at once: the values a
//! native frame may need to spill. The budget is chosen so that the deepest
//! chain of frames it admits stays far inside `max_wasm_stack` for every
//! frame shape measured (`tests/depth_limit_tests.rs`, ADR-142 amendment 9).
//!
//! What the rewrite does to each defined function:
//!
//! ```text
//! <locals>                      unchanged
//! depth += slots(f)             global.get, i32.const, i32.add, global.set
//! if depth > LIMIT: trap        global.get, i32.const, i32.gt_u, if, unreachable, end
//! block (result f's results)    so a branch to the function label lands here
//!   <body>                      `return` becomes `br` to this block
//! end
//! depth -= slots(f)             global.get, i32.const, i32.sub, global.set
//! end
//! ```
//!
//! A trap leaves the count wrong, which does not matter: a trapped call's
//! instance is never entered again. The added operators are metered like any
//! other, so every call pays 12 units of fuel for its own bookkeeping
//! (ADR-142 amendment 9 records the gas vectors this moved).

use crate::error::{Result, VmError};

/// Slots every guest frame may hold at once, over the whole call chain.
///
/// At [`FRAME_SLOTS`] a frame, the smallest function admits 4,096 frames.
/// The most native stack any measured shape used per slot times this budget
/// stays under a quarter of `max_wasm_stack` (ADR-142 amendment 9).
pub const DEPTH_LIMIT_SLOTS: u32 = 32_768;

/// Slots charged to every frame before its params, locals and operand
/// stack: the return address, the saved frame pointer, callee-saved
/// registers and the frame's fixed spill area.
pub const FRAME_SLOTS: u32 = 8;

/// Bumped whenever the rewrite changes, so a cached module compiled under an
/// older rewrite is never served (`crate::config::config_digest`).
pub const REWRITE_VERSION: &[u8] = b"maya-depth-v1";

/// `wasm` with the depth bound applied to every defined function.
///
/// # Errors
///
/// [`VmError::InvalidModule`] for bytes that are not a module this VM's
/// features can express (the shape scan refuses the same modules first).
pub fn instrument(wasm: &[u8]) -> Result<Vec<u8>> {
    let module = Module::read(wasm)?;
    let Some(code) = module.code else {
        return Ok(wasm.to_vec());
    };
    let counter = module.imported_globals + module.defined_globals;
    let mut wrappers = WrapperTypes::new(module.types.len());
    let mut code_out = Vec::with_capacity(code.len() + code.len() / 4);
    let mut reader = Reader::new(code);
    let bodies = reader.u32()?;
    uleb(&mut code_out, u64::from(bodies));
    for index in 0..bodies {
        let ty = module.defined_type(index)?;
        let size = reader.u32()? as usize;
        let body = reader.take(size)?;
        let rewritten = rewrite_body(body, &module, ty, counter, &mut wrappers)?;
        uleb(&mut code_out, rewritten.len() as u64);
        code_out.extend_from_slice(&rewritten);
    }
    Ok(module.emit(wasm, &code_out, &wrappers))
}

/// Each defined function's frame slots, in order: what a call into it adds
/// to the count. Public for the calibration tests.
///
/// # Errors
///
/// As [`instrument`].
pub fn frame_slots(wasm: &[u8]) -> Result<Vec<u32>> {
    let module = Module::read(wasm)?;
    let Some(code) = module.code else {
        return Ok(Vec::new());
    };
    let mut reader = Reader::new(code);
    let mut out = Vec::new();
    for index in 0..reader.u32()? {
        let ty = module.defined_type(index)?;
        let size = reader.u32()? as usize;
        let mut body = Reader::new(reader.take(size)?);
        let locals = read_locals(&mut body)?;
        let height = walk(&mut body, &module, ty, None)?;
        out.push(slots(&module.types[ty as usize], locals, height));
    }
    Ok(out)
}

/// A function type: its param count and its result types.
struct FuncType {
    params: u32,
    results: Vec<u8>,
}

/// What the rewrite needs from a module, and where its sections are.
struct Module<'a> {
    types: Vec<FuncType>,
    /// The type of each function, imported then defined.
    functions: Vec<u32>,
    imported_functions: u32,
    imported_globals: u32,
    defined_globals: u32,
    /// Each section's id and bytes, in order.
    sections: Vec<(u8, &'a [u8])>,
    code: Option<&'a [u8]>,
}

impl<'a> Module<'a> {
    fn read(wasm: &'a [u8]) -> Result<Self> {
        let mut reader = Reader::new(wasm);
        if reader.take(8)? != b"\0asm\x01\0\0\0" {
            return Err(invalid("not a version 1 wasm module"));
        }
        let mut module = Module {
            types: Vec::new(),
            functions: Vec::new(),
            imported_functions: 0,
            imported_globals: 0,
            defined_globals: 0,
            sections: Vec::new(),
            code: None,
        };
        while !reader.done() {
            let id = reader.byte()?;
            let size = reader.u32()? as usize;
            let payload = reader.take(size)?;
            module.sections.push((id, payload));
            let mut section = Reader::new(payload);
            match id {
                1 => module.types = read_types(&mut section)?,
                2 => module.read_imports(&mut section)?,
                3 => {
                    for _ in 0..section.u32()? {
                        module.functions.push(section.u32()?);
                    }
                }
                6 => module.defined_globals = section.u32()?,
                10 => module.code = Some(payload),
                _ => {}
            }
        }
        Ok(module)
    }

    fn read_imports(&mut self, section: &mut Reader<'_>) -> Result<()> {
        for _ in 0..section.u32()? {
            section.name()?;
            section.name()?;
            match section.byte()? {
                0 => {
                    self.functions.push(section.u32()?);
                    self.imported_functions += 1;
                }
                1 => {
                    section.byte()?;
                    section.limits()?;
                }
                2 => section.limits()?,
                3 => {
                    section.byte()?;
                    section.byte()?;
                    self.imported_globals += 1;
                }
                _ => return Err(invalid("unknown import kind")),
            }
        }
        Ok(())
    }

    fn defined_type(&self, index: u32) -> Result<u32> {
        let ty = index
            .checked_add(self.imported_functions)
            .and_then(|i| self.functions.get(i as usize))
            .copied()
            .ok_or_else(|| invalid("function index out of range"))?;
        self.func_type(ty).map(|_| ty)
    }

    fn func_type(&self, ty: u32) -> Result<&FuncType> {
        self.types
            .get(ty as usize)
            .ok_or_else(|| invalid("type index out of range"))
    }

    fn callee(&self, function: u32) -> Result<&FuncType> {
        let ty = *self
            .functions
            .get(function as usize)
            .ok_or_else(|| invalid("function index out of range"))?;
        self.func_type(ty)
    }

    /// The module again, with the wrapper types appended to the type
    /// section, the counter appended to (or a section for it inserted
    /// before) the globals, and the rewritten code.
    fn emit(&self, wasm: &[u8], code: &[u8], wrappers: &WrapperTypes) -> Vec<u8> {
        let mut out = Vec::with_capacity(wasm.len() + code.len());
        out.extend_from_slice(&wasm[..8]);
        let mut globals_written = false;
        for (id, payload) in &self.sections {
            if !globals_written && *id != 0 && order(*id) > order(6) {
                if self.defined_globals == 0 {
                    section(&mut out, 6, &counter_global(None));
                }
                globals_written = true;
            }
            match id {
                1 => section(&mut out, 1, &wrappers.append_to(payload)),
                6 => {
                    section(&mut out, 6, &counter_global(Some(payload)));
                    globals_written = true;
                }
                10 => section(&mut out, 10, code),
                _ => section(&mut out, *id, payload),
            }
        }
        out
    }
}

/// Where a section sits in a module (the binary format fixes the order):
/// data count (12) comes after elements and before code.
fn order(id: u8) -> u8 {
    match id {
        12 => 10,
        10 | 11 => id + 1,
        _ => id,
    }
}

/// The global section with the counter (`mut i32 = 0`) appended.
fn counter_global(existing: Option<&[u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    let (count, rest) = match existing {
        Some(payload) => {
            let mut reader = Reader::new(payload);
            // Read by `Module::read` already, so it parses.
            let count = reader.u32().unwrap_or(0);
            (count, &payload[reader.at..])
        }
        None => (0, &[][..]),
    };
    uleb(&mut out, u64::from(count) + 1);
    out.extend_from_slice(rest);
    out.extend_from_slice(&[0x7f, 0x01, 0x41, 0x00, 0x0b]);
    out
}

/// Types `[] -> results` for wrapper blocks of functions with more than one
/// result, appended after the module's own types.
struct WrapperTypes {
    first: usize,
    added: Vec<Vec<u8>>,
}

impl WrapperTypes {
    fn new(first: usize) -> Self {
        Self {
            first,
            added: Vec::new(),
        }
    }

    /// The block type a wrapper returning `results` uses.
    fn block_type(&mut self, results: &[u8], out: &mut Vec<u8>) {
        match results {
            [] => out.push(0x40),
            [one] => out.push(*one),
            many => {
                let at = match self.added.iter().position(|r| r == many) {
                    Some(at) => at,
                    None => {
                        self.added.push(many.to_vec());
                        self.added.len() - 1
                    }
                };
                // A type index is an s33; non-negative, so its LEB is the
                // unsigned one as long as bit 6 of the last byte is clear,
                // which `sleb` guarantees.
                sleb(out, (self.first + at) as i64);
            }
        }
    }

    fn append_to(&self, payload: &[u8]) -> Vec<u8> {
        let mut reader = Reader::new(payload);
        let count = reader.u32().unwrap_or(0);
        let mut out = Vec::new();
        uleb(&mut out, u64::from(count) + self.added.len() as u64);
        out.extend_from_slice(&payload[reader.at..]);
        for results in &self.added {
            out.extend_from_slice(&[0x60, 0x00]);
            uleb(&mut out, results.len() as u64);
            out.extend_from_slice(results);
        }
        out
    }
}

fn read_types(section: &mut Reader<'_>) -> Result<Vec<FuncType>> {
    let mut types = Vec::new();
    for _ in 0..section.u32()? {
        if section.byte()? != 0x60 {
            return Err(invalid("type outside the enabled proposals"));
        }
        let params = section.u32()?;
        section.take(params as usize)?;
        let results = section.u32()? as usize;
        types.push(FuncType {
            params,
            results: section.take(results)?.to_vec(),
        });
    }
    Ok(types)
}

/// Declared locals, leaving `body` at the first operator.
fn read_locals(body: &mut Reader<'_>) -> Result<u64> {
    let mut locals: u64 = 0;
    for _ in 0..body.u32()? {
        locals = locals.saturating_add(u64::from(body.u32()?));
        body.byte()?;
    }
    Ok(locals)
}

fn slots(ty: &FuncType, locals: u64, height: u64) -> u32 {
    let total = u64::from(FRAME_SLOTS)
        .saturating_add(u64::from(ty.params))
        .saturating_add(locals)
        // The prologue holds two values before the body runs.
        .saturating_add(height.max(2));
    u32::try_from(total).unwrap_or(u32::MAX)
}

fn rewrite_body(
    body: &[u8],
    module: &Module<'_>,
    ty: u32,
    counter: u32,
    wrappers: &mut WrapperTypes,
) -> Result<Vec<u8>> {
    let mut reader = Reader::new(body);
    let locals = read_locals(&mut reader)?;
    let operators_at = reader.at;
    let height = walk(&mut reader.clone(), module, ty, None)?;
    let func = module.func_type(ty)?;
    // Past the limit the entry traps anyway; capped so the immediate and
    // the counter stay far inside i32.
    let weight = slots(func, locals, height).min(DEPTH_LIMIT_SLOTS + 1);

    let mut out = Vec::with_capacity(body.len() + 48);
    out.extend_from_slice(&body[..operators_at]);
    adjust(&mut out, counter, weight, 0x6a);
    out.push(0x23);
    uleb(&mut out, u64::from(counter));
    out.push(0x41);
    sleb(&mut out, i64::from(DEPTH_LIMIT_SLOTS));
    // i32.gt_u; if (empty) unreachable end
    out.extend_from_slice(&[0x4b, 0x04, 0x40, 0x00, 0x0b]);
    out.push(0x02);
    wrappers.block_type(&func.results, &mut out);
    walk(&mut reader, module, ty, Some(&mut out))?;
    // `walk` wrote the body up to and without its final `end`: close the
    // wrapper, give the slots back, end the function.
    out.push(0x0b);
    adjust(&mut out, counter, weight, 0x6b);
    out.push(0x0b);
    Ok(out)
}

/// `global.get counter; i32.const weight; <op>; global.set counter`.
fn adjust(out: &mut Vec<u8>, counter: u32, weight: u32, op: u8) {
    out.push(0x23);
    uleb(out, u64::from(counter));
    out.push(0x41);
    sleb(out, i64::from(weight));
    out.push(op);
    out.push(0x24);
    uleb(out, u64::from(counter));
}

/// One open `block`, `loop`, `if` or the function itself.
struct Frame {
    base: u64,
    params: u64,
    results: u64,
}

/// Walks one body's operators, returning the most operand-stack values it
/// can hold at once (an upper bound: unreachable code is counted from its
/// block's base). With `out`, also copies the operators there, `return`
/// rewritten to a branch to the wrapper and the final `end` left out.
fn walk(
    body: &mut Reader<'_>,
    module: &Module<'_>,
    ty: u32,
    mut out: Option<&mut Vec<u8>>,
) -> Result<u64> {
    let own = module.func_type(ty)?;
    let mut frames = vec![Frame {
        base: 0,
        params: 0,
        results: own.results.len() as u64,
    }];
    let (mut height, mut max) = (0u64, 0u64);
    while !body.done() {
        let start = body.at;
        let op = body.byte()?;
        let base = frames.last().map_or(0, |f| f.base);
        let pop = |height: &mut u64, n: u64| *height = height.saturating_sub(n).max(base);
        match op {
            0x02..=0x04 => {
                let (params, results) = block_arity(body, module)?;
                if op == 0x04 {
                    pop(&mut height, 1);
                }
                pop(&mut height, params);
                frames.push(Frame {
                    base: height,
                    params,
                    results,
                });
                height += params;
            }
            0x05 => {
                let top = frames.last().ok_or_else(|| invalid("else outside if"))?;
                height = top.base + top.params;
            }
            0x0b => {
                let frame = frames.pop().ok_or_else(|| invalid("unbalanced end"))?;
                height = frame.base + frame.results;
                if frames.is_empty() {
                    if !body.done() {
                        return Err(invalid("operators after the function's end"));
                    }
                    return Ok(max);
                }
            }
            0x00 | 0x0c | 0x0f => {
                if op == 0x0c {
                    body.u32()?;
                }
                height = base;
            }
            0x0d => {
                body.u32()?;
                pop(&mut height, 1);
            }
            0x0e => {
                for _ in 0..=body.u32()? {
                    body.u32()?;
                }
                height = base;
            }
            0x10 => {
                let callee = module.callee(body.u32()?)?;
                pop(&mut height, u64::from(callee.params));
                height += callee.results.len() as u64;
            }
            0x11 => {
                let callee = module.func_type(body.u32()?)?;
                body.u32()?;
                pop(&mut height, u64::from(callee.params) + 1);
                height += callee.results.len() as u64;
            }
            _ => {
                let (pops, pushes) = effect(op, body)?;
                pop(&mut height, pops);
                height += pushes;
            }
        }
        max = max.max(height);
        if let Some(out) = out.as_deref_mut() {
            if op == 0x0f {
                // Branch to the wrapper: one label per open block above it.
                out.push(0x0c);
                uleb(out, (frames.len() - 1) as u64);
            } else {
                out.extend_from_slice(&body.bytes[start..body.at]);
            }
        }
    }
    Err(invalid("function body without its end"))
}

/// Params and results of a `block`, `loop` or `if` type.
fn block_arity(body: &mut Reader<'_>, module: &Module<'_>) -> Result<(u64, u64)> {
    match body.bytes.get(body.at) {
        Some(0x40) => body.byte().map(|_| (0, 0)),
        Some(0x6f..=0x7f) => body.byte().map(|_| (0, 1)),
        Some(byte) if byte & 0x40 == 0 => {
            let ty = module.func_type(body.u32()?)?;
            Ok((u64::from(ty.params), ty.results.len() as u64))
        }
        Some(_) => Err(invalid("block type outside the enabled proposals")),
        None => Err(invalid("truncated")),
    }
}

/// Values popped and pushed by an operator with no control effect, its
/// immediates skipped.
fn effect(op: u8, body: &mut Reader<'_>) -> Result<(u64, u64)> {
    Ok(match op {
        0x01 => (0, 0),
        0x1a => (1, 0),
        0x1b => (3, 1),
        0x20 | 0x23 => {
            body.u32()?;
            (0, 1)
        }
        0x21 | 0x24 => {
            body.u32()?;
            (1, 0)
        }
        0x22 => {
            body.u32()?;
            (1, 1)
        }
        0x28..=0x35 => {
            body.mem_arg()?;
            (1, 1)
        }
        0x36..=0x3e => {
            body.mem_arg()?;
            (2, 0)
        }
        0x3f => {
            body.u32()?;
            (0, 1)
        }
        0x40 => {
            body.u32()?;
            (1, 1)
        }
        0x41 | 0x42 => {
            body.leb()?;
            (0, 1)
        }
        0x43 => {
            body.take(4)?;
            (0, 1)
        }
        0x44 => {
            body.take(8)?;
            (0, 1)
        }
        // Unary: eqz, clz/ctz/popcnt, float unary ops, conversions, sign
        // extension.
        0x45 | 0x50 | 0x67..=0x69 | 0x79..=0x7b | 0x8b..=0x91 | 0x99..=0x9f | 0xa7..=0xc4 => {
            (1, 1)
        }
        // Binary: comparisons and arithmetic.
        0x46..=0x4f | 0x51..=0x66 | 0x6a..=0x78 | 0x7c..=0x8a | 0x92..=0x98 | 0xa0..=0xa6 => {
            (2, 1)
        }
        0xfc => match body.u32()? {
            0..=7 => (1, 1),
            8 | 10 | 12 | 14 => {
                body.u32()?;
                body.u32()?;
                (3, 0)
            }
            11 => {
                body.u32()?;
                (3, 0)
            }
            9 | 13 => {
                body.u32()?;
                (0, 0)
            }
            _ => return Err(invalid("operator outside the enabled proposals")),
        },
        _ => return Err(invalid("operator outside the enabled proposals")),
    })
}

fn section(out: &mut Vec<u8>, id: u8, payload: &[u8]) {
    out.push(id);
    uleb(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

fn uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn sleb(out: &mut Vec<u8>, mut value: i64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        if done {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn invalid(what: &str) -> VmError {
    VmError::InvalidModule(format!("depth bound: {what}"))
}

/// A bounds-checked cursor over module bytes.
#[derive(Clone)]
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn wasm(text: &str) -> Vec<u8> {
        wat::parse_str(text).unwrap()
    }

    #[test]
    fn frame_slots_count_params_locals_and_the_deepest_stack() {
        let code = wasm(
            r#"(module
              (func (param i64 i64) (result i64) (local i32 i32 i32)
                (i64.add (i64.add (local.get 0) (local.get 1)) (i64.const 1))))"#,
        );
        // 8 + 2 params + 3 locals + a stack of 2.
        assert_eq!(frame_slots(&code).unwrap(), vec![15]);
    }

    #[test]
    fn the_rewrite_validates_with_and_without_a_global_section() {
        for text in [
            r#"(module (memory 1) (func (export "f") (result i32) (i32.const 1) (return)))"#,
            r#"(module (global (mut i64) (i64.const 0)) (data "x") (memory 1)
               (func (result i32 i64) (i32.const 1) (i64.const 2) (br 0)))"#,
            r#"(module (import "env" "g" (global i32)) (import "env" "h" (func))
               (func (call 0) (block (br_if 1 (i32.const 0)))))"#,
        ] {
            let rewritten = instrument(&wasm(text)).unwrap();
            let engine = crate::config::deterministic_engine().unwrap();
            wasmtime::Module::validate(&engine, &rewritten).unwrap();
        }
    }

    #[test]
    fn truncated_bytes_are_refused_not_a_panic() {
        let code = wasm(r#"(module (func (result i32) (i32.const 7)))"#);
        for len in 0..code.len() {
            // Some prefixes end on a section boundary and are modules; the
            // rest must come back as errors. Neither may panic.
            let _ = instrument(&code[..len]);
        }
        assert!(instrument(&code[..code.len() - 1]).is_err());
    }
}
