# Examples

Small standalone tools that use the library, built because they were needed while developing it, and kept because they stay useful. Each one runs on its own with `cargo run --example <name>` — Cargo has no "tool" target kind, so "example" is the flag even though these are working tools, not sample code.

## dom-fixture

The desktop UI running on fake data. It shows the same panels the real workstation shows — price ladder, charts, monitor, positions — but fed from canned scenes instead of a live exchange, so you can check that a UI change looks right without starting an engine or connecting to anything. Press 1–9 to flick between scenes; arrow keys and shortcut keys switch sides and variants.

```shell
cargo run --example dom-fixture --features ui
```

## poly-probe

A read-only health check for a Polymarket account. It answers, in one report: is this host geoblocked, do the credentials in `.env` actually work, what kind of wallet is this, what does the account hold, and what orders are resting. It never places, changes, or cancels anything — run it as often as you like, and always before trusting any execution setup.

```shell
cargo run --example poly-probe              # reads .env
cargo run --example poly-probe path/to/env  # or a specific file
```

## poly-recover

A one-off emergency tool, kept for the record. A decode bug once stranded a real bought position on Polymarket, and this program sold that one position back — sell only, one order at most, never more than the venue says is actually held, and it checks the venue at every step. It is hard-wired to that single incident, so it is not a general-purpose tool; it stays here because it documents what a careful hand recovery looks like.
