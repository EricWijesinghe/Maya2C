# formal/aeneas

Empty, and nobody here has evaluated Aeneas.

The foundation brief lists it beside Lean 4 and Kani. It is a different bet
from both: it translates Rust into a pure functional model for a proof
assistant, rather than model-checking the Rust directly as Kani does.

Saying that nobody has assessed it is more useful than an empty directory
implying somebody had. What an assessment would have to answer:

- Does it handle the arithmetic crates' shape - `no_std`, integer-only,
  dependency-free - which is the only code in this tree a translation could
  reach today?
- What does it prove that `cargo kani` does not already prove about those same
  functions?

Until those have answers, this is a directory and not a plan.
