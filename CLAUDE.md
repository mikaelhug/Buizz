# AI Agent Rules

## Core Directives
1. **Never write code comments**: Zero inline, block, or trailing comments. No AI watermarks or commented-out code. Make code self-documenting through clear naming and types.
2. **Few, purposeful tests only**: tests are end-to-end scenarios of what a user does and sees (real compositor headless, real test bus, real apps), or a regression test for a real bug or security boundary that fails without its fix.
3. **Reuse code first**: Search the workspace before writing any helper, type, or utility. Never duplicate existing abstractions.
4. **Minimal surface area (YAGNI)**: Zero speculative code, unused builder patterns, or premature indirection. Solve problems with the fewest moving parts. Prefer standard library combinators and idiomatic composition over custom machinery.
5. **Purge code rot**: Delete unused imports, dead functions, unreferenced types, and stale tests in any file you touch. Leave code cleaner than you found it.
6. **Small, current docs and notes**: Update the docs in the same change as the code they describe. Delete a gap, plan or review finding once it is finished (git keeps the history). No dated logs, diaries or measurement histories in the repo; trimming matters as much as writing.
7. **Never select the "easiest" solution**: Always think about the solution you plan to implement, is that the best solution for the user? Never select a solution because it is the easiest or fastest, always prioritize the solution that is better for the user, higher quality and which follows best practice.
