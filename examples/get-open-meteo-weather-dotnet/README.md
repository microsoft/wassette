# Open-Meteo Weather Example (.NET)

This is the .NET 10 equivalent of `get-open-meteo-weather-js`. It geocodes a
city and returns the current temperature using explicit WASI HTTP imports.
Open-Meteo does not require an API key.

```bash
dotnet build -c Release
just inject-docs examples/get-open-meteo-weather-dotnet/bin/Release/net10.0/wasi-wasm/native/get-open-meteo-weather-dotnet.wasm examples/get-open-meteo-weather-dotnet/wit
```

Load the resulting component and ask for the weather in a city. The policy
must allow both Open-Meteo hosts listed in `policy.yaml`.
