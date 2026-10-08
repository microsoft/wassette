// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Text;
using System.Text.Json;
using GetOpenMeteoWeatherDotnetWorld;
using HttpImports = GetOpenMeteoWeatherDotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace GetOpenMeteoWeatherDotnetWorld;

public sealed class GetOpenMeteoWeatherDotnetWorldExportsImpl
    : IGetOpenMeteoWeatherDotnetWorldExports
{
    public static string GetWeather(string city)
    {
        try
        {
            var encodedCity = Uri.EscapeDataString(city);
            var geocoding = Request(
                $"https://geocoding-api.open-meteo.com/v1/search?name={encodedCity}&count=1&language=en&format=json");
            using var geocodingDocument = JsonDocument.Parse(geocoding);
            var results = geocodingDocument.RootElement.GetProperty("results");
            if (results.GetArrayLength() == 0)
            {
                throw new InvalidOperationException($"Location '{city}' not found.");
            }

            var location = results[0];
            var latitude = location.GetProperty("latitude").GetDouble();
            var longitude = location.GetProperty("longitude").GetDouble();
            var weather = Request(
                $"https://api.open-meteo.com/v1/forecast?latitude={latitude.ToString(System.Globalization.CultureInfo.InvariantCulture)}&longitude={longitude.ToString(System.Globalization.CultureInfo.InvariantCulture)}&current=temperature_2m");
            using var weatherDocument = JsonDocument.Parse(weather);
            return weatherDocument.RootElement
                .GetProperty("current")
                .GetProperty("temperature_2m")
                .ToString();
        }
        catch (Exception exception)
        {
            throw new WitException<string>(
                exception.Message,
                0);
        }
    }

    private static string Request(string url)
    {
        var uri = new Uri(url);
        var request = new HttpImports.ITypesImports.OutgoingRequest(
            new HttpImports.ITypesImports.Fields());
        request.SetMethod(HttpImports.ITypesImports.Method.Get());
        request.SetScheme(HttpImports.ITypesImports.Scheme.Https());
        request.SetAuthority(uri.Authority);
        request.SetPathWithQuery(uri.PathAndQuery);

        var future = HttpImports.IOutgoingHandlerImports.Handle(request, null);
        var response = WaitForResponse(future);
        var body = ReadBody(response);
        return response.Status() is >= 200 and < 300
            ? body
            : throw new InvalidOperationException($"HTTP request failed: {response.Status()}.");
    }

    private static HttpImports.ITypesImports.IncomingResponse WaitForResponse(
        HttpImports.ITypesImports.FutureIncomingResponse future)
    {
        while (true)
        {
            var result = future.Get();
            if (result is null)
            {
                future.Subscribe().Block();
                continue;
            }

            if (result.Value.IsErr || result.Value.AsOk.IsErr)
            {
                throw new InvalidOperationException("HTTP response failed.");
            }

            return result.Value.AsOk.AsOk;
        }
    }

    private static string ReadBody(HttpImports.ITypesImports.IncomingResponse response)
    {
        using var body = response.Consume();
        using var stream = body.Stream();
        var bytes = new List<byte>();
        while (true)
        {
            var chunk = stream.BlockingRead(8192);
            if (chunk.Length == 0)
            {
                return Encoding.UTF8.GetString(bytes.ToArray());
            }

            bytes.AddRange(chunk);
        }
    }
}
