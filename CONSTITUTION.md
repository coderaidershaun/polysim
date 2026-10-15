# Polysim Constitution

Permanent engineering law for polysim market-data + quant engine. All docs, modules, and code
comply. IF one cannot comply, THEN surface conflict explicitly. Amendments: §15.

## Words

| Word                | Meaning                                                         |
| ------------------- | --------------------------------------------------------------- |
| hot thread          | ONE synchronous hot-path thread that owns all market state (§2) |
| edge actor          | tokio async actor at I/O edge (§2)                              |
| lib                 | the one lib crate, `polysim` (§8)                               |
| bin                 | binary                                                          |
| TE                  | trading engine (§8)                                             |
| fitness             | fitness suite in `tests/fitness/` (§12)                         |
| Amended `<date>`    | amendment of that date, with its WHY (§15)                      |
| Replaced, WITHDRAWN | record of earlier law, not in force                             |

## 1. Prime Directives

- [ ] Simple beats clever. IF straightforward design meets requirements without heavy performance
  loss, THEN it wins. No exceptions for interesting ideas.
- [ ] Abstraction must pay rent at call sites. Deletion test: remove module. IF complexity
  vanishes, THEN module = pass-through. Delete it.
- [ ] One design language. Same problem = same solution everywhere.
- [ ] Keep comments to a minimum. Prefer ergonomics over comments. Use /rex-utils-caveman skill for
  commenting.
- [ ] Aim high, ship lean. 80% value from 20% code.
- [ ] No early factoring. Cut points emerge first. Then trap complexity behind narrow interface.
- [ ] No source file > 500 lines.
  - Amended 2026-07-21: files under strategies/ = exempt. WHY: strategy implementations are
    one-file customer code. A forced split serves no reader.
  - Amended 2026-07-22: cap counts production lines only. `#[cfg(test)]` blocks excluded. WHY: cap
    buys navigable production logic. Inline pins appended at bottom obstruct no reader, and §12
    invites them on quant calculators. To count them taxes testing.
  - Amended 2026-07-26: cap counts production lines under src/ and in binaries. Files under tests/
    = exempt. WHY: a file under tests/ is ENTIRELY test code. It sits in no `#[cfg(test)]` block,
    because a test target needs none. The 2026-07-22 carve-out's own logic reaches it.

## 2. Execution Model

- [ ] Two domains, permanent: tokio async actors at I/O edge + ONE synchronous hot-path thread
  that owns all market state. Nothing else touches that state.
- [ ] Hot path: no async, no locks, no shared mutable state, no blocking syscalls in steady state.
- [ ] All communication with hot path goes through fixed-capacity SPSC rings (`rtrb`).
- [ ] Hard max: 20 input queues.
- [ ] Single-writer: every piece of state has exactly one owning thread.
- [ ] CPU pinning required. Hot thread = pinned. Other affinity = design decision, not afterthought.
- [ ] Hot-path state = pure function of ordered input message sequence.
- [ ] Time-driven behavior (spin ticks) arrives as messages from async timer actor.
- [ ] Wall clock never drives state transition on hot thread. Clock reads there only stamp latency
  metrics.
- [ ] Consequence: replay of recorded input sequence gives identical hot-path state. This is basis
  of fitness tests + future backtesting. Never break it.

### Amended 2026-07-29: edge actor can synthesize a venue

M13 `exchange-sim`. Ratified O1.

- [ ] Edge-actor rule governs WHERE STATE LIVES, not whether a socket exists.
- [ ] An edge actor can SYNTHESIZE a venue instead of reaching one.
- Determinism above is untouched, and it is the reason this is safe. Output of a synthesizing
  actor is ordinary `InboundMessage` traffic on an ordinary input queue. Thus replay of a recorded
  tape reproduces hot state exactly. Hot thread cannot tell which side of the socket its input
  came from.
- [ ] Binding consequence: such an actor is still an EDGE actor and owns no hot state.
- [ ] Binding consequence: its own decisions must be a pure function of stamped input messages.
  IF it reads a clock to decide what to emit, THEN it relocates the nondeterminism. It does not
  remove it.

## 3. Memory

- [ ] Fixed capacity wherever practical: preallocated at startup, contiguous, predictable layout,
  cache-friendly access.
- [ ] Heap = initialization only.
- [ ] Steady-state hot path: zero alloc, zero dealloc, zero resize. Counting-allocator fitness test
  enforces this, not trust.
- [ ] Capacity hit = designed event with explicit per-structure policy. Never silent growth.
- [ ] Memory is cheap. Latency variance is not. Trade memory for simplicity + predictable movement.
  Canonical example: FastQueue oversized backing allocation.
- [ ] No strings in hot-path lookups. Identity = compact typed indices, assigned from config at
  startup. String-like types live at edges.

## 4. Numerics

- [ ] Prices and quantities cross adapter boundary exactly once. There, exchange decimal strings
  become `i64` fixed-point mantissas. Global scale is fixed during design.
- [ ] Book keys = exact integers: exact equality, total ordering, hashable.
- [ ] `f64` only downstream of exact state: derived features, statistics, model math.
- [ ] Float is never: a key, a money accumulator, an equality operand in book logic.

## 5. Time

- [ ] µs resolution everywhere.
- [ ] Every timestamp field ends `_ts_us` (`exchange_ts_us`, `received_ts_us`, `queued_ts_us`,
  `processed_ts_us`).
- [ ] Timestamps and durations = typed wrappers with ergonomic ops (`a.diff(b)`).
- [ ] Bare `i64` never crosses an API.
- [ ] Exact type set is fixed in design.
- [ ] Exchange time ≠ local receive time ≠ monotonic latency clock. Never interchange them
  silently.

### Amended 2026-10-14: MCP wire timestamps

WHY: the MCP wire is an external JSON contract read by agents and a UI, so its timestamps are
civil strings, and every internal field stays `TsUs`/`_ts_us`.

- [ ] A timestamp on the MCP wire is an RFC3339 UTC string with a µs fraction, for example
  `started_at: "2026-10-14T19:05:03.123456Z"`.
- [ ] It is formatted from a `TsUs` at the wire edge, by the crate's own civil-time conversion.
  No calendar crate.
- [ ] Internal fields stay `TsUs` with the `_ts_us` suffix.
- [ ] A duration on the MCP wire is an integer whose field name ends in its unit (`duration_ms`),
  while every internal duration stays `DurationUs`.
- [ ] Scope: the MCP wire ONLY. Every other API, record and file keeps the rules above.

## 6. Errors & Failure

- [ ] Two failure classes. Never confuse them.
  - **External, expected** (disconnect, malformed frame, HTTP fail, full queue, full disk): typed
    `thiserror` per concept. Variant = what failed + values seen. Handle by policy: reconnect,
    backoff, resync, drop + count.
  - **Internal invariant violation** (bug: corrupt book, impossible index, broken sequence logic):
    assert + panic immediately. Fail fast, fail loud.
- [ ] Queue-full policy = asymmetric by design.
  - IF INPUT queue is full, THEN fatal drain (engine cannot keep up). Exception: untrusted remote
    producer, amended 2026-07-25.
  - IF OUTPUT queue is full, THEN drop + count. Execution command ring: amended 2026-07-27.
- [ ] IF invariant panic occurs, THEN do coordinated bounded drain: flush persistence queue, close
  Parquet clean, exit non-zero.
- [ ] Drain deadline = HARD. Corrupt state never continues to produce research data.
- [ ] lib = `thiserror`. bin = `anyhow` + `.context()` on every `?`.
- [ ] Forbidden:
  - `Box<dyn Error>` in any API
  - `Result<T, String>`
  - catch-all `Other(String)`
  - `.unwrap()` outside tests (`.expect("WHY")` only at init boundaries)
  - `let _ =` swallow
- [ ] Messages: lowercase, no trailing period, concrete values included, actionable when fix is
  known.
- [ ] Every `pub` `Result` function documents `# Errors`.

### Amended 2026-07-25: INPUT queue fed by untrusted producer

WHY: fatal-on-full says "the engine cannot keep up". A remote peer that floods a UDP port is an
UNTRUSTED producer. IF it can trip `FatalSignal`, THEN anyone who can reach the port has a
remote-kill primitive.

- [ ] IF untrusted remote producer (the link) feeds an INPUT queue, THEN drop + count, not fatal
  drain.
- [ ] Engine-fed input queues keep fatal-on-full.
- [ ] Drop is sound ONLY under one semantic contract. That contract makes a dropped frame
  equivalent to one never sent. Contract: **link topics carry STATE (absolute values), never
  deltas or events.**
- [ ] That contract = constitutional law, not a code comment. WHY: a strategy that folds a delta
  stream through `on_link` breaks silently on first drop.
- [ ] Drop happens at edge. Recording happens downstream of drop, on hot side. Thus tape holds
  only consumed frames (§2).

### Amended 2026-07-27: execution command ring

WHY: execution command ring carries EVENTS, not state. Thus the precondition of drop + count fails.

- [ ] OUTPUT-queue drop + count is sound because consumer needs only LATEST state. A dropped frame
  must be equivalent to one never sent.
- An order command breaks that. A dropped `Cancel` leaves an order resting at a price that the
  strategy abandoned.
- [ ] The precondition is RESTORED, not waived.
- [ ] Hot thread never pushes to execution ring directly.
- [ ] Hot thread transitions order state at BANK time into a fixed-capacity buffer.
- [ ] A separate step drains that buffer into the ring, FIFO. It stops on first failure and
  retries next spin.
- This is the latched pattern that `PersistSink::request_seal` already uses for the one record
  that must never be dropped.
- [ ] Thus ring pressure delays the WIRE, never the STATE.
- [ ] Two consequences bind:
  - Sink still counts drops. §6's letter holds, and in steady state the count is unreachable.
  - Reachable failure becomes the bank's own high-water mark. It is a message-driven count, not a
    ring occupancy. Thus §2 replay determinism survives.
- IF hot pass gates on ring space instead, THEN hot state becomes a function of how fast the
  ASYNC consumer drained. That is a function of wall time. Replay diverges.

### Amended 2026-08-08: rate-limit pacing

WHY: a venue's order-rate budget gained its first enforcement. An edge-side pacer that silently
delayed placements would falsify the strategy's time-to-market assumption. It would also make
order flow a function of venue budget drain.

- [ ] Rate-limit pacing, like ring pressure, delays the WIRE, never the STATE.
- [ ] Compliant shape = hot-side REFUSAL that is a pure function of message stamps. Placement is
  not minted this spin. It is counted under its own reject reason. Thus a replay refuses at
  exactly the same point.
- [ ] An adapter never withholds a banked command to satisfy a budget.

### Amended 2026-10-14: sim block is inert outside sim mode

WHY: an operator or agent flips mode with a one-line edit, and a mode flip that makes a config fail
at boot is a trap.

- [ ] A `sim:` block can sit under `mode: off` or `mode: live`.
- [ ] It is validated whenever present, in every mode. Thus a flip back to `sim` never surfaces a
  bad value that sat unchecked.
- [ ] It is USED only under `sim`. Code may read it in any mode, but only a simulated venue acts
  on it.
- [ ] `mode: sim` with no block stays refused.

## 7. Logging

- [ ] Bespoke minimal subsystem. No logging framework dependency.
- [ ] Exactly three levels: `INFO`, `WARN`, `ERROR`.
- [ ] `ERROR` captures backtrace + call-site file/line by construction. API makes omission
  impossible.
- [ ] Producers hand records to dedicated logging thread. No thread blocks or formats inline.
- [ ] Debug builds: terminal. Release builds: efficient file sink.
- [ ] Hot path steady state emits nothing. `WARN`/`ERROR` from hot path = OK: exceptional by
  definition.

### Amended 2026-07-20: strategy telemetry

WHY: strategy telemetry is research output, not engine noise.

- [ ] Strategy callbacks can emit per-tick telemetry through the banked ctx logging lane, on a
  dedicated ring.
- [ ] ONE sink per TE run. Release: log file. Debug: terminal. File name: amended 2026-07-25 and
  2026-07-29.
- [ ] Sink carries engine-origin and strategy-origin records alike. Record's `[tag]` column
  (thread tag or strategy id) distinguishes origin. Thus one file loses nothing.
- [ ] Dedicated ring = drop isolation, not routing. A telemetry flood fills its own ring. It does
  not displace engine `WARN`/`ERROR`.
- [ ] Engine hot-path code stays bound by steady-state silence.
- [ ] Strategy records stamp event time (§2 purity). They format into the fixed-size record at
  bank time.
- [ ] Strategy-lane ERROR carries file/line, no backtrace. WHY: capture allocates, and §3 wins on
  hot thread.
- [ ] IF lane is full, THEN drop + count (§6).
- Replaced 2026-07-25: sink was ONE per run, named by strategy id. Release file was
  `logs/<strategy-id>-<date>.log`.

### Amended 2026-07-23: `log` facade

WHY: eframe pulls the `log` facade deeper into the graph. The no-framework rule needs an explicit
carve-out.

- [ ] `log` = transitive-only (already via reqwest/tungstenite, now also eframe/winit/wgpu), never
  a polysim choice.
- [ ] A polysim module must not call `log`.
- [ ] Bespoke subsystem stays the ONLY polysim logging API.
- [ ] No `log` logger is installed. Thus dependency facade emissions route nowhere: they drop at
  the uninitialized facade.
- [ ] Rule intact: polysim does not depend on `log` directly, and does not log through it.

### Amended 2026-07-25: sink name per TE

WHY: N trading engines per strategy fight over one strategy-id-named file.

- [ ] Sink name = two-part identity (§8): `logs/<strategy-id>-<te-id>-<date>.log`. ONE per TE run.
- [ ] Everything else in the 2026-07-20 amendment stands.

### Amended 2026-07-29: sim runs write separate artifacts

M13 `exchange-sim`. Ratified O5. WHY: simulated fills that read as live ones are an
execution-safety defect, not an observability preference.

- [ ] A run under `mode: sim` writes `logs/<strategy-id>-<te-id>-sim-<date>.log`.
- [ ] Live stem is unchanged BYTE-FOR-BYTE. Thus no existing path moves, and `off`/`live` runs are
  indistinguishable from today.
- [ ] Physical separation, not an in-line marker, is deliberate. WHY: a `[tag]` column
  distinguishes origin only to a reader who already reads. A glob, a tail, or an operator who
  grabs "the log" must not be able to reach simulated fills by accident.
- [ ] Same ruling binds the sibling artifacts:
  - exposure: `<strategy-id>-<te-id>-sim.json`
  - Parquet: below `<strategy>/<te>/sim/`, with an `execution_mode` footer
  - UI: mode badge (O6)

### Amended 2026-10-14: the MCP server's stdout is the wire

WHY: `polysim-mcp` speaks JSON-RPC on stdout, so one stray byte there corrupts the agent's
session.

- [ ] `polysim-mcp` never writes to stdout. Its own diagnostics go to stderr.
- [ ] An engine it starts never inherits its stdio. stdin is null. stdout and stderr append to
  `logs/<artifact-stem>-console.log`.
- [ ] A sim run gets the `-sim` stem, under the 2026-07-29 ruling.
- [ ] `tracing` = transitive-only, exactly like `log`. It arrives via reqwest -> hyper-util, and
  now via rmcp. No subscriber is installed. Thus its emissions drop.
- [ ] A polysim module must not call `tracing`.

## 8. Code Organisation

- [ ] One lib crate (`polysim`) holds all substance. Binaries only compose: load config, wire,
  start, orchestrate shutdown.
- [ ] Module = concept (`hot/`, `adapters/`), not type-per-file.
- [ ] Every module opens with 1–2 line `//!` WHY header.
- [ ] `hot/`: no async code, no exchange-specific wire formats. Adapters normalize at edge.
  Generalized internal messages = only currency inward. Review-enforced. Workspace split only if
  seam demonstrably needs compiler help.
- [ ] `pub(crate)` = default. `pub` = promise.

### Amended 2026-07-20: strategies live outside lib

WHY: strategy implementations are customer code (§9).

- [ ] Strategy implementations live outside the lib. Folder layout: amended 2026-07-25.
- [ ] The lib keeps the seam: `Strategy`, `StrategyConfig`, `StrategyCtx`, the Actions bank, and
  the runner. Runner: amended 2026-07-25.
- Replaced 2026-07-25: strategy lived in `strategies/<id>/` with main.rs (composition only) +
  strategy.rs + config.yaml. bin name = folder name = strategy id via `env!("CARGO_BIN_NAME")`.
  Runner in the seam was `run_strategy`. Breaking data-format change = new folder/id.

### Amended 2026-07-23: helper modules beside strategy.rs

WHY: a recorder outgrew one readable file.

- [ ] A strategy folder can carry helper modules beside strategy.rs. Declare them from strategy.rs
  via `#[path]`. Thus both compile roots (the bin and the fitness `#[path]` include) resolve the
  same flat sibling files.
- [ ] strategy.rs stays the single entry.
- [ ] The §1 strategies/ exemption from the 500-line cap covers these siblings too. They are the
  same customer code, split only for the reader.

### Amended 2026-07-25: two-part identity, one folder per TE

WHY: a strategy is now a SET of independently deployable trading engines, each bound 1:1 to one
source. Thus identity is two-part and the folder nests.

- [ ] A strategy folder holds one folder per trading engine: `strategies/<strategy-id>/<te-id>/`.
- [ ] That folder carries main.rs (composition only) + strategy.rs + its `#[path]` siblings +
  config.yaml.
- [ ] Identity = `(strategy-id, te-id)`.
- [ ] bin name = `<strategy-id>-<te-id>`. Path = `strategies/<strategy-id>/<te-id>/main.rs`.
- [ ] Pass both ids as literals to the lib seam's runner, now
  `run_trading_engine::<S>(strategy_id, te_id)`.
- [ ] `env!("CARGO_BIN_NAME")` no longer derives identity.
- [ ] Enforcement is structural: a fitness config-guard parses `Cargo.toml` and asserts that
  name/path pair on every `[[bin]]`. Thus global bin-name uniqueness is a test failure, not a hope.
- [ ] Cross-TE sharing has exactly ONE authorized shape: a `#[path]` sibling at strategy level
  (`strategies/<strategy-id>/common.rs`), included from each te strategy.rs.
- [ ] Otherwise per-TE duplication is accepted (customer code, §9).
- [ ] Breaking data-format change = new te folder/id.

### Amended 2026-08-08: venue execution adapter

WHY: three venues implement the execution seam by convention. A fourth needs law, not archaeology.

- [ ] A venue execution adapter is a COMPOSITION over `adapters/exec`. It:
  - drives `ExecCore` through its own codec and transport
  - funnels every exec and account event through its ONE assigned input queue, stamped
    monotonically
  - declares its venue physics in a `VenueCapabilities` record and its lease-nonce namespace
    beside it at construction
  - implements the `EdgeDriver` session chassis, not its own lifecycle loop
- [ ] Never re-implement shared exec machinery per venue: core, mirror, in-flight table, resync
  pass, exit lifecycle, synthesized-event constructors.
- [ ] Obligations that no signature can carry live in `src/adapters/exec/README.md` and their
  fitness pins:
  - cumulative totals
  - reconcile-never-resend
  - cancel finality
  - readiness order
  - fatal on an undecodable open order
- [ ] The README = prose half of the adapter contract. Changes to it are seam changes, not doc
  edits.

### Amended 2026-10-14: agent control plane

WHY: an agent operates the engines through an MCP server, and that server must not become a second
place where engine state lives.

- [ ] The MCP server (`src/mcp/`, bin `polysim-mcp`) is an OPERATOR process, not an edge actor.
- [ ] It holds no market state and links no strategy. It reaches an engine only through the OS:
  spawn, signal, exit status, files.
- [ ] Every tool is a thin shell: params -> one `Fleet` call -> reply or refusal. The substance
  stays in the supervisor. Thus a CLI or UI can reuse it.
- [ ] `Fleet` is `pub` on its own merit, as the reusable operator supervisor. Its fitness pins
  follow from that ruling and are not the reason for it.
- [ ] All tools live in one file. A second tool family becomes `src/mcp/tools/<family>.rs`, with
  routers merged by `+`.
- [ ] The server makes no edit it cannot read back. `set_execution_mode` writes only when the new
  file parses as the requested mode.
- [ ] Server exit stops every engine it started: SIGTERM first, SIGKILL only past one shared
  deadline.

## 9. Naming & API Ergonomics

- [ ] rex-code-ergonomics binds.
- [ ] No abbreviations (`cfg`, `ctx` idiomatic OK).
- [ ] Predicates: `is_`/`has_`/`can_`.
- [ ] Verbs on functions, nouns on types.
- [ ] Domain primitives = newtypes (`SourceId`, `InstrumentId`, `Price`, `Qty`, `TsUs`, …). Bare
  `u64` never impersonates another.
- [ ] No `bool` parameters in `pub` functions.
- [ ] IF >3 parameters, THEN use struct.
- [ ] Accept `&str`/`&[T]`/`&Path`. Return owned when allocating.
- [ ] Behavior = methods on type, not free functions in util modules.
- [ ] Derive `Debug, Clone, PartialEq, Eq, Hash` wherever semantics let you.
- [ ] Flat beats nested. Guard clauses / early return / `let-else` / extracted functions over
  branch-in-branch. Deep nesting = review flag. Restructure before merge.
  - Amended 2026-07-18. WHY: deep indent pyramids hide control flow.
- [ ] IF conditional fits one line, THEN write it on one line. The fmt gate enforces this, not
  hand-formatting: `rustfmt.toml` sets `single_line_if_else_max_width` +
  `single_line_let_else_max_width` = 100.
  - Amended 2026-07-18. WHY: trivial branches do not earn vertical space.
- [ ] Statement `if` without else is always multi-line under rustfmt. Prefer expression form /
  `matches!` / combinators where the one-liner reads better.
- [ ] Design APIs from call site backwards. Strategy code = customer.

## 10. Comments & Documentation

- [ ] rex-code-commenting binds.
- [ ] WHY only, never WHAT. Default = no comment.
- [ ] Fix design first (rename, extract, newtype). Comment only what code cannot say.
- [ ] In-body comments only when behavior genuinely cannot be self-evident: vendor quirk, trap,
  measured justification.
- [ ] Every module: `//!` why-header.
- [ ] `pub` item: `///` ONLY when contract exceeds name + signature (invariants, units, panics,
  warnings, non-obvious errors).
- [ ] Self-evident `pub` item = zero docstring. Never restate signature.
- [ ] Forbidden:
  - commented-out code
  - history notes
  - drifting TODOs (ticket or fix)
  - section dividers (extract functions instead)
  - generated boilerplate commentary

### Amended 2026-08-07: no document references in comments

WHY: user directive. A comment that cites a plan, milestone id, audit finding number, or a
numbered section of any document is noise to a reader without that artifact. It rots the moment
the artifact moves.

- [ ] Comments never cite documents by number:
  - no PLAN/CHECKLIST/MASTER references
  - no milestone or ruling ids
  - no `§N`
  - no `finding #N`
  - no vendor-doc section numbers
- [ ] A comment states its WHY in its own words or does not exist.
- [ ] Existing offenders = cleanup debt, not precedent.

### Amended 2026-08-07: comment-cleanup pass

WHY: agents write comments at volume, and a writer cannot proof its own tone.

- [ ] IF agents of a multi-agent run wrote or rewrote comments, THEN the run ends with a dedicated
  comment-cleanup subagent pass (the rex-cleaner-comments charter). The run is not complete before
  that pass.
- [ ] The pass enforces this whole section, including the no-document-references rule above.

## 11. Dependencies

- [ ] Allowlist = table + crate notes below. Allowlist = constitutional.
- [ ] To add a crate, do a major upgrade, or expand features: amend this doc with one-line
  justification.
- [ ] Always: at adoption, make sure version = latest stable. `default-features = false`. Only
  features that are in use.

| Crate                                                        | Status                                             | Role                             |
| ------------------------------------------------------------ | -------------------------------------------------- | -------------------------------- |
| `tokio`                                                      | ratified. See note.                                | async actor runtime              |
| `rtrb`                                                       | ratified                                           | SPSC rings                       |
| `serde` + `serde_json`                                       | ratified                                           | exchange payload deserialization |
| `thiserror`                                                  | ratified                                           | library error types              |
| `anyhow`                                                     | ratified (binaries only)                           | binary error context             |
| `parquet` (`arrow`, `zstd`) + `arrow-array` + `arrow-schema` | ratified 2026-07-18. See note.                     | persistence                      |
| `proptest`                                                   | ratified (dev)                                     | fitness suite                    |
| `tokio-tungstenite`                                          | ratified (user directive 2026-07-18). See note.    | WS streams                       |
| `reqwest`                                                    | ratified (user directive 2026-07-18). See note.    | REST snapshots + backfills       |
| `futures-util`                                               | ratified 2026-07-18. See note.                     | WS stream/sink combinators       |
| `serde-saphyr`                                               | ratified 2026-07-18. See note.                     | configuration                    |
| `core_affinity`                                              | ratified 2026-07-18. See note.                     | pinning                          |
| `eframe` (`wgpu`, `accesskit`, `default_fonts`)              | ratified 2026-07-23, amended 2026-07-25. See note. | desktop GUI (feature `ui`)       |
| `ring`                                                       | ratified 2026-07-27. See note.                     | request signing                  |
| `k256`                                                       | ratified 2026-08-07. See note.                     | secp256k1 signing                |
| `tiny-keccak`                                                | ratified 2026-08-07. See note.                     | keccak256                        |
| `rmcp`                                                       | ratified 2026-10-14. See note.                     | MCP server (feature `mcp`)       |
| `rustix`                                                     | ratified 2026-10-14. See note.                     | SIGTERM (feature `mcp`)          |
| `smol_str`                                                   | candidate: only on demonstrated ergonomic need     | edge string handling             |
| `criterion`                                                  | candidate (dev): only if benchmark warranted       | measurement                      |

### Crate notes

Each note = justification + limits of that crate's row. Same force as the row.

- `tokio`
  - Features grow by milestone. Minimal set only. Never umbrella `full`.
  - M09: `rt` + `time`.
  - M11: `net`.
  - M12 adds `rt-multi-thread` + `signal` + `macros`. For: the config-sized multi-worker runtime
    that PROJECT §2 mandates, SIGINT/SIGTERM-driven graceful drain, and `select!` on the shutdown
    wait.
  - Under feature `mcp`, rmcp adds `io-std`. `io-util` and `sync` were already unified in.
    polysim adds no tokio feature itself.
- `parquet` (`arrow`, `zstd`) + `arrow-array` + `arrow-schema`
  - M09: arrow-rs 59.1.0.
  - For: ArrowWriter batched append + footer KV + explicit row-group flush + arrow reader for M12
    read-back.
  - `arrow` feature pulls only needed arrow-\* sub-crates, not umbrella.
  - ZSTD = doc-recommended balance codec. Encode is off hot path.
  - MSRV 1.85 < toolchain.
  - Breaking majors ≤ quarterly. Three crates lock-step.
- `tokio-tungstenite`
  - 0.30, `connect` + `rustls-tls-webpki-roots`: ring stack.
- `reqwest`
  - 0.12 PINNED with `rustls-tls`: ring + webpki, ONE TLS stack shared with tungstenite.
  - 0.13 rejected: it bundles aws-lc-rs second crypto provider + C toolchain.
- `futures-util`
  - M11: `StreamExt::next`/`SinkExt::send` on WebSocketStream. tungstenite does not re-export
    them.
  - `sink` + `std` features only.
  - Already transitive in lock.
- `serde-saphyr`
  - YAML config.
  - `serde_yaml` = archived. `serde_yaml_ng`/`serde_norway` forks = stale >12mo.
  - saphyr family alone is actively maintained. It is pure-safe-Rust.
  - It honors `deny_unknown_fields`, tagged enums included, with field-naming errors. Verified.
- `core_affinity`
  - Hot-thread CPU pinning.
  - Maintained + far wider use than `affinity`.
  - Per-OS implementations.
  - `set_for_current` needs no unsafe on our side.
  - macOS: best-effort no-op.
- `eframe` (`wgpu`, `accesskit`, `default_fonts`)
  - Native macOS trading GUI. PINNED `=0.35.0`.
  - `wgpu`: Metal renderer. `accesskit`: native accessibility.
  - `default_fonts` = TEMPORARY until repo-owned faces are embedded. Then drop it.
  - egui/eframe types are UI-thread only. They never cross the hot-path boundary (§2).
  - No `egui_plot`/docking/table/theme crate. No direct `wgpu` dep.
  - Upgrades deliberate: egui breaks APIs between minors. Re-verify the handbook on any bump.
  - `optional = true` behind feature `ui`.
    - Amended 2026-07-25. WHY: a headless trading engine deployed in another region must not need
      wgpu/winit/X11/wayland merely to BUILD.
    - `desktop/` is `#[cfg(feature = "ui")]`.
    - The `polysim-ui` bin + the `dom-fixture` example carry `required-features = ["ui"]`.
    - The §14 gate consequence binds.
- `ring`
  - HMAC-SHA256 signing of Binance private WS-API + REST requests.
  - ALREADY transitive in the lock via rustls' ring stack, reached from both `reqwest` and
    `tokio-tungstenite`. This is the `futures-util` precedent. Thus the direct dep adds no build
    cost.
  - A RustCrypto `hmac` + `sha2` pair would put a SECOND crypto stack in the tree for no gain.
  - Only `ring::hmac` is used.
  - Hex encoding and query-string building are hand-rolled. `hex`/`form_urlencoded` are not
    pulled.
  - `default-features = false` and NO features. `ring::hmac` carries no `alloc` gate. Only
    `ring::rsa` does. Verified against ring 0.17.14 `src/lib.rs:142`.
- `k256`
  - Polymarket execution plan.
  - secp256k1 recoverable ECDSA for the EIP-712 `ClobAuth` and order signatures. The venue's
    entire auth model rests on those signatures.
  - `ring` exposes NO secp256k1. Thus there is no in-stack alternative.
  - `default-features = false` + `ecdsa` only.
  - Signing is RFC-6979 DETERMINISTIC. Thus no RNG is used, no `getrandom` edge is added, and
    every signature is replayable under §2 and pinnable by vector.
  - Pure Rust. MSRV 1.85 < toolchain.
  - Knowingly a SECOND crypto stack beside `ring`. `ring` keeps L2 HMAC. Accepted because the
    only alternative is `alloy`: an enormous tree for two struct hashes.
  - Honest cost, measured not assumed: `rfc6979` drags RustCrypto `hmac` + `sha2` in
    transitively, and neither was in the lock before. Unavoidable: RFC-6979 is defined in terms of
    HMAC over the curve's digest.
- `tiny-keccak`
  - Polymarket execution plan.
  - keccak256 for EIP-712 struct/domain hashing, EIP-55 address checksums, and the ERC-7739 wrap.
  - Ethereum uses ORIGINAL Keccak padding, NOT SHA3-256. Thus neither `ring` nor `sha2` can
    supply it.
  - `default-features = false` + `keccak` only.
  - Its one dependency, `crunchy`, was already in the lock. Thus it adds no build cost.
- `rmcp`
  - The official MCP SDK. 3.5.1 = latest stable at adoption. MSRV 1.88 < toolchain.
  - Features: `server`, `macros`, `transport-io`. `client` only as a dev-dependency, for the
    fitness suite's in-process client.
  - `optional = true` behind feature `mcp`. Only the `polysim-mcp` bin requires it. Thus a trading
    engine never builds it.
  - Honest cost, measured: 18 new crates in the lock (darling, darling_core, darling_macro,
    dyn-clone, futures, futures-executor, ident_case, pastey, ref-cast, ref-cast-impl, rmcp,
    rmcp-macros, schemars, schemars_derive, serde_derive_internals, strsim, tokio-stream,
    tokio-util).
  - `tracing` is an unconditional rmcp dependency. It was already in the lock via reqwest ->
    hyper-util. The §7 2026-10-14 amendment binds it.
  - A dev-dependency cannot be optional. Thus every test build compiles rmcp's client half, with
    or without `mcp`. A deploy build never builds dev-dependencies.
- `rustix`
  - 1.1.5 = latest stable at adoption. `default-features = false` + `process` only.
  - The only safe-Rust way to send SIGTERM under `forbid(unsafe_code)`. std's `Child::kill` sends
    only SIGKILL, which skips the engine's drain. SIGKILL still goes through std.
  - Honest cost: it was in the lock at 1.1.4 only through eframe -> zbus/accesskit_unix, which is
    Linux-only. Thus a macOS build or an mcp-only build compiles it for the first time. This is
    not the `ring` precedent.
  - `optional = true` behind feature `mcp`.

## 12. Testing

- [ ] Two tiers only: fitness suite + live-network tests.
  - Amended 2026-07-18: unit tier removed. WHY: test mass concentrates in fitness + human-run
    integration.
- [ ] Fitness suite: small, immortal. `tests/fitness/`, proptest + recorded fixtures. The only CI
  tests. Content:
  - FastQueue never loses/reorders data
  - order-book snapshot+deltas ≡ reconstructed state
  - ingress ordering rules
  - zero steady-state alloc
  - adapter parse/sequencing vs recorded WS + REST payloads
  - link wire format (amended 2026-07-25)
  - quant leaf-math pins (amended 2026-07-28)
- [ ] Live-network tests: `tests/integration/`, gated `#[ignore]`. Never CI. The agent runs them
  deliberately.
  - Amended 2026-07-18. WHY: user directive. Agent runs ALL tests, live-network included. Network
    reach to venue endpoints is verified. Human is not required.
- [ ] Contract seam tests only when specific seam proves unstable.
- [ ] IF bug is found, THEN write failing fitness regression first. Then fix.
- [ ] No mocks unless forced. Preferred fixture = recorded real exchange payloads.
- [ ] §2 determinism makes replay fixtures the backbone of testing.

### Amended 2026-07-20: inline quant unit tests (WITHDRAWN 2026-07-28)

Not in force. Record only.

- WHY then: pure quant calculators can carry inline unit tests.
- Text then: the amendment let `hot/quant` numeric modules pin leaf math with inline
  `#[cfg(test)]` tests. Those tests were kept to the FEW highest-value pins (core correctness +
  found-bug regressions), so the fitness charter stays architectural. It was not a general
  reopening of the unit tier: it was scoped to pure calculators, and sparseness was deliberate.

### Amended 2026-07-25: link wire format in fitness

WHY: the link hand-rolls its wire format across a process boundary. Thus encode/decode agreement
is an architectural invariant. Socket behavior is not, and the charter stays architectural.

- [ ] Fitness gains the link wire format:
  - encode/decode round-trip proptest per frame kind
  - the `(sender, boot, topic)` seq-gate logic (dedup, reorder, restart resets the gate)
- [ ] Socket-level behavior stays OUT: subscription TTL expiry, subscribe refresh, the
  peer-restart gate over a real socket. It is contract-seam/integration territory.

### Amended 2026-07-28: tests live only under `tests/`

WHY: user directive. `src/` carries production code only, and a reader who looks for a test looks
in ONE place.

- [ ] The 2026-07-20 carve-out is WITHDRAWN.
- [ ] No `#[cfg(test)]` block lives anywhere outside `tests/`: not in `src/`, not in `strategies/`,
  not in `tools/`.
- [ ] Quant leaf-math pins live in the fitness target under `tests/fitness/quant/`. One module per
  calculator, named for the concept, not the source path.
- [ ] The cost is deliberate and binding: a test under `tests/` sees only the crate's PUBLIC API.
  Thus re-express a pin on a private item against a public seam, or drop it.
- [ ] Forbidden: `pub(crate) -> pub` to make a test compile. WHY (§8): `pub` is a promise, and a
  test is not a caller worth promising to.
- [ ] An item can still be promoted on its OWN merits by explicit ruling.
- [ ] A test that follows a merit-based promotion is a consequence of it, never the argument for
  it.
- Two items were promoted on the day this passed:
  - `runtime::ExecutionLease`: exclusive live-execution ownership is an engine-level promise that
    its already-exported peers imply.
  - `hot::quant::optimise`: every sibling calculator module was `pub` already. It was the lone
    exception.

## 13. Performance

- [ ] Design principles protect performance, not a standing benchmark mandate. Principles: zero
  steady-state alloc, contiguous fixed layouts, integer keys, no strings/formatting/syscalls on hot
  path.
- [ ] Review rejects premature optimization.
- [ ] IF performance question arises, THEN profile first. Act on evidence.
- [ ] Predictable performance beats peak performance.
- [ ] Reader must infer hot-path cost model from source.
- [ ] Branch temperature: declared, not guessed.
  - Amended 2026-07-18. WHY: hot/cold split + inlining hints are design, not premature
    optimization.
  - `#[cold]`: on failure/drain/reconnect/capacity-hit functions reachable from hot path.
  - `#[inline]`: on small hot accessors + newtype ops.
  - `#[inline(always)]`: sparingly. Tiny leaf functions on proven hot path only. Inline WHY
    required.
- [ ] Cache-line layout on hot structures where it makes sense.
  - Amended 2026-07-18. WHY: predictable memory movement is §3's whole point.
  - Group hot fields.
  - `#[repr(align(64))]` / padding against false sharing on cross-thread-touched state.
  - Contiguous arrays over pointer-chasing.
  - Layout choice = visible in review, not accidental.

## 14. Toolchain & Gates

- [ ] Toolchain: pinned stable via `rust-toolchain.toml`. Edition 2024. Upgrades deliberate.
- [ ] Gate every change: `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` +
  `cargo test`. All green or no merge.
- [ ] Run clippy + test in BOTH configurations: no features, and `--all-features` (`ui` + `mcp`).
  - Amended 2026-07-25. WHY: `eframe` is optional behind feature `ui` (§11), and a single-config
    gate rots the path it never compiles.
  - Amended 2026-10-14. WHY: `mcp` joins `ui` as an optional feature, and one `--all-features` run
    compiles both gated paths.
  - Replaced 2026-10-14: the second configuration was `--features ui`.
- [ ] `#[allow]` requires inline WHY.
- [ ] `#![forbid(unsafe_code)]` at crate root. An amendment requires a benchmark that proves safe
  implementation cannot meet measured requirement.

## 15. Amendments

- [ ] Changes only by explicit agreement, never silent.
- [ ] Every amendment carries one-line WHY.
- [ ] PROJECT.md, PLAN.md, CHECKLIST.json must comply. IF they cannot, THEN surface conflict +
  resolve it here first.

## 16. Smell Doctrine

Added 2026-08-07 (user directive). Operative playbook = the `rex-code-smells` skill. This section
is its constitutional core.

- [ ] Detection ≠ fixing.
- [ ] A finding = file:line + evidence + concrete cost + one-line fix direction.
- [ ] Severity, three only: BLOCK (bug factory) / REFACTOR (taxes every future edit) / NIT (real
  but cheap).
- [ ] Sweeps (grep, clippy) yield CANDIDATES. Only reading in context confirms. One false positive
  poisons the whole report.
- [ ] Clean is a valid verdict. A fabricated finding costs more trust than an empty report.
- [ ] Naming lens:
  - booleans are predicates
  - conversion prefixes tell the truth (`as_` free view, `to_` does work, `into_` consumes)
  - no noise words
  - domain primitives are newtyped, so a swapped argument cannot compile
- [ ] Brittleness lens:
  - `unwrap`/`expect` never meets external input
  - knowledge is encoded once: the same magic literal in two places is a defect
  - ordering is enforced by types, not asserts
  - every seam needs its writer AND its reader: data without control flow compiles green and lies
  - tests pin behavior, never formatting
- [ ] Structure lens:
  - the deletion test (§1) kills speculative generality
  - a long function is N jobs sharing one name
  - an `Option` field alive in only one phase is a missing enum state
  - the same match in three files is one change costing three edits
