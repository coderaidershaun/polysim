# polysim

A toy market making educational and visualisation tool for live simulation and data collection of running spike tests on market making strategies.

Yes, you can also execute live trades, but this is not the purpose of this tool.

This project is part of a personal project and experiment to see how high of code quality can be written when applying the right skills via the `rex-skills` cargo install.

Whether you are an engineer, hobbyist, student or whatever, the idea is that with cutting edge LLMs you have a foundation to learn from and work with.

The project is NOT optimised for latency. It is rather optimised for adaptability, whereby exchanges are popped on and off easily as adapters. The project uses a generic structure described below to help ensure any type of exchange could be bolted on. Right now the library does not support FIX although if time permits, will work on that perhaps in the future.

Note that live trading is untested and just there for now as a placeholder. This repo is NOT meant for live trading execution so please use at your own risk.

## Run

```shell
# Credentials — copy the template, then fill it in. .env is never tracked.
mv .env.example .env

# Trading engines — headless, one process per data source
cargo run --release --bin strat-micro-recorder-te-binance-spot-btcusdt
cargo run --release --bin strat-micro-recorder-te-polymarket-btc-updown-5m

# Desktop workstation — attaches to running engines over UDP, needs the `ui` feature.
cargo run --release --features ui --bin polysim-ui -- --strategy strat-micro-recorder --link 127.0.0.1:9310
cargo run --release --features ui --bin polysim-ui -- --strategy strat-micro-recorder --link 127.0.0.1:9311

# Gate — all five must pass before anything merges
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features ui -- -D warnings
cargo test
cargo test --features ui

# ARM64 Linux binary for deployment, built in Docker, lands in dist/
./scripts/build-strategy.sh strat-micro-recorder-te-binance-spot-btcusdt

# ...which is a name check plus this. Run it directly if you prefer:
docker buildx build \
    --platform linux/arm64 \
    --build-arg BIN=strat-micro-recorder-te-binance-spot-btcusdt \
    --output type=local,dest=dist \
    .
```

## Live Tests

Live-network tests are `#[ignore]`d and never run in CI:

```shell
cargo test --test integration -- --ignored --nocapture --skip poly_exec
```

## Architecture

![Deterministic single-thread trading architecture](static/architecture.jpeg)
