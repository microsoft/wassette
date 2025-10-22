# Get Weather Example (JavaScript)

This example demonstrates how to get the weather for a given location using a Wassette component written in JavaScript.

For more information on installing Wassette, please see the [installation instructions](https://github.com/microsoft/wassette?tab=readme-ov-file#installation).

## Usage

To use this component, you will need an API key from [OpenWeather](https://openweathermap.org/api). Set the API key as an environment variable:

```bash
export OPENWEATHER_API_KEY="your_api_key_here"
```

See the [Environment Variables reference](../../docs/reference/environment-variables.md) for alternative methods including config files and Docker.

Then, load the component from the OCI registry and provide a latitude and longitude.

**Load the component:**

```
Please load the component from oci://ghcr.io/microsoft/get-weather-js:latest
```

**Get the weather:**

```
get the weather for latitude 43.65 and longitude -79.38
```

## Policy

By default, WebAssembly (Wasm) components do not have any access to the host machine or network. The `policy.yaml` file is used to explicitly define what network resources and environment variables are made available to the component. This ensures that the component can only access the resources that are explicitly allowed.

Example:

```yaml
version: "1.0"
description: "Permission policy for wassette weather demo"
permissions:
  network:
    allow:
      - host: "api.openweathermap.org"
  environment:
    allow:
      - key: "OPENWEATHER_API_KEY"
```

The source code for this example can be found in [`weather.js`](weather.js).
