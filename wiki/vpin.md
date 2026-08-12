# VPIN flow toxicity: calculation, parameters and market-making use

VPIN—Volume-Synchronized Probability of Informed Trading—is a volume-clock order-flow imbalance statistic proposed as a proxy for flow toxicity. It is not a standalone pricing model and should not replace the [Avellaneda–Stoikov](./avellaneda_stoikov.md) or [Guéant](./gueant.md) quote equations. Its practical role is as a separately validated risk overlay on spreads, sizes, quote symmetry or participation.

## 1. Core definition and notation

Let:

- \(V\) be the target volume in every completed bucket;
- \(\tau\) index completed equal-volume buckets;
- \(V\_\tau^B\) be buyer-initiated or buy-classified volume in bucket \(\tau\);
- \(V\_\tau^S\) be seller-initiated or sell-classified volume in bucket \(\tau\);
- \(n\) be the number of completed buckets in the rolling VPIN window; and
- \(OI\_\tau\) be the absolute order imbalance in bucket \(\tau\).

Every completed bucket satisfies

\[
V*\tau^B+V*\tau^S=V.
\]

Its absolute imbalance is

\[
OI*\tau
=
\left|V*\tau^B-V\_\tau^S\right|.
\]

VPIN at the close of bucket \(\tau\) is

\[
\boxed{
\operatorname{VPIN}_\tau
=
\frac{
\displaystyle\sum_{i=\tau-n+1}^{\tau}
\left|V_i^B-V_i^S\right|
}{nV}
}.
\tag{1}
\]

Equivalently,

\[
\operatorname{VPIN}_\tau
=
\frac{1}{n}
\sum_{i=\tau-n+1}^{\tau}
\frac{OI_i}{V}.
\]

Therefore \(0\leq\operatorname{VPIN}\leq1\). A value of 0.30 means that the average absolute classified imbalance in the rolling window is 30% of one bucket's volume. It should **not** automatically be read as “a 30% probability that traders are informed.” The probability interpretation relies on the structural assumptions connecting VPIN to the earlier PIN model; operationally, it is safest to treat the computed series as a normalized imbalance/toxicity feature.

### Direction is separate from toxicity

VPIN uses an absolute value and is directionless. Retain the signed bucket imbalance

\[
d*\tau
=
\frac{V*\tau^B-V\_\tau^S}{V}
\in[-1,1],
\]

and, if useful, a rolling signed-flow measure

\[
z*\tau
=
\frac{
\displaystyle\sum*{i=\tau-n+1}^{\tau}
(V_i^B-V_i^S)
}{nV}.
\]

Positive \(z*\tau\) indicates net aggressive buying; negative \(z*\tau\) indicates net aggressive selling. VPIN says how one-sided the recent flow has been, while \(z\_\tau\) preserves the side.

### What the core quantities tell a market maker

| Quantity | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(\operatorname{VPIN}_\tau\) | The average absolute classified imbalance as a fraction of bucket volume over the last \(n\) completed buckets | One exact bucket size, volume convention, classifier, and rolling window | Rank how unusually one-sided recent flow is and decide whether a separately validated spread, size, or participation overlay should become more defensive |
| \(OI_\tau/V\) | How one-sided the latest completed bucket was, without retaining direction | The newest equal-volume slice of trading | Detect whether the current VPIN average is being driven by a fresh imbalance shock or older buckets |
| \(d_\tau\) | The signed imbalance of the latest bucket, from \(-1\) for all classified sells to \(+1\) for all classified buys | Aggressor direction within one completed bucket | Identify which quote side is exposed: positive flow threatens an ask with adverse selection, while negative flow threatens a bid |
| \(z_\tau\) | The rolling signed imbalance over the same \(nV\) volume horizon as VPIN | Persistent buy-versus-sell pressure | Allocate a toxicity buffer asymmetrically instead of treating directionless VPIN as a directional forecast |
| Bucket duration | How much clock time was required to accumulate \(V\) volume | The endogenous volume clock | Distinguish the same VPIN reading during intense rapid trading from one accumulated slowly in a quiet market |

VPIN tells us **one-sidedness**, not why the flow occurred and not the probability of a future price move. Its usefulness to a market maker comes from validating whether high readings precede worse passive-fill markouts after controlling for volatility, volume rate, spread, and book state.

---

## 2. Constructing equal-volume buckets

Choose a target number of buckets per average trading day, denoted by \(B\). Estimate average daily volume using only past data, then set

\[
\boxed{
V=\frac{\operatorname{ADV}}{B}
}.
\]

The original VPIN work commonly uses \(B=50\), so each bucket contains roughly 1/50 of average daily volume. This is a published reference setting, not a universal optimum.

Process trades chronologically. If a trade or source bar would overfill the current bucket, split its volume: use exactly the amount required to close the bucket and carry the excess into the next bucket. A single large trade or bar may fill several buckets.

### Volume units

Use one stable volume convention:

- futures: contracts, adjusted consistently through contract rolls;
- equities: shares;
- spot crypto: base-asset quantity; or
- notional volume, if that is deliberately chosen and used consistently.

Classical VPIN uses shares/contracts. Notional volume can make cross-price regimes more comparable, but it defines a different bucket clock. Do not switch conventions without rebuilding the historical distribution and thresholds.

### Streaming algorithm

```text
bucket_target = historical_ADV / target_buckets_per_day

for each chronological trade:
    remaining = chosen_volume_measure(trade)

    while remaining > 0:
        take = min(remaining, bucket_target - bucket.total)

        allocate `take` to bucket.buy and bucket.sell
        using the selected classification method

        bucket.total += take
        remaining -= take

        if bucket.total == bucket_target:
            imbalance = abs(bucket.buy - bucket.sell)
            signed_imbalance = (bucket.buy - bucket.sell) / bucket_target
            append the completed bucket
            update VPIN if at least n completed buckets exist
            start a new empty bucket
```

Publish the production VPIN value only after a bucket closes. A “partial-bucket VPIN” has a changing denominator and different statistical behaviour; if used for faster monitoring, label and calibrate it as a separate feature.

---

## 3. Classifying buy and sell volume

The classifier is one of the most consequential VPIN choices. Record its name and version with every generated series.

### Method A — known aggressor side

If the exchange or normalized trade feed reliably identifies the aggressor, allocate each trade directly:

\[
V*\tau^B
=
\sum*{j\in\tau}
v_j\,\mathbf 1\{\text{aggressor}\_j=\text{buy}\},
\]

\[
V*\tau^S
=
\sum*{j\in\tau}
v_j\,\mathbf 1\{\text{aggressor}\_j=\text{sell}\}.
\]

This measures actual aggressive volume in the observed feed. Verify the venue's flag semantics carefully: a field saying that the buyer was the maker implies that the aggressor was the seller.

Known-side VPIN is usually the clearest starting point for a live market maker, but it is not numerically interchangeable with bulk-classified VPIN. Build separate historical distributions if both are retained.

### Method B — original bulk volume classification (BVC)

When aggressor side is unavailable or when reproducing the original VPIN procedure, aggregate trades into short source bars—one-minute time bars in the original empirical implementation. For bar \(i\), let:

- \(v_i\) be total bar volume;
- \(P*i-P*{i-1}=\Delta P_i\) be the bar-to-bar price change;
- \(\sigma\_{\Delta P}\) be a causal estimate of the standard deviation of comparable price changes; and
- \(\Phi\) be the standard normal cumulative distribution function.

The buy fraction is

\[
\pi*i^B
=
\Phi\!\left(
\frac{\Delta P_i}{\sigma*{\Delta P}}
\right),
\]

and the sell fraction is

\[
\pi_i^S=1-\pi_i^B.
\]

For the parts of source bars assigned to bucket \(\tau\),

\[
V*\tau^B
=
\sum*{i\in\tau}v_i\pi_i^B,
\]

\[
V*\tau^S
=
\sum*{i\in\tau}v*i\pi_i^S
=
V-V*\tau^B.
\tag{2}
\]

If \(\Delta P_i=0\), BVC assigns half the volume to buys and half to sells. A positive standardized change assigns more volume to buys; a negative change assigns more to sells. When a source bar is split across buckets, preserve the same \(\pi_i^B,\pi_i^S\) fractions for every split portion.

Possible price inputs include last price, bar VWAP, mid-price or microprice. The original construction is based on transaction-price changes. Changing the price input changes the classifier and therefore the VPIN series.

### Method C — inferred transaction side

If aggressor flags are unavailable, tick-rule, quote-rule or Lee–Ready-style classifications can be used trade by trade. These generate another distinct VPIN specification. Their accuracy depends on quote/trade timestamp alignment, locked/crossed markets and feed latency.

### Why classification must be validated

The original authors argue that bulk classification can capture information in aggregate flow. Later critiques show that VPIN's behaviour can change materially with classification and that BVC can mechanically inject absolute price changes into the toxicity estimate. Consequently:

- do not compare raw levels from different classifiers;
- do not silently change source-bar length or \(\sigma\_{\Delta P}\);
- compare classified imbalance with known aggressor data where available; and
- validate each version against future maker adverse-selection losses, not only future volatility.

---

## 4. Parameters and what they mean

| Parameter or choice | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(B\), target buckets per average day | The intended update granularity in volume time | Average daily activity, with larger \(B\) producing smaller buckets | Choose between faster but noisier toxicity updates and slower but more stable ones |
| \(V=\mathrm{ADV}/B\), volume per bucket | How much new flow must arrive before one production observation closes | A declared volume unit and causally estimated ADV | Define the volume clock and avoid comparing signals built from materially different amounts of trading |
| ADV window | The activity history used to set bucket size | Regime changes in normal daily volume | Balance a stable bucket clock against the need to adapt when the instrument's activity changes |
| \(n\), rolling bucket count | How many completed volume slices contribute to VPIN | A horizon of \(nV\) volume whose clock-time duration changes with activity | Choose how quickly old imbalances leave the signal and how smooth the toxicity estimate should be |
| Volume measure | What “equal volume” represents | Contracts, shares, base quantity, or notional | Keep bucket construction and historical thresholds comparable across time, rolls, and price regimes |
| Classifier | How volume is assigned to the buy and sell sides | Known aggressor flags, BVC, tick rule, or quote rule | Decide what VPIN actually measures and avoid mixing levels from non-equivalent classifications |
| Source-bar interval | The time aggregation used before BVC assigns side fractions | Bulk classification when aggressor side is unavailable | Control the speed-versus-noise trade-off in inferred buy and sell volume |
| \(P_i\), the BVC price input | Which observed price change drives bulk side classification | Last price, VWAP, midpoint, or microprice | Select a classifier aligned with the intended flow interpretation and rebuild thresholds if it changes |
| \(\sigma_{\Delta P}\) window | The causal scale against which BVC standardises price changes | The current volatility regime and chosen source-bar interval | Prevent ordinary moves from being classified as extreme solely because the scale estimate is noisy or stale |
| CDF family | How a standardised price move maps into a buy fraction | Normal \(\Phi\), Student-t, or another BVC mapping | Control how aggressively large moves are assigned to one side and treat any change as a new VPIN specification |
| Percentile lookback | Which past VPIN observations define “unusual” today | The same instrument and exact VPIN specification | Convert a raw, specification-dependent value into a causal regime rank without using future data |
| Alert threshold | The percentile or calibrated score at which an overlay activates | Out-of-sample maker markouts and intervention costs | Decide when evidence is strong enough to widen, reduce size, or change participation |
| Persistence rule | How many completed buckets must sustain the alert | Autocorrelated VPIN readings and noisy one-bucket spikes | Reduce unnecessary quote changes while accepting a measured delay in protection |
| Session/roll policy | How gaps, auctions, partial buckets, and futures rolls affect continuity | The instrument's actual trading calendar and contract lifecycle | Keep live signal freshness and historical calibration consistent |

### Published reference combinations

The original literature frequently uses:

- \(B=50\) buckets per average day;
- one-minute BVC source bars;
- \(n=50\) for roughly one average day's volume; and
- \(n=250\) with \(B=50\) for roughly five average days' volume.

These are reference specifications. The effective horizon is \(nV\) units of volume, not a fixed number of hours or days. During a volume shock, a “five-day” configuration can cover far less clock time.

### Choosing parameters for toxicity tracking

Start with multiple pre-declared candidates rather than optimizing one combination exhaustively. For example:

- fast: \(B=100,n=25\);
- balanced reference: \(B=50,n=50\); and
- slow: \(B=50,n=250\).

These labels describe responsiveness only; they are not universal recommendations. Select parameters by walk-forward performance on the market maker's actual loss function, including adverse markouts, fill rate, inventory excursions and foregone spread capture. Penalize unnecessary quote cancellations and long periods of false alarm.

### Sensible research starting configuration

For an initial implementation—not a production claim—use:

- reliable venue aggressor flags as the primary classifier;
- contracts/shares/base quantity as the volume measure;
- a causal 20-session ADV estimator;
- \(B=50\) and \(n=50\) as the primary responsive series;
- a parallel \(B=50,n=250\) series as a slower confirmation;
- completed buckets only;
- a causal 60-session percentile history with a substantial warm-up sample;
- 90% and 97.5% as provisional high/extreme monitoring labels; and
- two completed buckets of persistence before a strong quoting intervention.

Every number in this starting configuration should be challenged with walk-forward maker markouts and intervention P&L. For a 24/7 or rapidly changing market, compare the fixed bucket size against a scheduled causal ADV update; for session markets, explicitly decide whether overnight flow, auctions and incomplete buckets are carried or reset.

---

## 5. Converting VPIN into a toxicity regime

Raw VPIN depends on bucket size, sample length and classifier. Rank it relative to a causal historical distribution for the same instrument and exact specification:

\[
u*\tau
=
\widehat F*{\tau-1}
\!\left(\operatorname{VPIN}\_\tau\right),
\]

where \(\widehat F*{\tau-1}\) uses only observations available before bucket \(\tau\) closes. Then \(u*\tau\in[0,1]\) is the historical percentile of the current reading.

Example regime labels might be:

|  Percentile | Descriptive regime |
| ----------: | ------------------ |
|   Below 75% | Normal             |
|      75–90% | Elevated           |
|    90–97.5% | High               |
| Above 97.5% | Extreme            |

These are monitoring labels, not proven universal decision thresholds. Fit the actual boundaries from out-of-sample economic outcomes.

Because overlapping VPIN windows are highly autocorrelated, a bucket-level 99th percentile does not mean that only 1% of trading days will experience an alert. For daily operational-frequency statements, separately model the historical distribution of each day's or session's **maximum** VPIN percentile.

### Worked example

Suppose \(V=1{,}000\), \(n=5\), and the last five bucket imbalances are

\[
400,\ 200,\ 600,\ 100,\ 500.
\]

Then

\[
\operatorname{VPIN}
=
\frac{400+200+600+100+500}{5\times1{,}000}
=0.36.
\]

\(\operatorname{VPIN}=0.36\) tells us that recent buckets averaged 36% absolute classified imbalance, in the context of \(V=1{,}000\), \(n=5\), and the chosen classifier, so that we can compare the reading with the causal history of that exact specification. It does not yet tell us which side is dominant or whether 0.36 is unusual. If the same-specification historical percentile were 95%, that percentile would tell us the reading is unusually high relative to the past, so that a previously validated high-toxicity overlay could activate; signed \(d_\tau\) or \(z_\tau\) would still be needed to choose which side receives more protection.

---

## 6. Applying VPIN to AS and Guéant quotes

Both companion pricing notes write their continuous depths as

\[
\delta^{b,\mathrm{model}}=h+jq,
\qquad
\delta^{a,\mathrm{model}}=h-jq.
\]

VPIN should normally be added as an independently calibrated control layer after the base-model depths are computed and before final tick rounding.

### A continuous toxicity score

Choose an intervention percentile \(u_0\), and define

\[
x*\tau
=
\operatorname{clip}
\!\left(
\frac{u*\tau-u_0}{1-u_0},
0,1
\right).
\]

This is zero below \(u_0\) and rises continuously to one at the top of the historical range.

### Symmetric or direction-aware spread buffers

For a symmetric maximum buffer \(b\_{\max}\) in price units,

\[
\delta^{b,\mathrm{final}}
=
\delta^{b,\mathrm{model}}+b*{\max}x*\tau,
\]

\[
\delta^{a,\mathrm{final}}
=
\delta^{a,\mathrm{model}}+b*{\max}x*\tau.
\]

To allocate more protection to the side exposed to signed aggressive flow, choose \(0\leq\rho\leq1\) and use

\[
b*\tau^a
=
b*{\max}x*\tau(1+\rho z*\tau),
\]

\[
b*\tau^b
=
b*{\max}x*\tau(1-\rho z*\tau),
\]

\[
\delta^{a,\mathrm{final}}
=
\delta^{a,\mathrm{model}}+b*\tau^a,
\qquad
\delta^{b,\mathrm{final}}
=
\delta^{b,\mathrm{model}}+b*\tau^b.
\]

Positive buy pressure makes the ask more defensive; negative sell pressure makes the bid more defensive. The coefficient \(\rho\) and maximum buffer must be learned from maker-side markouts. Quantise only after adding the overlay.

### Size and participation controls

A simple size overlay is

\[
Q*\tau
=
Q_0\max(Q*{\min},1-\alpha x\_\tau),
\qquad 0\leq\alpha\leq1,
\]

where \(Q\_{\min}\) is expressed as a fraction of normal size. More severe policies can disable the flow-exposed side or pause quoting after a persistent extreme signal, but such actions should require stronger validation than a one-bucket threshold.

Prefer explicit buffers and size controls over silently changing \(\gamma\). Risk aversion has a defined role in AS and Guéant, while a separate toxicity overlay is easier to attribute, test and monitor.

The overlay controls should be interpreted as policy parameters rather than facts estimated by VPIN:

| Parameter | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(u_\tau\) | Where the current VPIN reading ranks in its own causal history | The same instrument, bucket size, window, classifier, and session policy | Compare toxicity regimes without assuming that one raw VPIN threshold transfers across specifications |
| \(u_0\) | The percentile at which the continuous intervention begins | A threshold validated against future maker markouts and intervention cost | Leave ordinary readings untouched and reserve protection for sufficiently unusual flow |
| \(x_\tau\) | The severity of the current alert on a zero-to-one scale above \(u_0\) | Smooth overlay activation | Scale quote changes gradually instead of jumping from no action to maximum action at one boundary |
| \(b_{\max}\) | The largest extra quote-depth buffer allowed by the overlay | Price units or ticks and the strategy's maximum tolerated protection | Cap spread widening and make the economic cost of the toxicity rule explicit |
| \(\rho\) | How much of the buffer is allocated according to signed flow | Directionless VPIN combined with \(z_\tau\), with \(0\leq\rho\leq1\) | Protect the flow-exposed ask during aggressive buying or the bid during aggressive selling without pretending VPIN itself has a sign |
| \(\alpha\) | The maximum proportional strength of the size reduction | The rule \(Q_\tau=Q_0\max(Q_{\min},1-\alpha x_\tau)\) | Reduce capital exposed to toxic flow while keeping the size response separately testable from spread changes |
| \(Q_0\) and \(Q_{\min}\) | Normal quote size and the minimum retained fraction | Venue size constraints and the strategy's participation policy | Bound the size overlay and decide whether extreme toxicity reduces exposure or disables quoting entirely |

### Avoid double-counting volatility

BVC uses price changes to infer signed volume, and VPIN may therefore be correlated mechanically with realised volatility. If short-term volatility already widens the AS or Guéant model, adding a large VPIN buffer can count the same shock twice. Test the incremental value of VPIN after controlling for the volatility, volume rate, spread and order-book features already used by the strategy.

---

## 7. Validating toxicity rather than merely volatility

The market maker's relevant outcome is adverse selection after a passive fill. For a fill at time \(t_f\), define maker side \(s_m=+1\) for a maker buy and \(s_m=-1\) for a maker sell.

The signed future-mid move is

\[
m*h^{\mathrm{mid}}
=
s_m\left(S*{t*f+h}-S*{t_f}\right).
\]

A negative value means that the mid-price moved against the maker after the fill. The execution-price markout is

\[
m*h^{\mathrm{px}}
=
s_m\left(S*{t*f+h}-P*{\mathrm{fill}}\right),
\]

which includes captured spread. Evaluate several economically relevant horizons, such as seconds, future volume buckets and expected inventory-holding time.

For every candidate VPIN specification:

1. Compute it causally and use only values available before each quote/fill decision.
2. Group fills by VPIN percentile and signed-flow direction.
3. Compare mean, median and tail markouts after fees/rebates.
4. Measure fill rate, spread capture, inventory tails and missed profitable fills under the proposed overlay.
5. Compare against simpler predictors: recent signed trade imbalance, volume rate, short realised volatility, order-book imbalance, current spread and recent returns.
6. Test whether VPIN adds incremental predictive or economic value after those controls.
7. Select parameters and thresholds with walk-forward data; reserve a final untouched period.

Useful operational diagnostics include:

- adverse markout by VPIN decile;
- probability of a negative markout conditional on percentile and side;
- bucket duration and current volume rate;
- alert frequency by day/session, not only by bucket;
- time spent in each toxicity regime;
- cancellations and lost queue priority caused by the overlay; and
- P&L decomposition into spread capture, adverse selection, fees and inventory revaluation.

---

## 8. Limitations and safeguards

- **VPIN is not directional.** Use signed imbalance separately.
- **Raw levels are specification-dependent.** Bucket size, window length, bar interval, price scale and classifier all matter.
- **BVC can be mechanically volatility-linked.** It transforms price changes into buy/sell fractions, so predictive tests must control for current volatility.
- **Classifier results can disagree.** Published work disputes whether BVC or transaction-side approaches provide the more meaningful signal in particular settings.
- **High VPIN is not a crash probability.** It is neither a calibrated probability of an imminent crash nor proof of informed trading.
- **A percentile is not an event frequency.** Overlapping windows are autocorrelated and many VPIN observations occur per day.
- **The clock is endogenous.** High volume produces faster updates and shortens the clock-time history covered by \(n\) buckets.
- **Partial buckets are not comparable.** Keep previews separate from production completed-bucket VPIN.
- **Missing venues distort flow.** For fragmented markets, decide whether the signal represents the quoting venue or a consolidated market and validate accordingly.
- **Bad prints, self-trades and wash trading matter.** Apply data-quality filters before bucketing, particularly in digital-asset markets.
- **ADV changes create structural breaks.** Re-estimate bucket size on a declared schedule and rebuild or normalize history when contract specifications or market regimes change materially.

VPIN is best treated as one feature in a toxicity stack, alongside direct aggressor imbalance, order-book depletion, markouts, volatility and liquidity. Its value is empirical and instrument-specific.

---

## 9. Recommended production sequence

1. Choose and document the venue set, volume measure and trade-quality filters.
2. Estimate historical ADV causally and set \(V=\mathrm{ADV}/B\).
3. Select the classifier; prefer reliable aggressor flags for direct flow tracking and retain BVC only as a separately named specification.
4. Build exact equal-volume buckets, splitting overflow without discarding volume.
5. Store \(V^B,V^S,OI,d\), start/end timestamps and bucket duration.
6. After \(n\) completed buckets, update VPIN using equation (1).
7. Map raw VPIN to a causal historical percentile and retain signed \(z\) separately.
8. Compute the AS or Guéant continuous quote depths.
9. Apply the validated toxicity spread/size overlay.
10. Apply position limits, post-only constraints and final tick rounding.
11. Log the VPIN value actually available at every quote and fill for markout validation.
12. Monitor classifier drift, bucket-duration drift, alert frequency and incremental predictive value.

### Compact known-aggressor pseudocode

```text
for trade in chronological_trades:
    remaining = normalized_volume(trade)

    while remaining > 0:
        take = min(remaining, bucket_volume - current.total)

        if trade.aggressor == BUY:
            current.buy += take
        else if trade.aggressor == SELL:
            current.sell += take
        else:
            handle_unknown_side_under_declared_policy(take)

        current.total += take
        remaining -= take

        if current.total == bucket_volume:
            abs_imbalance = abs(current.buy - current.sell)
            signed_fraction = (current.buy - current.sell) / bucket_volume

            rolling_abs_sum += abs_imbalance
            rolling_signed_sum += current.buy - current.sell
            completed_buckets.push(current)

            if completed_buckets.count > n:
                old = completed_buckets.pop_oldest()
                rolling_abs_sum -= abs(old.buy - old.sell)
                rolling_signed_sum -= old.buy - old.sell

            if completed_buckets.count == n:
                vpin = rolling_abs_sum / (n * bucket_volume)
                signed_flow = rolling_signed_sum / (n * bucket_volume)
                percentile = causal_historical_cdf(vpin)
                publish(vpin, signed_flow, percentile)

            current = new_empty_bucket()
```

---

## Sources

1. David Easley, Marcos M. López de Prado and Maureen O'Hara, [“Flow Toxicity and Liquidity in a High-frequency World” (PDF)](https://www.stern.nyu.edu/sites/default/files/assets/documents/con_035928.pdf), especially Sections 2.2–2.4, equations (7) and (9), and the implementation appendix.
2. David Easley, Marcos M. López de Prado and Maureen O'Hara, [“Discerning Information from Trade Data”](https://papers.ssrn.com/sol3/papers.cfm?abstract_id=1989555), for the comparison of bulk volume classification with transaction-based classifiers.
3. David Abad and José Yagüe, [“From PIN to VPIN: An introduction to order flow toxicity” (PDF)](https://www.quantresearch.org/From%20PIN%20to%20VPIN.pdf), for a detailed worked description of source bars, bucket filling, sample length and parameter notation.
4. Torben G. Andersen and Oleg Bondarenko, [“VPIN and the Flash Crash”](https://ideas.repec.org/a/eee/finmar/v17y2014icp1-46.html), and [“Reflecting on the VPIN Dispute” (PDF)](https://repec.econ.au.dk/repec/creates/rp/13/rp13_42.pdf), for evidence and criticism concerning classification sensitivity, volatility correlation, incremental predictive power and percentile interpretation.
