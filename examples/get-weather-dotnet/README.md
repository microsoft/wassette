# Weather Example (.NET)

This is the .NET 10 equivalent of `get-weather-js`. It reads
`OPENWEATHER_API_KEY` through the explicit WASI environment import, geocodes a
city, and returns the current temperature from OpenWeather.

```bash
dotnet build -c Release
just inject-docs examples/get-weather-dotnet/bin/Release/net10.0/wasi-wasm/native/get-weather-dotnet.wasm examples/get-weather-dotnet/wit
```

Grant the API host and `OPENWEATHER_API_KEY` using `policy.yaml`; never place a
real key in this directory or in CI.
