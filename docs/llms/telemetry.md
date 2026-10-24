# Grenat — OpenTelemetry (`OTEL_*` variables)

Read this when traces must reach Datadog, Grafana, Honeycomb or any OpenTelemetry collector. It assumes the core reference, `llms.txt`.

Nothing to write in the program: tracing is configured by environment variables.

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=https://otlp.example.com \
OTEL_EXPORTER_OTLP_HEADERS="x-api-key=…" \
OTEL_SERVICE_NAME=desk \
grenat serve
```

- With `OTEL_EXPORTER_OTLP_ENDPOINT` (and `OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_SERVICE_NAME`) set, `grenat run` and `grenat serve` export spans over OTLP/HTTP: requests, triggers, jobs, workflows and steps, `ask`s, model calls (tokens, `grenat.usage.cost_usd`), tool calls, `Http` (sends `traceparent`) and SSH calls.
- Prompts, answers and secrets are never exported unless `OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT=true` (prompts and answers; secrets still never).
- A caller's `traceparent` with flags `00` (not sampled) is followed: its spans are not exported.
- `429`/`502`/`503`/`504` and an unreachable collector are retried (Retry-After, backoff, 4 sends); redirects are never followed.
- Tests export nothing.
