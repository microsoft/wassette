// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Text;
using System.Text.Json;
using GetWeatherDotnetWorld;
using EnvironmentImports = GetWeatherDotnetWorld.wit.Imports.wasi.cli.v0_2_1.IEnvironmentImports;
using HttpImports = GetWeatherDotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace GetWeatherDotnetWorld;

public sealed class GetWeatherDotnetWorldExportsImpl : IGetWeatherDotnetWorldExports
{
    public static string GetWeather(string city)
    {
        try
        {
            var apiKey = FindEnvironmentValue("OPENWEATHER_API_KEY");
            if (string.IsNullOrWhiteSpace(apiKey))
            {
                throw new InvalidOperationException("OPENWEATHER_API_KEY is not set.");
            }

            var geocode = Request(
                $"https://api.openweathermap.org/geo/1.0/direct?q={Uri.EscapeDataString(city)}&limit=1&appid={Uri.EscapeDataString(apiKey)}");
            using var geocodeDocument = JsonDocument.Parse(geocode);
            var location = geocodeDocument.RootElement[0];
            var latitude = location.GetProperty("lat").GetDouble();
            var longitude = location.GetProperty("lon").GetDouble();
            var weather = Request(
                $"https://api.openweathermap.org/data/2.5/weather?lat={latitude}&lon={longitude}&appid={Uri.EscapeDataString(apiKey)}&units=metric");
            using var weatherDocument = JsonDocument.Parse(weather);
            return weatherDocument.RootElement.GetProperty("main").GetProperty("temp").ToString();
        }
        catch (Exception exception)
        {
            throw new WitException<string>(exception.Message, 0);
        }
    }

    private static string? FindEnvironmentValue(string name)
    {
        foreach (var pair in EnvironmentImports.GetEnvironment())
        {
            if (pair.Item1 == name)
            {
                return pair.Item2;
            }
        }

        return null;
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
        var response = WaitForResponse(HttpImports.IOutgoingHandlerImports.Handle(request, null));
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
