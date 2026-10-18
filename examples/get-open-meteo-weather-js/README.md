# Get Weather Example (JavaScript)

This example demonstrates how to get the weather for a given location using a Wassette component written in JavaScript.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Usage

This component uses the [Open-Meteo API](https://open-meteo.com/), which provides free weather data without requiring authentication.

Load the component from the OCI registry and provide a city name.

**Load the component:**

```
Please load the component from oci://ghcr.io/microsoft/get-weather-js:latest
```

**Get the weather:**

```
get the weather for Toronto
```

## Local Wassette workflow

From the repository root:

```bash
just build-examples release
STORE="$(mktemp -d)"

wassette component load \
  "file://$PWD/bin/get-open-meteo-weather-js.wasm" \
  --component-dir "$STORE"

wassette permission grant network \
  get-open-meteo-weather-js geocoding-api.open-meteo.com \
  --component-dir "$STORE"

wassette permission grant network \
  get-open-meteo-weather-js api.open-meteo.com \
  --component-dir "$STORE"

wassette run --component-dir "$STORE" --disable-builtin-tools
```

The checked-in [`policy.yaml`](policy.yaml) documents both required network
grants but is not automatically attached by a local file load or included in
the currently published OCI example. See the
[examples permission workflow](../README.md).

## Policy

By default, WebAssembly (Wasm) components do not have any access to the host machine or network. The `policy.yaml` file is used to explicitly define what network resources and environment variables are made available to the component. This ensures that the component can only access the resources that are explicitly allowed.

Example:

```yaml
version: "1.0"
description: "Permission policy for wassette weather demo"
permissions:
  network:
    allow:
      - host: "geocoding-api.open-meteo.com"  # For geocoding city names to coordinates
      - host: "api.open-meteo.com"            # For fetching weather data
```

The source code for this example can be found in [`weather.js`](weather.js).
