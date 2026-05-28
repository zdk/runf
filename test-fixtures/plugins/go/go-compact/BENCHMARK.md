# go-compact — Token Saving Benchmark

Run: `sh bench.sh`

| Sample | Level | Input | Output | Saved | % |
|---|---|---|---|---|---|
| go-build-ultra | lite | 58t | 51t | 7t | 12% |
| go-build-ultra | full | 58t | 51t | 7t | 12% |
| go-build-ultra | ultra | 58t | 38t | 20t | **34%** |
| go-test-ultra | lite | 131t | 89t | 42t | 32% |
| go-test-ultra | full | 131t | 89t | 42t | 32% |
| go-test-ultra | ultra | 131t | 78t | 53t | **40%** |
