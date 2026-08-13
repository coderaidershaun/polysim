# Polysim Constitution

Permanent engineering law for polysim market-data + quant engine. All docs, modules, code
comply or conflict surfaced explicitly. Amendments = §15.

## 1. Prime Directives

- Simple beats clever. Straightforward design meets requirements w/o heavy perf loss -> wins.
  No exceptions for interesting ideas.
- Abstraction must pay rent at call sites. Deletion test: remove module -> complexity
  vanish = pass-through, delete it.
- One design language. Same problem -> same solution everywhere.
- Keep comments to a minimum. Prefer ergonomics over comments. Use /rex-utils-caveman skill for commenting.
- Aim high, ship lean. 80% value from 20% code. No early factoring — cut points emerge,
  then trap complexity behind narrow interface.
- No source file > 500 lines. Amended 2026-07-21 (strategy impls are one-file customer code; a
  forced split serves no reader): files under strategies/ are exempt. Amended 2026-07-22 (the cap
  buys navigable production logic; inline pins appended at the bottom obstruct no reader, and §12
  invites them on quant calculators — counting them taxes testing): the limit counts production
  lines only, `#[cfg(test)]` blocks excluded. Amended 2026-07-26 (a file under tests/ is ENTIRELY
  test code that merely sits in no `#[cfg(test)]` block, because a test target needs none — the
  2026-07-22 carve-out's own logic reaches it): the cap counts production lines under src/ and in
  binaries; files under tests/ are exempt.

## 2. Execution Model

- Two domains, permanent: tokio async actors at I/O edge + ONE synchronous hot-path thread
  owning all market state. Nothing else touches that state.
- Hot path: no async, no locks, no shared mutable state, no blocking syscalls in steady
  state. All communication w/ hot path via fixed-capacity SPSC rings (`rtrb`). Hard max
  20 input queues.
- Single-writer: every piece of state has exactly one owning thread.
- CPU pinning required. Hot thread pinned; other affinity = design decision, not afterthought.
- Hot-path state = pure fn of ordered input message sequence. Time-driven behaviour (spin
  ticks) arrives as messages from async timer actor. Wall clock never drives state
  transition on hot thread. Clock reads there only stamp latency metrics.
- Consequence: replay recorded input sequence -> identical hot-path state. Basis of fitness
  tests + future backtesting. Never break.
- Amended 2026-07-29 (M13 `exchange-sim`; ratified O1): the edge-actor rule governs WHERE STATE
  LIVES, not whether a socket exists. An edge actor may SYNTHESISE a venue instead of reaching one.
  The determinism above is untouched and is the reason this is safe: a synthesising actor's output
  is ordinary `InboundMessage` traffic on an ordinary input queue, so replaying a recorded tape
  reproduces hot state exactly, and the hot thread cannot tell which side of the socket its input
  came from. Binding consequence: such an actor is still an EDGE actor and owns no hot state, and
  its own decisions must be a pure fn of stamped input messages — reading a clock to decide what to
  emit would relocate the nondeterminism rather than remove it.

## 3. Memory

- Fixed capacity wherever practical: preallocated at startup, contiguous, predictable
  layout, cache-friendly access.
- Heap = initialisation only. Steady-state hot path: zero alloc, zero dealloc, zero resize.
  Enforced by counting-allocator fitness test, not trust.
- Capacity hit = designed event w/ explicit per-structure policy. Never silent growth.
- Memory cheap; latency variance not. Trade memory for simplicity + predictable movement
  (FastQueue oversized backing allocation = canonical example).
- No strings in hot-path lookups. Identity = compact typed indices assigned from config at
  startup. String-like types live at edges.

## 4. Numerics

- Prices/quantities cross adapter boundary exactly once: exchange decimal strings -> `i64`
  fixed-point mantissas (global scale fixed during design). Book keys = exact integers:
  exact equality, total ordering, hashable.
- `f64` only downstream of exact state: derived features, statistics, model math.
- Float never: a key, a money accumulator, an equality operand in book logic.

## 5. Time

- µs resolution everywhere. Every timestamp field ends `_ts_us` (`exchange_ts_us`,
  `received_ts_us`, `queued_ts_us`, `processed_ts_us`).
- Timestamps/durations = typed wrappers w/ ergonomic ops (`a.diff(b)`). Bare `i64` never
  crosses an API. Exact type set fixed in design.
- Exchange time ≠ local receive time ≠ monotonic latency clock. Never silently interchanged.

## 6. Errors & Failure

- Two failure classes, never confused:
  - **External, expected** (disconnect, malformed frame, HTTP fail, full queue, full
    disk): typed `thiserror` per concept. Variant = what failed + values seen. Handled by
    policy: reconnect, backoff, resync, drop-count. Queue-full policy asymmetric by
    design: INPUT queue full = fatal drain (engine can't keep up); OUTPUT queue full =
    drop + count.
  - **Internal invariant violation** (bug: corrupt book, impossible index, broken
    sequence logic): assert + panic immediately. Fail fast, fail loud.
- Invariant panic -> coordinated bounded drain: flush persistence queue, close Parquet
  clean, exit non-zero. Drain deadline HARD. Corrupt state never keeps producing research
  data.
- Lib = `thiserror`. Bin = `anyhow` + `.context()` on every `?`.
- Forbidden: `Box<dyn Error>` in any API, `Result<T, String>`, catch-all `Other(String)`,
  `.unwrap()` outside tests (`.expect("WHY")` only at init boundaries), `let _ =` swallow.
- Messages: lowercase, no trailing period, concrete values included, actionable when fix
  known. Every pub `Result` fn documents `# Errors`.
- Amended 2026-07-25 (fatal-on-full says "the engine can't keep up" — a remote peer flooding a UDP
  port is an UNTRUSTED producer, and letting it trip `FatalSignal` hands anyone who can reach the
  port a remote-kill primitive): an INPUT queue fed by an untrusted remote producer (the link) is
  drop + count, not fatal drain. Engine-fed input queues keep fatal-on-full. Dropping is sound ONLY
  under the semantic contract that makes a dropped frame equivalent to one never sent: **link topics
  carry STATE (absolute values), never deltas or events.** A strategy folding a delta stream through
  `on_link` breaks silently on the first drop, so the contract is constitutional law, not a code
  comment. The drop happens at the edge and recording happens downstream of it, on the hot side, so
  the tape holds only consumed frames (§2).
- Amended 2026-07-27 (the execution command ring carries EVENTS, not state, so drop+count's own
  precondition fails): OUTPUT-queue drop + count is sound because the consumer needs only the LATEST
  state — a dropped frame must be equivalent to one never sent. An order command breaks that: a
  dropped `Cancel` leaves an order resting at a price the strategy abandoned. The precondition is
  RESTORED, not waived. The hot thread never pushes to the execution ring directly; it transitions
  order state at BANK time into a fixed-capacity buffer, and a separate step drains that buffer into
  the ring FIFO, stopping on the first failure and retrying next spin — the latched pattern
  `PersistSink::request_seal` already uses for the one record that must never be dropped. Ring
  pressure therefore delays the WIRE, never the STATE. Two consequences bind: the sink still counts
  drops (§6's letter holds, and in steady state the count is unreachable), and the reachable failure
  becomes the bank's own high-water mark — a message-driven count, not a ring occupancy, so §2
  replay determinism survives. Gating the hot pass on ring space instead would make hot state a
  function of how fast the ASYNC consumer drained, i.e. of wall time, and replay would diverge.
- Amended 2026-08-08 (a venue's order-rate budget gained its first enforcement, and an edge-side
  pacer that silently delayed placements would falsify the strategy's time-to-market assumption
  while making order flow a function of venue budget drain): rate-limit pacing, like ring
  pressure, delays the WIRE, never the STATE. The compliant shape is a hot-side REFUSAL that is a
  pure function of message stamps — the placement is not minted this spin and is counted under its
  own reject reason — so a replay refuses at exactly the same point. An adapter never holds a
  banked command back to satisfy a budget.

## 7. Logging

- Bespoke minimal subsystem. No logging framework dependency.
- Exactly three levels: `INFO`, `WARN`, `ERROR`.
- `ERROR` captures backtrace + call-site file/line by construction. API makes omission
  impossible.
- Producers hand records to dedicated logging thread. No thread blocks or formats inline.
- Debug builds -> terminal. Release builds -> efficient file sink.
- Hot path steady state emits nothing. `WARN`/`ERROR` from hot path OK — exceptional by
  definition.
- Amended 2026-07-20 (strategy telemetry is research output, not engine noise): strategy
  callbacks may emit per-tick telemetry through the banked ctx logging lane on a dedicated
  ring. ONE sink per run, named by strategy id — `logs/<strategy-id>-<date>.log` in release,
  terminal in debug — carrying engine-origin and strategy-origin records alike; the record's
  `[tag]` column (thread tag or strategy id) distinguishes origin, so one file loses nothing.
  The dedicated ring is drop isolation, not routing: a telemetry flood fills its own ring
  instead of displacing engine `WARN`/`ERROR`. Engine hot-path code remains bound by
  steady-state silence. Strategy records stamp event time (§2 purity) and format into the
  fixed-size record at bank time; strategy-lane ERROR carries file/line, no backtrace
  (capture allocates — §3 wins on the hot thread). Lane full = drop + count (§6).
- Amended 2026-07-23 (eframe pulls the `log` facade deeper into the graph — the no-framework rule
  needs an explicit carve-out): `log` is transitive-only (already via reqwest/tungstenite, now also
  eframe/winit/wgpu), never a polysim choice. No polysim module may call it; the bespoke
  subsystem stays the ONLY polysim logging API. No `log` logger is installed, so dependency facade
  emissions route nowhere (dropped at the uninitialised facade). Rule intact: polysim neither
  depends on `log` directly nor logs through it.
- Amended 2026-07-25 (N trading engines per strategy fight over one strategy-id-named file): the
  sink is named by the two-part identity (§8) — `logs/<strategy-id>-<te-id>-<date>.log`, ONE per TE
  run. Everything else in the 2026-07-20 amendment stands.
- Amended 2026-07-29 (M13 `exchange-sim`; ratified O5 — simulated fills that read as live ones are
  an execution-safety defect, not an observability preference): a run under `mode: sim` writes
  `logs/<strategy-id>-<te-id>-sim-<date>.log`. The live stem is unchanged BYTE-FOR-BYTE, so no
  existing path moves and `off`/`live` runs are indistinguishable from today. Physical separation
  rather than an in-line marker is deliberate: a `[tag]` column distinguishes origin only to a
  reader who is already reading, whereas a glob, a tail, or an operator grabbing "the log" must not
  be able to reach simulated fills by accident. The same ruling binds the sibling artifacts —
  exposure `<strategy-id>-<te-id>-sim.json`, Parquet below `<strategy>/<te>/sim/` with an
  `execution_mode` footer, and the UI mode badge (O6).

## 8. Code Organisation

- One lib crate (`polysim`) holds all substance. Binaries only compose: load config,
  wire, start, orchestrate shutdown.
- Module = concept (`hot/`, `adapters/`), not type-per-file. Every module opens w/ 1–2
  line `//!` WHY header.
- `hot/`: no async code, no exchange-specific wire formats. Adapters normalise at edge;
  generalised internal messages = only currency inward. Review-enforced; workspace split
  only if seam demonstrably needs compiler help.
- `pub(crate)` default. `pub` = promise.
- Amended 2026-07-20 (strategy impls are customer code, §9): strategy implementations live
  outside the lib in `strategies/<id>/` — main.rs (composition only) + strategy.rs +
  config.yaml; bin name = folder name = strategy id via `env!("CARGO_BIN_NAME")`. The lib
  keeps the seam: `Strategy`, `StrategyConfig`, `StrategyCtx`, the Actions bank,
  `run_strategy`. Breaking data-format change = new folder/id. Amended 2026-07-23 (a recorder
  outgrew one readable file): a strategy folder may carry helper modules beside strategy.rs,
  declared from strategy.rs via `#[path]` so both compile roots — the bin and the fitness `#[path]`
  include — resolve the same flat sibling files; strategy.rs stays the single entry. The §1
  strategies/ exemption from the 500-line cap covers these siblings too — they are the same
  customer code, split only for the reader. Amended 2026-07-25 (a strategy is now a SET of
  independently deployable trading engines, each bound 1:1 to one source, so identity is two-part
  and the folder nests): a strategy folder holds one folder per trading engine —
  `strategies/<strategy-id>/<te-id>/` carrying main.rs (composition only) + strategy.rs + its
  `#[path]` siblings + config.yaml. Identity = `(strategy-id, te-id)`; bin name =
  `<strategy-id>-<te-id>` and path = `strategies/<strategy-id>/<te-id>/main.rs`, with both ids
  passed as literals to the lib seam's runner, now `run_trading_engine::<S>(strategy_id, te_id)` —
  `env!("CARGO_BIN_NAME")` no longer derives identity. Enforcement is structural: a fitness
  config-guard parses `Cargo.toml` and asserts that name/path pair on every `[[bin]]`, so global
  bin-name uniqueness is a test failure, not a hope. Cross-TE sharing has exactly ONE authorised
  shape — a `#[path]` sibling at strategy level (`strategies/<strategy-id>/common.rs`) included from
  each te strategy.rs; otherwise per-TE duplication is accepted (customer code, §9). Breaking
  data-format change = new te folder/id.
- Amended 2026-08-08 (three venues implement the execution seam by convention; a fourth needs law,
  not archaeology): a venue execution adapter is a COMPOSITION over `adapters/exec` — it drives
  `ExecCore` through its own codec and transport, funnels every exec and account event through its
  ONE assigned input queue stamped monotonically, declares its venue physics in a
  `VenueCapabilities` record and its lease-nonce namespace beside it at construction, and
  implements the `EdgeDriver` session chassis rather than its own lifecycle loop. Shared exec
  machinery (core, mirror, in-flight table, resync pass, exit lifecycle, synthesised-event
  constructors) is never re-implemented per venue. The obligations no signature can carry —
  cumulative totals, reconcile-never-resend, cancel finality, readiness order, fatal on an
  undecodable open order — live in `src/adapters/exec/README.md` and their fitness pins; the
  README is the adapter contract's prose half and changes to it are seam changes, not doc edits.

## 9. Naming & API Ergonomics

- rex-code-ergonomics binds. No abbreviations (`cfg`, `ctx` idiomatic OK). Predicates
  `is_`/`has_`/`can_`. Verbs on fns, nouns on types.
- Domain primitives = newtypes (`SourceId`, `InstrumentId`, `Price`, `Qty`, `TsUs`, …).
  Bare `u64` never impersonates another.
- No `bool` params in pub fns. >3 params -> struct. Accept `&str`/`&[T]`/`&Path`. Return
  owned when allocating. Behaviour = methods on type, not free fns in util modules.
- Derive `Debug, Clone, PartialEq, Eq, Hash` wherever semantics allow.
- Flat beats nested (amended 2026-07-18: deep indent pyramids hide control flow). Guard
  clauses / early return / `let-else` / extracted fns over branch-in-branch. Deep nesting
  = review flag; restructure before merge.
- Conditional fits one line -> one line (amended 2026-07-18: trivial branches don't earn
  vertical space). Enforced by fmt gate, not hand-formatting: `rustfmt.toml` sets
  `single_line_if_else_max_width` + `single_line_let_else_max_width` = 100. Statement `if`
  w/o else always multi-line under rustfmt — prefer expression form / `matches!` /
  combinators where the one-liner reads better.
- APIs designed from call site backwards. Strategy code = customer.

## 10. Comments & Documentation

- rex-code-commenting binds. WHY only, never WHAT. Default = no comment. Fix design first
  (rename, extract, newtype); comment only what code cannot say.
- In-body comments only when behaviour genuinely can't be self-evident: vendor quirk,
  trap, measured justification.
- Every module: `//!` why-header. Pub item: `///` ONLY when contract exceeds name +
  signature (invariants, units, panics, warnings, non-obvious errors). Self-evident pub
  item = zero docstring. Never restate signature.
- Forbidden: commented-out code, history notes, drifting TODOs (ticket or fix), section
  dividers (extract fns instead), generated boilerplate commentary.
- Amended 2026-08-07 (user directive: a comment citing a plan, milestone id, audit finding
  number, or a numbered section of any document is noise to a reader without that artifact and
  rots the moment the artifact moves): comments never cite documents by number — no
  PLAN/CHECKLIST/MASTER references, no milestone or ruling ids, no `§N`, no `finding #N`, no
  vendor-doc section numbers. A comment states its WHY in its own words or does not exist.
  Existing offenders are cleanup debt, not precedent.
- Amended 2026-08-07 (agents write comments at volume, and a writer cannot proof its own tone):
  any multi-agent run whose agents wrote or rewrote comments ends with a dedicated
  comment-cleanup subagent pass (the rex-cleaner-comments charter) before the run is complete.
  The pass enforces this whole section, including the no-document-references rule above.

## 11. Dependencies

- Allowlist below = constitutional. Adding crate, major upgrade, or feature expansion =
  amend this doc w/ one-line justification.
- Always: latest stable verified at adoption, `default-features = false`, only features
  actually used.

| Crate                                                                                | Status                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Role                             |
| ------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------- |
| `tokio`                                                                              | ratified (features grow by milestone, minimal set only, never umbrella `full`: M09 `rt`+`time`, M11 `net`; M12 adds `rt-multi-thread`+`signal`+`macros` — the config-sized multi-worker runtime PROJECT §2 mandates, SIGINT/SIGTERM-driven graceful drain, and `select!` on the shutdown wait)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | async actor runtime              |
| `rtrb`                                                                               | ratified                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | SPSC rings                       |
| `serde` + `serde_json`                                                               | ratified                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | exchange payload deserialisation |
| `thiserror`                                                                          | ratified                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | library error types              |
| `anyhow`                                                                             | ratified (binaries only)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | binary error context             |
| `parquet` (`arrow`, `zstd`) + `arrow-array` + `arrow-schema`                         | ratified 2026-07-18 (M09: arrow-rs 59.1.0 — ArrowWriter batched append + footer KV + explicit row-group flush + arrow reader for M12 read-back; `arrow` feature pulls only needed arrow-\* sub-crates, not umbrella; ZSTD = doc-recommended balance codec, encode off hot path; MSRV 1.85 < toolchain; breaking majors ≤ quarterly, three crates lock-step)                                                                                                                                                                                                                                                                                                                                                                                                                               | persistence                      |
| `proptest`                                                                           | ratified (dev)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | fitness suite                    |
| `tokio-tungstenite`                                                                  | ratified (user directive 2026-07-18; 0.30 `connect`+`rustls-tls-webpki-roots` — ring stack)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | WS streams                       |
| `reqwest`                                                                            | ratified (user directive 2026-07-18; 0.12 PINNED w/ `rustls-tls` — ring+webpki, ONE TLS stack shared w/ tungstenite; 0.13 rejected: bundles aws-lc-rs second crypto provider + C toolchain)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | REST snapshots + backfills       |
| `futures-util`                                                                       | ratified 2026-07-18 (M11: `StreamExt::next`/`SinkExt::send` on WebSocketStream — tungstenite doesn't re-export them; `sink`+`std` features only, already transitive in lock)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | WS stream/sink combinators       |
| `serde-saphyr`                                                                       | ratified 2026-07-18 (YAML config: `serde_yaml` archived, `serde_yaml_ng`/`serde_norway` forks stale >12mo; saphyr family alone actively maintained, pure-safe-Rust, honours `deny_unknown_fields` incl. tagged enums w/ field-naming errors — verified)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | configuration                    |
| `core_affinity`                                                                      | ratified 2026-07-18 (hot-thread CPU pinning: maintained + far wider use than `affinity`, per-OS impls, `set_for_current` needs no unsafe on our side, macOS best-effort no-op)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | pinning                          |
| `eframe` (`wgpu`, `accesskit`, `default_fonts`)                                      | ratified 2026-07-23 (native macOS trading GUI, PINNED `=0.35.0`: `wgpu`→Metal renderer + `accesskit` native accessibility; `default_fonts` TEMPORARY until repo-owned faces are embedded, then dropped; egui/eframe types are UI-thread only, never cross the hot-path boundary (§2); no `egui_plot`/docking/table/theme crate, no direct `wgpu` dep; upgrades deliberate — egui breaks APIs between minors, re-verify the handbook on any bump; `optional = true` behind feature `ui`, amended 2026-07-25 — a headless trading engine deployed in another region must not need wgpu/winit/X11/wayland merely to BUILD: `desktop/` is `#[cfg(feature = "ui")]`, the `polysim-ui` bin + the `dom-fixture` example carry `required-features = ["ui"]`, and the §14 gate consequence binds) | desktop GUI (feature `ui`)       |
| `ring`                                                                               | ratified 2026-07-27 (HMAC-SHA256 signing of Binance private WS-API + REST requests; ALREADY transitive in the lock via rustls' ring stack, reached from both `reqwest` and `tokio-tungstenite` — the `futures-util` precedent, so the direct dep adds no build cost. A RustCrypto `hmac`+`sha2` pair would put a SECOND crypto stack in the tree for no gain. Only `ring::hmac` is used; hex encoding and query-string building are hand-rolled rather than pulling `hex`/`form_urlencoded`. `default-features = false` and NO features — `ring::hmac` carries no                                                                                                                                                                                                                         |
| `alloc` gate, only `ring::rsa` does, verified against ring 0.17.14 `src/lib.rs:142`) | request signing                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `k256`                                                                              | ratified 2026-08-07 (Polymarket execution plan: secp256k1 recoverable ECDSA for the EIP-712 `ClobAuth` and order signatures the venue's entire auth model rests on — `ring` exposes NO secp256k1, so there is no in-stack alternative. `default-features = false` + `ecdsa` only; signing is RFC-6979 DETERMINISTIC, so no RNG is used, no `getrandom` edge is added, and every signature is replayable under §2 and pinnable by vector. Pure Rust, MSRV 1.85 < toolchain. Knowingly a SECOND crypto stack beside `ring`, which keeps L2 HMAC: accepted because the only alternative is `alloy`, an enormous tree for two struct hashes. Honest cost, measured not assumed — `rfc6979` drags RustCrypto `hmac`+`sha2` in transitively and neither was in the lock before; unavoidable, since RFC-6979 is defined in terms of HMAC over the curve's digest) | secp256k1 signing               |
| `tiny-keccak`                                                                       | ratified 2026-08-07 (Polymarket execution plan: keccak256 for EIP-712 struct/domain hashing, EIP-55 address checksums, and the ERC-7739 wrap. Ethereum uses ORIGINAL Keccak padding, NOT SHA3-256, so neither `ring` nor `sha2` can supply it. `default-features = false` + `keccak` only; its one dependency, `crunchy`, was already in the lock, so it adds no build cost) | keccak256                       |
| `smol_str`                                                                           | candidate — only on demonstrated ergonomic need                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | edge string handling             |
| `criterion`                                                                          | candidate (dev) — only if benchmark warranted                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | measurement                      |

## 12. Testing

- Two tiers only (amended 2026-07-18: unit tier removed — test mass concentrates in
  fitness + human-run integration):
  - Small immortal fitness suite (`tests/fitness/`, proptest + recorded fixtures, the
    only CI tests) — FastQueue never loses/reorders data, order-book snapshot+deltas ≡
    reconstructed state, ingress ordering rules, zero steady-state alloc, adapter
    parse/sequencing vs recorded WS + REST payloads.
  - Live-network tests (`tests/integration/`) gated `#[ignore]` — never CI; run
    deliberately by the agent (amended 2026-07-18, user directive: agent runs ALL tests
    incl. live-network — verified network reach to venue endpoints; human not required).
- Amended 2026-07-20 (pure quant calculators may carry inline unit tests): `hot/quant` numeric
  modules may pin leaf math with inline `#[cfg(test)]` tests, kept to the FEW highest-value
  pins (core correctness + found-bug regressions), so the fitness charter stays architectural.
  Not a general reopening of the unit tier; scoped to pure calculators, sparseness deliberate.
- Amended 2026-07-25 (the link hand-rolls its wire format across a process boundary, so
  encode/decode agreement is an architectural invariant — socket behaviour is not, and the charter
  stays architectural): fitness gains the link wire format — encode/decode round-trip proptest per
  frame kind, and the `(sender, boot, topic)` seq-gate logic (dedup, reorder, restart resets the
  gate). Socket-level behaviour — subscription TTL expiry, subscribe refresh, the peer-restart gate
  over a real socket — stays OUT: contract-seam/integration territory.
- Amended 2026-07-28 (user directive: `src/` carries production code only, and a reader looking for
  a test looks in ONE place): the 2026-07-20 carve-out is WITHDRAWN. No `#[cfg(test)]` block lives
  anywhere outside `tests/` — not in `src/`, not in `strategies/`, not in `tools/`. The quant
  leaf-math pins live in the fitness target under `tests/fitness/quant/`, one module per calculator,
  named for the concept rather than the source path. The cost is deliberate and binding: a test
  under `tests/` sees only the crate's PUBLIC API, so a pin on a private item is re-expressed
  against a public seam or dropped, and `pub(crate) -> pub` to make a test compile is forbidden
  (§8 — `pub` is a promise, and a test is not a caller worth promising to). An item may still be
  promoted on its OWN merits by explicit ruling, and two were on the day this passed:
  `runtime::ExecutionLease` (exclusive live-execution ownership is an engine-level promise its
  already-exported peers imply) and `hot::quant::optimise` (every sibling calculator module was
  `pub` already; it was the lone exception). The test that follows a merit-based promotion is a
  consequence of it, never the argument for it.
- Contract seam tests only when specific seam proves unstable.
- Bug found -> failing fitness regression first -> then fix.
- No mocks unless forced. Recorded real exchange payloads = preferred fixture. §2
  determinism makes replay fixtures the backbone of testing.

## 13. Performance

- Perf protected by design principles, not standing benchmark mandate: zero steady-state
  alloc, contiguous fixed layouts, integer keys, no strings/formatting/syscalls on hot path.
- Premature optimisation rejected in review. Perf question arises -> profile first, act on
  evidence.
- Predictable perf beats peak perf. Reader must infer hot-path cost model from source.
- Branch temperature declared, not guessed (amended 2026-07-18: hot/cold split + inlining
  hints are design, not premature optimisation): `#[cold]` on failure/drain/reconnect/
  capacity-hit fns reachable from hot path; `#[inline]` on small hot accessors + newtype
  ops; `#[inline(always)]` sparingly — tiny leaf fns on proven hot path only, inline WHY
  required.
- Cache-line layout on hot structures where it makes sense (amended 2026-07-18: predictable
  memory movement is §3's whole point): hot fields grouped, `#[repr(align(64))]` / padding
  against false sharing on cross-thread-touched state, contiguous arrays over
  pointer-chasing. Layout choice visible in review, not accidental.

## 14. Toolchain & Gates

- Pinned stable via `rust-toolchain.toml`. Edition 2024. Upgrades deliberate.
- Gate every change: `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` +
  `cargo test`. All green or no merge. Amended 2026-07-25 (`eframe` is optional behind feature `ui`
  (§11) and a single-config gate rots the path it never compiles): clippy + test run in BOTH feature
  configurations — with and without `--features ui`.
- `#[allow]` requires inline WHY.
- `#![forbid(unsafe_code)]` at crate root. Amendment requires benchmark proving safe impl
  cannot meet measured requirement.

## 15. Amendments

- Changes only by explicit agreement, never silent. Every amendment carries one-line WHY.
- PROJECT.md, PLAN.md, CHECKLIST.json must comply; where they cannot, conflict surfaced +
  resolved here first.

## 16. Smell Doctrine

Added 2026-08-07 (user directive; the operative playbook is the `rex-code-smells` skill — this
section is its constitutional core).

- Detection ≠ fixing. A finding = file:line + evidence + concrete cost + one-line fix direction.
  Severity, three only: BLOCK (bug factory) / REFACTOR (taxes every future edit) / NIT (real but
  cheap).
- Sweeps (grep, clippy) yield CANDIDATES; only reading in context confirms. One false positive
  poisons the whole report.
- Clean is a valid verdict. A fabricated finding costs more trust than an empty report.
- Naming lens: booleans are predicates; conversion prefixes tell the truth (`as_` free view,
  `to_` does work, `into_` consumes); no noise words; domain primitives newtyped so a swapped
  argument cannot compile.
- Brittleness lens: `unwrap`/`expect` never meets external input; knowledge is encoded once —
  the same magic literal in two places is a defect; ordering enforced by types, not asserts;
  every seam needs its writer AND its reader — data without control flow compiles green and
  lies; tests pin behaviour, never formatting.
- Structure lens: the deletion test (§1) kills speculative generality; a long fn is N jobs
  sharing one name; an `Option` field alive in only one phase is a missing enum state; the same
  match in three files is one change costing three edits.
