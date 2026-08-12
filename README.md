# polysim

## Run

```sh
# Trading engines — headless, one process per data source
cargo run --release --bin strat-micro-recorder-te-binance-spot-btcusdt
cargo run --release --bin strat-micro-recorder-te-polymarket-btc-updown-5m

# Desktop workstation — attaches to a running engine over UDP, needs the `ui` feature
cargo run --release --features ui --bin polysim-ui -- --strategy strat-micro-recorder --link 127.0.0.1:9310

# Gate — all five must pass before anything merges
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features ui -- -D warnings
cargo test
cargo test --features ui

# ARM64 Linux binary for deployment, built in Docker, lands in dist/
./scripts/build-strategy.sh strat-micro-recorder-te-binance-spot-btcusdt
```

Each engine defaults to the `config.yaml` beside its own `main.rs`; `--config <path>`
overrides it. Run either engine alone — they find each other over UDP if both are up, and
the Binance engine's `poly_*` columns simply stay empty if the Polymarket one is not.

Live-network tests are `#[ignore]`d and never run in CI:

```sh
cargo test --test integration -- --ignored --nocapture
```

## What this is

A market-data collection engine. It connects to a venue, keeps an exact order book, and
records microstructure features to Parquet — one row per feature per tick — so the data
can be analysed offline. Two sources ship: Binance spot BTC/USDT, and the Polymarket BTC
5-minute Up/Down series.

Collection is the point. The engine can also place orders, and ships disarmed
(`execution.mode: off`) with a second independent switch (`strategy.params.enabled`) also
off. Arming it is a deliberate two-step edit, never a default.

The shape is fixed: Tokio actors at the I/O edge, one pinned synchronous thread owning all
market state, fixed-capacity queues between them, zero allocation on the hot path. Because
hot state is a pure function of its ordered input messages, replaying a recording
reproduces it exactly — which is what makes the test suite and any future backtest
trustworthy.

**This repository was written entirely by AI.** `CONSTITUTION.md` is why that produced
something coherent rather than a pile: it is the permanent engineering law every agent
works under, and it is the first thing to read.

## Navigating the repo

| Path                 | What lives there                                                                                                                                                                |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `CONSTITUTION.md`    | The law. Execution model, memory, numerics, errors, testing. Read first.                                                                                                        |
| `CODEBASE.md`        | Generated map of every file in the tree.                                                                                                                                        |
| `src/hot/`           | The synchronous hot thread — order book, feature tracker, execution state machine, and `quant/` (volatility, Hawkes, Kyle's lambda, VPIN, Guéant pricing). No async here, ever. |
| `src/adapters/`      | Venue edges: Binance, Polymarket, and a simulated exchange. `adapters/exec/README.md` is the execution contract every venue adapter implements.                                 |
| `src/config/`        | The YAML schema. Start here to understand what a `config.yaml` may say.                                                                                                         |
| `src/persist/`       | Parquet writing — schemas, tables, hourly rotation.                                                                                                                             |
| `src/desktop/`       | Native GUI, behind the `ui` cargo feature so a headless engine never builds it.                                                                                                 |
| `src/link/`          | UDP wire between engines and the workstation.                                                                                                                                   |
| `strategies/`        | Customer code, one folder per trading engine: `<strategy-id>/<te-id>/` holds `main.rs`, `strategy.rs` and `config.yaml`. Exempt from the source-file size cap.                  |
| `tests/fitness/`     | The only CI tests. Few, immortal, architectural — replay determinism, book reconstruction, zero allocation.                                                                     |
| `tests/integration/` | Live-network tests, `#[ignore]`d.                                                                                                                                               |
| `fixtures/`          | Recorded real venue payloads. Preferred over mocks.                                                                                                                             |
| `wiki/`              | Notes on the quant models and where they come from.                                                                                                                             |

Identity is the pair `(strategy-id, te-id)` and it is structural, not configured: a
trading engine lives at `strategies/<strategy-id>/<te-id>/`, its binary is named
`<strategy-id>-<te-id>`, and that same pair names its log file (`logs/`) and its data tree
(`data/<strategy-id>/<te-id>/`). A fitness test parses `Cargo.toml` and fails the build if
any binary breaks the pairing, so the naming cannot drift.

For development, `rex-cli` is recommended — `cargo install rex-cli`, then `rex init` inside
the repo. `rex codebase` regenerates `CODEBASE.md`, so the tree map above stays current
instead of being maintained by hand.
