# Startup Benchmark

This benchmark measures how quickly Wassette becomes responsive when a predefined set of example components are preloaded. It also tracks how long the background loader takes to finish compiling the components once the server reports that it is ready.

## Running the benchmark locally

1. Build the server and the curated set of benchmark components:
   ```bash
   just build-benchmark-components mode=release
   cargo build --release --bin benchmark-startup
   ```
   Alternatively, run everything in one go with:
   ```bash
   just benchmark-startup mode=release runs=3
   ```
2. Inspect the generated report at `target/startup-benchmark.json`.

### Output fields

The JSON report uses the schema produced by `benchmark-startup`:

| Field | Description |
|-------|-------------|
| `timestamp` | ISO-8601 UTC timestamp for the measurement. |
| `components` | List of component stems copied into the temporary plugin directory. |
| `component_count` | Number of `.wasm` components benchmarked. |
| `runs` | Per-run timings with `ready_seconds`, `load_complete_seconds`, and `component_load_seconds` (the delta between load completion and initial readiness). |
| `summary` | Aggregated metrics across all runs (average, min, max for the three timing categories). |
| `git_sha` / `git_ref` | Optional metadata included by CI to track the commit under test. |

## Continuous measurements

A dedicated GitHub Actions workflow runs the benchmark every day:

- Builds the Wassette binary and a stable set of example components.
- Records timing data via `benchmark-startup`.
- Appends the result to `benchmarks/startup/data.json` on the `gh-pages` branch (retaining the most recent 120 entries).
- Publishes a static dashboard (HTML/JS) to the same location.

The dashboard visualises historical trends and surfaces the latest run. After the first scheduled execution completes you can access it at:

```
https://microsoft.github.io/wassette/benchmarks/startup/
```

If you make local changes to the benchmark or need to reseed the dashboard, run `just benchmark-startup` and copy the updated `benchmarks/startup/dashboard` assets along with any refreshed `data.json` to the `gh-pages` branch before pushing.
