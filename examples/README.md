# Wassette examples

The examples demonstrate WebAssembly components with different host-capability
requirements. Build all examples from the repository root with:

```bash
just build-examples release
```

## Permissions during local development

Loading a component does not grant it access to the host. Make configuration
values available to the Wassette process, then grant each component access to
only the values and resources it needs. Keep the component load, permission
commands, and server on the same component directory:

```bash
STORE="$(mktemp -d)"

wassette component load \
  "file://$PWD/bin/get-weather-js.wasm" \
  --component-dir "$STORE"

export OPENWEATHER_API_KEY="<developer-key>"

wassette permission grant network \
  get-weather-js api.openweathermap.org \
  --component-dir "$STORE"

wassette permission grant environment-variable \
  get-weather-js OPENWEATHER_API_KEY \
  --component-dir "$STORE"

wassette run --component-dir "$STORE" --disable-builtin-tools
```

An environment variable being present does not authorize a component to read
it. A component without a policy receives no configured environment values or
stored secrets.

## Source policies and published OCI artifacts

The `policy.yaml` files in the example source directories document the minimum
permissions expected by those components. The current
[Publish Examples](../.github/workflows/examples.yml) workflow publishes only
the built `.wasm` file with `wkg oci push`; it does **not** include these source
policies as OCI layers. Loading a published `ghcr.io/microsoft/*` example
therefore does not automatically install its checked-in policy. Grant the
permissions explicitly or use a deployment manifest such as those under
[`examples/manifests`](manifests/).
