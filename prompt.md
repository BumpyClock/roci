## Porting Tachikoma to Roci
Roci is a port of Tachikoma in rust using rust native patterns , idioms and optimizations.
Port is in progress but partity not achieved yet. Continue iterating and testing until full parity is reached.

## Flow
0. Read feature-gap-analysis.md for current gaps. If it doesn't exist, create it by comparing Tachikoma and Roci. Do a thorough analysis. Write down all gaps in detail.
1. Identify the feature / implementation gap between Tachikoma and Roci.
2. Implement feature / fix gap in Roci.
3. Write test against real providers to validate implementation.
4. Run tests to ensure parity.
5. Ensure test completes successfully. If not, iterate from step 2. Use Tachikoma as reference.
6. Run Tachikoma from ~/Projects/references/Tachikoma and it's associated tests to cross compare behavior and outputs.
7. Update docs if needed.
8. Commit and push changes.
9. update `LEARNINGS.md` with any new learnings from the implementation.
10. Update `feature-gap-analysis.md` to reflect closed gaps.
11. Validate that `feature-gap-analysis.md` is accurate and no new gaps were introduced. If they were update feature-gap-analysis.md.
12. If all gaps are closed, output `Task Complete` and nothing else.
13. Run Tachikoma from ~/Projects/references/Tachikoma and it's associated tests to cross compare behavior and outputs.
