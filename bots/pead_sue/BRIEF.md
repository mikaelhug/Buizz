# pead_sue

Medium · Gaps: G2, G7, G10

## Idea
Post-earnings-announcement drift: prices underreact to earnings surprises and keep drifting in the surprise's direction for about 60 trading days. Measure the surprise as standardised unexpected earnings (SUE) under a seasonal random walk. Go long the top SUE decile and short the bottom from the announcement day, and hold for 60 sessions.

## Sources
- Ball & Brown (1968), *JAR*.
- Foster, Olsen & Shevlin (1984), "Earnings Releases, Anomalies, and the Behavior of Security Returns", *TAR* (the SUE definition).
- Bernard & Thomas (1989), "Post-Earnings-Announcement Drift: Delayed Price Response or Risk Premium?", *JAR*.
- Verify: Chordia, Goyal, Sadka et al. on PEAD decay in large caps, and Martineau (2022) "Rest in Peace Post-Earnings Announcement Drift".

## Manifest
```toml
description = "Post-earnings drift on standardised unexpected earnings"
timeframe = "1d"
wake = "bar"
universe = { top_dollar_volume = 1000 }
params = { hold_sessions = 60, decile = 0.1, sue_quarters = 8 }
```

## Logic
- SUE = (EPS_q − EPS_{q−4}) / σ(EPS_q − EPS_{q−4}) over the prior `sue_quarters`, from quarterly `EpsDiluted`.
- Q4 has no 10-Q. The agent derives it from FY minus Q1–Q3 (or from `NetIncome` / shares) and documents the choice.
- On the announcement event, rank the surprise against the trailing SUE distribution (decile breakpoints from the prior quarter's cross-section).
- Long top-decile names and short bottom-decile names, equal weight, enter at the next open and exit after `hold_sessions`. Reasons: "SUE {x}, top decile" and "SUE {x}, bottom decile".

## Gaps
- G7: the drift is measured from the earnings release (8-K Item 2.02), which comes weeks before the 10-Q. Using `available_at` of the 10-Q would test a different, weaker strategy.
- G2: stocks only.
- G10: the short leg needs per-name borrow.

## Done when
- The cumulative abnormal return after the announcement shows drift for top versus bottom deciles, and the PR compares its size with the decay literature for the same years.
