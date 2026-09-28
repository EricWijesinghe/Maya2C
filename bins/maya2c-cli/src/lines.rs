//! Source lines for the debugger: a contract's DWARF line table, read from
//! its wasm custom sections, mapping a call site (a wasm module offset, as
//! [`maya_vm::trace::Timeline::sites`] records it) to `file:line`.
//!
//! In wasm, DWARF addresses are offsets from the start of the code section's
//! contents, while a call site is an offset from the start of the module, so
//! the code section's position is subtracted first. A contract built without
//! debug info has no `.debug_line`, and every lookup is `None`: the debugger
//! then shows host calls only, as before.

use std::borrow::Cow;
use std::collections::BTreeMap;

use gimli::{EndianSlice, LittleEndian};

/// A source position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Source file path as the compiler recorded it.
    pub file: String,
    /// 1-based line.
    pub line: u64,
}

impl Line {
    /// The file's last path component, whichever separator it was written
    /// with.
    #[must_use]
    pub fn file_name(&self) -> &str {
        std::path::Path::new(&self.file)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.file)
    }
}

/// A contract's address-to-line table.
#[derive(Clone, Debug, Default)]
pub struct LineTable {
    code_start: usize,
    /// Code-section address to position, for every row of every sequence.
    /// A sequence's end is `None`, so an address past the last instruction
    /// of one function and before the next maps to nothing rather than to
    /// the previous function's last line.
    rows: BTreeMap<u64, Option<Line>>,
}

/// Custom sections by name, and where the code section's contents begin.
fn sections(wasm: &[u8]) -> anyhow::Result<(BTreeMap<String, &[u8]>, usize)> {
    let mut custom = BTreeMap::new();
    let mut code_start = 0;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload? {
            wasmparser::Payload::CustomSection(c) => {
                custom.insert(c.name().to_owned(), c.data());
            }
            wasmparser::Payload::CodeSectionStart { range, .. } => {
                code_start = usize::try_from(range.start)
                    .map_err(|_| anyhow::anyhow!("code section offset"))?;
            }
            _ => {}
        }
    }
    Ok((custom, code_start))
}

impl LineTable {
    /// Reads the line table of `wasm`; empty when it carries no DWARF.
    ///
    /// # Errors
    ///
    /// A malformed module or malformed DWARF.
    pub fn read(wasm: &[u8]) -> anyhow::Result<Self> {
        let (custom, code_start) = sections(wasm)?;
        if !custom.contains_key(".debug_line") {
            return Ok(Self {
                code_start,
                rows: BTreeMap::new(),
            });
        }
        let load = |id: gimli::SectionId| -> Result<Cow<'_, [u8]>, gimli::Error> {
            Ok(Cow::Borrowed(custom.get(id.name()).copied().unwrap_or(&[])))
        };
        let owned = gimli::DwarfSections::load(load)?;
        let dwarf = owned.borrow(|section| EndianSlice::new(section, LittleEndian));
        let mut rows = BTreeMap::new();
        let mut units = dwarf.units();
        while let Some(header) = units.next()? {
            let unit = dwarf.unit(header)?;
            let Some(program) = unit.line_program.clone() else {
                continue;
            };
            let mut program_rows = program.rows();
            while let Some((header, row)) = program_rows.next_row()? {
                if row.end_sequence() {
                    // Only where no sequence starts at the same address.
                    rows.entry(row.address()).or_insert(None);
                    continue;
                }
                let (Some(file), Some(line)) = (row.file(header), row.line()) else {
                    continue;
                };
                let mut path = String::new();
                if let Some(dir) = file.directory(header) {
                    path.push_str(&dwarf.attr_string(&unit, dir)?.to_string_lossy());
                    path.push('/');
                }
                path.push_str(
                    &dwarf
                        .attr_string(&unit, file.path_name())?
                        .to_string_lossy(),
                );
                rows.insert(
                    row.address(),
                    Some(Line {
                        file: path,
                        line: line.get(),
                    }),
                );
            }
        }
        Ok(Self { code_start, rows })
    }

    /// Whether the contract carried a line table.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The source position of a call site (a module offset).
    #[must_use]
    pub fn at(&self, site: usize) -> Option<&Line> {
        let address = u64::try_from(site.checked_sub(self.code_start)?).ok()?;
        self.rows
            .range(..=address)
            .next_back()
            .and_then(|(_, line)| line.as_ref())
    }
}
