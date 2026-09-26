# Legal notice

**Nothing in this repository is legal, tax, investment or financial advice.**

Several modules implement rules that look like law or finance — ISO 20022
payment messages (`crates/iso20022`), real-world-asset transfer rules
(`crates/rwa`), identity claims and selective disclosure (`crates/identity`),
compliance and sanctions screening hooks, per-jurisdiction tax calculators,
and an economic simulator (`econ/`). They are **tooling**:

- A jurisdiction rule encoded here is a programmer's reading of a public
  source at a date, not a lawyer's opinion. Qualified counsel in each
  jurisdiction must review it before anyone relies on it.
- Tax calculators are pure functions over the inputs they are given; they do
  not know a user's circumstances and are not a tax return.
- No module guarantees price stability, a yield, or the value of any asset.
  The economic simulator treats prices as *inputs* to scenarios and never
  predicts them.
- Sanctions screening and travel-rule message formats are hooks for front
  ends and exchanges, which carry the legal obligation; the protocol does not.

Questions that need counsel are collected in
[legal/QUESTIONS_FOR_COUNSEL.md](legal/QUESTIONS_FOR_COUNSEL.md).
