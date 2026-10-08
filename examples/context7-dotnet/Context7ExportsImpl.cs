// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Text;
using System.Text.Json;
using Context7DotnetWorld.wit.Exports.microsoft.context7Dotnet.v0_1_0;
using EnvironmentImports = Context7DotnetWorld.wit.Imports.wasi.cli.v0_2_1.IEnvironmentImports;
using HttpImports = Context7DotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace Context7DotnetWorld.wit.Exports.microsoft.context7Dotnet.v0_1_0;

public sealed class Context7ExportsImpl : IContext7Exports
{
    public static IContext7Exports.SearchResponse ResolveLibraryId(string libraryName)
    {
        if (string.IsNullOrWhiteSpace(libraryName))
        {
            return new(false, new List<IContext7Exports.LibraryResult>(), "Library name is required.");
        }

        try
        {
            using var document = JsonDocument.Parse(
                Request($"/v1/search?query={Uri.EscapeDataString(libraryName.Trim())}"));
            var results = new List<IContext7Exports.LibraryResult>();
            if (document.RootElement.TryGetProperty("results", out var entries))
            {
                foreach (var entry in entries.EnumerateArray())
                {
                    results.Add(new IContext7Exports.LibraryResult(
                        StringProperty(entry, "id"),
                        StringProperty(entry, "name", "title"),
                        StringProperty(entry, "description"),
                        UIntProperty(entry, "codeSnippets", "totalSnippets"),
                        UIntProperty(entry, "trustScore", "trust_score"),
                        StringArrayProperty(entry, "versions")));
                }
            }

            return new(true, results, null);
        }
        catch (Exception exception)
        {
            return new(false, new List<IContext7Exports.LibraryResult>(), exception.Message);
        }
    }

    public static IContext7Exports.DocsResponse GetLibraryDocs(
        string context7CompatibleLibraryId,
        string? topic,
        uint? tokens)
    {
        if (string.IsNullOrWhiteSpace(context7CompatibleLibraryId))
        {
            return new(false, null, "Context7 compatible library ID is required.");
        }

        try
        {
            var tokenCount = Math.Max(tokens ?? 10_000, 10_000);
            var endpoint =
                $"/v1/{context7CompatibleLibraryId.TrimStart('/')}?tokens={tokenCount}&type=txt";
            if (!string.IsNullOrWhiteSpace(topic))
            {
                endpoint += $"&topic={Uri.EscapeDataString(topic.Trim())}";
            }

            var content = Request(endpoint);
            return string.IsNullOrWhiteSpace(content) ||
                content is "No content available" or "No context data available"
                ? new(false, null, "No documentation available for this library.")
                : new(true, content, null);
        }
        catch (Exception exception)
        {
            return new(false, null, exception.Message);
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

    private static string Request(string endpoint)
    {
        var apiKey = FindEnvironmentValue("CONTEXT7_API_KEY");
        var uri = new Uri($"https://context7.com/api{endpoint}");
        var request = new HttpImports.ITypesImports.OutgoingRequest(
            new HttpImports.ITypesImports.Fields());
        request.SetMethod(HttpImports.ITypesImports.Method.Get());
        request.SetScheme(HttpImports.ITypesImports.Scheme.Https());
        request.SetAuthority(uri.Authority);
        request.SetPathWithQuery(uri.PathAndQuery);
        using var headers = request.Headers();
        headers.Set("Accept", new List<byte[]> { Encoding.UTF8.GetBytes("*/*") });
        if (!string.IsNullOrWhiteSpace(apiKey))
        {
            headers.Set("context7-api-key", new List<byte[]> { Encoding.UTF8.GetBytes(apiKey) });
        }

        var response = WaitForResponse(HttpImports.IOutgoingHandlerImports.Handle(request, null));
        var body = ReadBody(response);
        if (response.Status() is < 200 or >= 300)
        {
            throw new InvalidOperationException($"Context7 returned {response.Status()}: {body}");
        }

        return body;
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

    private static string StringProperty(JsonElement element, params string[] names)
    {
        foreach (var name in names)
        {
            if (element.TryGetProperty(name, out var property) &&
                property.ValueKind == JsonValueKind.String)
            {
                return property.GetString() ?? string.Empty;
            }
        }

        return string.Empty;
    }

    private static uint UIntProperty(JsonElement element, params string[] names)
    {
        foreach (var name in names)
        {
            if (element.TryGetProperty(name, out var property) &&
                property.TryGetUInt32(out var value))
            {
                return value;
            }
        }

        return 0;
    }

    private static List<string> StringArrayProperty(JsonElement element, string name)
    {
        var values = new List<string>();
        if (element.TryGetProperty(name, out var property) &&
            property.ValueKind == JsonValueKind.Array)
        {
            foreach (var item in property.EnumerateArray())
            {
                if (item.ValueKind == JsonValueKind.String)
                {
                    values.Add(item.GetString() ?? string.Empty);
                }
            }
        }

        return values;
    }
}
