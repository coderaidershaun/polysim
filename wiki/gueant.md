# Guéant market-making quotes: equations and practical tick-size implementation

## 1. Model equations

Let:

- \(S\) be the fair price (for example, the mid-price or a microprice);
- \(q\) be signed inventory, with \(q>0\) denoting a long position;
- \(\Delta\) be the size of one execution/order, in the same inventory units as \(q\);
- \(A\) and \(k\) parameterise the execution intensity
  \[
  \lambda(\delta)=A\exp(-k\delta);
  \]
- \(\gamma\) be the inventory-risk-aversion parameter;
- \(\xi\) select the paper's objective: \(\xi=\gamma\) for **Model A** (CARA utility of terminal marked-to-market wealth), and \(\xi=0\) for **Model B** (expected terminal wealth minus a running quadratic inventory penalty); and
- \(\sigma\) be the instantaneous **absolute price-volatility coefficient**.

For \(\xi>0\), the approximate optimal bid and ask depths from the fair price are

\[
\delta_{\mathrm{approx}}^{b*}(q)
=
\frac{1}{\xi\Delta}
\log\!\left(1+\frac{\xi\Delta}{k}\right)
+
\frac{2q+\Delta}{2}
\sqrt{
\frac{\gamma\sigma^2}{2A\Delta k}
\left(1+\frac{\xi\Delta}{k}\right)^{\frac{k}{\xi\Delta}+1}
},
\tag{4.6}
\]

\[
\delta_{\mathrm{approx}}^{a*}(q)
=
\frac{1}{\xi\Delta}
\log\!\left(1+\frac{\xi\Delta}{k}\right)
-
\frac{2q-\Delta}{2}
\sqrt{
\frac{\gamma\sigma^2}{2A\Delta k}
\left(1+\frac{\xi\Delta}{k}\right)^{\frac{k}{\xi\Delta}+1}
}.
\tag{4.7}
\]

For \(\xi>0\), define

\[
c_1=
\frac{1}{\xi\Delta}
\log\!\left(1+\frac{\xi\Delta}{k}\right),
\]

\[
c_2=
\sqrt{
\frac{\gamma}{2A\Delta k}
\left(1+\frac{\xi\Delta}{k}\right)^{\frac{k}{\xi\Delta}+1}
}.
\]

For \(\xi=0\), as required by Model B, use the limits

\[
c_1=\frac{1}{k},
\qquad
c_2=\sqrt{\frac{\gamma e}{2A\Delta k}}.
\]

Consequently, the paper's \(\xi=0\) branches of equations (4.6) and (4.7) are

\[
\delta_{\mathrm{approx}}^{b*}(q)
=
\frac{1}{k}
+\frac{2q+\Delta}{2}
\sqrt{\frac{\gamma\sigma^2e}{2A\Delta k}},
\]

\[
\delta_{\mathrm{approx}}^{a*}(q)
=
\frac{1}{k}
-\frac{2q-\Delta}{2}
\sqrt{\frac{\gamma\sigma^2e}{2A\Delta k}}.
\]

Then

\[
\delta_{\mathrm{approx}}^{b*}(q)
=c_1+\frac{\Delta}{2}\sigma c_2+q\sigma c_2,
\]

\[
\delta_{\mathrm{approx}}^{a*}(q)
=c_1+\frac{\Delta}{2}\sigma c_2-q\sigma c_2.
\]

Writing

\[
h=c_1+\frac{\Delta}{2}\sigma c_2
\qquad\text{and}\qquad
j=\sigma c_2,
\]

where \(h\) is the continuous half-spread and \(j\) is the inventory skew per unit of inventory, gives

\[
\delta^{b*}(q)=h+jq,
\qquad
\delta^{a*}(q)=h-jq.
\]

The continuous model prices are therefore

\[
P_b^{\mathrm{cont}}=S-(h+jq),
\qquad
P_a^{\mathrm{cont}}=S+(h-jq).
\]

Equivalently, the model's reservation-price centre is \(S-jq\), around which the half-spread is \(h\).

### What the parameters mean to a market maker

The inputs fall into three different categories and should not all be treated as statistically estimated parameters:

- **Current state:** \(S\) and \(q\) come from the fair-price estimator and the live position.
- **Estimated market behaviour:** \(A\), \(k\), and \(\sigma\) are calibrated from executions/order flow and price data.
- **Strategy choices:** \(\gamma\), \(\Delta\), the objective selected by \(\xi\), and the inventory limit \(Q\) express the maker's risk budget and operating constraints.

The following table turns each quantity into a decision statement. The units assume spot trading with price measured in quote currency per unit of the asset and inventory measured in units of the asset. Contract multipliers must be included consistently for futures and options.

| Parameter | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(S\), in price units | The current fair value around which risk-neutral quotes would be centred | The selected mid-price, microprice, or other fair-price estimator | Anchor both quotes to current market value; changing \(S\) moves both quotes, while inventory then shifts their centre away from it |
| \(q\), in inventory units | The direction and size of the current exposure | Signed inventory, with \(q>0\) long and \(q<0\) short | Move the quote centre by \(-jq\), making the inventory-reducing side more aggressive and the inventory-increasing side less aggressive |
| \(\Delta\), in inventory units per fill | How much one modeled execution changes the position | The order size and inventory grid used by the arrival calibration | Size orders consistently and account for the inventory shock created by each fill |
| \(\delta\), in price units | How far a quote is placed from fair value | The trade-off between spread earned per fill and execution opportunity | Compare candidate quote depths using both expected edge and expected fill rate |
| \(\lambda(\delta)\), in executions per unit time | How quickly the maker's resting order is expected to execute at depth \(\delta\) | A locally stable quote, queue state, and arrival model | Calculate an expected fill count \(\lambda u\), a fill probability \(1-e^{-\lambda u}\), or a mean wait \(1/\lambda\) over the intended quote lifetime |
| \(A\), in executions per unit time | The fitted execution-rate intercept \(\lambda(0)=A\) | The execution curve for an order of size \(\Delta\); it is often extrapolated and is not the market-wide trade rate | Measure available fill opportunity and how quickly inventory could be unwound; higher \(A\) reduces the model's required inventory skew |
| \(k\), in inverse price | How rapidly executions disappear as the quote moves away from fair value | The price-depth sensitivity of \(\lambda(\delta)=Ae^{-k\delta}\) | Quantify the flow sacrificed by quoting farther out: \(1/k\) cuts the rate by \(e\), while \(\log(2)/k\) halves it |
| \(\sigma\), in price per square-root time | The rate at which mark-to-market uncertainty accumulates | The arithmetic reference-price diffusion and the same time basis used for \(A\) | Widen the risk component of the spread and strengthen inventory skew when waiting for fills becomes more dangerous |
| \(\gamma\), in inverse wealth | How strongly the strategy penalises inventory risk | The maker's chosen risk budget, not an estimated market observable | Set how defensively the strategy widens and skews quotes as exposure and volatility increase |
| \(\xi\), in the same units as \(\gamma\), or zero | Which paper objective is being solved | \(\xi=\gamma\) for Model A and \(\xi=0\) for Model B | Use the correct analytic branch without treating \(\xi\) as an independent tuning parameter |
| \(Q\), in inventory units | The hard maximum absolute position | The inventory grid \([-Q,Q]\) and the approximation's risk controls | Stop bidding at \(q=Q\), stop asking at \(q=-Q\), and prevent the affine skew from being the only exposure limit |

The intensity should represent executions of **the maker's resting order**, not every trade printed by the market. On the bid, an execution is normally caused by incoming sell flow reaching the maker's queue; on the ask, it is caused by incoming buy flow. Queue position, order size, latency, partial fills, cancellations ahead, and the venue's matching rules therefore affect the estimates of \(A\) and \(k\). A market-wide trade-arrival regression is only a proxy unless it is calibrated to actual or realistically simulated maker fills.

The paper's displayed closed form assumes the same \(A\) and \(k\) on both sides. In live data, it is useful to estimate

\[
\lambda^b(\delta)=A_b e^{-k_b\delta},
\qquad
\lambda^a(\delta)=A_a e^{-k_a\delta}
\]

as diagnostics for sell-initiated flow reaching the bid and buy-initiated flow reaching the ask. Persistent differences reveal directional flow or liquidity asymmetry, although inserting separate parameters into the control problem requires using the corresponding asymmetric solution rather than the symmetric closed form above.

### Reading the derived quote coefficients

The intermediate quantities are easier to interpret through \(h\) and \(j\):

| Quantity | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(c_1\) | The inventory-independent economic quote depth generated by the execution curve and objective | The continuous-price solution; in Model B it is the e-folding distance \(1/k\) | Separate the basic spread-versus-fill trade-off from the additional volatility and inventory adjustments |
| \(c_2\) | The sensitivity of quotes to risk relative to available liquidity | The combined effect of \(\gamma\), \(A\), \(k\), \(\Delta\), and \(\xi\) | See how market liquidity and the risk budget jointly determine the size of volatility-driven adjustments |
| \(h=c_1+\frac{\Delta}{2}\sigma c_2\) | The zero-inventory continuous half-spread | Quotes before tick rounding, fees, queue effects, and operational buffers | Set a symmetric starting spread of \(2h\) before applying the inventory skew |
| \(j=\sigma c_2\) | The price shift required per unit of inventory | The reservation-price centre \(S-jq\) | Translate the live position into a quote-centre shift; one fill of size \(\Delta\) changes that centre by \(j\Delta\) |

Under the symmetric intensity assumption, the quoted depths imply

\[
\lambda^b(q)=Ae^{-k(h+jq)},
\qquad
\lambda^a(q)=Ae^{-k(h-jq)}.
\]

The model's local expected inventory drift and total execution rate are therefore

\[
\frac{\mathbb{E}[dq]}{dt}
=
\Delta\left(\lambda^b(q)-\lambda^a(q)\right),
\qquad
\lambda_{\mathrm{total}}(q)
=
\lambda^b(q)+\lambda^a(q).
\]

For a long position, \(q>0\), the ask is closer and the bid is farther away, so \(\lambda^a>\lambda^b\) and the expected inventory drift is negative. The strength of this rebalancing can also be read from

\[
\frac{\lambda^a(q)}{\lambda^b(q)}=e^{2kjq}.
\]

This relationship turns a quote skew into an expected fill imbalance. It does not include price drift or adverse selection after a fill, so those should be measured separately in backtests.

### Worked market-making example

Consider Model B calculated entirely in ticks with

\[
\Delta=1,\quad q=3,\quad
A=5\ \text{executions/second},\quad
\widetilde k=0.5\ \text{per tick},
\]

\[
\widetilde\sigma=2\ \text{ticks}/\sqrt{\text{second}},
\qquad
\widetilde\gamma\approx0.0736\ \text{per tick-inventory unit}.
\]

The Model B formulas give

\[
\widetilde c_1=\frac{1}{0.5}=2\ \text{ticks},
\qquad
\widetilde c_2
=
\sqrt{\frac{0.0736e}{2(5)(1)(0.5)}}
\approx0.2,
\]

and hence

\[
\widetilde h=2+\frac{1}{2}(1)(2)(0.2)=2.2\ \text{ticks},
\qquad
\widetilde j=(2)(0.2)=0.4\ \text{ticks per inventory unit}.
\]

Because the maker is long three units, the continuous bid and ask depths are

\[
\widetilde\delta^b=2.2+(0.4)(3)=3.4\ \text{ticks},
\qquad
\widetilde\delta^a=2.2-(0.4)(3)=1.0\ \text{tick}.
\]

The associated execution-arrival rates are approximately

\[
\lambda^b=5e^{-0.5(3.4)}\approx0.91\ \text{per second},
\qquad
\lambda^a=5e^{-0.5(1.0)}\approx3.03\ \text{per second}.
\]

Here, \(q=3\) and \(j=0.4\) tell us that the maker needs a 1.2-tick downward centre shift, in the context of a three-unit long position, so that the ask attracts more inventory-reducing flow than the bid. The fitted \(A\) and \(k\) then tell us that the chosen depths imply rates of 3.03 ask fills and 0.91 bid fills per second, in the context of the exponential execution curve, so that we can estimate the local inventory drift as \((1)(0.91-3.03)=-2.12\) units per second. Finally, those two rates tell us that the probability of at least one fill over the next 100 ms is approximately \(1-e^{-3.03(0.1)}=26.2\%\) on the ask and \(1-e^{-0.91(0.1)}=8.7\%\) on the bid, so that we can compare the intended quote lifetime with the expected speed of inventory reduction.

### Scope of the paper's approximation

The paper derives these formulas under assumptions that matter in implementation:

- The reference price follows the arithmetic diffusion
  \[
  dS_t=\sigma\,dW_t,
  \]
  with constant absolute volatility \(\sigma\) and no drift in the base model.
- Every execution has the same size \(\Delta\), and inventory lies on the grid \(\{-Q,-Q+\Delta,\ldots,Q\}\). The model stops bidding at the long limit and stops asking at the short limit.
- Bid and ask arrival processes are assumed independent of the Brownian reference-price process.
- Equations (4.6) and (4.7) specialise the approximation to identical bid/ask exponential intensity functions, \(\Lambda^b(\delta)=\Lambda^a(\delta)=Ae^{-k\delta}\).
- The closed forms are asymptotic, far-from-terminal approximations obtained through a continuous-inventory/PDE argument. They are not the exact finite-horizon solution of the original nonlinear ODE system.
- In the paper's numerical comparison, the affine-in-inventory approximation is satisfactory near small inventory but becomes less reliable for large \(|q|\); its accuracy also improves when volatility is lower.
- The paper motivates using the approximation in quote-driven markets and in order-driven markets when tick size is small. It does not derive a discrete tick-grid optimum.

Accordingly, the tick rounding below and the use of time-varying/blended volatility are practical engineering extensions. They should be validated against a discrete, event-driven backtest or against the paper's exact ODE solution when the approximation error matters.

---

## 2. The important meaning of “per second”

Volatility is not normally measured in price units per second. In this continuous-time model,

\[
\sigma \quad\text{has units}\quad
\frac{\text{price}}{\sqrt{\text{second}}},
\]

and

\[
\sigma^2 \quad\text{has units}\quad
\frac{\text{price}^2}{\text{second}}.
\]

The time unit of \(A\) must match the time unit of \(\sigma^2\). If volatility is expressed per \(\sqrt{\text{second}}\), use \(A\) in expected executions per second. For example, if an arrival estimate is expressed per 100 ms interval, convert it to a per-second rate by dividing it by \(0.1\) (multiplying it by 10).

This dimensional interpretation is confirmed by the paper's empirical parameter table: it reports \(\sigma\) in absolute price units times \(s^{-1/2}\), \(A\) in \(s^{-1}\), and \(k\) in inverse price units.

The parameter \(k\) controls decay with price distance, not time, so it is unaffected by changing seconds to minutes. It does change when distance is converted from price units to ticks.

### Converting a GARCH volatility

Suppose the GARCH model produces a conditional standard deviation \(g_H\) for a log return over an interval of \(H\) seconds. Its local per-second variance rate and volatility coefficient are

\[
v_{L,\log}=\frac{g_H^2}{H},
\qquad
\sigma_{L,\log}=\frac{g_H}{\sqrt H}.
\]

If the GARCH output is an annualised log-return volatility \(g_{\mathrm{ann}}\), use

\[
\sigma_{L,\log}
=
\frac{g_{\mathrm{ann}}}{\sqrt{N_{\mathrm{active\ seconds/year}}}}.
\]

For a 24/7 market, \(N_{\mathrm{active\ seconds/year}}=365\times24\times3600\). For an exchange-traded instrument, use the active trading seconds represented by the GARCH calibration rather than automatically using calendar seconds.

> **GARCH caveat.** Square-root-of-time conversion is appropriate for expressing a one-step conditional variance as a local variance rate. A multi-step GARCH forecast should ideally be obtained by aggregating the model's forecast conditional variances, because GARCH mean reversion means that multi-step volatility does not in general scale exactly with \(\sqrt H\).

### Calculating the short-term one-minute realised volatility

Prefer mid-prices or microprices to last-trade prices. With one-second log mid-price returns

\[
r_i=\log\!\left(\frac{M_i}{M_{i-1}}\right),
\]

the realised **variance rate** over the latest 60-second window is

\[
v_{S,\log}
=
\frac{1}{60}\sum_{i=t-59}^{t} r_i^2,
\]

and the per-second log-volatility coefficient is

\[
\sigma_{S,\log}=\sqrt{v_{S,\log}}.
\]

With irregular observations, divide the sum of squared returns by the window's elapsed seconds. If “one-minute realised volatility” means \(\sqrt{\sum r_i^2}\) for the minute, divide that number by \(\sqrt{60}\), **not by 60**, to obtain a per-\(\sqrt{\text{second}}\) coefficient.

If only one close-to-close one-minute return \(r_{60}\) is available, the fallback estimate is

\[
v_{S,\log}=\frac{r_{60}^2}{60},
\qquad
\sigma_{S,\log}=\frac{|r_{60}|}{\sqrt{60}}.
\]

That fallback is based on only one return and is much noisier than realised variance constructed from multiple intra-minute returns. Very high-frequency sampling can also inflate realised volatility through bid-ask bounce and other microstructure noise; using one-second mid-prices, subsampling, or an appropriate realised-volatility estimator should be validated for the instrument.

### Combining the long- and short-term estimates

The Guéant formula contains one \(\sigma\), so combine the two estimates **before** calculating \(c_1,c_2,h,j\). Combine variances rather than taking an arbitrary arithmetic average of volatilities.

> **Implementation extension.** The paper assumes a constant \(\sigma\); it does not prescribe GARCH/realised-volatility blending. The policies below are operational overlays that treat the closed form as a locally recalculated quote rule, not a newly derived optimum for stochastic volatility.

A conservative operational choice is to use the GARCH conditional volatility as a floor and the one-minute realised estimate as a fast shock detector:

\[
v_{\mathrm{eff},\log}
=
\max\!\left(v_{L,\log},v_{S,\log}\right),
\qquad
\sigma_{\mathrm{eff},\log}
=
\sqrt{v_{\mathrm{eff},\log}}.
\]

This prevents a quiet or stale one-minute window—possibly with no mid-price movement because of the tick grid—from collapsing the risk term. Its cost is that one noisy minute can widen quotes sharply.

A smoother alternative is

\[
v_{\mathrm{eff},\log}
=
v_{L,\log}
+w\max\!\left(0,v_{S,\log}-v_{L,\log}\right),
\qquad 0\leq w\leq1.
\]

Here \(w\) controls how much of a short-term volatility shock is admitted. It should be selected by walk-forward testing against fill rate, adverse selection, inventory tails, and P&L—not fitted on the same sample used to report performance. If it is acceptable for quotes to become narrower when recent volatility falls below the GARCH forecast, use the symmetric variance blend

\[
v_{\mathrm{eff},\log}
=(1-w)v_{L,\log}+wv_{S,\log}.
\]

The displayed Guéant equations use **absolute price volatility**, not percentage/log volatility. If the two inputs above are log-return volatility coefficients, convert at the current fair price:

\[
\boxed{\sigma=S\,\sigma_{\mathrm{eff},\log}}
\]

which has price units per \(\sqrt{\text{second}}\). If the GARCH and realised-volatility calculations already operate on absolute price changes, do not multiply by \(S\) again.

---

## 3. Using `tick_size` correctly

Let

\[
\tau=\texttt{tick\_size}.
\]

There are two valid implementations. Choose one and do not mix its units with the other.

> **Implementation extension.** The paper works in continuous prices and specifically associates the formulas with small-tick settings. The rounding and post-only rules below make the output executable but are not claimed as the discrete-price optimum of the paper's control problem.

### Method A — calculate in price units, then round the final prices

Calibrate \(k\) against depth \(\delta\) in price units, so \(k\) has inverse-price units. Use \(\sigma\) in price units per \(\sqrt{\text{second}}\), calculate \(h\) and \(j\), and obtain \(P_b^{\mathrm{cont}}\) and \(P_a^{\mathrm{cont}}\).

For conservative passive rounding,

\[
n_b=\left\lfloor\frac{P_b^{\mathrm{cont}}}{\tau}\right\rfloor,
\qquad
n_a=\left\lceil\frac{P_a^{\mathrm{cont}}}{\tau}\right\rceil,
\]

\[
P_b=n_b\tau,
\qquad
P_a=n_a\tau.
\]

Rounding the bid down and the ask up ensures that tick quantisation does not make either quote more aggressive than the continuous model requested. Round the **final prices**, not \(h\) and \(jq\) separately. If \(S\) is itself between ticks, rounding the depths separately can produce invalid prices or an unintended asymmetry.

If the venue's price grid has a non-zero origin \(P_0\), use

\[
n_b=\left\lfloor\frac{P_b^{\mathrm{cont}}-P_0}{\tau}\right\rfloor,
\qquad
n_a=\left\lceil\frac{P_a^{\mathrm{cont}}-P_0}{\tau}\right\rceil,
\]

and reconstruct \(P=P_0+n\tau\). Read the actual grid rule from the instrument metadata. For example, Binance's spot `PRICE_FILTER` requires `price % tickSize == 0` when that filter is enabled.

### Method B — calculate entirely in ticks (often preferable)

Define

\[
\widetilde S=\frac{S}{\tau},
\qquad
\widetilde\sigma=\frac{\sigma}{\tau}
\quad\text{(ticks per }\sqrt{\text{second}}\text{)},
\]

\[
\widetilde k=k\tau,
\qquad
\widetilde\gamma=\gamma\tau,
\qquad
\widetilde\xi=\xi\tau.
\]

These transformations assume that one inventory unit gains or loses one price unit when the asset price moves by one price unit, as in spot with quantity measured in base-asset units. For futures or other contracts, consistently absorb the contract multiplier into the P&L/risk-aversion convention.

If execution intensity is fitted directly against integer depth \(n\) in ticks,

\[
\lambda(n)=A\exp(-\kappa n),
\]

then \(\kappa\) is already \(\widetilde k\): do **not** multiply it by `tick_size` again. Fitting a straight line

\[
\log\lambda(n)=\log A-\kappa n
\]

gives \(A=\exp(\text{intercept})\) in executions per second and \(\kappa=-\text{slope}\) per tick.

For \(\widetilde\xi>0\), calculate in tick space:

\[
\widetilde c_1
=
\frac{1}{\widetilde\xi\Delta}
\log\!\left(1+\frac{\widetilde\xi\Delta}{\widetilde k}\right),
\]

\[
\widetilde c_2
=
\sqrt{
\frac{\widetilde\gamma}{2A\Delta\widetilde k}
\left(1+\frac{\widetilde\xi\Delta}{\widetilde k}\right)^{\frac{\widetilde k}{\widetilde\xi\Delta}+1}
},
\]

For Model B, where \(\widetilde\xi=0\), use

\[
\widetilde c_1=\frac{1}{\widetilde k},
\qquad
\widetilde c_2
=
\sqrt{\frac{\widetilde\gamma e}{2A\Delta\widetilde k}}.
\]

\[
\widetilde h
=
\widetilde c_1+\frac{\Delta}{2}\widetilde\sigma\widetilde c_2,
\qquad
\widetilde j=\widetilde\sigma\widetilde c_2.
\]

The unrounded tick-index quotes are

\[
\widetilde P_b^{\mathrm{cont}}
=
\widetilde S-(\widetilde h+\widetilde jq),
\]

\[
\widetilde P_a^{\mathrm{cont}}
=
\widetilde S+(\widetilde h-\widetilde jq).
\]

Finally,

\[
n_b=\left\lfloor\widetilde P_b^{\mathrm{cont}}\right\rfloor,
\qquad
n_a=\left\lceil\widetilde P_a^{\mathrm{cont}}\right\rceil,
\qquad
P_b=n_b\tau,
\qquad
P_a=n_a\tau.
\]

This tick-space formulation is especially convenient when price changes, volatility, and order-arrival depths are all recorded in ticks. A reference GLFT implementation from `hftbacktest` takes this approach.

### Post-only and top-of-book constraints

For a post-only strategy that may improve the current quote but must not cross, clamp the integer tick indices after the model calculation:

```text
model_bid_tick = floor((fair_price - bid_depth) / tick_size)
model_ask_tick = ceil((fair_price + ask_depth) / tick_size)

bid_tick = min(model_bid_tick, best_ask_tick - 1)
ask_tick = max(model_ask_tick, best_bid_tick + 1)
```

If the strategy is only allowed to join the best quotes and never improve them, use the stricter clamps

```text
bid_tick = min(model_bid_tick, best_bid_tick)
ask_tick = max(model_ask_tick, best_ask_tick)
```

Then enforce

```text
ask_tick >= bid_tick + 1
```

and apply the venue's minimum/maximum-price, quantity-step, minimum-notional, and price-band rules. If both quotes collide after rounding, retain the inventory-reducing side and move the inventory-increasing side outward by at least one tick.

Use integer tick indices or decimal/fixed-point arithmetic for order prices. Binary floating-point expressions such as `price % tick_size` can fail at apparently exact decimal values.

### What tick quantisation changes

- A model half-spread or skew smaller than one tick is still meaningful internally, but it will not change an order until a final quote crosses a tick boundary.
- Do not force each depth to a whole number of ticks before combining it with the fair price; quantise only the final executable prices.
- Outward `floor`/`ceil` rounding is conservative. Nearest-tick rounding is also possible, but it can move either side up to half a tick closer to the market than the continuous optimum. Its behaviour must be included in the backtest.
- A minimum executable spread of one tick is a market constraint, not a replacement for the model spread. Fees, rebates, latency, queue position and adverse selection are absent from the displayed equations and may justify an additional calibrated spread buffer.
- If inventory is large enough that \(h-jq<0\) or \(h+jq<0\), the continuous model may request a quote through the fair price. This can be a deliberate liquidation skew. A passive-only implementation should rely on the post-only clamp, inventory limits, and possibly one-sided quoting rather than silently changing the sign of the model depth.

---

## 4. Recommended calculation sequence

1. Read `tick_size`, the valid price-grid origin/rules, the top of book, and all quantity/notional filters from instrument metadata.
2. Put \(q\) and \(\Delta\) in the same units. If they are lots, use both in lots; if they are base-asset units, use both in base-asset units.
3. Estimate \(A\) in executions per second for an order of size \(\Delta\), and estimate \(k\) either per price unit or per tick. Queue position matters in live fills even if it is omitted from a simple calibration.
4. Convert the GARCH conditional variance and one-minute realised variance to log-variance rates per second.
5. Combine those **variance rates** using the selected policy, then take the square root.
6. Convert log volatility to absolute price volatility with \(\sigma=S\sigma_{\mathrm{eff},\log}\), unless the volatility models already use absolute price changes.
7. Calculate \(c_1,c_2,h,j\) in either price space or tick space.
8. Calculate the continuous bid and ask prices.
9. Quantise final prices to integer ticks, apply post-only/top-of-book constraints, and validate all venue filters.
10. Apply position limits: normally disable or reduce bids at the long limit and asks at the short limit.
11. Recalculate when the fair price, inventory, volatility, intensity estimates, or instrument filters change—but use quote-update thresholds/hysteresis so insignificant sub-tick changes do not cause unnecessary cancel/replace traffic.

### Compact tick-space pseudocode

```text
# Inputs use seconds and ticks:
# A: executions / second
# k_tick: intensity decay per tick
# sigma_tick: ticks / sqrt(second)
# q and order_size: same inventory unit

if xi_tick > 0:                 # Model A uses xi_tick = gamma_tick
    c1 = log1p(xi_tick * order_size / k_tick) / (xi_tick * order_size)
    power = k_tick / (xi_tick * order_size) + 1
    c2 = sqrt(
        gamma_tick
        / (2 * A * order_size * k_tick)
        * (1 + xi_tick * order_size / k_tick) ** power
    )
else:                           # Model B uses xi_tick = 0
    c1 = 1 / k_tick
    c2 = sqrt(gamma_tick * e / (2 * A * order_size * k_tick))

half_spread_tick = c1 + 0.5 * order_size * sigma_tick * c2
skew_per_inventory_tick = sigma_tick * c2

bid_depth_tick = half_spread_tick + skew_per_inventory_tick * q
ask_depth_tick = half_spread_tick - skew_per_inventory_tick * q

model_bid_tick = floor(fair_price / tick_size - bid_depth_tick)
model_ask_tick = ceil(fair_price / tick_size + ask_depth_tick)

bid_tick = min(model_bid_tick, best_ask_tick - 1)
ask_tick = max(model_ask_tick, best_bid_tick + 1)

if ask_tick <= bid_tick:
    preserve the inventory-reducing side and move the other side outward

bid_price = bid_tick * tick_size
ask_price = ask_tick * tick_size
```

For numerical stability when \(\xi\Delta/k\) is small, use a `log1p` implementation. Model B requires the analytic \(\xi=0\) branch instead of division by zero. Consistently,

\[
c_1\longrightarrow\frac{1}{k},
\qquad
c_2^2\longrightarrow\frac{\gamma e}{2A\Delta k}.
\]

---

## 5. Practical checks before live use

- Verify \(A>0\), \(k>0\), \(\gamma>0\), \(\Delta>0\), `tick_size > 0`, and finite volatility. Do not quote from stale or invalid estimates.
- Confirm that \(A\) and \(\sigma^2\) use the same time unit. A seconds/minutes mismatch changes the risk term materially.
- Confirm whether volatility inputs are log/percentage volatility or absolute price volatility. Multiplying by \(S\) twice—or not at all—is a common scale error.
- Confirm whether \(k\) was fitted against price distance or tick distance. Applying `tick_size` twice is another common scale error.
- Backtest with discrete ticks, the actual order-price rules, fees/rebates, latency, queue priority, partial fills, position limits, and cancel/replace behaviour. A continuous-price backtest will overstate precision.
- Monitor the distribution of \(\delta^{b*}\), \(\delta^{a*}\), quoted spread in ticks, fill rate by depth, adverse selection after fills, and time spent at inventory limits.
- Use walk-forward/out-of-sample calibration. Parameters \(A\), \(k\), \(\gamma\), the volatility-combination rule, and any extra spread buffer should not be selected from the same period used for final evaluation.

---

## Sources

1. Olivier Guéant, [“Optimal market making” (PDF)](https://arxiv.org/pdf/1605.01862), especially the model definition in Section 2, the Model A/Model B mapping in Section 3, equations (4.6)–(4.9), the approximation discussion in Section 4, and the units/calibration example in Section 6.
2. `hftbacktest`, [“Guéant–Lehalle–Fernandez-Tapia Market Making Model and Grid Trading”](https://hftbacktest.readthedocs.io/en/py-v2.1.0/tutorials/GLFT%20Market%20Making%20Model%20and%20Grid%20Trading.html), for a practical example calibrated and quoted in tick space.
3. Binance Developer Documentation, [“Filters — PRICE_FILTER”](https://developers.binance.com/en/docs/products/spot/filters#price_filter), for an example of an exchange rule requiring price increments to be exact multiples of `tickSize`.
4. Torben G. Andersen and Luca Benzoni, [“Realized Volatility”](https://www.chicagofed.org/-/media/publications/working-papers/2008/wp2008-14-pdf.pdf), for realised variance as the sum of finely sampled squared returns and discussion of microstructure-noise issues.
