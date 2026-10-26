# ADR-064: A zero-quote trade cancels the dust order instead of stopping the book

Status: Accepted (2026-10-27, Eric: cancel dust and a minimum notional).
Takes effect at the next genesis: it changes what the matcher does and what a
placement may be.

## Context
`match_book` stopped the whole pass on a trade whose quote rounds to zero, with
the book still crossed. The audit of 2026-10-26 found that anyone could rest an
ask at price 1 for 1 base unit: it is the best ask and, once older than a taker,
the maker that sets the price, so every cross against it yields a zero quote and
the pass stops. Honest orders behind it never trade, and only its owner can
cancel it. Placement checked only for a zero price and a zero amount. The
failing test `a_resting_dust_order_does_not_freeze_the_honest_orders_behind_it`
reproduced it, and a placement test showed the order was accepted.

## Decision
1. **Matching.** On a cross whose quote is zero, the side that the trade would
   consume whole (remaining equal to the traded base) is cancelled and returned
   in `MatchOutcome::dust`; when both sides would be consumed whole, both are.
   Matching continues. The node refunds the escrow of any order missing from the
   matcher's book, as it does for fills and expiry. A cancelled order counts
   against `max_fills`, so a book of them cannot make a pass unbounded.
2. **Placement.** An order is refused when filling it entirely would produce a
   zero quote (`quote_for_base(amount, price, Ask)` is zero). This is the floor
   the arithmetic requires, not an economic minimum; a larger minimum is a
   governed parameter if wanted.

## Consequences
- An honest small order can be cancelled and refunded when it crosses a very
  cheap resting order that sets a zero-quote price. Nothing is lost; the owner
  gets the escrow back.
- Partial-fill dust (an order that becomes tiny after fills) is covered by (1),
  which the placement rule alone would not cover.
- `MatchOutcome` gains a field; the node's `expired` handling is unchanged.
- No spec file covers the DEX, so there are no conformance vectors to change.
