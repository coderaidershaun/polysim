# Toxic-flow markouts: measuring adverse selection without private fills

## 1. Pseudo-fill semantics

We compute quotes but do not place orders, so there are no private executions to measure adverse
selection against. A **pseudo-fill** substitutes for one: a public print that reaches a quoted level
we were standing at is treated as the execution we would have taken.

Let \(P^b\) and \(P^a\) be our quoted bid and ask. A print at price \(p\) with aggressor side \(s\)
reaches our quote when

\[
s=\text{sell}\ \text{and}\ p\le P^b
\qquad\text{or}\qquad
s=\text{buy}\ \text{and}\ p\ge P^a .
\]

The aggressor inverts: a **selling** aggressor hits our **bid**, a buying aggressor lifts our ask.
The engine keeps a private `QuoteSide` enum for this reason and never reuses `Side`, which names the
aggressor.

Four conventions make the measurement well defined:

- **At or through.** Equality counts. The comparison is on exact `Price` mantissas, never floats —
  a float compare would fill on rounding dust (§4).
- **Queued last** (TEMPORARY heuristic). Reaching the level is not on its own a fill. We assume our
  order joined the **back** of that level's queue, so prints *at* \(P^b\) (or \(P^a\)) eat the qty
  standing in front of us, and we fill only once nine tenths of the qty resting there when we joined
  has traded away. Let \(Q\) be that arm-time qty and \(E\) the qty printed at the level since:
  \[ 10E\ \ge\ 9Q \]
  exactly, on integer mantissas. A print *through* the level swept everything resting at it, ours
  included, so it fills outright whatever \(Q\) — no dust filter there. \(Q=0\) (the book showed no
  level at our price) is an empty queue and its first print fills. An unbroken run of re-arms at the
  same price keeps the position already earned; any fill, disarm or continuity reset makes the next
  arm a fresh join at the back. Cancellations ahead of us are not observable on a public feed and are
  ignored — the nine tenths rather than the whole queue stands in for them. There is no queue model
  behind the fraction.
- **Fill price is OUR level**, not the print price. A print sweeping three ticks through our bid
  still fills us at our bid, which is where our order would have rested.
- **One fill per placement per side.** Firing disarms the side until the next placement re-arms it,
  so a sweep of forty prints produces one pseudo-fill, not forty.

Fills stamp `received_ts_us` — the same clock the mid feed runs on, so the two are comparable.

## 2. Forward markout

The forward markout asks: after we filled, did the price run against us? For a fill at price \(F\)
at time \(t\), and mid \(M\) at horizon \(h\),

\[
\text{fwd}_h=\varepsilon\cdot\frac{M_{t+h}-F}{F}\cdot 10^4\ \text{bps},
\qquad
\varepsilon=\begin{cases}+1 & \text{bid fill}\\ -1 & \text{ask fill}\end{cases}
\]

**Negative is toxic**, on both sides. We bought the bid and the mid fell away: adverse. We sold the
ask and the mid ran up: adverse. The ask formula is exactly the bid formula negated, which is all
the sign \(\varepsilon\) does.

Horizons are fixed at \(h\in\{1,3,5,10,30,60\}\) seconds (`ForwardHorizon`). Each is an independent
FIFO lane, so a fill is measured six times, once as each horizon elapses.

## 3. Reverse markout

The reverse markout asks the complementary question: was the price *already* running toward our
quote before it filled? That is momentum toxicity — we were picked off by flow that had been coming
for a second.

\[
\text{rev}_h=\varepsilon\cdot\frac{M_{t}-M_{t-h}}{M_{t-h}}\cdot 10^4\ \text{bps},
\qquad h\in\{1,5\}\ \text{seconds}
\]

with the same \(\varepsilon\). One sign convention across both families: **negative is toxic**.
Mid falling into our bid means we caught a falling knife.

\(M_{t-h}\) is the newest sample at or **before** \(t-h\), found by binary search over the
time-ordered mid ring — not the nearest sample, which could sit after the target and shorten the
lookback.

Both families are kept **per side**: bid-fill and ask-fill markouts are separate series, because
one-sided toxicity is exactly the thing worth seeing.

## 4. Windows, smoothing and gate counters

Every series is a `FastQueue<f64>` holding ten minutes of realised markouts at the spin cadence,
preallocated from `MarkoutSpec { spin_interval, max_mids_per_sec }`. Nothing allocates or grows
after construction (§3): the mid ring holds six seconds (the deepest reverse lookback plus slack),
and each pending lane holds \(h/\text{spin}+2\) in-flight fills.

Raw series are exposed, not smoothed values — the caller applies `FastQueue::ema(halflife_samples)`,
an exponentially weighted mean with \(\lambda=2^{-1/H}\) seeded from the oldest sample. That is the
same decay convention the RiskMetrics volatility estimators use, so half-lives mean one thing
everywhere in the engine.

Three gates drop samples rather than record a lie, and each drop lands on a lifetime counter:

| Counter | Dropped when | Read it as |
|---|---|---|
| `reverse_gap_count` | history doesn't reach back a whole horizon | warm-up, expected early in a run |
| `stale_maturation_count` | the first ripe mid arrives more than two spins past the ideal instant | a feed gap, not a markout |
| `pending_overflow_count` | a lane is already at its sized depth | fills arriving faster than the sizing assumed |

A stale fill still pops its lane — a lane that held out for a punctual sample would wedge behind one
feed gap forever. `reset_continuity()` (book resync) drops the quotes, the in-flight fills and the
mid ring but keeps realised series: those markouts happened. `clear()` (rotation onto a different
instrument) wipes the series too. Counters survive both.

## 5. As wired in `strat-micro-recorder`

Binance only — the arm source is the Guéant quote, which needs a tick grid a polymarket row does not
have. One tracker per recorded binance slot, sized from the engine spin interval and 12 mids/second.

**Feeds:**

- `on_mid(ts, mid)` at every committed book update (`is_last_chunk` on a `BookState::Valid` book,
  mid off the tracker's touch) *and* every spin, both through one clamp on the strategy:
  `ts = max(ts, last_fed_ts)`. Spins and book chunks arrive on different ingress queues, whose
  cross-producer order is best-effort, so a straggling chunk can be stamped behind a spin already
  fed; the ring is binary-searched and must stay time-ordered. The clamp is never reset — a
  monotone clock survives resyncs.
- `on_trade(&trade)` on every public print for the slot, ungated by the features table: feeding
  state is not emitting it.
- `arm_bid` / `arm_ask` once per spin from the **zero-inventory** Guéant quote — the exact snapped
  price the `gueant_bid_price` / `gueant_ask_price` columns carry, so the level measured and the
  level recorded cannot drift apart. Zero inventory matters: markouts must measure the flow, not our
  own inventory skew. Each arm carries \(Q\), the qty resting at that level in the engine book right
  then (`level_qty_at` over `ctx.book(instrument)`), which is what a quote joining the level now
  would rest behind. A price the book shows no level at arms an empty queue.
- A spin where a side produces no quote (no tick, no σ, a stale fit, an out-of-range snap)
  **disarms** that side. Pseudo-fills stand in for our executions, and we only execute at a level we
  are showing.
- `reset_continuity()` on book reset. `clear()` is never called: binance slots do not rotate.

**Emission**, 12 columns on the per-spin row, EMA halflife 8 *fills* (not spins — the series only
advance when a print reaches a level). The whole block emits only on a spin where the instrument
has a live mid — the only spins on which a side can be armed at all, since the Guéant scale starts
from the mid:

| Columns | Content |
|---|---|
| `markout_{bid,ask}_{1s,3s,5s}_bps` | forward EMAs, null until that horizon realises |
| `markout_{bid,ask}_rev_{1s,5s}_bps` | reverse EMAs, null until that horizon realises |
| `markout_{bid,ask}_fills` | lifetime pseudo-fill count, emitted on every live-mid spin — a zero then is the reading "we were quoting and nothing reached us", which an absent value cannot express. A row with no mid never armed, so it emits nothing rather than a misleading zero |

Forward 10 s, 30 s and 60 s are computed and left unrecorded: on a market whose windows are five
minutes long, a minute-deep markout answers a different question from the one the row asks. The
three gate counters are tracked but not emitted — they are diagnostics about the measurement, not
features of the market.

## Sources

1. Olivier Guéant, [“Optimal market making” (PDF)](https://arxiv.org/pdf/1605.01862), Section 6, for
   the calibration checklist that names adverse selection after fills as a quantity to monitor
   separately from fill rate. See also [`gueant.md`](gueant.md) for the quoting model these
   pseudo-fills are armed from.
2. David Easley, Marcos M. López de Prado and Maureen O'Hara,
   [“Flow Toxicity and Liquidity in a High-frequency World” (PDF)](https://www.stern.nyu.edu/sites/default/files/assets/documents/con_035928.pdf),
   for order-flow toxicity as the market maker's central risk, and [`vpin.md`](vpin.md) for the
   volume-clock estimator of the same phenomenon. Markouts measure realised adverse selection
   ex post; VPIN estimates its probability ex ante.
3. Lawrence R. Glosten and Paul R. Milgrom, “Bid, ask and transaction prices in a specialist market
   with heterogeneously informed traders”, *Journal of Financial Economics* 14(1), 1985, for the
   result that the post-trade price revision *is* the adverse-selection cost — the forward markout
   is that revision, measured.
4. Álvaro Cartea, Sebastian Jaimungal and José Penalva, *Algorithmic and High-Frequency Trading*,
   Cambridge University Press, 2015, Chapters 3 and 10, for trade classification and post-trade
   price-impact measurement.
