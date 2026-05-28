# npm-compact — Token Saving Benchmark

Run: `sh bench.sh`

| Sample | Level | Input | Output | Saved | % |
|---|---|---|---|---|---|
| npm-audit-ultra | lite | 110t | 110t | 0t | 0% |
| npm-audit-ultra | full | 110t | 110t | 0t | 0% |
| npm-audit-ultra | ultra | 110t | 22t | 88t | **80%** |
| npm-install-full | lite | 184t | 58t | 126t | 68% |
| npm-install-full | full | 184t | 58t | 126t | 68% |
| npm-install-full | ultra | 184t | 13t | 171t | **92%** |
| npm-install-ultra | lite | 184t | 58t | 126t | 68% |
| npm-install-ultra | full | 184t | 58t | 126t | 68% |
| npm-install-ultra | ultra | 184t | 13t | 171t | **92%** |
| npm-test-ultra | lite | 109t | 109t | 0t | 0% |
| npm-test-ultra | full | 109t | 109t | 0t | 0% |
| npm-test-ultra | ultra | 109t | 39t | 70t | **64%** |
