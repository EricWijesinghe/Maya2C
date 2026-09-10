//! Generates a LaTeX technical reference from the workspace's module docs.
//!
//! ```text
//! cargo run -p maya-docgen -- --out docs/reference.tex
//! ```
//!
//! # This produces a reference, not a whitepaper
//!
//! Worth being plain about, because the two get conflated and the difference
//! matters to whoever reads the output expecting one and getting the other.
//!
//! A whitepaper argues. It states a problem, proposes a design, and defends the
//! choices against alternatives — and it is *written*, by someone who decided
//! what to leave out. This tool scrapes `//!` module documentation and arranges
//! it. What comes out is a thorough, accurate, and genuinely useful technical
//! reference. It is not an argument, and calling it a whitepaper would oversell
//! it to exactly the audience least able to tell.
//!
//! # It marks research branches rather than laundering them
//!
//! Several modules in this workspace are shipped deliberately disabled —
//! `lattice-pow`, `blockgraph`, the dual-KEM transport. Their own documentation
//! says so in a `# Status` section. A generator that flattened those into
//! prose alongside the consensus rules would produce a document describing a
//! chain that does not exist.
//!
//! So [`Status`] is detected from the module's own words, and a disabled module
//! is rendered with a banner. The generator does not decide what is shipped —
//! it repeats what the module already says, which is the only claim it is
//! entitled to make.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Crates whose documentation goes into the reference, in reading order.
///
/// An explicit list rather than a directory walk. A walk would silently pick up
/// whatever crate somebody added next, and the order of a reference is part of
/// what makes it readable — consensus before the things layered on it.
const CRATES: &[(&str, &str)] = &[
    ("src", "The node"),
    ("ledger-math", "Ledger arithmetic"),
    ("crypto-pq", "Post-quantum primitives"),
    ("vrf", "Verifiable random function"),
    ("zk-privacy", "Shielded pool"),
    ("vm", "Contract execution"),
    ("dex", "Trading engine"),
    ("governance", "Governance"),
    ("mev", "Threshold-encrypted mempool"),
    ("l2-flash", "Payment channels"),
    ("light-client", "SPV light client"),
    ("stratum-v2", "Pool protocol"),
    ("pool-service", "Pool daemon"),
    ("lattice-pow", "Lattice proof of work"),
    ("blockgraph", "Batch references and shard scheduling"),
    ("api-gateway", "API gateway"),
    ("sdk-ffi", "Language bindings"),
    ("sdk-wasm", "Browser bindings"),
];

/// Lines of a doc block treated as its opening claim, when there is no
/// `# Status` section.
///
/// Four is enough for a module that leads with "**Research branch.**" and
/// short enough that a paragraph two screens down about a different subsystem
/// cannot reach it.
const OPENING_LINES: usize = 4;

/// Whether a module describes something the chain actually runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// Part of the running chain.
    Shipped,
    /// Present in the tree, deliberately not reachable by consensus.
    ResearchBranch,
    /// Present and reachable, but off unless an operator opts in.
    OptIn,
}

impl Status {
    /// Reads the status out of the module's own documentation.
    ///
    /// Detected, not decided. The generator has no independent knowledge of
    /// what is enabled, and inventing one would mean two places to keep in
    /// sync — with the document being the one nobody updates.
    ///
    /// # Why only the status region is searched
    ///
    /// Scanning the whole doc block produced a false positive immediately.
    /// `api-gateway/src/graphql.rs` is a shipped module whose documentation
    /// explains what it deliberately does *not* expose, and in doing so says
    /// "the block-graph work in `maya-blockgraph` is a research branch that no
    /// consensus path reaches". A whole-document scan labelled that module a
    /// research branch — which would have told a reader the gateway's GraphQL
    /// layer was not real.
    ///
    /// A module states its own status in a `# Status` section, or in its
    /// opening lines. Prose further down is discussing something else, and a
    /// generator that cannot tell the difference between a module's claim
    /// about itself and its description of a neighbour will mislabel both.
    fn detect(doc: &str) -> Self {
        let region = Self::status_region(doc).to_lowercase();
        if region.contains("research branch")
            || region.contains("no consensus path reaches")
            || region.contains("nothing in consensus calls this")
        {
            Status::ResearchBranch
        } else if region.contains("off by default") || region.contains("shipped disabled") {
            Status::OptIn
        } else {
            Status::Shipped
        }
    }

    /// The part of a doc block in which a module speaks about itself.
    ///
    /// The `# Status` section if there is one, otherwise the opening lines.
    fn status_region(doc: &str) -> String {
        let mut lines = doc.lines().peekable();
        let mut opening = Vec::new();

        while let Some(line) = lines.next() {
            let trimmed = line.trim();
            if trimmed.eq_ignore_ascii_case("# Status") {
                // Everything until the next heading at the same level.
                let mut section = Vec::new();
                for following in lines.by_ref() {
                    if following.trim_start().starts_with("# ") {
                        break;
                    }
                    section.push(following);
                }
                return section.join("\n");
            }
            if opening.len() < OPENING_LINES {
                opening.push(line);
            }
        }

        opening.join("\n")
    }

    /// The banner printed above a module's section, if it needs one.
    fn banner(self) -> Option<&'static str> {
        match self {
            Status::Shipped => None,
            Status::ResearchBranch => Some(
                "This module is a research branch. It is present in the tree and \
                 no consensus path reaches it. Nothing described here is part of \
                 the running chain.",
            ),
            Status::OptIn => Some(
                "This module ships disabled. It is reachable only when an operator \
                 opts in, and a stock node behaves as though it were absent.",
            ),
        }
    }
}

/// One module's extracted documentation.
struct Module {
    /// Path relative to the crate root, e.g. `consensus/chain.rs`.
    path: String,
    /// The `//!` block, with the markers stripped.
    doc: String,
    /// What the module says about its own status.
    status: Status,
}

/// Extracts the leading `//!` block from a Rust source file.
///
/// Stops at the first line that is not an inner doc comment and not blank,
/// because everything after that is code. A file with no module documentation
/// yields `None` and is skipped rather than contributing an empty section.
fn extract_module_doc(source: &str) -> Option<String> {
    let mut lines = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("//!") {
            lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if trimmed.is_empty() && lines.is_empty() {
            continue;
        } else {
            break;
        }
    }

    let doc = lines.join("\n");
    if doc.trim().is_empty() {
        None
    } else {
        Some(doc)
    }
}

/// Escapes the characters LaTeX treats as syntax.
///
/// # Why this is its own function with its own tests
///
/// It is the part most likely to be silently wrong. An unescaped `_` in an
/// identifier — and this codebase is full of `snake_case` — produces either a
/// compile error or, worse, a subscript that renders as something else
/// entirely. A document that builds and says the wrong thing is harder to
/// notice than one that does not build.
fn escape_latex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str(r"\textbackslash{}"),
            '{' => out.push_str(r"\{"),
            '}' => out.push_str(r"\}"),
            '$' => out.push_str(r"\$"),
            '&' => out.push_str(r"\&"),
            '#' => out.push_str(r"\#"),
            '^' => out.push_str(r"\textasciicircum{}"),
            '_' => out.push_str(r"\_"),
            '~' => out.push_str(r"\textasciitilde{}"),
            '%' => out.push_str(r"\%"),
            other => out.push(other),
        }
    }
    out
}

/// Converts one module's markdown-ish doc block into LaTeX.
///
/// Deliberately handles a small, known subset: headings, fenced code, inline
/// code, bold, and paragraphs. A general markdown engine would be a
/// dependency, and the input is not arbitrary markdown — it is this
/// workspace's doc comments, which use a narrow and consistent style.
fn doc_to_latex(doc: &str) -> String {
    let mut out = String::new();
    let mut in_code = false;
    let mut in_table = false;

    for line in doc.lines() {
        let trimmed = line.trim_end();

        if trimmed.starts_with("```") {
            if in_code {
                out.push_str("\\end{verbatim}\n");
            } else {
                out.push_str("\\begin{verbatim}\n");
            }
            in_code = !in_code;
            continue;
        }

        if in_code {
            // Verbatim: no escaping, by definition.
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }

        // Markdown tables are common in these docs and have no cheap LaTeX
        // equivalent that survives arbitrary column counts. Rendered verbatim,
        // which keeps them readable and keeps the generator honest about what
        // it can do.
        let is_table_row = trimmed.starts_with('|');
        if is_table_row && !in_table {
            out.push_str("\\begin{verbatim}\n");
            in_table = true;
        } else if !is_table_row && in_table {
            out.push_str("\\end{verbatim}\n");
            in_table = false;
        }
        if in_table {
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push_str(&format!("\\paragraph{{{}}}\n", escape_latex(rest)));
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push_str(&format!("\\subsubsection{{{}}}\n", escape_latex(rest)));
        } else if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push_str(&format!("\\subsubsection{{{}}}\n", escape_latex(rest)));
        } else if trimmed.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&inline_to_latex(trimmed));
            out.push('\n');
        }
    }

    if in_code || in_table {
        out.push_str("\\end{verbatim}\n");
    }
    out
}

/// Handles inline `code` and **bold** within a paragraph.
fn inline_to_latex(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;

    // Inline code first: its contents must not be interpreted as bold markers
    // or escaped twice.
    while let Some(open) = rest.find('`') {
        out.push_str(&bold_to_latex(&rest[..open]));
        let after = &rest[open + 1..];
        match after.find('`') {
            Some(close) => {
                out.push_str(&format!("\\texttt{{{}}}", escape_latex(&after[..close])));
                rest = &after[close + 1..];
            }
            None => {
                // An unmatched backtick is text, not a delimiter.
                out.push_str(&escape_latex(&rest[open..]));
                return out;
            }
        }
    }
    out.push_str(&bold_to_latex(rest));
    out
}

/// Handles `**bold**` within a span that contains no inline code.
fn bold_to_latex(span: &str) -> String {
    let mut out = String::new();
    let mut rest = span;

    while let Some(open) = rest.find("**") {
        out.push_str(&escape_latex(&rest[..open]));
        let after = &rest[open + 2..];
        match after.find("**") {
            Some(close) => {
                out.push_str(&format!("\\textbf{{{}}}", escape_latex(&after[..close])));
                rest = &after[close + 2..];
            }
            None => {
                out.push_str(&escape_latex(&rest[open..]));
                return out;
            }
        }
    }
    out.push_str(&escape_latex(rest));
    out
}

/// Collects every documented module under `root`.
fn collect(root: &Path) -> Vec<Module> {
    let mut modules = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // `target` and `tests` are not part of the reference: one is
                // build output, the other documents the tests rather than the
                // system.
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "target" && name != "tests" && name != "bindings" {
                    stack.push(path);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let Ok(source) = fs::read_to_string(&path) else {
                    continue;
                };
                if let Some(doc) = extract_module_doc(&source) {
                    let status = Status::detect(&doc);
                    modules.push(Module {
                        path: path
                            .strip_prefix(root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/"),
                        doc,
                        status,
                    });
                }
            }
        }
    }

    modules.sort_by(|a, b| a.path.cmp(&b.path));
    modules
}

const PREAMBLE: &str = r"\documentclass[11pt,a4paper]{article}
\usepackage[utf8]{inputenc}
\usepackage[T1]{fontenc}
\usepackage{geometry}
\geometry{margin=1in}
\usepackage{fancyvrb}
\usepackage{parskip}
\usepackage{hyperref}
\hypersetup{colorlinks=true,linkcolor=black,urlcolor=blue}
\usepackage{framed}

\title{Maya2C Technical Reference}
\author{Generated from the workspace's module documentation}
\date{\today}

\begin{document}
\maketitle

\begin{framed}
\noindent This document is generated. It is a \emph{technical reference}
assembled from the module documentation in the Maya2C source tree, not a
whitepaper: it describes what the code says about itself, and it does not argue
a design.

\medskip

\noindent Sections describing modules that are not reachable by consensus carry
a banner saying so. Those banners are taken from the modules' own status
documentation, not from an editorial judgement made here.
\end{framed}

\tableofcontents
\newpage
";

fn main() {
    let mut out_path = PathBuf::from("docs/reference.tex");
    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--out" => {
                let Some(value) = argv.next() else {
                    eprintln!("--out requires a value");
                    std::process::exit(1);
                };
                out_path = PathBuf::from(value);
            }
            "-h" | "--help" => {
                println!("maya-docgen — generate a LaTeX technical reference\n");
                println!("USAGE:\n  maya-docgen [--out <PATH>]");
                return;
            }
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(1);
            }
        }
    }

    let mut document = String::from(PREAMBLE);
    let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();

    for (dir, title) in CRATES {
        let root = if *dir == "src" {
            PathBuf::from("src")
        } else {
            PathBuf::from(dir).join("src")
        };
        if !root.exists() {
            continue;
        }

        let mut modules = collect(&root);
        if modules.is_empty() {
            continue;
        }

        // A crate whose root is a research branch is a research branch
        // throughout. Without this, only `lib.rs` carries a banner and a
        // reader landing on `blockgraph/schedule.rs` sees a detailed
        // description of a scheduler with nothing saying no block ever reaches
        // it — which is the failure this whole mechanism exists to prevent.
        let crate_status = modules
            .iter()
            .find(|m| m.path == "lib.rs" || m.path == "main.rs")
            .map(|m| m.status)
            .unwrap_or(Status::Shipped);
        if crate_status != Status::Shipped {
            for module in &mut modules {
                if module.status == Status::Shipped {
                    module.status = crate_status;
                }
            }
        }

        let disabled = modules
            .iter()
            .filter(|m| m.status != Status::Shipped)
            .count();
        counts.insert(title, (modules.len(), disabled));

        document.push_str(&format!("\\section{{{}}}\n", escape_latex(title)));
        document.push_str(&format!(
            "\\noindent\\texttt{{{}}}\n\n",
            escape_latex(&root.to_string_lossy())
        ));

        for module in modules {
            document.push_str(&format!("\\subsection{{{}}}\n", escape_latex(&module.path)));
            if let Some(banner) = module.status.banner() {
                document.push_str(&format!(
                    "\\begin{{framed}}\\noindent\\textbf{{Status.}} {}\\end{{framed}}\n\n",
                    escape_latex(banner)
                ));
            }
            document.push_str(&doc_to_latex(&module.doc));
            document.push('\n');
        }
    }

    document.push_str("\\end{document}\n");

    if let Some(parent) = out_path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(error) = fs::write(&out_path, &document) {
        eprintln!("writing {}: {error}", out_path.display());
        std::process::exit(1);
    }

    let total: usize = counts.values().map(|(n, _)| n).sum();
    let flagged: usize = counts.values().map(|(_, d)| d).sum();
    println!("Wrote {} ({} bytes)", out_path.display(), document.len());
    println!("{total} documented modules across {} crates", counts.len());
    println!("{flagged} carry a status banner (research branch or opt-in)");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latex_specials_are_escaped() {
        // The `_` case is the one that matters: this codebase is snake_case
        // throughout, and an unescaped underscore either fails to compile or
        // renders as a subscript, which is worse because it builds.
        assert_eq!(
            escape_latex("send_raw_transaction"),
            r"send\_raw\_transaction"
        );
        assert_eq!(escape_latex("100%"), r"100\%");
        assert_eq!(escape_latex("a&b"), r"a\&b");
        assert_eq!(escape_latex("#[test]"), r"\#[test]");
        assert_eq!(escape_latex("${x}"), r"\$\{x\}");
    }

    #[test]
    fn a_module_doc_stops_at_the_first_code_line() {
        let source = "//! First line.\n//! Second line.\nuse std::fs;\n//! not a module doc";
        assert_eq!(
            extract_module_doc(source).expect("has docs"),
            "First line.\nSecond line."
        );
    }

    #[test]
    fn a_file_without_module_docs_is_skipped() {
        assert!(extract_module_doc("use std::fs;\n\nfn main() {}").is_none());
        assert!(extract_module_doc("//!\n//!  \n").is_none());
    }

    #[test]
    fn inline_code_is_not_treated_as_bold() {
        // A backtick span containing `**` must survive intact, or a doc
        // comment showing a markdown example would render as bold text.
        let rendered = inline_to_latex("use `a_b` and **bold**");
        assert!(rendered.contains(r"\texttt{a\_b}"));
        assert!(rendered.contains(r"\textbf{bold}"));
    }

    #[test]
    fn an_unmatched_backtick_is_text() {
        // Prose sometimes contains a stray backtick. Treating it as an opener
        // would swallow the rest of the paragraph into a code span.
        let rendered = inline_to_latex("a ` stray tick");
        assert!(rendered.contains("stray tick"));
        assert!(!rendered.contains(r"\texttt{"));
    }

    #[test]
    fn fenced_code_is_verbatim_and_unescaped() {
        let rendered = doc_to_latex("text\n```\nlet x = a_b;\n```\nmore");
        assert!(rendered.contains("\\begin{verbatim}\nlet x = a_b;\n\\end{verbatim}"));
    }

    #[test]
    fn an_unterminated_code_fence_is_closed() {
        // A doc comment with an unbalanced fence would otherwise produce a
        // .tex file that does not compile, and the failure would point at the
        // end of the document rather than at the module that caused it.
        let rendered = doc_to_latex("```\nunclosed");
        assert!(rendered.trim_end().ends_with("\\end{verbatim}"));
    }

    #[test]
    fn a_research_branch_is_detected_from_its_own_words() {
        assert_eq!(
            Status::detect("# Status\n\n**Research branch.** Nothing calls this."),
            Status::ResearchBranch
        );
        assert_eq!(
            Status::detect("No consensus path reaches this code."),
            Status::ResearchBranch
        );
        assert_eq!(
            Status::detect("**Off by default.** Negotiated per connection."),
            Status::OptIn
        );
        assert_eq!(
            Status::detect("The chain's transaction codec."),
            Status::Shipped
        );
    }

    #[test]
    fn prose_about_another_module_does_not_change_this_one() {
        // The regression this narrowing exists for. `api-gateway/graphql.rs`
        // is shipped, and its documentation explains what it declines to
        // expose by naming a research branch. A whole-document scan labelled
        // it a research branch, which would have told a reader the gateway's
        // GraphQL layer was not real.
        let doc = "\
GraphQL over the same node methods the REST layer serves.\n\
\n\
# What is queryable\n\
\n\
Notably absent: anything about the DAG. The block-graph work in\n\
`maya-blockgraph` is a research branch that no consensus path reaches.";
        assert_eq!(Status::detect(doc), Status::Shipped);
    }

    #[test]
    fn a_status_section_wins_over_the_opening_lines() {
        let doc = "\
The dual-KEM combiner.\n\
\n\
# Status\n\
\n\
**Off by default.** Negotiated per connection.\n\
\n\
# What the combiner buys\n\
\n\
Confidentiality that survives a break in either primitive.";
        assert_eq!(Status::detect(doc), Status::OptIn);
    }

    #[test]
    fn every_non_shipped_status_carries_a_banner() {
        // The property the honesty of the document rests on: if a status is
        // ever added without a banner, a disabled module would render as
        // though it were live.
        assert!(Status::Shipped.banner().is_none());
        assert!(Status::ResearchBranch.banner().is_some());
        assert!(Status::OptIn.banner().is_some());
    }
}
