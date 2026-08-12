# Kyle's lambda: calculation, liquidity, toxicity and market-making use

Kyle's lambda, \(\lambda\), measures how strongly price responds to signed order flow. A larger value means that less flow is required to move the price: liquidity is thinner and flow is more expensive to absorb. Its reciprocal is a depth measure.

Kyle's lambda is **not** a complete market-making quote model and is not, by itself, a probability that trading is informed. Use it as a measured price-impact/liquidity state and, after forward validation, as a toxicity overlay on the [Avellaneda–Stoikov](./avellaneda_stoikov.md) or [Guéant](./gueant.md) quotes. The directionless volume-imbalance companion is [VPIN](./vpin.md).

---

## 1. The original Kyle model

In the one-auction version of Kyle's model, let:

- \(v\) be the asset's eventual liquidation value, with prior mean \(p_0\) and variance \(\Sigma_0=\sigma_v^2\);
- \(x\) be the informed trader's signed order;
- \(u\) be signed noise-trader flow, with variance \(\sigma_u^2\);
- \(y=x+u\) be the total order flow seen by competitive market makers; and
- \(p\) be the transaction-clearing price.

The linear pricing rule is

\[
\boxed{p=p_0+\lambda y}.
\tag{1}
\]

For a linear projection of value on observed flow,

\[
\lambda
=
\frac{\operatorname{Cov}(v,y)}{\operatorname{Var}(y)}.
\tag{2}
\]

In the one-auction Gaussian Kyle equilibrium,

\[
\beta=\frac{\sigma_u}{\sigma_v},
\qquad
x=\beta(v-p_0),
\]

and therefore

\[
\boxed{
\lambda=\frac{\sigma_v}{2\sigma_u}
}.
\tag{3}
\]

The factor \(1/2\) is specific to the one-auction equilibrium. In Kyle's normalized continuous-auction equilibrium,

\[
dp(t)=\lambda\,[dx(t)+du(t)],
\qquad
\lambda=\frac{\sigma_v}{\sigma_u},
\]

where \(\sigma_u^2\) is the noise-flow variance rate over the paper's normalized trading interval. The auction structure and horizon therefore matter; this is another reason to estimate and horizon-label lambda in live data rather than transplanting a theoretical constant.

The key liquidity interpretation is

\[
\boxed{D=\frac{1}{\lambda}},
\tag{4}
\]

where \(D\) is the amount of net order flow required to move price by one price unit. Thus:

- higher uncertainty about private value, \(\sigma_v\), raises \(\lambda\);
- more noise trading, \(\sigma_u\), lowers \(\lambda\); and
- small \(\lambda\) means a deep market.

This is the theoretical origin of the empirical price-impact regressions below. The exact equilibrium formula (3) should not be imposed directly on ordinary market data: empirical flow mixes informed, uninformed, inventory, hedging and reactive orders.

### What the theoretical parameters tell a market maker

| Parameter | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(p_0\) | The market's prior mean estimate of liquidation value before auction flow is observed | The one-auction Kyle model | Separate the initial value anchor from the price revision attributed to signed flow |
| \(\sigma_v\) | How uncertain the asset's liquidation value is | Private-value uncertainty in the equilibrium model | Understand why market makers demand more price response per unit of flow when adverse-information risk is greater |
| \(\sigma_u\) | How much noise-trader flow masks informed trading | The variance or variance rate of uninformed signed flow under the stated auction horizon | Understand why abundant noise flow allows the market to absorb more quantity for the same price change |
| \(\beta=\sigma_u/\sigma_v\) | How aggressively the informed trader trades on a value difference | The one-auction Gaussian equilibrium, not an empirical execution coefficient | Relate the information advantage to the amount of flow market makers must interpret |
| \(x\) | The informed trader's signed order | A structural decomposition that is not directly observed in ordinary market data | Avoid treating all empirical signed flow as informed flow |
| \(u\) | The signed noise-trader order | The same structural decomposition | Recognise that large observed flow can move through the market without representing private information |
| \(y=x+u\) | The total signed flow visible to competitive market makers | The auction clearing decision | Update price from observable aggregate flow while acknowledging that its composition is latent |
| \(\lambda\) | The price change associated with one unit of signed flow | A specified auction structure, response variable, flow unit, clock, and horizon | Measure local market fragility and translate a stress flow into expected price displacement |
| \(D=1/\lambda\) | The signed-flow depth available per one price unit of movement | A positive, statistically usable linear lambda estimate | Express liquidity as capacity, making it easier to compare expected fill clusters with the flow needed to move price |

The original paper also distinguishes three dimensions of liquidity: tightness, depth and resiliency. Kyle's \(\lambda\) primarily measures **depth/price impact**. It does not fully measure the quoted spread (tightness) or the speed with which impact decays (resiliency).

---

## 2. What practitioners call “Kyle's lambda”

There is no single universal empirical specification. Always store the response variable, flow units, sampling clock and horizon with the estimate.

The most common estimates answer different market-making questions:

| Estimate | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(\lambda^{\mathrm{trade}}\) | How much midpoint movement is associated with same-interval aggressive trade flow | A declared bar type, flow unit, and contemporaneous regression | Monitor realised price impact and depth without mislabelling the association as a forward toxicity forecast |
| \(\lambda_h^{\mathrm{fwd}}\) | How much signed flow predicts a later midpoint move at horizon \(h\) | Strictly separated flow and future-response windows with causal controls | Estimate adverse-selection risk over the intended quote lifetime and decide whether a directional quote overlay has evidence |
| \(\lambda^r\) | The fractional return associated with one flow unit | A log-return response rather than an absolute-price response | Compare impact after price normalisation and convert with \(S\lambda^r\) before using an absolute-price quote buffer |
| \(\lambda_{\$}\) | The price or return response per unit of signed notional | Cross-sectional scaling with a consistent contract multiplier | Compare instruments more consistently without mixing the result with quantity-based lambda thresholds |
| \(\lambda^+\) and \(\lambda^-\) | Whether buy-side and sell-side flow have different price sensitivity | An asymmetric regression under a documented sign convention | Protect the ask and bid differently when liquidity is thinner on one side |
| \(\beta^{\mathrm{OFI}}\) | The midpoint response to limit-order additions, cancellations, and trades at the best quotes | Order-flow imbalance rather than signed trades alone | Measure top-of-book fragility while keeping OFI impact distinct from Kyle trade-flow lambda |
| \(\eta,\rho\) in nonlinear impact | How impact scale and curvature change across flow sizes | A power-law or square-root response when linearity fails | Avoid extrapolating a constant slope into large-flow regimes where it overstates or understates risk |
| \(\lambda_h\) across several horizons | How much immediate impact persists or reverses | Short and long forward responses estimated on comparable data | Distinguish temporary mechanical fragility from persistent adverse-information risk |

### 2.1 Contemporaneous signed-flow lambda

For completed interval \(t\), calculate

\[
Q*t=\sum*{i\in t}s_i v_i,
\tag{5}
\]

where \(v_i\) is trade size and the aggressor sign is

\[
s_i=
\begin{cases}
+1,&\text{buyer initiated},\\
-1,&\text{seller initiated}.
\end{cases}
\]

Using midpoint \(m_t=(b_t+a_t)/2\), estimate

\[
\boxed{
\Delta m_t=\alpha+\lambda^{\mathrm{trade}} Q_t+\varepsilon_t
},
\qquad
\Delta m_t=m_t^{\mathrm{end}}-m_t^{\mathrm{start}}.
\tag{6}
\]

This is the most direct empirical analogue of the Kyle pricing rule. It measures the price move associated with same-interval aggressive flow. It is useful for **realised impact and liquidity**, but it is partly contemporaneous/mechanical and is not automatically a forward toxicity forecast.

Use midpoints rather than last-trade prices so that bid–ask bounce does not dominate the dependent variable.

### 2.2 Forward or markout lambda

To test whether flow predicts later adverse movement, separate the flow-measurement interval from the forward response:

\[
\boxed{
m*{t+h}-m_t
=
\alpha_h+\lambda_h^{\mathrm{fwd}}Q_t+\mathbf c_t'\boldsymbol\theta+\varepsilon*{t,h}
}.
\tag{7}
\]

Here \(h\) is a fixed future horizon and \(\mathbf c_t\) contains controls known at time \(t\). This is more directly relevant to adverse selection. Estimate several horizons, for example immediate, short and longer markouts.

For a trade-level study,

\[
m(t*i+h)-m(t_i^-)
=
\alpha_h+\lambda_h^{\mathrm{trade}}s_i v_i
+\mathbf c_i'\boldsymbol\theta+\varepsilon*{i,h}.
\tag{8}
\]

Overlapping horizons and clustered trades make ordinary regression standard errors too optimistic. Use heteroskedasticity-and-autocorrelation-consistent errors, block bootstrap or appropriate clustering.

### 2.3 Return lambda

Some implementations regress log returns rather than absolute price changes:

\[
r_t=\log(m_t^{\mathrm{end}}/m_t^{\mathrm{start}})
=\alpha+\lambda^{r}Q_t+\varepsilon_t.
\tag{9}
\]

Near current price \(S\), convert it approximately to absolute-price lambda with

\[
\lambda^{\mathrm{price}}\approx S\lambda^r.
\tag{10}
\]

Return lambda and price lambda have different units and must not be placed in the same raw threshold.

### 2.4 Signed-dollar-volume lambda

Another common convention uses signed notional:

\[
Q*t^{\$}
=
\sum*{i\in t}s_i p_i v_i M,
\tag{11}
\]

where \(M\) is the contract or point-value multiplier when applicable. Regress either \(\Delta m_t\) or \(r_t\) on \(Q_t^{\$}\). This helps cross-sectional scaling but changes the units.

If \(\Delta m\) is the response, \(\lambda\_{\$}\) has units of price change per currency unit of signed notional. Around price \(S\), the approximate price impact of one contract is

\[
\lambda*{\mathrm{contract}}
\approx
\lambda*{\$}SM.
\tag{12}
\]

### 2.5 Fixed-cost/spread decomposition

A Glosten–Harris/Brennan–Subrahmanyam-style version separates proportional impact from a trade-direction/fixed-cost component:

\[
\Delta p*k
=
\lambda q_k
+\psi(D_k-D*{k-1})
+u_k,
\tag{13}
\]

where \(q_k\) is signed trade size and \(D_k\) is trade direction. This is useful when transaction-price changes are used, but midpoint responses are usually cleaner for a live market-making impact feature.

### 2.6 Nonlinear impact

Linear impact often breaks down over large flow ranges. Two alternatives are

\[
\Delta m_t
=
\alpha+\eta\,\operatorname{sign}(Q_t)|Q_t|^\rho+\varepsilon_t,
\qquad 0<\rho\leq1,
\tag{14}
\]

or the fixed square-root form

\[
\Delta m_t
=
\alpha+\eta\,\operatorname{sign}(Q_t)\sqrt{|Q_t|}+\varepsilon_t.
\tag{15}
\]

The coefficient \(\eta\) is **not** in the same units as linear \(\lambda\). Name and threshold it separately.

### 2.7 Buy/sell asymmetry

Define

\[
Q_t^+=\max(Q_t,0),
\qquad
Q_t^-=\min(Q_t,0).
\]

Estimate

\[
\Delta m*t
=
\alpha+\lambda*+Q*t^+ + \lambda*-Q_t^-+\varepsilon_t.
\tag{16}
\]

Both coefficients should normally be positive under this sign convention: \(Q*t^-<0\), so a positive \(\lambda*-\) implies a negative price move. The estimates reveal whether buy-side or sell-side liquidity is currently thinner.

### 2.8 Order-flow-imbalance impact is related, but different

Trade flow ignores limit-order additions and cancellations. For consecutive best quotes indexed by event \(n\), a top-of-book event contribution can be defined as

\[
\begin{aligned}
e*n={}&
\mathbf 1*{\{P*n^B\ge P*{n-1}^B\}}q*n^B
-\mathbf 1*{\{P*n^B\le P*{n-1}^B\}}q*{n-1}^B\\
&-\mathbf 1*{\{P*n^A\le P*{n-1}^A\}}q*n^A
+\mathbf 1*{\{P*n^A\ge P*{n-1}^A\}}q\_{n-1}^A,
\end{aligned}
\tag{17}
\]

where \(P^B,P^A\) are best prices and \(q^B,q^A\) their displayed sizes. Aggregate within interval \(t\):

\[
\operatorname{OFI}_t=\sum_{n\in t}e_n.
\tag{18}
\]

Then estimate

\[
\boxed{
\Delta m_t=\alpha+\beta^{\mathrm{OFI}}\operatorname{OFI}\_t+\varepsilon_t
}.
\tag{19}
\]

Cont, Kukanov and Stoikov find a robust short-horizon linear relationship whose slope is inversely related to observed market depth. This is extremely relevant to market making, but call the estimate **OFI impact** rather than silently treating it as the same quantity as signed-trade Kyle lambda.

### 2.9 Persistent or information impact

Signed flow is serially correlated, price changes can influence later flow, and some immediate impact reverses. A Hasbrouck-style vector autoregression jointly models quote revisions and signed trades. A shock to the **unexpected** component of trade flow is propagated through the system; its cumulative quote response is

\[
\alpha*m(v*{2,0})
=
\sum*{j=0}^{m}
\mathbb E[r_j\mid v*{2,0}],
\tag{20}
\]

where \(r*j\) is the midpoint revision and \(v*{2,0}\) is the trade innovation. As \(m\) grows, the persistent response is interpreted as the information component after transient effects decay.

This is more demanding than rolling OLS, but is the cleaner choice when the objective is **persistent toxicity** rather than immediate impact.

---

## 3. Units and tick-size conversion

Let the exchange tick size be \(\tau\). If equation (6) uses price changes and base quantity, then

\[
[\lambda]
=
\frac{\text{price units}}{\text{shares/contracts/coins}}.
\]

Convert it to ticks per unit:

\[
\boxed{
\lambda\_{\mathrm{tick}}=\frac{\lambda}{\tau}
}.
\tag{21}
\]

The expected move associated with net signed flow \(Q\) is

\[
I(Q)=\lambda Q
\quad\text{price units},
\]

\[
\boxed{
I*{\mathrm{tick}}(Q)=\lambda*{\mathrm{tick}}Q
}.
\tag{22}
\]

The linear-model flow required for a one-tick move is

\[
\boxed{
Q*{1\mathrm{tick}}
=
\frac{\tau}{\lambda}
=
\frac{1}{\lambda*{\mathrm{tick}}}
}.
\tag{23}
\]

This is usually the most intuitive live liquidity measure.

For signed-notional lambda,

\[
N*{1\mathrm{tick}}=\frac{\tau}{\lambda*{\$}}
\tag{24}
\]

is the approximate signed notional required for a one-tick move. Around \(S\), convert to contracts or units using

\[
Q*{1\mathrm{tick}}
\approx
\frac{N*{1\mathrm{tick}}}{SM}.
\tag{25}
\]

### Worked example

Suppose

- \(\widehat\lambda=0.0002\) price units per contract; and
- \(\tau=0.01\).

Then

\[
\lambda\_{\mathrm{tick}}=0.0002/0.01=0.02
\quad\text{ticks per contract},
\]

and

\[
Q\_{1\mathrm{tick}}=1/0.02=50
\quad\text{contracts}.
\]

A 200-contract net aggressive imbalance corresponds to

\[
I\_{\mathrm{tick}}=0.02\times200=4\text{ ticks}
\]

under the fitted local linear relationship.

The decision interpretation is:

- \(\widehat\lambda=0.0002\) tells us that one signed contract is associated with 0.0002 price units of movement, in the context of the regression's exact response, bar construction, and horizon, so that we can translate a candidate flow shock into an expected move without treating the coefficient as universal.
- \(\lambda_{\mathrm{tick}}=0.02\) tells us that each signed contract is associated with 0.02 ticks, in the context of a 0.01 tick size, so that we can compare impact directly with quoted edge and executable price increments.
- \(Q_{1\mathrm{tick}}=50\) tells us that about 50 net aggressive contracts are associated with a one-tick move, in the context of the fitted local linear relationship, so that we can compare market depth with typical fill clusters, displayed depth, and liquidation slices.
- \(I_{\mathrm{tick}}(200)=4\) tells us that a 200-contract imbalance corresponds to four ticks of fitted movement, in the context of the same local model, so that we can decide whether the quote's spread and lifetime adequately compensate for that stress flow.

Do not round \(\lambda\) itself to the tick. Price is discrete; the estimated average impact coefficient need not be.

---

## 4. Practical calculation from trades and quotes

### Step 1: create a clean, causal event stream

For every trade, retain:

- exchange and instrument timestamp;
- price and base quantity;
- best bid, best ask and midpoint immediately before the trade;
- aggressor side from the venue when available; and
- contract multiplier and tick size valid at that timestamp.

Filter corrections, busts, impossible quotes, crossed/locked states according to venue rules, self-trades if appropriate, and roll discontinuities. Consolidate fragmented venues consistently if the strategy sees consolidated liquidity.

### Step 2: sign trades

Prefer the venue's explicit aggressor flag. Otherwise use a documented classifier such as quote matching, followed by a tick rule for unresolved trades. Measure classifier disagreement: sign errors bias \(\lambda\) and can even reverse short-window estimates.

### Step 3: choose the sampling clock

Reasonable choices are:

- fixed time bars, such as one or five seconds;
- fixed numbers of trades; or
- equal-volume buckets.

Time bars align with quote horizons. Volume bars stabilise the quantity per observation and adapt to activity. Do not treat estimates from different clocks as interchangeable.

### Step 4: construct \(Q_t\) and \(\Delta m_t\)

Within each completed bar, sum signed base quantity using equation (5). Take start and end midpoint snapshots from causally available quotes. Exclude the current incomplete bar from a production regression.

For predictive toxicity, use the already-completed flow bar to forecast a strictly later midpoint. Do not let any observation use a future trade, quote or finalised bucket state.

### Step 5: estimate a rolling slope

With an intercept, ordinary least squares gives

\[
\boxed{
\widehat\lambda
=
\frac{
\sum*{j=1}^{N}(Q_j-\bar Q)(\Delta m_j-\overline{\Delta m})
}{
\sum*{j=1}^{N}(Q_j-\bar Q)^2
}
}.
\tag{26}
\]

For exponentially weighted estimation, use weights \(w_j\):

\[
\widehat\lambda_w
=
\frac{
\sum_jw_j(Q_j-\bar Q_w)(\Delta m_j-\overline{\Delta m}\_w)
}{
\sum_jw_j(Q_j-\bar Q_w)^2
}.
\tag{27}
\]

In production, prefer:

- robust regression or winsorisation rules fixed before the test;
- weighted least squares if impact variance changes with activity;
- ridge shrinkage toward a slow estimate when the fast sample is sparse;
- heteroskedasticity/autocorrelation-robust uncertainty estimates; and
- a minimum denominator, observation count and sign-balance requirement.

Do not force the intercept to zero merely because the theoretical pricing rule has zero conditional residual mean. Empirical drift, timestamp mismatch and public news can create a nonzero sample intercept. Compare both specifications out of sample.

### Step 6: calculate a conservative live value

Store the raw point estimate and its uncertainty. For a risk control, a conservative upper impact estimate can be

\[
\lambda^{\mathrm{risk}}
=
\max\!\left(\lambda\_{\min},
\widehat\lambda+z\,\operatorname{SE}(\widehat\lambda)\right),
\tag{28}
\]

where \(z\) and \(\lambda\_{\min}\) are policy choices validated in simulation and backtests. Calculate conservative one-tick capacity from \(\lambda^{\mathrm{risk}}\), not from a noisy negative estimate.

### Minimal streaming pseudocode

```text
for each trade:
    sign = explicit_aggressor_side_or_classifier(trade, prior_quote)
    current_bar.net_flow += sign * trade.base_quantity

when a bar closes:
    x = current_bar.net_flow
    y_now = end_midpoint - start_midpoint
    append completed observation (x, y_now)

    lambda_now = robust_weighted_slope(completed_history)
    lambda_tick = lambda_now / tick_size

    if lambda_tick is positive and statistically usable:
        one_tick_flow = 1 / lambda_tick
    else:
        mark estimate unavailable and use the documented fallback

    schedule future midpoint snapshots for forward horizons h
```

---

## 5. Horizon, volatility and time units

Kyle lambda is indexed by the bar construction and response horizon. It is not automatically “per second.” Label estimates, for example,

\[
\lambda^{\mathrm{trade}}_{\text{1 s bar}},
\qquad
\lambda^{\mathrm{fwd}}_{h=5\mathrm{s}},
\qquad
\lambda^{\mathrm{OFI}}\_{\text{volume bucket}}.
\]

Do **not** scale lambda between horizons with a square-root-of-time rule. Order-flow autocorrelation, liquidity replenishment and impact decay determine the scaling empirically.

Your long-term conditional GARCH volatility and one-minute short-term realised volatility may both be converted to per-second **volatility coefficients** for AS/Guéant, as described in those notes. Kyle lambda is different: it is a price-per-flow coefficient.

Use volatility with lambda in one of these explicit ways:

1. estimate separate lambdas in low-, normal- and high-volatility regimes;
2. include a causal interaction,
   \[
   \Delta m_t
   =\alpha+
   (\lambda_0+\lambda_1 z_t^{\sigma})Q_t+\varepsilon_t;
   \tag{29}
   \]
3. report volatility-normalised impact for comparison,
   \[
   \widetilde\lambda*h
   =\frac{\lambda_h Q*{\mathrm{scale}}}{\sigma\_{\mathrm{abs,sec}}\sqrt h},
   \tag{30}
   \]
   which is expected impact in units of an \(h\)-second volatility move.

Equation (30) is a derived comparison score, not Kyle's original lambda. Use the same absolute-price volatility basis as the quote model. If volatility is a fractional return coefficient, first convert it to absolute price units.

---

## 6. Liquidity uses in market making

The derived control quantities complete the path from an impact estimate to a market-making action:

| Quantity or policy | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(\lambda^{\mathrm{risk}}\) | A conservative impact estimate after accounting for statistical uncertainty | A positive point estimate, its standard error, a floor \(\lambda_{\min}\), and policy level \(z\) | Base risk limits on plausible adverse impact rather than invert a noisy or negative raw slope |
| \(Q_{1\mathrm{tick}}=\tau/\lambda^{\mathrm{risk}}\) | How much net signed flow is associated with one tick of movement | A horizon-labelled linear impact estimate | Compare market fragility with displayed depth, typical trades, and fill-cluster volume |
| \(B\), the impact budget in ticks | The maximum displacement tolerated for one aggressive child order | Local liquidation sizing under equation (33) | Cap child size at \(B\tau/\lambda^{\mathrm{risk}}\) instead of submitting the whole inventory into thin liquidity |
| \(J_t=\lambda_{\mathrm{tick},t}^{\mathrm{risk}}|Q_t^{\mathrm{stress}}|\) | The tick movement associated with a declared stress-flow burst | Passive quote exposure to flow likely to arrive while the order rests | Reduce quote size, lifetime, or participation when expected flow is large relative to current depth |
| \(R_{\mathrm{perm}}=\lambda_{\mathrm{long}}/\lambda_{\mathrm{short}}\) | The fraction of immediate impact that remains at a longer horizon | Stable positive estimates at comparable short and long horizons | Distinguish quickly replenished mechanical fragility from more persistent adverse-information risk |
| \(T_t^{\mathrm{spread}}\) | Expected adverse movement relative to the half-spread being offered | A forward lambda, declared stress flow, and current model quote | Judge whether the quoted edge is large enough to compensate for predicted adverse movement before fees and execution effects |
| \(\kappa\) | The fraction of a validated predicted flow move applied to fair value | The predictive centre shift in equation (43), with \(0\leq\kappa\leq1\) | Control how strongly signed flow moves both quotes without automatically accepting the full regression forecast |
| \(\zeta\) | The scale of the adverse-side protection buffer | The directional buffer in equation (44) | Widen only the flow-exposed side by a separately testable amount |

### 6.1 Live depth and fragility

Track

\[
Q\_{1\mathrm{tick},t}=\frac{\tau}{\lambda_t^{\mathrm{risk}}}.
\]

A falling value means that smaller aggressive bursts are associated with a tick move. Compare it with displayed depth, recent trade size, fill clusters and typical bucket volume.

### 6.2 Normalised liquidity regimes

Raw lambda is asset- and unit-specific. For live regime labels, use the instrument's causal historical distribution:

\[
L*t
=
F*{\lambda,\mathrm{past}}(\lambda_t),
\tag{31}
\]

where \(L_t\) is a rolling percentile. High percentiles mean unusually high impact/low depth for that instrument. Separate intraday seasonality before calculating percentiles.

For cross-asset comparison, fix the response, horizon and flow convention, then use ticks per a standard fraction of ADV or volatility-normalised impact—not raw coefficients.

### 6.3 Capacity for inventory liquidation

If an aggressive child order \(q_c\) is modelled locally by linear impact, its terminal price displacement is approximately

\[
I_c\approx\lambda |q_c|.
\tag{32}
\]

To constrain a child order to an impact budget of \(B\) ticks,

\[
|q_c|
\leq
\frac{B\tau}{\lambda^{\mathrm{risk}}}.
\tag{33}
\]

Under a simple linear marginal-impact curve, the mechanical impact cost of completing \(|q|\) units is approximately \(\tfrac12\lambda q^2\). Treat this only as a local execution heuristic: a regression of market flow is not automatically a causal estimate of your own order's impact.

### 6.4 Quote size and exposure

A passive quote does not mechanically move the market in the same way as aggressive flow. Use lambda to estimate the fragility of the flow likely to arrive while the quote is exposed.

For a stress burst \(Q_t^{\mathrm{stress}}\), define

\[
J*t
=
\lambda*{\mathrm{tick},t}^{\mathrm{risk}}
|Q_t^{\mathrm{stress}}|.
\tag{34}
\]

Then reduce displayed size, participation or quote lifetime as \(J_t\) rises. Calibrate the mapping to actual fill clusters and maker markouts; do not equate passive displayed size with \(Q_t^{\mathrm{stress}}\).

### 6.5 Venue and time-of-day selection

Estimate venue-specific lambda only when trade signs and midpoint formation can be aligned correctly. Compare:

- impact per standard quantity;
- one-tick flow;
- persistent versus transient impact;
- fees/rebates; and
- realised maker markouts.

A low displayed spread with high impact can be worse for a maker than a slightly wider but resilient market.

### 6.6 Resiliency from the impact curve

Estimate \(\lambda_h\) at several horizons. A diagnostic decomposition is

\[
\lambda*{\mathrm{temporary}}
\approx
\lambda*{\mathrm{short}}-\lambda\_{\mathrm{long}},
\tag{35}
\]

and, when both estimates are stable and positive,

\[
R*{\mathrm{perm}}
=
\frac{\lambda*{\mathrm{long}}}{\lambda\_{\mathrm{short}}}.
\tag{36}
\]

A large immediate lambda with a small permanence ratio suggests mechanical fragility followed by replenishment. A large persistent component is more consistent with adverse information. These are empirical diagnostics, not structural identities.

---

## 7. Toxicity uses

### 7.1 What lambda does and does not say

High lambda is consistent with thin liquidity and, in Kyle's theory, a high information-to-noise ratio. Empirically it does **not** identify why the market moved. Public news, low displayed depth, forced liquidation and reactive order flow can all raise measured impact.

For toxicity, require at least two ingredients:

1. a flow or trade innovation that identifies direction and magnitude; and
2. a forward or persistent response demonstrating adverse price continuation.

### 7.2 Impact-weighted signed imbalance

For a completed VPIN volume bucket with signed imbalance

\[
I*\tau=V*\tau^B-V\_\tau^S,
\]

define the descriptive impact-weighted imbalance

\[
\boxed{
\operatorname{IWI}_\tau
=
\lambda_\tau I\_\tau
}.
\tag{37}
\]

In ticks,

\[
\operatorname{IWI}_{\tau,\mathrm{tick}}
=
\lambda_{\mathrm{tick},\tau}I\_\tau.
\tag{38}
\]

Its sign gives the pressure direction; its magnitude estimates the price response associated with the imbalance. “Impact-weighted imbalance” is a derived implementation feature, not a canonical Kyle or VPIN statistic.

### 7.3 Combining lambda with VPIN

Because

\[
\operatorname{VPIN}_\tau V
=
\frac1n\sum_{j=\tau-n+1}^{\tau}|I_j|,
\]

a constant-lambda approximation to average absolute bucket impact is

\[
\boxed{
\operatorname{IWV}_\tau
\approx
\lambda_\tau\operatorname{VPIN}\_\tau V
}.
\tag{39}
\]

In ticks, divide by \(\tau\), or use \(\lambda\_{\mathrm{tick}}\). If lambda varies materially within the VPIN window, calculate the more faithful statistic

\[
\operatorname{IWV}_\tau
=
\frac1n\sum_{j=\tau-n+1}^{\tau}\lambda_j|I_j|.
\tag{40}
\]

VPIN contributes one-sidedness; lambda contributes the market's price sensitivity to flow. Neither multiplication produces a literal probability of informed trading.

### 7.4 Spread-relative toxicity

Let \(h_t^{\mathrm{quote}}\) be the relevant quoted or model half-spread in price units. Define

\[
\boxed{
T_t^{\mathrm{spread}}
=
\frac{\lambda_h^{\mathrm{fwd}}|Q_t^{\mathrm{stress}}|}
{h_t^{\mathrm{quote}}}
}.
\tag{41}
\]

This asks whether expected adverse movement is small or large relative to the edge being offered. A value above one is a useful alarm interpretation, not a universal trading threshold; fees, rebates, queue position, fill probability and model error still matter.

### 7.5 Toxicity-aware quote overlay

Let \(S*t^{\mathrm{model}}\), \(\delta_t^{b,0}\) and \(\delta_t^{a,0}\) come from AS or Guéant. Let a strictly causal flow model predict signed future flow \(\widehat Q*{t,h}\). The implied move is

\[
\mu*{t,h}^{\mathrm{flow}}
=
\lambda_h^{\mathrm{fwd}}\widehat Q*{t,h}.
\tag{42}
\]

Two distinct overlays are possible.

**Predictive centre shift**

\[
S*t^{\mathrm{tox}}
=
S_t^{\mathrm{model}}
+\kappa\mu*{t,h}^{\mathrm{flow}},
\qquad 0\leq\kappa\leq1.
\tag{43}
\]

Use this only if signed flow predicts future midpoint movement out of sample.

**Adverse-side buffer**

\[
b*t^a=\zeta\max(0,\mu*{t,h}^{\mathrm{flow}}),
\qquad
b*t^b=\zeta\max(0,-\mu*{t,h}^{\mathrm{flow}}).
\tag{44}
\]

Positive predicted pressure protects the ask from selling too cheaply; negative predicted pressure protects the bid from buying too expensively. A separate symmetric uncertainty buffer may be based on \(\lambda^{\mathrm{risk}}\mathbb E|Q\_{t,h}|\).

Do not apply the full centre shift and the full side buffer to the same forecast without joint calibration; that double-counts one signal.

After the chosen overlay, round away from the centre:

\[
p_t^b
=
\tau\left\lfloor
\frac{S_t^{\mathrm{tox}}-\delta_t^{b,0}-b_t^b}{\tau}
\right\rfloor,
\tag{45}
\]

\[
p_t^a
=
\tau\left\lceil
\frac{S_t^{\mathrm{tox}}+\delta_t^{a,0}+b_t^a}{\tau}
\right\rceil.
\tag{46}
\]

Then enforce non-crossing, minimum-spread, price-band, inventory, size and venue rules.

Keep this overlay separate from the AS/Guéant volatility \(\sigma\), risk aversion \(\gamma\), and fill-decay \(k\). The components have different meanings and can be validated separately.

### 7.6 Risk controls

Lambda can also drive:

- a size multiplier when one-tick flow falls below typical fill-cluster volume;
- shorter quote lifetime in fragile markets;
- wider cancel/reprice thresholds;
- reduced inventory limits when unwind impact rises;
- a pause on one or both sides when impact, flow imbalance and adverse markouts jointly breach causal thresholds; and
- a switch from passive liquidation to slower scheduling when estimated impact cost rises.

Use several confirming signals. A lambda-only kill switch will confuse ordinary volatility/liquidity changes with informed toxicity.

---

## 8. Parameters that must be specified

| Parameter or choice | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| Response | What kind of price change lambda explains | Midpoint change, transaction-price change, or log return | Interpret the coefficient correctly and avoid importing a return-based threshold into a price-based quote model |
| Flow measure | What one unit of the explanatory variable represents | Signed base quantity, signed notional, or OFI | Give lambda auditable units and compare only estimates built from compatible flow definitions |
| Trade sign | Which trades are treated as buyer- or seller-initiated | Explicit aggressor flags or a documented classifier | Preserve the direction of impact and detect when classification error makes the estimate unreliable |
| Sampling clock | How trades and quotes are grouped into observations | Time, trade-count, or equal-volume bars | Match the liquidity estimate to the quote horizon and understand how activity is weighted |
| Horizon \(h\) | When the price response is measured relative to the flow | Contemporaneous, forward, or persistent markout regressions | Separate immediate mechanical impact from the adverse movement that persists over the maker's exposure window |
| Lookback \(N\) | How much completed history supports the current slope | A rolling estimator in a changing liquidity regime | Balance fast adaptation against sampling noise and unstable inversions |
| Weight half-life | How quickly older observations lose influence | An exponentially weighted estimator | Set the reaction speed independently of the raw observation count |
| Intercept | Whether average drift or timing bias is absorbed separately | Empirical data that need not satisfy the theoretical zero-intercept rule in sample | Prevent persistent drift from being forced incorrectly into the impact slope |
| Robust loss/outlier rule | How unusual news moves and bad observations influence the fit | Heavy-tailed market data and predefined cleaning rules | Stabilise the live estimate without silently deleting genuine stress regimes |
| Controls | Which observable confounders are held fixed | Volatility, spread, depth, time of day, news, and market regime | Test whether flow contributes incremental impact information instead of proxying for an already-known state |
| Side asymmetry | Whether buy and sell flow have different slopes | Separate \(\lambda^+\) and \(\lambda^-\) under one sign convention | Detect side-specific thinness and apply protection to the exposed quote side |
| Tick size \(\tau\) | The venue's minimum executable price increment | Conversion from price lambda to ticks per flow unit | Express impact as one-tick capacity and compare it directly with spread and quote placement |
| Multiplier \(M\) | The cash notional represented by one contract and one price-point move | Futures or other derivative flow | Convert quantity and notional impact without losing the contract's economic scale |
| Minimum data rule | Whether the fast estimate has enough observations, flow variance, and sign balance | Live slope estimation and inversion into depth | Suppress statistically unusable values before they create extreme quote or size controls |
| Uncertainty level \(z\) | How much estimation uncertainty is added to the point estimate | The conservative value \(\lambda^{\mathrm{risk}}=\widehat\lambda+z\operatorname{SE}(\widehat\lambda)\) | Increase protection when impact may be understated without pretending the upper estimate is the measured mean |
| Fallback | What value or action replaces an unavailable fast estimate | Sparse data, negative slopes, feed failure, or broken assumptions | Define deterministic behaviour such as a slow prior, conservative regime estimate, or no-quote state |

Store these fields beside every lambda value. A naked column named `lambda` is not auditable.

---

## 9. Validation against market-maker outcomes

### 9.1 Maker markouts

Let

\[
s_i^{\mathrm{maker}}=
\begin{cases}
+1,&\text{maker bought at the bid},\\
-1,&\text{maker sold at the ask}.
\end{cases}
\]

The midpoint markout at horizon \(h\) is

\[
M\_{i,h}
=
s_i^{\mathrm{maker}}
\left(m(t_i+h)-p_i^{\mathrm{fill}}\right).
\tag{47}
\]

Negative values are adverse. Test whether lambda, signed flow, \(\lambda\times\)flow, OFI impact and \(\lambda\times\)VPIN add out-of-sample explanatory power for these markouts after controlling for spread, volatility, depth, time of day and inventory.

### 9.2 Required diagnostics

At minimum:

1. plot realised midpoint change against fitted impact by decile;
2. report slope confidence intervals, residual autocorrelation and heteroskedasticity;
3. compare fast and slow estimates through regime changes;
4. test buy and sell sides separately;
5. test multiple non-overlapping and overlapping horizons correctly;
6. compare direct book depth with \(Q\_{1\mathrm{tick}}\);
7. measure incremental markout and P&L value after costs;
8. run replay with causal timestamps, latency, queue position and tick rounding; and
9. reserve the latest period or entire sessions for untouched validation.

### 9.3 Descriptive impact is not causal impact

Price changes can cause trades just as trades can cause price changes. Public information may move both. Same-window OLS therefore measures association, not the counterfactual impact of submitting your own order.

For stronger inference, use lagged flow, explicit forward horizons, dynamic VAR models, natural instruments where defensible, and execution experiments with strict risk controls. Still label the result according to what was actually identified.

---

## 10. Common failure modes

1. **Wrong sign convention.** Verify that net buyer-initiated flow produces a positive fitted response in ordinary periods.
2. **Bid–ask bounce.** Transaction prices can manufacture apparent reversals; prefer midpoint responses.
3. **Timestamp leakage.** Quotes stamped after a trade must not be used as its pre-trade quote.
4. **Mixing units.** Price/base-unit, return/base-unit and price/notional lambdas are different features.
5. **Inverting noise.** Never calculate one-tick depth from a zero, negative or statistically unusable estimate.
6. **Forcing linearity.** Inspect residuals and large-flow bins; use nonlinear models when required.
7. **Ignoring autocorrelation.** Flow persistence makes ordinary errors and naive forward tests misleading.
8. **Confusing OFI and trade flow.** Both are useful, but their coefficients measure different inputs.
9. **Treating lambda as informed-trading probability.** The empirical relationship depends materially on specification and context.
10. **Assuming horizon scaling.** Lambda does not follow the volatility square-root-time rule.
11. **Ignoring intraday seasonality.** Market open, close and news periods can have different depth-impact relations.
12. **Double-counting toxicity.** Do not put the same flow signal into fair price, spread, size, volatility and risk aversion at full strength.
13. **Using your passive size as aggressive flow.** Quote exposure and aggressive market impact are not the same causal object.
14. **Fragmented-market mismatch.** Local flow with consolidated price, or the reverse, can create spurious impact.

---

## 11. Suggested research configuration

Use this as a starting experiment, not as universal production calibration:

1. estimate trade-flow and OFI impact as separate models;
2. use completed one-second or volume bars and midpoint changes;
3. maintain a fast exponentially weighted estimate and a slow session/regime estimate;
4. estimate contemporaneous, 1-second, 5-second and longer markout horizons appropriate to quote lifetime;
5. run separate buy/sell slopes;
6. store price lambda, tick lambda, one-tick flow, uncertainty and fit quality;
7. condition or interact with the short-term realised-volatility regime while using long-term GARCH volatility as a slower state variable;
8. combine lambda with signed VPIN-bucket imbalance only after aligning volume units and horizon;
9. drive the quote overlay from out-of-sample maker markouts, not in-sample \(R^2\); and
10. use a documented fallback whenever flow variance, sample count or data quality is inadequate.

The practical hierarchy is:

\[
\boxed{
\text{clean signed flow}
\rightarrow
\text{horizon-labelled impact}
\rightarrow
\text{tick-normalised liquidity}
\rightarrow
\text{validated adverse markout}
\rightarrow
\text{quote/size control}
}.
\]

---

## 12. Sources

1. Albert S. Kyle, _Continuous Auctions and Insider Trading_, Econometrica 53(6), 1985: [published record](https://www.econometricsociety.org/publications/econometrica/1985/11/01/continuous-auctions-and-insider-trading) and [paper PDF](https://people.duke.edu/~qc2/BA532/1985%20EMA%20Kyle.pdf).
2. Rama Cont, Arseniy Kukanov and Sasha Stoikov, _The Price Impact of Order Book Events_: [arXiv paper](https://arxiv.org/abs/1011.6402).
3. Joel Hasbrouck, _Measuring the Information Content of Stock Trades_, Journal of Finance 46(1), 1991: [paper PDF](https://www.acsu.buffalo.edu/~keechung/MGF743/Readings/K2.pdf).
4. Kenneth R. Ahern, _Do Proxies for Informed Trading Measure Informed Trading? Evidence from Illegal Insider Trades_: [NBER working paper](https://www.nber.org/papers/w24297). This paper is useful for its comparison of transaction-, window-, price-change- and return-based lambda specifications; it also cautions that the relationship with observed informed trading depends on how lambda is calculated.

These sources do not prescribe the production overlays in sections 6 and 7. Those formulas are explicitly labelled implementation heuristics and require causal, instrument-specific validation.
