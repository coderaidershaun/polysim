# Avellaneda–Stoikov market making: equations and practical tick-size implementation

This note covers the classic finite-horizon Avellaneda–Stoikov (AS) approximation. The companion Guéant note is [gueant.md](./gueant.md).

## 1. Original model and notation

Let:

- \(S_t\) be the market mid-price at time \(t\);
- \(p_t^b\) and \(p_t^a\) be the market maker's bid and ask prices;
- \(\delta_t^b=S_t-p_t^b\) be the bid depth below the mid-price;
- \(\delta_t^a=p_t^a-S_t\) be the ask depth above the mid-price;
- \(q_t\) be signed inventory, with \(q_t>0\) denoting a long position;
- \(X_t\) be cash wealth;
- \(T\) be the terminal time and \(H_t=T-t\) the remaining horizon;
- \(\gamma>0\) be the CARA risk-aversion parameter;
- \(\sigma\) be the constant absolute price-volatility coefficient;
- \(A\) and \(k\) parameterise execution intensity; and
- \(N_t^b,N_t^a\) count executions at the market maker's bid and ask.

The paper models the mid-price as

\[
dS_t=\sigma\,dW_t,
\tag{2.1}
\]

and assumes symmetric exponential execution intensities

\[
\lambda^b(\delta^b)=A e^{-k\delta^b},
\qquad
\lambda^a(\delta^a)=A e^{-k\delta^a}.
\]

### What the parameters tell a market maker

The inputs have different roles and should not all be treated as statistically estimated parameters:

- **Current state:** \(S_t\), \(q_t\), \(X_t\), and \(t\) describe fair value, position, cash, and time now.
- **Estimated market behaviour:** \(A\), \(k\), and \(\sigma\) are calibrated from executions/order flow and price data.
- **Strategy choices:** \(\gamma\), \(T\), and the practical fill size \(\Delta\) encode the maker's risk budget, liquidation horizon, and order sizing.

The following table makes the decision link explicit. Units assume spot trading with price measured in quote currency per unit of the asset and inventory measured in units of the asset. Contract multipliers must be included consistently for derivatives.

| Parameter | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(S_t\), in price units | The current mid-price or fair-value anchor | The model's exogenous reference-price process | Centre risk-neutral quotes on current value before applying inventory and spread adjustments |
| \(q_t\), in inventory units | The direction and size of current exposure | Signed inventory, with \(q_t>0\) long and \(q_t<0\) short | Shift both quotes toward the inventory-reducing side and away from the inventory-increasing side |
| \(X_t\), in cash units | The cash component of current marked-to-market wealth | The terminal utility \(X_T+q_TS_T\) | Account for realised trading cash flows consistently, although \(X_t\) does not appear in the approximate quote depths |
| \(H_t=T-t\), in time units | How long price risk can accumulate before the terminal objective is evaluated | A finite trading session or an explicitly chosen rolling horizon | Scale the inventory skew and risk spread to the remaining exposure window |
| \(\gamma\), in inverse wealth | How strongly the strategy penalises uncertain terminal inventory value | The maker's CARA risk budget, not a market observable | Choose how defensively quotes widen and shift as inventory and forecast variance increase |
| \(\sigma\), in price per square-root time | The rate at which mark-to-market uncertainty accumulates | The arithmetic diffusion \(dS_t=\sigma dW_t\) | Forecast remaining price variance and increase spread and inventory skew when waiting risk is high |
| \(V_t\), in price squared | The total price variance expected to accumulate from now until \(T\) | \(V_t=\sigma^2H_t\) under constant volatility, or a forecast integrated variance under time-varying volatility | Apply the risk term once over the intended horizon without confusing a variance rate with an already integrated variance |
| \(A\), in executions per unit time | The fitted execution-rate intercept, \(\lambda(0)=A\) | The maker's resting order at zero modeled depth, for a specified size and queue environment | Convert selected depths into expected fill counts, waiting times, and inventory drift; \(A\) cancels from this quote approximation but not from its trading outcomes |
| \(k\), in inverse price | How rapidly execution opportunity falls as a quote moves away from fair value | The exponential curve \(\lambda(\delta)=Ae^{-k\delta}\) | Quantify the fill flow sacrificed by quoting farther out: \(1/k\) reduces intensity by \(e\), while \(\log(2)/k\) halves it |
| \(\lambda^b(\delta^b)\) and \(\lambda^a(\delta^a)\), in executions per unit time | How quickly the maker's bid and ask are expected to fill at their chosen depths | Incoming sell flow reaching the bid and incoming buy flow reaching the ask | Calculate short-horizon fill probabilities and the expected direction and speed of inventory change |
| \(\Delta\), in inventory units per execution | How much one practical fill changes inventory | The fixed-size extension below; the original paper sets \(\Delta=1\) share | Keep order sizing, execution calibration, risk aversion, and inventory units consistent |

The execution intensity should describe fills of **the maker's resting order**, not every trade printed by the market. Queue position, order size, latency, partial fills, cancellations ahead, and matching rules affect \(A\) and \(k\). A market-wide trade-arrival model is only a proxy unless it is mapped to actual or realistically simulated maker fills.

The symmetric specification assumes identical \(A\) and \(k\) on the two sides. Separate estimates \(A_b,k_b\) and \(A_a,k_a\) can tell us whether executable sell flow at the bid differs from executable buy flow at the ask, in the context of directional liquidity and adverse-selection risk, so that we can diagnose when the symmetric closed form is a poor representation. Using asymmetric parameters inside the optimiser requires the corresponding asymmetric control solution rather than silently substituting them into the symmetric formulas.

In the original paper, each fill is one share, so

\[
dq_t=dN_t^b-dN_t^a,
\]

\[
dX_t=p_t^a\,dN_t^a-p_t^b\,dN_t^b.
\]

An ask execution reduces inventory and increases cash; a bid execution increases inventory and reduces cash. The market maker maximises terminal expected exponential utility:

\[
\max\_{\delta^a,\delta^b}
\mathbb E_t\!\left[
-\exp\!\left(-\gamma(X_T+q_T S_T)\right)
\right].
\]

### Reservation prices

For the frozen-inventory problem, the reservation ask and bid prices are

\[
r^a(S,q,t)
=
S+\frac{1-2q}{2}\gamma\sigma^2(T-t),
\tag{2.6}
\]

\[
r^b(S,q,t)
=
S+\frac{-1-2q}{2}\gamma\sigma^2(T-t).
\tag{2.7}
\]

Their average is the reservation, or indifference, price

\[
\boxed{
r_t=S_t-q_t\gamma\sigma^2(T-t)
}.
\]

A positive inventory lowers the reservation price, moving both quotes down to favour selling. A negative inventory raises it to favour buying.

### Approximate optimal spread and quotes

Using an expansion in inventory and a first-order approximation of the arrival term, the paper obtains

\[
\boxed{
\Psi_t
=
\delta_t^a+\delta_t^b
=
\gamma\sigma^2(T-t)
+\frac{2}{\gamma}
\log\!\left(1+\frac{\gamma}{k}\right)
}
\tag{3.18}
\]

for the full quoted spread. The quotes are centred on \(r_t\):

\[
\boxed{
p_t^b=r_t-\frac{\Psi_t}{2}
},
\qquad
\boxed{
p_t^a=r_t+\frac{\Psi_t}{2}
}.
\]

Equivalently, define

\[
c\_{\mathrm{AS}}
=
\frac{1}{\gamma}
\log\!\left(1+\frac{\gamma}{k}\right),
\]

\[
j*{\mathrm{AS}}
=
\gamma\sigma^2(T-t),
\qquad
h*{\mathrm{AS}}
=
c*{\mathrm{AS}}+\frac{j*{\mathrm{AS}}}{2}.
\]

Then the optimal depths are

\[
\boxed{
\delta*t^{b\*}=h*{\mathrm{AS}}+q*tj*{\mathrm{AS}}
},
\]

\[
\boxed{
\delta*t^{a\*}=h*{\mathrm{AS}}-q*tj*{\mathrm{AS}}
}.
\]

Thus \(h*{\mathrm{AS}}\) is the continuous half-spread and \(j*{\mathrm{AS}}\) is the inventory skew per inventory unit.

### Reading the derived quote quantities

The closed form is most useful when each intermediate quantity is connected to a quote decision:

| Quantity | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(r_t=S_t-q_t\gamma\sigma^2H_t\) | The price at which the maker is indifferent to buying or selling one marginal unit after accounting for inventory risk | Current inventory and the price variance remaining until \(T\) | Shift the quote centre below \(S_t\) when long and above \(S_t\) when short, encouraging inventory to return toward zero |
| \(c_{\mathrm{AS}}=\gamma^{-1}\log(1+\gamma/k)\) | The inventory-independent liquidity component of each quote depth | The trade-off between spread capture, risk aversion, and the execution curve | Establish the basic half-spread before remaining-variance and inventory effects are added |
| \(j_{\mathrm{AS}}=\gamma\sigma^2H_t\) | The price skew required per unit of inventory | Finite-horizon inventory risk | Translate the live position into a reservation-price displacement of \(-q_tj_{\mathrm{AS}}\) |
| \(h_{\mathrm{AS}}=c_{\mathrm{AS}}+j_{\mathrm{AS}}/2\) | The continuous half-spread at zero inventory | Unit-sized fills in the original approximation | Set a symmetric starting spread of \(2h_{\mathrm{AS}}\) before inventory moves its centre |
| \(\Psi_t=2h_{\mathrm{AS}}\) | The total continuous bid-ask spread | Quotes before tick rounding, fees, latency, queue effects, and adverse-selection buffers | Compare the model spread with the executable minimum and the net edge required by the strategy |
| \(\delta_t^{b*}=h_{\mathrm{AS}}+q_tj_{\mathrm{AS}}\) and \(\delta_t^{a*}=h_{\mathrm{AS}}-q_tj_{\mathrm{AS}}\) | The bid and ask distances implied by spread and inventory risk together | The symmetric execution model | Place the inventory-increasing quote farther away and the inventory-reducing quote closer to fair value |

### Extension to an execution size \(\Delta\)

The original AS paper fixes the fill size at one share. To align the notation with the Guéant model, suppose instead that every fill has size \(\Delta\), with \(q\) and \(\Delta\) expressed in the same units:

\[
dq_t=\Delta\,dN_t^b-\Delta\,dN_t^a,
\]

\[
dX_t=\Delta p_t^a\,dN_t^a-\Delta p_t^b\,dN_t^b.
\]

Applying the same CARA indifference-price and first-order-condition calculation gives

\[
c\_{\mathrm{AS},\Delta}
=
\frac{1}{\gamma\Delta}
\log\!\left(1+\frac{\gamma\Delta}{k}\right),
\]

\[
j\_{\mathrm{AS},\Delta}
=
\gamma\sigma^2(T-t),
\]

\[
h*{\mathrm{AS},\Delta}
=
c*{\mathrm{AS},\Delta}
+\frac{\Delta}{2}j\_{\mathrm{AS},\Delta}.
\]

The reservation price, depths, and full spread become

\[
r*t=S_t-q_tj*{\mathrm{AS},\Delta},
\]

\[
\boxed{
\delta*t^{b\*}=h*{\mathrm{AS},\Delta}+q*tj*{\mathrm{AS},\Delta}
},
\qquad
\boxed{
\delta*t^{a\*}=h*{\mathrm{AS},\Delta}-q*tj*{\mathrm{AS},\Delta}
},
\]

\[
\Psi*t
=
2h*{\mathrm{AS},\Delta}
=
2c\_{\mathrm{AS},\Delta}
+\gamma\Delta\sigma^2(T-t).
\]

This \(\Delta\)-sized form is a practical extension of the unit-fill equations, not an equation numbered in the original paper. Setting \(\Delta=1\) recovers the published approximation.

In the risk-neutral limit \(\gamma\to0\),

\[
c*{\mathrm{AS},\Delta}\longrightarrow\frac{1}{k},
\qquad
j*{\mathrm{AS},\Delta}\longrightarrow0,
\]

so the quotes become symmetric around \(S_t\) with half-spread \(1/k\).

### Worked market-making example

Consider the fixed-size extension calculated in ticks with

\[
\Delta=1,\quad q_t=3,\quad H_t=5\ \text{seconds},
\]

\[
\widetilde\sigma=2\ \text{ticks}/\sqrt{\text{second}},\quad
\widetilde\gamma=0.02\ \text{per tick-inventory unit},\quad
\widetilde k=0.5\ \text{per tick},
\]

and \(A=5\) executions per second. The remaining integrated variance is

\[
\widetilde V_t=\widetilde\sigma^2H_t=2^2(5)=20\ \text{ticks}^2.
\]

The static depth, skew, and half-spread are

\[
\widetilde c
=
\frac{1}{0.02}
\log\!\left(1+\frac{0.02}{0.5}\right)
\approx1.96\ \text{ticks},
\]

\[
\widetilde j=\widetilde\gamma\widetilde V_t=(0.02)(20)=0.4
\ \text{ticks per inventory unit},
\]

\[
\widetilde h=1.96+\frac{1}{2}(1)(0.4)=2.16\ \text{ticks}.
\]

The three-unit long position gives a reservation-price shift of \(-q_t\widetilde j=-1.2\) ticks and continuous depths

\[
\widetilde\delta^b=2.16+(3)(0.4)=3.36\ \text{ticks},
\qquad
\widetilde\delta^a=2.16-(3)(0.4)=0.96\ \text{ticks}.
\]

The parameters now support an explicit decision chain:

- \(q_t=3\) and \(\widetilde j=0.4\) tell us that the quote centre should move down by 1.2 ticks, in the context of a three-unit long position with five seconds of remaining price risk, so that the ask attracts inventory-reducing flow while the bid becomes less aggressive.
- \(\widetilde h=2.16\) tells us that the continuous spread is 4.32 ticks, in the context of the model before tick rounding and operational costs, so that we can compare the theoretical spread with the venue grid, fees, and required net edge.
- \(A=5\) and \(\widetilde k=0.5\) tell us that these depths imply \(\lambda^b\approx5e^{-0.5(3.36)}=0.93\) and \(\lambda^a\approx5e^{-0.5(0.96)}=3.09\) executions per second, in the context of the exponential fill curve, so that we can estimate local inventory drift as \((1)(0.93-3.09)=-2.16\) units per second.
- The two execution rates tell us that the 100 ms probability of at least one fill is about \(1-e^{-0.93(0.1)}=8.9\%\) on the bid and \(1-e^{-3.09(0.1)}=26.6\%\) on the ask, in the context of locally constant independent Poisson arrivals, so that we can compare quote lifetime with the expected speed of inventory reduction.

The fill-rate calculations use \(A\) even though \(A\) cancels from the approximate quote formula. This distinction tells us that a closed-form price can be unchanged while its expected execution and inventory outcomes change materially.

### Scope of the approximation

The following qualifications come directly from the model structure and derivation:

- The simple formulas above are an approximation to the HJB problem. The paper obtains them from an inventory expansion and a first-order approximation of the order-arrival term.
- The base model assumes an arithmetic Brownian mid-price with constant \(\sigma\), zero drift and no autocorrelation.
- The intensities on both sides are symmetric and exponential.
- Limit orders may be updated continuously without cost; latency, queue priority, fees, rebates, partial fills and adverse selection are omitted.
- The reference mid-price is exogenous to the market maker's own quoting and trading.
- The finite-horizon inventory term shrinks as \(t\to T\). On a 24/7 venue, choosing and resetting a synthetic horizon is an implementation decision, not part of the original finite-session interpretation.
- The parameter \(A\) cancels from this particular first-order closed-form quote. It still determines expected fill rates, inventory dynamics and simulated P&L. Its absence from \(r_t\) and \(\Psi_t\) is not a calibration licence to ignore it.

The original paper also studies a stationary infinite-horizon objective, but its reservation prices are not the same as simply replacing \(T-t\) by an arbitrary constant. For continuous, stationary quoting, the companion Guéant approximation provides a cleaner directly stationary comparison.

---

## 2. Per-second volatility and the remaining horizon

With time measured in seconds,

\[
\sigma
\quad\text{has units}\quad
\frac{\text{price}}{\sqrt{\text{second}}},
\]

\[
\sigma^2
\quad\text{has units}\quad
\frac{\text{price}^2}{\text{second}},
\]

and \(H_t=T-t\) must be expressed in seconds. Therefore

\[
V_t=\sigma^2H_t
\]

is the remaining absolute price variance, with units of price squared. The AS equations can be written more generally as

\[
r_t=S_t-q_t\gamma V_t,
\]

\[
j*{\mathrm{AS},\Delta}=\gamma V_t,
\qquad
h*{\mathrm{AS},\Delta}
=
c\_{\mathrm{AS},\Delta}
+\frac{\Delta}{2}\gamma V_t.
\]

This \(V_t\) form is useful when volatility is time-varying. The theoretically relevant input is then the forecast integrated variance

\[
V_t
=
\mathbb E_t\!\left[
\int_t^T\sigma_u^2\,du
\right],
\]

rather than merely the latest instantaneous variance rate multiplied by a long horizon.

When seconds are the clock unit, calibrate \(A\) in expected fills per second and \(k\) in inverse price units. If the arrival intensity is measured per minute or per 100 ms interval, convert \(A\) to a per-second rate before using it in fill simulation. Although \(A\) cancels from the approximate quote formula, the time unit still matters for executions and backtesting.

### Long-term GARCH volatility

If the GARCH model produces a conditional log-return standard deviation \(g_L\) for an interval of \(L\) seconds, convert it to a per-second log-variance rate:

\[
v*{L,\log}=\frac{g_L^2}{L},
\qquad
\sigma*{L,\log}=\frac{g_L}{\sqrt L}.
\]

If the GARCH output is already a volatility coefficient per \(\sqrt{\text{second}}\), simply use \(v*{L,\log}=\sigma*{L,\log}^2\); do not divide by the horizon again.

If the model can produce multi-step forecasts, the preferred remaining log variance is the sum of forecast one-second conditional variances:

\[
V*{L,\log}(t,T)
=
\sum*{u=t+1}^{T}v\_{\log,u\mid t}.
\]

This respects GARCH mean reversion. Using \(v\_{L,\log}H_t\) is the constant-rate approximation.

### Short-term one-minute realised volatility

Using one-second log mid-price returns

\[
r*i=\log\!\left(\frac{M_i}{M*{i-1}}\right),
\]

calculate the latest 60-second realised log-variance rate as

\[
v*{S,\log}
=
\frac{1}{60}\sum*{i=t-59}^{t}r_i^2,
\]

with

\[
\sigma*{S,\log}=\sqrt{v*{S,\log}}.
\]

If a reported one-minute realised volatility is \(\sqrt{\sum r_i^2}\), divide it by \(\sqrt{60}\), not by 60, to obtain a per-\(\sqrt{\text{second}}\) coefficient. Prefer mid-prices or microprices to last trades and validate the sampling interval for microstructure noise.

### Combining the long- and short-term estimates

> **Implementation extension.** The original AS approximation assumes constant \(\sigma\). The following rules treat it as a locally recalculated strategy and are not derived as the optimum of a stochastic-volatility AS problem.

The simplest conservative rule is

\[
v*{\mathrm{eff},\log}
=
\max(v*{L,\log},v\_{S,\log}),
\]

\[
V*{\log}(t,T)
=
v*{\mathrm{eff},\log}H_t.
\]

This applies a one-minute volatility shock to the entire remaining horizon and can therefore over-widen quotes when \(H_t\) is long.

A more natural finite-horizon overlay lets the short-term excess variance mean-revert toward the GARCH baseline. Let \(h\_{1/2}\) be the chosen shock half-life in seconds,

\[
\beta=\frac{\log 2}{h*{1/2}},
\qquad
e_t=\max(0,v*{S,\log}-v\_{L,\log}),
\]

and choose \(0\leq w\leq1\). Then

\[
V*{\log}(t,T)
=
v*{L,\log}H_t
+w e_t\frac{1-e^{-\beta H_t}}{\beta}.
\]

If multi-step GARCH forecasts are available, replace \(v*{L,\log}H_t\) by their summed forecast variance. Select \(w\) and \(h*{1/2}\) with walk-forward testing rather than fitting them on final evaluation data.

These overlay parameters should also be tied to an operational decision:

- \(v_{L,\log}\) tells us the baseline conditional variance rate, in the context of the slower GARCH state, so that we can prevent a temporarily quiet one-minute window from erasing the longer-run risk estimate.
- \(v_{S,\log}\) tells us the latest realised variance rate, in the context of fast mid-price movement over the recent window, so that we can detect a volatility shock quickly.
- \(w\) tells us how much of the short-term excess variance enters the forecast, in the context of the practical volatility overlay rather than the original constant-\(\sigma\) model, so that we can balance shock responsiveness against quote instability.
- \(h_{1/2}\), equivalently \(\beta=\log(2)/h_{1/2}\), tells us how quickly that excess variance is expected to decay, in the context of the remaining horizon, so that we can avoid applying a brief shock at full strength all the way to \(T\).

The AS price process uses absolute rather than log volatility. For log-return inputs, use the local conversion

\[
\boxed{
V*t\approx S_t^2V*{\log}(t,T)
}.
\]

This treats the price level as approximately constant over the risk horizon. If the GARCH and realised-volatility models already use absolute price changes, sum their absolute variance forecasts directly and do not multiply by \(S_t^2\).

---

## 3. Using `tick_size` correctly

Let

\[
\eta=\texttt{tick_size}.
\]

The paper uses continuous prices. Tick rounding is an execution-layer extension and should be represented in the backtest.

The tick size \(\eta\) tells us the venue's minimum executable price increment, in the context of mapping continuous model prices onto the actual order grid, so that we can round final bids outward, final asks outward, and measure when a sub-tick change is too small to justify cancel/replace traffic.

### Method A — calculate in price units, then quantise

Calculate

\[
c=\frac{1}{\gamma\Delta}
\log\!\left(1+\frac{\gamma\Delta}{k}\right),
\]

\[
j=\gamma V_t,
\qquad
h=c+\frac{\Delta}{2}j,
\]

\[
P_b^{\mathrm{cont}}=S_t-(h+jq_t),
\]

\[
P_a^{\mathrm{cont}}=S_t+(h-jq_t).
\]

For conservative passive rounding,

\[
n_b=\left\lfloor\frac{P_b^{\mathrm{cont}}}{\eta}\right\rfloor,
\qquad
n_a=\left\lceil\frac{P_a^{\mathrm{cont}}}{\eta}\right\rceil,
\]

\[
P_b=n_b\eta,
\qquad
P_a=n_a\eta.
\]

Round the final prices, not the reservation price, half-spread and skew independently. This avoids unintended asymmetry when \(S_t\) lies between ticks.

If the price grid has a non-zero origin \(P_0\), apply `floor` and `ceil` to \((P-P_0)/\eta\) and reconstruct \(P=P_0+n\eta\). Always use the venue's instrument metadata rather than assuming the grid begins at zero.

### Method B — calculate entirely in ticks

Define

\[
\widetilde S_t=\frac{S_t}{\eta},
\qquad
\widetilde k=k\eta,
\qquad
\widetilde\gamma=\gamma\eta,
\qquad
\widetilde V_t=\frac{V_t}{\eta^2}.
\]

Here \(\widetilde V*t\) is integrated variance measured in ticks squared. The transformation assumes the spot-style P&L convention used by the equations. For a futures contract, first absorb the contract multiplier into the effective price-risk parameter; equivalently, if \(m\) is cash P&L per one-unit price move per contract, use \(\gamma*{\mathrm{price}}=m\gamma\_{\mathrm{wealth}}\).

If intensity is fitted directly against integer depth \(n\),

\[
\lambda(n)=A e^{-\kappa n},
\]

then \(\kappa=\widetilde k\). Do not multiply an already tick-based \(\kappa\) by `tick_size` again.

Calculate

\[
\widetilde c
=
\frac{1}{\widetilde\gamma\Delta}
\log\!\left(1+\frac{\widetilde\gamma\Delta}{\widetilde k}\right),
\]

\[
\widetilde j=\widetilde\gamma\widetilde V_t,
\qquad
\widetilde h=\widetilde c+\frac{\Delta}{2}\widetilde j,
\]

\[
\widetilde r_t=\widetilde S_t-q_t\widetilde j,
\]

\[
n_b=\left\lfloor\widetilde r_t-\widetilde h\right\rfloor,
\qquad
n_a=\left\lceil\widetilde r_t+\widetilde h\right\rceil.
\]

Finally,

\[
P_b=n_b\eta,
\qquad
P_a=n_a\eta.
\]

Use `log1p` for the logarithm. When \(\gamma\Delta/\widetilde k\) is numerically close to zero, use the limit \(\widetilde c=1/\widetilde k\).

### Post-only and top-of-book constraints

For post-only quotes that may improve the current best prices but must not cross,

```text
model_bid_tick = floor(reservation_tick - half_spread_tick)
model_ask_tick = ceil(reservation_tick + half_spread_tick)

bid_tick = min(model_bid_tick, best_ask_tick - 1)
ask_tick = max(model_ask_tick, best_bid_tick + 1)
```

For a join-only policy,

```text
bid_tick = min(model_bid_tick, best_bid_tick)
ask_tick = max(model_ask_tick, best_ask_tick)
```

Then enforce `ask_tick >= bid_tick + 1` and all venue price, quantity, notional and price-band filters. Use integer tick indices or decimal/fixed-point arithmetic rather than binary floating-point modulo operations.

If \(h-jq<0\) or \(h+jq<0\), the AS inventory adjustment is asking for a quote through the mid-price. A passive implementation should preserve the inventory-reducing intent with the post-only clamp and should combine it with hard inventory limits or one-sided quoting.

---

## 4. Recommended calculation sequence

1. Read `tick_size`, top-of-book prices, the price-grid origin and all quantity/notional filters.
2. Choose \(T\) and calculate \(H_t=T-t\) in seconds. For continuous markets, explicitly document whether this is a rolling risk horizon or a countdown session.
3. Express \(q*t\) and \(\Delta\) in the same inventory units. The original model targets zero inventory; a non-zero target is an extension implemented by replacing \(q_t\) with \(q_t-q*{\mathrm{target}}\).
4. Estimate \(A\) in fills per second and \(k\) either per price unit or per tick for orders comparable to size \(\Delta\).
5. Convert the GARCH and one-minute realised volatility estimates to log-variance rates per second.
6. Forecast or approximate the integrated log variance over \(H_t\), then convert it to absolute integrated variance \(V_t\).
7. Calculate \(c,j,h,r_t\) in either price space or tick space.
8. Calculate continuous bid and ask prices, then quantise the final prices outward.
9. Apply post-only/top-of-book constraints and validate all exchange filters.
10. Enforce inventory limits, stale-data guards and volatility/parameter sanity checks before placing orders.

### Compact tick-space pseudocode

```text
# Time is in seconds; prices are represented as integer ticks.
# V_tick is forecast integrated variance over the remaining horizon,
# measured in ticks^2.

inventory = position - target_position
gamma_tick = gamma_price * tick_size

if gamma_tick > small_number:
    static_half_spread_tick = (
        log1p(gamma_tick * order_size / k_tick)
        / (gamma_tick * order_size)
    )
else:
    static_half_spread_tick = 1 / k_tick

skew_per_inventory_tick = gamma_tick * V_tick
half_spread_tick = (
    static_half_spread_tick
    + 0.5 * order_size * skew_per_inventory_tick
)

reservation_tick = fair_price / tick_size - inventory * skew_per_inventory_tick

model_bid_tick = floor(reservation_tick - half_spread_tick)
model_ask_tick = ceil(reservation_tick + half_spread_tick)

bid_tick = min(model_bid_tick, best_ask_tick - 1)
ask_tick = max(model_ask_tick, best_bid_tick + 1)

if ask_tick <= bid_tick:
    preserve the inventory-reducing side and move the other side outward

bid_price = bid_tick * tick_size
ask_price = ask_tick * tick_size
```

---

## 5. Practical checks before live use

- Confirm that \(H_t\) is in seconds when variance rates are per second. A seconds/minutes mismatch scales the entire AS inventory-risk term.
- Confirm whether volatility is a log-return coefficient or an absolute price coefficient. The AS diffusion uses absolute price volatility.
- Confirm whether \(V_t\) is a variance **rate** or an **integrated variance**. The quote equations require the latter; do not multiply an already integrated variance by \(H_t\) again.
- Confirm whether \(k\) was fitted against price distance or tick distance.
- Treat the absence of \(A\) from the approximate quote equation carefully. Backtests and fill simulations must still use calibrated arrival rates, queue effects and order size.
- Recalculate the quote when inventory, fair price, remaining horizon, volatility forecast or liquidity parameters change, but use sub-tick hysteresis to avoid excessive cancel/replace traffic.
- Backtest with discrete prices, post-only rejects, queue priority, latency, partial fills, fees/rebates, position limits and adverse selection.
- Monitor reservation-price displacement, spread in ticks, negative model depths, fills by depth, inventory tails and terminal inventory.
- Use walk-forward calibration for \(A,k,\gamma\), the horizon policy and the short-volatility overlay.

---

## 6. Comparison with the Guéant approximation

Using the common \(\Delta\)-sized notation, both models can be written as

\[
\delta^{b*}=h+jq,
\qquad
\delta^{a*}=h-jq.
\]

Their main differences are:

| Feature                        |                                               Classic AS approximation |                           Guéant exponential-intensity approximation |
| ------------------------------ | ---------------------------------------------------------------------: | -------------------------------------------------------------------: |
| Time structure                 |                                         Finite horizon through \(T-t\) |                                         Stationary/far-from-terminal |
| Static half-spread term        |               \(\frac{1}{\gamma\Delta}\log(1+\frac{\gamma\Delta}{k})\) |     \(\frac{1}{\xi\Delta}\log(1+\frac{\xi\Delta}{k})\) for \(\xi>0\) |
| Skew per inventory unit        | \(j=\gamma V_t\), with \(V_t=\sigma^2(T-t)\) under constant volatility |                                                     \(j=\sigma c_2\) |
| Half-spread                    |                                     \(h=c+\frac{\Delta}{2}\gamma V_t\) |                                 \(h=c_1+\frac{\Delta}{2}\sigma c_2\) |
| Direct dependence on \(A\)     |                       Cancels from the first-order quote approximation |                                                   Present in \(c_2\) |
| Behaviour near a terminal time |                              Risk spread and skew shrink as \(t\to T\) |                                                No terminal countdown |
| Natural volatility input       |                Forecast integrated variance over the remaining horizon | Local volatility coefficient consistent with the intensity time unit |

For Model A in the Guéant paper, \(\xi=\gamma\), so the two models share the same static liquidity term. Their risk terms differ because AS uses an explicit remaining horizon, whereas Guéant uses a stationary balance involving volatility and execution intensity.

---

## Sources

1. Marco Avellaneda and Sasha Stoikov, [“High-frequency trading in a limit order book” (PDF)](https://people.orie.cornell.edu/sfs33/LimitOrderBook.pdf), especially Sections 2.1–2.5 and equations (2.6), (2.7), (3.17) and (3.18).
2. Hummingbot Foundation, [“Guide to the Avellaneda & Stoikov Strategy”](https://hummingbot.org/blog/guide-to-the-avellaneda--stoikov-strategy/), for a practical explanation of the reservation price, optimal spread and synthetic trading-session horizon used in continuous crypto markets.
3. Olivier Guéant, [“Optimal market making” (PDF)](https://arxiv.org/pdf/1605.01862), for the stationary/far-from-terminal Guéant approximation used in the companion note and the Model A mapping \(\xi=\gamma\).
4. Torben G. Andersen and Luca Benzoni, [“Realized Volatility”](https://www.chicagofed.org/-/media/publications/working-papers/2008/wp2008-14-pdf.pdf), for realised variance as the sum of squared high-frequency returns and the effects of microstructure noise.
5. Binance Developer Documentation, [“Filters — PRICE_FILTER”](https://developers.binance.com/en/docs/products/spot/filters#price_filter), for an example of an exchange price-grid rule based on `tickSize`.
