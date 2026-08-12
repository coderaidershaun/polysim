# Chapter 4

# Nonlinear Hawkes Processes

## Mathematical Foundation

Nonlinear Hawkes processes extend traditional linear formulations by allowing the intensity function to incorporate nonlinear transformations of historical events. This approach enhances flexibility in capturing complex dependencies typical in economic and financial applications.

The intensity function in a nonlinear Hawkes process can be expressed as:

\[
\lambda(t)=\mu+\sum\_{t_i<t}\phi\left(\alpha_i e^{-\beta(t-t_i)}\right)
\]

where \(\mu\) represents the baseline intensity, and the function \(\phi(\cdot)\) introduces nonlinearity into the process. The choice of \(\phi(\cdot)\) shapes how past events modulate current intensity levels beyond simple linear aggregation.

## What the Parameters Mean in Market Data

The interpretation starts with the definition of an **event** and the unit of time. An event might be any trade, a buyer-initiated market order, a seller-initiated market order, a cancellation at the best bid, or a mid-price move. These choices produce different models and different trading signals. If time is measured in seconds and an event is a trade, then both \(\lambda(t)\) and \(\mu\) are measured in trades per second.

More formally, the conditional intensity is the expected event-arrival rate given all information immediately before \(t\):

\[
\lambda(t)
=
\lim_{\Delta t\downarrow0}
\frac{
\mathbb{E}\!\left[N(t+\Delta t)-N(t)\mid\mathcal{F}_{t^-}\right]
}{\Delta t}.
\]

Consequently, over a short interval \(\Delta t\):

\[
\mathbb{E}[\text{number of arrivals in }(t,t+\Delta t]
\mid\mathcal{F}_{t^-}]
\approx \lambda(t)\Delta t.
\]

If the intensity is roughly constant over that interval, the probability of at least one arrival is approximately \(1-e^{-\lambda(t)\Delta t}\), and the expected waiting time to the next event is approximately \(1/\lambda(t)\). Thus, \(\lambda(t)=5\) trades per second means roughly 0.5 expected trades over the next 100 ms and a typical wait of about 200 ms if conditions do not change. Intensity is a rate, not a probability, and it is not trade volume unless volume is explicitly incorporated as an event mark.

The parameters in the exponential specification should be read as decision inputs:

| Parameter | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(\lambda(t)\), the current conditional intensity | The live event-arrival rate, including quiet flow and excitation from recent events | The chosen event type and time unit; for example, buyer-initiated trades per second | Convert the rate into a near-term event count, arrival probability, or waiting time and adjust quote size, placement, or refresh urgency |
| \(\mu\), the baseline intensity | How much flow is expected when no recent event is still influential; the baseline wait is approximately \(1/\mu\) | Quiet or exogenous flow for the same event definition, rather than the unconditional sample-average rate | Establish a normal-flow benchmark and distinguish routine activity from a self-excited burst |
| \(\alpha_i\), the excitation amplitude | How strongly event \(i\) changes the arrival rate immediately after it occurs | Clustering after a trade, cancellation, or price move; \(\alpha_i\) may also depend on trade size or aggressor side | Identify which events warrant an immediate defensive or opportunistic quote response |
| \(\beta\), the exponential decay rate | How quickly that event's influence disappears; memory is \(1/\beta\) and half-life is \(\log(2)/\beta\) | The duration of an order-flow burst, with \(\beta\) measured in inverse time | Choose how long to retain a quote adjustment before returning toward the baseline regime |
| \(\phi(\cdot)\), the response function | Whether event effects remain proportional, become amplified, saturate, or activate around a threshold | The way recent events are translated into a new arrival rate | Decide whether quote responses should grow linearly with activity or become more conservative in crowded or extreme regimes |
| \(\gamma\) in \(\phi(x)=\gamma x^2\) | The strength of quadratic amplification; one event contributes \(\gamma\alpha_i^2e^{-2\beta(t-t_i)}\) | A model in which large excitation inputs receive disproportionate weight | Quantify whether large trades or intense bursts justify a much stronger response than small events |
| \(\theta\), the logistic steepness | How abruptly the response switches around its threshold | A saturating or threshold response; \(\theta\) has inverse-input units | Choose whether the model reacts gradually or almost like a regime switch |
| \(\delta\), the logistic inflection point | The excitation-input level at which the response changes most rapidly | The activity threshold for the logistic response; \(\delta\) has the same units as the input to \(\phi\) | Identify when normal flow has become sufficiently unusual to trigger the strongest quote adjustment |

In the linear benchmark, \(\alpha\) has units of events per unit time. With a nonlinear \(\phi\), its units must be read together with that function. Under the equation above, \(\phi\)'s output must have units of events per unit time; if \(x\) has rate units in the quadratic example, \(\gamma\) has inverse-rate units.

All parameter values depend on the chosen time unit. Changing from seconds to milliseconds changes the numerical values of rate and decay parameters, even though the economic behaviour is the same.

### Endogenous Flow and the Branching Ratio

The linear exponential model is a useful benchmark for interpreting fitted parameters. If \(\phi(x)=x\), \(\alpha_i=\alpha\), and

\[
h(u)=\alpha e^{-\beta u},\qquad u>0,
\]

then the branching ratio is

\[
n=\int_0^\infty h(u)\,du=\frac{\alpha}{\beta}.
\]

Here, \(n\) is the expected number of direct descendant events triggered by one event. For a stationary linear model, \(n<1\), the long-run mean arrival rate is

\[
\bar\lambda=\frac{\mu}{1-n},
\]

and the fraction of flow attributed by the model to endogenous excitation is \(n\). Values close to one indicate highly clustered, persistent flow and make estimates more sensitive to model error. Some software instead defines \(h(u)=\alpha\beta e^{-\beta u}\); under that convention the branching ratio is \(\alpha\), not \(\alpha/\beta\). The kernel definition must therefore be checked before comparing estimates.

A general nonlinear model need not have these closed-form interpretations. Its effective amplification and stability should be assessed from the chosen \(\phi\), fitted marginal effects, and simulation. In particular, any per-event transformed contribution should tend to zero as the event becomes old. The uncentred logistic function shown later has \(\phi(0)>0\), so it should normally be centred, multiplied by a decaying kernel, or used as a link on the aggregate predictor; otherwise every historical event leaves a permanent positive contribution.

### Worked Market-Making Example

Suppose events are market orders, time is measured in seconds, and a fitted linear benchmark gives

\[
\mu=2,\qquad \alpha=1.5,\qquad \beta=4.
\]

This gives the market maker the following decision-relevant information:

- \(\mu=2\) tells us that quiet-flow arrivals run at 2 orders per second, in the context of market orders during the fitted regime, so that we can use a baseline wait of about 0.5 seconds when no excitation is active.
- \(\alpha=1.5\) tells us that one isolated market order initially adds 1.5 orders per second, in the context of the linear exponential kernel, so that we can raise the immediate arrival forecast from 2 to 3.5 orders per second.
- \(\beta=4\) tells us that the added flow has a half-life of \(\log(2)/4\approx0.173\) seconds, in the context of a post-trade burst, so that we can treat the response as strong but short-lived rather than hold a defensive quote adjustment indefinitely.
- \(n=\alpha/\beta=0.375\) tells us that the model attributes about 37.5% of stationary flow to self-excitation, in the context of the stationary linear benchmark, so that we can distinguish clustered flow from the quiet baseline. The corresponding long-run mean rate is \(2/(1-0.375)=3.2\) orders per second.
- If no further order has yet arrived, the cumulative intensity over the next 100 ms is

\[
\int_0^{0.1}\left(2+1.5e^{-4s}\right)ds
\approx0.324.
\]

The conditional probability of at least one market order in that interval is therefore \(1-e^{-0.324}\approx27.7\%\). This is more informative for a quoting decision than treating \(\mu\) or \(\alpha\) alone as a forecast.

### How a Market Maker Can Use the Estimates

Parameter estimates are inputs to a quoting and risk model rather than trading instructions on their own:

- \(\lambda(t)\) tells us how quickly events are expected to arrive, in the context of the specific stream being modeled, so that we can estimate fill opportunity, inventory accumulation, or cancellation risk over the quote's intended lifetime.
- \(\alpha\), \(\beta\), and \(\phi\) tell us the strength, duration, and shape of a burst, in the context of activity following recent events, so that we can choose quote size, refresh speed, and how long a defensive adjustment remains active.
- The branching ratio tells us how much flow the linear model attributes to recent market activity, in the context of the fitted regime, so that we can reduce reliance on the quiet-flow baseline during a strongly endogenous cluster.
- Separate \(\lambda_{\mathrm{buy}}(t)\) and \(\lambda_{\mathrm{sell}}(t)\) estimates tell us whether predicted activity is directionally imbalanced, in the context of aggressor-side trade arrivals, so that we can manage the execution and adverse-selection risk of bid and ask quotes separately. This signal should be validated against subsequent price moves.
- Cross-kernel parameters tell us which event types trigger other event types, in the context of a multivariate model of trades, cancellations, limit orders, and price moves, so that we can anticipate liquidity interactions that an all-trades intensity would hide.

For event types \(a\) and \(b\), a linear multivariate model can be written as

\[
\lambda_a(t)
=
\mu_a+
\sum_b\sum_{t_j^{(b)}<t}
\alpha_{ab}e^{-\beta_{ab}(t-t_j^{(b)})}.
\]

Here, \(\alpha_{ab}\) measures how an event of source type \(b\) changes the arrival rate of target type \(a\), while \(\beta_{ab}\) measures how quickly that cross-effect disappears. In the linear case, the matrix with entries \(\alpha_{ab}/\beta_{ab}\) summarizes expected offspring across event types; stationarity requires its spectral radius to be below one.

Interpretation also requires basic model checks. Intraday seasonality should be included in \(\mu(t)\), otherwise the open, close, and scheduled announcements may be mistaken for self-excitation. Trade direction and timestamps must be classified consistently, parameters should be estimated on rolling windows when regimes change, and confidence intervals or out-of-sample calibration should accompany point estimates. Hawkes parameters describe conditional association; they do not by themselves establish that one event economically causes another.

## Nonlinear Transformations in Economic Contexts

In economic systems, nonlinear Hawkes processes accommodate phenomena such as diminishing returns or threshold effects, which are prevalent in high-frequency trading dynamics and information-driven markets. The transformation \(\phi(\cdot)\) can take various forms, such as quadratic or logistic functions, capturing different economic realities.

Consider a quadratic nonlinearity:

\[
\phi(x)=\gamma x^2
\]

where \(\gamma\) is a scaling factor dictating the curvature of the relationship. This transformation implies that event magnitudes have a squared contribution to intensity, emphasizing large effects disproportionately.

## Parameter Estimation for Nonlinear Models

Parameter estimation in nonlinear Hawkes models necessitates adaptation of traditional techniques to account for the nonlinear \(\phi(\cdot)\) function, typically requiring iterative optimization methods.

An example algorithm is formulated using maximum likelihood estimation:

**Data:** Event times \(\{t_1,t_2,\ldots,t_n\}\)  
**Result:** Estimated parameters \(\mu,\alpha,\beta,\gamma\)

Initialize parameters;

**while not converged do**

 Evaluate likelihood function \(L(\mu,\alpha,\beta,\gamma)\) using:

\[
L=\sum_i\log\lambda(t_i)-\int_0^T\lambda(t)\,dt
\]

 Update parameters using optimization technique;

Return optimized parameter set;

## Application in High-Frequency Trading

Nonlinear Hawkes processes provide a robust framework for modeling intricate patterns of transactional activity. The ability to incorporate nonlinearities allows for a refined view of market dynamics, capturing bursts of activity and sudden shifts in liquidity.

When the nonlinearity \(\phi(x)\) is defined as a logistic function:

\[
\phi(x)=\frac{1}{1+e^{-\theta(x-\delta)}}
\]

where \(\theta\) and \(\delta\) regulate the steepness and the inflection point, respectively, the process adeptly models saturation effects, where the impact of events levels off beyond a threshold value.

## Simulation of Nonlinear Hawkes Models

Simulating nonlinear Hawkes processes involves incorporating the nonlinear effects into the generation of events, adjusting intensity calculations accordingly.

**Data:** Baseline intensity \(\mu\), nonlinearity function \(\phi\), end time \(T\)  
**Result:** Simulated event times

Set time \(t=0\) and initialize events list;

**while \(t<T\) do**

 Compute current intensity \(\lambda(t)\) incorporating nonlinearity;

 Generate next event time using stochastic simulation;

 Append event time to events;

Return events list;

Incorporating nonlinearity effectively emulates the real-world complexities of economic systems, offering a powerful tool for both theoretical exploration and practical application in sophisticated trading environments. By utilizing these enhancements, analysts capture nuanced interactions often present in financial transaction data.

## Python Code Snippet

Below is a Python code snippet that demonstrates the core computation involved in modeling nonlinear Hawkes processes, including intensity function definition, parameter estimation via maximum likelihood, and simulation of event sequences.

```python
import numpy as np
from scipy.optimize import minimize

def intensity_function(mu, alpha, beta, events, t, phi,
                       method='quadratic'):
    '''
    Calculate the intensity function for a nonlinear Hawkes process at
    time t.
    :param mu: Baseline intensity.
    :param alpha: Excitation parameter.
    :param beta: Decay rate.
    :param events: List of historical event times.
    :param t: Current time.
    :param phi: Nonlinear transformation.
    :param method: Nonlinearity applied ('quadratic' or 'logistic').
    :return: Intensity value.
    '''
    intensity = mu
    for ti in events:
        if ti < t:
            if method == 'quadratic':
                x = alpha * np.exp(-beta * (t - ti))
                intensity += phi(x, method=method)
            elif method == 'logistic':
                x = alpha * np.exp(-beta * (t - ti))
                intensity += phi(x, method=method)
    return intensity

def phi_function(x, gamma=None, theta=None, delta=None,
                 method='quadratic'):
    '''
    Apply nonlinearity transformation to intensity contribution.
    :param x: Value to transform.
    :param gamma: Parameter for quadratic transformation.
    :param theta: Steepness for logistic transformation.
    :param delta: Inflection point for logistic transformation.
    :param method: Nonlinearity applied ('quadratic' or 'logistic').
    :return: Transformed value.
    '''
    if method == 'quadratic':
        return gamma * x**2
    elif method == 'logistic':
        return 1 / (1 + np.exp(-theta * (x - delta)))
    raise ValueError("Unknown method for nonlinearity")

def log_likelihood(params, events, T):
    '''
    Evaluate the log likelihood for given Hawkes process parameters.
    :param params: Parameter tuple (mu, alpha, beta, gamma).
    :param events: List of event times.
    :param T: Observation window end time.
    :return: Negative log likelihood.
    '''
    mu, alpha, beta, gamma = params
    ll = 0
    for t in events:
        lambda_t = intensity_function(
            mu, alpha, beta, events, t,
            lambda x: phi_function(x, gamma=gamma)
        )
        ll += np.log(lambda_t)

    ll -= np.trapz(
        [
            intensity_function(
                mu, alpha, beta, events, t,
                lambda x: phi_function(x, gamma=gamma)
            )
            for t in np.linspace(0, T, 1000)
        ],
        dx=T/1000
    )

    return -ll

def maximum_likelihood_estimation(events, T):
    '''
    Perform maximum likelihood estimation for Hawkes process
    parameters.
    :param events: List of event times.
    :param T: Observation window end time.
    :return: Estimated parameters.
    '''
    initial_guess = [0.1, 0.1, 0.1, 0.1]
    bounds = [(0, None), (0, None), (0, None), (None, None)]
    result = minimize(
        log_likelihood,
        initial_guess,
        args=(events, T),
        bounds=bounds
    )
    return result.x if result.success else None

def simulate_hawkes(T, mu, alpha, beta, phi, method='quadratic'):
    '''
    Simulate event times for a nonlinear Hawkes process.
    :param T: End time for simulation.
    :param mu: Baseline intensity.
    :param alpha: Excitation parameter.
    :param beta: Decay rate.
    :param phi: Nonlinear transformation function.
    :param method: Nonlinearity method.
    :return: Simulated event time list.
    '''
    events = []
    t = 0
    while t < T:
        lambda_t = intensity_function(
            mu, alpha, beta, events, t,
            phi, method
        )
        t += np.random.exponential(1.0 / lambda_t)
        if t < T:
            events.append(t)
    return events

# Example parameters for Monte Carlo simulation
mu, alpha, beta, gamma = 0.1, 0.2, 1.0, 0.5
phi = lambda x: phi_function(x, gamma=gamma)

# Simulate and estimate parameters
simulated_events = simulate_hawkes(
    10, mu, alpha, beta, phi, 'quadratic'
)
estimated_params = maximum_likelihood_estimation(
    simulated_events, 10
)

print("Simulated Events:", simulated_events)
print("Estimated Parameters:", estimated_params)
```

This code defines the core functional components for handling nonlinear Hawkes processes:

- `intensity_function`: Computes the intensity function given historical events and nonlinearity.
- `phi_function`: Applies a nonlinear transformation to the contributions of past events.
- `log_likelihood`: Constructs the log likelihood function for evaluating parameter fit.
- `maximum_likelihood_estimation`: Optimizes parameters of the Hawkes model based on event data.
- `simulate_hawkes`: Generates synthetic event sequences according to the model dynamics.

The example illustrates the simulation of event sequences and the subsequent parameter estimation for nonlinearly enhanced Hawkes processes.

# Chapter 5

# Discrete-Time Hawkes Processes

## Introduction to Discrete-Time Modeling

Discrete-time Hawkes processes serve as a powerful framework for modeling event occurrences within fixed time intervals. Unlike their continuous counterparts, these models are structured around a sequence of fixed-length periods, leading to a formulation that is particularly suitable for empirical applications where data is aggregated over specific time frames. The discrete setup involves an intensity process that dictates the probability of events within each time slot.

## Mathematical Framework

Let the sequence of intervals be denoted by \(t_1,t_2,\ldots,t_n\), where each \(t_i\) corresponds to a discrete time step. The counting process, \(N(t_i)\), registers the number of events in the interval ending at \(t_i\). The core of discrete-time Hawkes models lies in the conditional intensity function, which is modified for discrete time:

\[
\lambda(t_i)=\mu+\sum_{j=1}^{i-1}\alpha(t_i-t_j)N(t_j),
\tag{5.1}
\]

where \(\mu\) is the baseline rate, and the excitation function \(\alpha(\cdot)\) describes the influence of past events on future event likelihoods. Within discrete-time settings, \(\alpha\) often takes a form that reflects the decaying influence over time, such as:

\[
\alpha(t_i-t_j)=
\begin{cases}
\beta^{(t_i-t_j)}, & \text{if } t_i-t_j\leq m,\\
0, & \text{otherwise}
\end{cases}
\tag{5.2}
\]

Here, \(\beta\) is a decay factor, and \(m\) is the memory length that specifies the influence range of past events.

### Interpreting the Discrete-Time Parameters

Let each bin have width \(\Delta\), such as 100 ms, and let an event be a trade. Then \(N(t_i)\) is the observed number of trades in bin \(i\), while \(\lambda(t_i)\) is the conditional expected number of trades in that bin. The comparable continuous-time arrival rate is approximately \(\lambda(t_i)/\Delta\). For example, \(\lambda(t_i)=0.4\) trades per 100 ms corresponds to an average rate of 4 trades per second during that bin.

| Parameter | Tells us | In the context of | So that we can |
| --- | --- | --- | --- |
| \(\mu\) | The baseline expected event count per bin; \(\mu/\Delta\) is the baseline rate per unit time | Bins with no recent excitation | Establish the normal fill-flow benchmark against which a burst is measured |
| \(\alpha(k)\) | How much an event observed \(k\) bins ago changes the current expected count | The complete lagged flow-response shape | Translate recent trades into a near-term count forecast |
| \(\beta\) | How much excitation is retained from one bin to the next; the half-life is \(\log(0.5)/\log(\beta)\) bins | The geometric kernel \(\alpha(k)=\beta^k\), normally with \(0\leq\beta<1\) | Decide how long a discrete-time quote or risk response should persist |
| \(m\) | The hard memory cutoff beyond which events have no modeled effect | Bins of width \(\Delta\), giving a calendar memory of \(m\Delta\) | Bound the history used for both forecasts and quote adjustments |
| \(\gamma\) in Equation (5.3) | How much \(Y\) changes for one additional event in a bin | Mapping predicted counts into an economic quantity such as volume or price impact | Convert an arrival forecast into the downstream quantity relevant to the strategy |
| \(\epsilon_i\) | How much variation in \(Y(t_i)\) remains unexplained by event counts | Model validation rather than Hawkes excitation | Measure whether the arrival model omits too much information for the intended decision |

In the simplified kernel in Equation (5.2), \(\beta\) controls both the next-bin weight and the persistence because there is no separate excitation-amplitude parameter. A more flexible production model commonly uses \(\alpha(k)=a\beta^k\), where \(a\) controls the initial response and \(\beta\) controls its persistence. Keeping those roles separate makes comparisons across instruments and regimes easier.

Bin width is part of the model specification. With bins that are too wide, fast clustering is hidden inside a single count; with bins that are too narrow, most observations are zero and estimates can become noisy. A market maker should choose \(\Delta\) to match the quote-update or risk horizon and compare the predicted count with realized out-of-sample arrivals.

## Algorithmic Implementation

Discrete-time Hawkes processes can be efficiently implemented using algorithmic techniques that iterate over time intervals, updating the intensity based on past occurrences. A basic representation of the algorithm for simulating events under a discrete-time Hawkes model is given as:

**Data:** Time steps \(T\), baseline \(\mu\), decay factor \(\beta\), memory \(m\)  
**Result:** Sequence of event counts events

Initialize events list as empty;

**for \(t_i\) in \(T\) do**

 Compute intensity \(\lambda(t_i)\) using:

\[
\lambda(t_i)=\mu+\sum_{j=1}^{i-1}\beta^{(t_i-t_j)}N(t_j)
\]

 Generate number of events \(N(t_i)\) using Poisson distribution with rate \(\lambda(t_i)\);

 Append \(N(t_i)\) to events

Return events;

The algorithm utilizes a Poisson distribution to model the probability of occurrences based on the intensity function at each step. It leverages the decaying memory kernel for simpler computational requirements while maintaining the capability to capture temporal dependencies.

## Applications in Economic Models

In economic and financial contexts, discrete-time Hawkes processes are well-suited for analyzing transaction counts or order arrivals over predefined trading intervals. Particularly, they capture the episodic nature of financial markets where volumes and transactions typically surge around specific times, such as market openings or news releases.

Let \(Y(t_i)\) represent the observable financial variable, modeled as:

\[
Y(t_i)=\gamma\cdot N(t_i)+\epsilon_i,
\tag{5.3}
\]

where \(\gamma\) is a scaling factor, and \(\epsilon_i\) denotes an error term capturing unobserved influences. The inherent flexibility of discrete Hawkes models allows for these processes to accommodate various economic phenomena, including autocorrelated trading volumes or clustered market impacts.

## Parameter Estimation Techniques

For parameter estimation within discrete-time Hawkes models, likelihood-based methods are predominantly used. The likelihood \(L\) for a sequence of observed events \(\{N(t_1),\ldots,N(t_n)\}\) over time steps can be expressed as:

\[
L(\mu,\beta)=
\prod\_{i=1}^{n}
\frac{\lambda(t_i)^{N(t_i)}e^{-\lambda(t_i)}}{N(t_i)!}
\tag{5.4}
\]

Optimization algorithms such as Expectation-Maximization (EM) can be adapted to iteratively solve for parameter estimates \(\mu\) and \(\beta\), considering the discretized nature of data.

These elements collectively render discrete-time Hawkes processes a veritable tool for quantifying and modeling temporal dynamics in various applications possessing innate stochastic or irregular behavior. Such models are critical in unraveling the intricacies of high-frequency data’s temporal patterning.

## Python Code Snippet

Below is a Python code snippet that encompasses the core computational elements of discrete-time Hawkes processes, including event simulation, parameter estimation, and economic application modeling.

```python
import numpy as np
from scipy.optimize import minimize

def simulate_discrete_hawkes(T, mu, beta, m):
    '''
    Simulate a sequence of events using a discrete-time Hawkes
    process.
    :param T: Number of time steps.
    :param mu: Baseline rate.
    :param beta: Decay factor.
    :param m: Memory length.
    :return: Sequence of event counts.
    '''
    events = np.zeros(T)
    for t in range(T):
        intensity = mu + np.sum(
            [
                beta**(t - j) * events[j]
                for j in range(t - m, t)
                if j >= 0
            ]
        )
        events[t] = np.random.poisson(intensity)
    return events

def likelihood(events, mu, beta, m):
    '''
    Calculate the log likelihood for a sequence of events given
    Hawkes parameters.
    :param events: Sequence of event counts.
    :param mu: Baseline rate.
    :param beta: Decay factor.
    :param m: Memory length.
    :return: Log likelihood value.
    '''
    T = len(events)
    log_likelihood = 0
    for t in range(T):
        intensity = mu + np.sum(
            [
                beta**(t - j) * events[j]
                for j in range(t - m, t)
                if j >= 0
            ]
        )
        log_likelihood += (
            events[t] * np.log(intensity) - intensity
        )
    return -log_likelihood

def estimate_parameters(events, m):
    '''
    Estimate Hawkes process parameters using maximum likelihood.
    :param events: Sequence of event counts.
    :param m: Memory length.
    :return: Estimated parameters (mu, beta).
```

## Continuation of Chapter 5

```python
    '''
    result = minimize(
        lambda params: likelihood(events, params[0], params[1], m),
        x0=[0.1, 0.5],
        bounds=[(0, None), (0, 1)]
    )
    return result.x

def model_economic_variable(events, gamma):
    '''
    Model an economic variable based on event occurrences.
    :param events: Sequence of event counts.
    :param gamma: Scaling factor.
    :return: Simulated economic variable.
    '''
    T = len(events)
    epsilon = np.random.normal(0, 0.1, T)
    return gamma * events + epsilon

# Example parameters
T = 100
mu = 0.5
beta = 0.8
m = 5
gamma = 1.5

# Simulate events
events = simulate_discrete_hawkes(T, mu, beta, m)

# Parameter estimation
estimated_mu, estimated_beta = estimate_parameters(events, m)

# Model economic variable
economic_var = model_economic_variable(events, gamma)

print("Simulated Events:", events)
print("Estimated mu:", estimated_mu)
print("Estimated beta:", estimated_beta)
print("Modeled Economic Variable:", economic_var)
```

This code defines several key functions necessary for the implementation and exploration of discrete-time Hawkes processes:

- `simulate_discrete_hawkes` simulates event sequences based on given Hawkes process parameters.
- `likelihood` calculates the log likelihood for parameter estimation purposes.
- `estimate_parameters` uses the maximum likelihood estimation method to determine optimal Hawkes parameters.
- `model_economic_variable` generates economic variables by leveraging the event sequence and incorporating random noise for realism.

The final block of code provides examples of generating and analyzing these events and their applications to economic modeling scenarios.

# Chapter 6

# Parameter Estimation via Maximum Likelihood

## Introduction to Maximum Likelihood for Hawkes Processes

In the realm of econometric modeling, estimating parameters accurately is crucial for ensuring the predictive utility and interpretative power of any stochastic process model. The maximum likelihood estimation (MLE) method stands as a cornerstone in this endeavor, providing a statistically sound framework to infer Hawkes process parameters from empirical data.

The likelihood function \(L(\theta)\), where \(\theta\) denotes the vector of parameters, encapsulates the probability of observing the given data under a specified model configuration. For Hawkes processes, this involves the baseline intensity \(\mu\) and any kernel parameters that define the self-exciting nature of the process.

## Log-Likelihood Function Derivation

For a point process like the Hawkes process, the likelihood function is often elaborated in terms of the conditional intensity function \(\lambda(t)\). The likelihood \(L\) for a sequence of observed events \(\{N(t_1),\ldots,N(t_n)\}\) is determined through:

\[
L(\theta)=\prod*{i=1}^{n}f\left(N(t_i)\mid\mathcal{F}*{t_i}\right)
\]

where \(f\left(N(t*i)\mid\mathcal{F}*{t_i}\right)\) is the density function conditioned on the history up to time \(t_i\). The log-likelihood, being more tractable, is expressed as:

\[
\log L(\theta)
=
\sum\_{i=1}^{n}
\left(
N(t_i)\log\lambda(t_i)-\lambda(t_i)
\right)
\]

This transformation is crucial in simplifying the optimization process.

## Parameter Vector and Conditional Intensity

Let’s explicitly define the parameter vector \(\theta=(\mu,\alpha,\beta)\), where \(\alpha\) represents the self-excitation parameter, and \(\beta\) defines the decay rate. The conditional intensity function for the Hawkes process, \(\lambda(t)\), is modeled as:

\[
\lambda(t)
=
\mu+
\sum\_{j:t_j<t}
\alpha\exp\left(-\beta(t-t_j)\right)
\]

In practical applications, discretization may substitute integrals with summations, aligning with the data collection methodology in high-frequency finance.

## Optimization Methods for MLE

Maximizing the log-likelihood function with respect to the parameter vector \(\theta\) poses a non-trivial optimization challenge, often addressed via numerical methods. Gradient ascent and Expectation-Maximization (EM) algorithms are frequently employed. For complex models, the computational efficiency of EM overcomes the limitations of direct optimization.

**Data:** Initial estimates \(\theta^{(0)}\), convergence criterion \(\epsilon\)  
**Result:** Estimated parameters \(\theta^\*\)

Set \(\Delta\theta=\infty\) and iteration counter \(k=0\);

**while \(\Delta\theta>\epsilon\) do**

 Compute the complete data log-likelihood \(Q(\theta\mid\theta^{(k)})\) using the current estimates;

 Update the parameter estimates:

\[
\theta^{(k+1)}
=
\underset{\theta}{\arg\max}\,
Q\left(\theta\mid\theta^{(k)}\right)
\]

 Compute

\[
\Delta\theta
=
\left\|
\theta^{(k+1)}-\theta^{(k)}
\right\|;
\]

 Increment \(k\);

**return** \(\theta^\*=\theta^{(k)}\);

## Practical Implementation Issues

In deploying MLE for Hawkes processes, challenges surface in terms of computational cost and convergence issues, especially when handling extensive datasets typical in high-frequency trading environments. Implementing robust numerical solvers and preconditioning of data are strategies that ameliorate these practical concerns.

Adjustment and bootstrap methods are often used to validate parameter uncertainty, ensuring robust parameter inference even under model misspecifications or data irregularities.

## Econometric Insight into Parameter Estimation

The fitted parameters are useful only when tied back to the event definition and a decision horizon:

- \(\widehat\mu\) tells us the estimated background arrival rate, in the context of the fitted event stream and intraday baseline specification, so that we can benchmark normal activity before recent-event effects are added.
- \(\widehat\alpha\) tells us the estimated immediate excitation strength, in the context of events following one another, so that we can quantify how much a new trade, cancellation, or price move should change the short-horizon forecast.
- \(\widehat\beta\) tells us how quickly that fitted excitation decays, in the context of the same time unit used for timestamps, so that we can match the duration of a risk response to the estimated memory of the flow.

Confidence intervals and out-of-sample calibration tell us how uncertain those effects are, in the context of sampling noise and possible model misspecification, so that we can avoid treating unstable point estimates as precise quote signals.

## Python Code Snippet

Below is a Python code snippet that encompasses the core computational elements for parameter estimation via maximum likelihood for Hawkes processes, including the formulation of the log-likelihood function, optimization algorithms, and practical implementation adjustments.

```python
import numpy as np
from scipy.optimize import minimize

def log_likelihood(params, events, T):
    '''
    Calculate the log-likelihood of a Hawkes process.
    :param params: Array containing [mu, alpha, beta].
    :param events: Observed event times.
    :param T: Time window length.
    :return: Negative log-likelihood value.
    '''
    mu, alpha, beta = params
    n = len(events)
    integral_part = mu * T  # Baseline intensity across time period
    sum_log_part = -n * mu

    for i in range(n):
        sum_exp = 0
        for j in range(i):
            sum_exp += alpha * np.exp(
                -beta * (events[i] - events[j])
            )
        sum_log_part += np.log(mu + sum_exp)

    logL = integral_part - sum_log_part - sum(mu + sum_exp)
    return -logL  # Minimize negative log-likelihood

def fit_hawkes_mle(events, T):
    '''
    Fit Hawkes process to data using MLE.
    :param events: Observed event times.
    :param T: Time window length.
    :return: Estimated parameters [mu, alpha, beta].
    '''
    initial_params = np.array([0.1, 0.1, 0.1])  # Initial guesses
                                                    # for mu, alpha, beta
    bounds = [(1e-5, None), (1e-5, None), (1e-5, None)]
                                                    # Constraints to avoid
                                                    # negative values
    result = minimize(
        fun=log_likelihood,
        x0=initial_params,
        args=(events, T),
        bounds=bounds
    )

    if result.success:
        mu_hat, alpha_hat, beta_hat = result.x
        return mu_hat, alpha_hat, beta_hat
    else:
        raise RuntimeError("MLE optimization failed.")

# Example usage
event_times = np.array([0.2, 0.5, 0.7, 1.2, 1.5, 2.1])
# Example event data
T_total = 3.0  # Total observation time

mu_est, alpha_est, beta_est = fit_hawkes_mle(event_times, T_total)
print("Estimated mu:", mu_est)
print("Estimated alpha:", alpha_est)
print("Estimated beta:", beta_est)
```

This code defines key functions and methodologies to estimate parameters for Hawkes processes using maximum likelihood estimation:

- `log_likelihood` function computes the negative log-likelihood value for a given set of parameters and event data, accounting for the conditional intensity of the Hawkes process.
- `fit_hawkes_mle` optimizes the parameters \(\mu\), \(\alpha\), and \(\beta\) using numeric optimization techniques, specifically leveraging the `scipy.optimize.minimize` function.
- The example at the end demonstrates the parameter estimation process using simulated event data, providing practical application insights.

These tools facilitate the precise estimation of Hawkes process parameters, enhancing econometric insight into high-frequency trading dynamics and supporting strategic decision-making.

# Chapter 8

# Expectation-Maximization (EM) Algorithm

## Introduction to the EM Algorithm

The Expectation-Maximization (EM) algorithm is a fundamental approach widely utilized for maximum likelihood estimation in models involving latent variables. Its application to Hawkes processes is particularly salient due to the complexities associated with observed event data and the underlying unobserved processes.

## Formulation of the EM Algorithm for Hawkes Processes

In the context of Hawkes processes, let \(\{N(t_i)\}\) represent the observed events up to time \(T\), and \(\theta=(\mu,\alpha,\beta)\) denote the parameter vector consisting of the baseline intensity \(\mu\), the self-excitation parameter \(\alpha\), and the decay rate \(\beta\). The goal is to maximize the likelihood function of the observed data.

The complete data likelihood, assuming the latent branching structure, considers not only the observations but also latent variables \(z\_{ij}\) indicating the contribution of past events to the intensity at time \(t_i\).

## 1 E-Step: Expectation

In the E-step, the expected value of the log-likelihood of the complete data is computed, given the observed data and a current estimate of the parameters \(\theta^t\). The expected complete log-likelihood can be expressed as:

\[
\mathbb{E}_{\theta^t}
\left[
\log L_c\left(\theta;\{N(t_i),Z\}\right)
\right]
=
\sum_i
\mathbb{E}_{\theta^t}
\left[
\log\lambda(t_i;\theta)
\right]

- \int_0^T \lambda(t;\theta)\,dt
  \]

Where \(Z=\{z\_{ij}\}\) encompasses the latent branching variables. The expectation is taken with respect to the distribution of the latent data under the current parameter estimates \(\theta^t\).

## 2 M-Step: Maximization

In the M-step, the parameters \(\theta\) are updated by maximizing the expected complete log-likelihood obtained from the E-step. This includes finding:

\[
\theta^{t+1}
=
\underset{\theta}{\arg\max}\,
\mathbb{E}\_{\theta^t}
\left[
\log L_c\left(\theta;\{N(t_i),Z\}\right)
\right]
\]

The maximization yields new estimates for \(\mu,\alpha,\beta\), refined iteratively. This step exploits the decoupling provided by the E-step expectations concerning the dependencies within the Hawkes process.

## Algorithm Implementation

To efficiently implement the EM algorithm for Hawkes processes, precise numerical methods are required. These involve iterative computation, convergence criteria verification, and leveraging computational efficiency techniques specific to the structure of Hawkes models.

**Data:** Initial parameter values \(\theta^0\), tolerance \(\epsilon\)  
**Result:** Estimated parameters \(\hat{\theta}\)

Initialize \(\theta^{(0)}\);

**repeat**

 // E-Step

 Compute \(\mathbb{E}\_{\theta^{(k)}}[Z]\) based on \(\theta^{(k)}\);

 // M-Step

 Update \(\mu^{(k+1)},\alpha^{(k+1)},\beta^{(k+1)}\) by maximizing

\[
\mathbb{E}\_{\theta^{(k)}}[\log L_c];
\]

 Set

\[
\theta^{(k+1)}
=
\left(
\mu^{(k+1)},
\alpha^{(k+1)},
\beta^{(k+1)}
\right);
\]

**until**

\[
\left|
\theta^{(k)}-\theta^{(k-1)}
\right|<\epsilon;
\]

**return** \(\hat{\theta}=\theta^{(k+1)}\);

## 1 Computational Considerations

Computational stability in the EM algorithm for Hawkes processes is enhanced by accurate log-likelihood evaluations and efficient handling of data which is potentially extensive in a high-frequency setting. The choice of initial parameters, convergence thresholds, and iteration limits all significantly impact algorithmic performance.

The successful deployment of the EM algorithm in Hawkes processes underscores its robustness in settings characterized by intricate temporal dependencies and latent feedback loops. This highlights the confluence of theoretical elegance and practical efficacy, inherent in the EM framework.

## Python Code Snippet

Below is a Python code snippet that illustrates the implementation of the Expectation-Maximization (EM) algorithm specifically for parameter estimation in Hawkes processes, focusing on the necessary computations such as the expectation, maximization steps, and algorithm convergence.

```python
import numpy as np

def e_step(N, theta):
    '''
    Computes the E-step of the EM algorithm for Hawkes processes.
    :param N: Observed events times.
```

## Continuation of Chapter 8

```python
    :param theta: Current estimate of parameters (mu, alpha, beta).
    :return: Expected values of latent variables Z.
    '''
    mu, alpha, beta = theta
    Z = np.zeros((len(N), len(N)))  # Latent branching variables

    # Compute expected value of latent branching variables
    for i in range(1, len(N)):
        for j in range(i):
            decay = np.exp(-beta * (N[i] - N[j]))
            Z[i, j] = (alpha * decay) / (
                mu + alpha *
                np.sum(np.exp(-beta * (N[i] - N[:i])))
            )
    return Z

def m_step(N, Z):
    '''
    Computes the M-step of the EM algorithm for Hawkes processes.
    :param N: Observed events times.
    :param Z: Expected values of latent variables Z from E-step.
    :return: Updated parameters (mu, alpha, beta).
    '''
    T = N[-1]
    n_events = len(N)

    # Update mu
    mu = n_events / T

    # Update alpha and beta using maximum likelihood estimation
    alpha_sum = np.sum(Z)
    beta_sum = np.sum(
        Z * np.array([
            N[i] - N[j]
            for i in range(n_events)
            for j in range(i)
        ])
    )

    alpha = alpha_sum / n_events
    beta = alpha_sum / beta_sum

    return mu, alpha, beta

def em_algorithm(N, init_theta, tol=1e-6, max_iter=100):
    '''
    Runs the EM algorithm on a set of event times for Hawkes process
    parameter estimation.
    :param N: Observed event times.
    :param init_theta: Initial parameters (mu, alpha, beta).
    :param tol: Convergence tolerance.
    :param max_iter: Maximum number of iterations.
    :return: Estimated parameters.
    '''
    theta = np.array(init_theta)

    for iteration in range(max_iter):
        # E-step
        Z = e_step(N, theta)

        # M-step
        new_theta = m_step(N, Z)

        # Check convergence
        if np.linalg.norm(new_theta - theta) < tol:
            print(f"Converged after {iteration+1} iterations.")
            break

        theta = new_theta
    else:
        print("Max iterations reached without convergence.")

    return theta

# Example usage with dummy data
N_events = np.array([0.2, 1.0, 1.8, 3.6, 5.0])  # Example event times
init_params = (0.1, 0.5, 1.0)  # Example initial parameter guesses
estimated_params = em_algorithm(N_events, init_params)

print("Estimated Parameters:", estimated_params)
```

This code includes functions necessary for executing the EM algorithm on Hawkes processes:

- `e_step` calculates the expected latent variables \(Z\) given the current parameter estimates and observed event times.
- `m_step` updates the parameters \(\mu,\alpha,\beta\) based on the expected values from the E-step.
- `em_algorithm` manages the iteration between the E-step and M-step, checking for convergence or iteration limits.

The example at the end demonstrates the application of the EM algorithm using synthetic event times, showcasing how the algorithm refines parameter estimates iteratively.

# Chapter 9

# Multivariate Hawkes Processes

## Introduction to Multivariate Hawkes Processes

In the realm of econometrics and finance, the Hawkes process is instrumental in modeling the temporal clustering of events, often characterized by self-exciting properties. Extending this concept to multivariate cases enables the analysis of multiple, interacting event streams. This extension is particularly relevant in financial markets where multiple assets or order flows may exhibit complex interdependencies. The multivariate Hawkes process accounts for these interactions, allowing the construction of intensity functions that reflect the influence of events across different streams.

## Modeling Interacting Event Streams

Consider a system involving \(d\) interacting event processes, denoted as \(\{N^{(k)}(t)\}\_{k=1}^{d}\). The intensity function for each process \(k\) is influenced not only by its own past events but also by events in all other processes. Formally, the intensity function for process \(k\) at time \(t\), \(\lambda^{(k)}(t)\), is expressed as:

\[
\lambda^{(k)}(t)
=
\mu^{(k)}

- \sum*{j=1}^{d}
  \int*{0}^{t}
  \alpha^{(kj)}
  g^{(kj)}(t-s)\,dN^{(j)}(s)
  \]

where \(\mu^{(k)}\) is the background intensity of process \(k\), \(\alpha^{(kj)}\) is the matrix of self- and cross-excitation coefficients, and \(g^{(kj)}(\cdot)\) represents the kernel function describing the decay of influence of past events in process \(j\) on the intensity of process \(k\).

## Joint Intensity and Probability Structure

The joint characterization of multivariate Hawkes processes involves defining a multivariate counting measure

\[
N(t)=\left(N^{(1)}(t),\ldots,N^{(d)}(t)\right).
\]

The associated joint probability structure is specified via the product of the marginal probabilities of counts in each dimension, given by:

\[
\mathbb{P}
\left(
N^{(k)}((t,t+dt])=1\mid\mathcal{F}\_t
\right)
=
\lambda^{(k)}(t)\,dt
\]

where \(\mathcal{F}\_t\) denotes the history up to time \(t\).

## Parameter Estimation

Estimating the parameters \(\{\mu^{(k)},\alpha^{(kj)},\beta^{(kj)}\}\) typically involves maximizing the log-likelihood function over the observed data. For a multivariate Hawkes process, the log-likelihood function \(\log L\) can be expressed as:

\[
\log L
=
\sum*{k=1}^{d}
\left(
\int*{0}^{T}
\log\lambda^{(k)}(t)\,dN^{(k)}(t)

- \int\_{0}^{T}
  \lambda^{(k)}(t)\,dt
  \right)
  \]

This optimization task is inherently complex due to the intertwined nature of the processes, necessitating methods such as the Expectation-Maximization algorithm or numerical optimization techniques that can handle multidimensional parameter spaces efficiently.

## Applications in Financial Economics

In a multivariate market model, each parameter should identify both a source event and a target decision:

- \(\mu^{(k)}\) tells us the background rate of target event \(k\), in the context of a particular stream such as buy trades, sell trades, cancellations, or price moves, so that we can establish a separate normal-flow benchmark for each risk.
- \(\alpha^{(kj)}\) tells us how strongly a source event of type \(j\) excites target events of type \(k\), in the context of limit-order-book interactions or transmission across instruments, so that we can anticipate which observable event should change which quote-side forecast.
- \(\beta^{(kj)}\) tells us how long that source-to-target effect persists, in the context of the model's time unit, so that we can decide how long the corresponding hedge, skew, or liquidity adjustment should remain active.
- \(\lambda^{(k)}(t)\) tells us the resulting live arrival rate for target type \(k\), in the context of all modeled event history, so that we can convert cross-market or order-book interactions into a near-term event probability.

These quantities describe conditional propagation rather than economic causation. Their usefulness for quoting or risk control should be validated against subsequent fills, price moves, and inventory outcomes.

## Python Code Snippet

Below is a Python code snippet that provides an implementation for modeling multivariate Hawkes processes, including parameter estimation using maximum likelihood.

```python
import numpy as np
from scipy.optimize import minimize
from scipy.integrate import quad

class MultivariateHawkesProcess:
    def __init__(self, baseline_intensity, excitation_matrix,
                 decay_functions):
        '''
        Initialize the multivariate Hawkes process.
        :param baseline_intensity: Baseline intensities for each
            process.
        :param excitation_matrix: Matrix of excitation coefficients.
        :param decay_functions: List of decay functions for each
            coefficient.
        '''
        self.baseline_intensity = baseline_intensity
        self.excitation_matrix = excitation_matrix
        self.decay_functions = decay_functions

    def intensity_function(self, history, t, process_index):
        '''
        Calculate the intensity function for a specific process at
        time t.
        :param history: List of event histories for each process.
        :param t: Current time.
        :param process_index: Index of the process.
        :return: Intensity value.
        '''
        baseline = self.baseline_intensity[process_index]
        excitation = 0

        for j in range(len(history)):
            for s in history[j]:
                if s < t:
                    excitement_contribution = (
                        self.excitation_matrix[process_index][j] *
                        self.decay_functions[j](t - s)
                    )
                    excitation += excitement_contribution

        return baseline + excitation

    def log_likelihood(self, history, T):
        '''
        Compute the log-likelihood of the observed history under the
        model.
        :param history: List of event histories for each process.
        :param T: Time horizon for the observation.
        :return: Log-likelihood value.
        '''
        log_likelihood = 0

        for k in range(len(history)):
            intensity_at_events = sum(
                np.log(self.intensity_function(history, t, k))
                for t in history[k]
            )

            integral_of_intensity = quad(
                lambda t: self.intensity_function(history, t, k),
                0,
                T
            )[0]

            log_likelihood += (
                intensity_at_events - integral_of_intensity
            )

        return -log_likelihood  # Negative because we'll use
                               # minimization

    def fit(self, history, T):
        '''
        Fit the model parameters to the given history data.
        :param history: List of event histories for each process.
        :param T: Time horizon for the observation.
        '''
        def objective(params):
            self.baseline_intensity = (
                params[:len(self.baseline_intensity)]
            )
            flat_matrix = params[len(self.baseline_intensity):]
            self.excitation_matrix = np.reshape(
                flat_matrix,
                self.excitation_matrix.shape
            )
            return self.log_likelihood(history, T)

        initial_params = np.concatenate(
            (
                self.baseline_intensity.ravel(),
                self.excitation_matrix.ravel()
            )
        )

        result = minimize(
            objective,
            initial_params,
            method='L-BFGS-B'
        )

        print("Optimization success:", result.success)
        print("Estimated parameters:", result.x)

# Example usage
baseline_intensity = np.array([0.1, 0.2])  # Example baseline
                                               # intensities

excitation_matrix = np.array([
    [0.3, 0.4],
    [0.2, 0.5]
])  # Example excitation coefficients

# Exponential decay functions
def decay_function_1(t):
    return np.exp(-1.0 * t)

def decay_function_2(t):
    return np.exp(-1.5 * t)

decay_functions = [decay_function_1, decay_function_2]

# Simulated history of events for two processes
history = [[0.2, 0.5, 0.7], [0.3, 0.6, 0.8]]
T = 1.0  # Time horizon

# Instantiate the Multivariate Hawkes Process
hawkes_model = MultivariateHawkesProcess(
    baseline_intensity,
    excitation_matrix,
    decay_functions
)

# Estimate parameters based on the history
hawkes_model.fit(history, T)
```

This code encapsulates several key aspects of multivariate Hawkes process modeling and parameter estimation:

- The `MultivariateHawkesProcess` class initializes multivariate Hawkes processes, taking baseline intensities, an excitation matrix, and decay functions as input.
- The `intensity_function` calculates the intensity of a process at a given time, considering contributions from all events in the history.
- The `log_likelihood` computes the log-likelihood of event history, crucial for parameter estimation.
- The `fit` method optimizes baseline intensities and excitation coefficients using a minimization approach to maximize the likelihood.

The snippet gracefully handles the complexity of fitting multivariate Hawkes processes to observed event data, aiding researchers and practitioners in financial econometrics and related fields.
