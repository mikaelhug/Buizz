# buffett: the long/short variant

Complex · Blocked by G10 · `long_only` is implemented in `bot.py`

## Idea
Frazzini, Kabiller & Pedersen (FKP) explain Berkshire's alpha with market, size, value, momentum, betting against beta (BAB) and quality minus junk (QMJ), levered about 1.6-to-1 (working paper) or 1.7-to-1 (*FAJ*). Their systematic Buffett-style portfolio holds those factor tilts in Berkshire's proportions at Berkshire's market beta and idiosyncratic volatility: a diversified, levered long/short book. `bot.py` builds every factor's stock weights already and holds the `long_only` variant; this brief is what remains.

## Sources
- Frazzini, Kabiller & Pedersen (2018), "Buffett's Alpha", *FAJ* 74(4); NBER w19681 §7 (systematic portfolio), Appendix C (real-time version), Tables 2, 4 and 5.
- Asness, Frazzini & Pedersen (2019), "Quality Minus Junk", *RAS* 24(1), Appendix 1.
- Frazzini & Pedersen (2014), "Betting Against Beta", *JFE*.

## Logic
- Params to add: `beta = 0.68` (β_B, Berkshire's univariate beta, Table 2) and `idio_vol = 0.224` (σ_I, Table 2); `variant = "long_short"`.
- At the last session of each month, take the factor stock weights `factor_weights` already computes and the active book a = Σ loading_f × w_f (`active_book`).
- Scale c = σ_I / σ̂(a), with σ̂(a) the annualised volatility of today's active weights applied to the last 252 days of total-return daily returns (holdings-based, no lookahead).
- Target weights = β_B on MKT plus c × a, netted per stock. The working paper reaches about 3.7× gross (Table 2); set `max_gross_exposure` to fit, financed at the T-bill rate plus the spread. Reason: "net tilt {w}: qmj {q}, bab {b}, hml {h}".
- Variant `realtime` (Appendix C): each month, re-estimate β_B, the loadings and σ_I from BRK-B against the factor returns using data up to t, with at least 60 months; needs factor return history before the run (G12, G14 or published factor series).

## Gaps
- G10: every factor leg shorts, including small, high-beta and junk names; per-name borrow is recorded but not exposed to runs.

## Done when
- A monthly regression of the bot's excess returns on its six factors (or AQR's published MKT, SMB, HML, UMD, BAB and QMJ for the same months) recovers market beta ≈ 0.68 and loadings in the target proportions.
- The correlation with BRK-B's monthly excess returns is reported against FKP Table 5's 0.47 (0.73 for the 13F portfolio), and volatility is compared with BRK-B's.
