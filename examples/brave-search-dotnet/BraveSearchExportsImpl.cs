// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Text;
using System.Text.Json;
using BraveSearchDotnetWorld;
using EnvironmentImports = BraveSearchDotnetWorld.wit.Imports.wasi.cli.v0_2_1.IEnvironmentImports;
using HttpImports = BraveSearchDotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace BraveSearchDotnetWorld;

public sealed class BraveSearchDotnetWorldExportsImpl : IBraveSearchDotnetWorldExports
{
    public static string Search(string query)
    {
        try
        {
            var key = FindEnvironmentValue("BRAVE_SEARCH_API_KEY");
            if (string.IsNullOrWhiteSpace(key))
            {
                throw new InvalidOperationException("BRAVE_SEARCH_API_KEY is not set.");
            }

            var uri = new Uri($"https://api.search.brave.com/res/v1/web/search?q={Uri.EscapeDataString(query)}");
            var request = new HttpImports.ITypesImports.OutgoingRequest(
                new HttpImports.ITypesImports.Fields());
            request.SetMethod(HttpImports.ITypesImports.Method.Get());
            request.SetScheme(HttpImports.ITypesImports.Scheme.Https());
            request.SetAuthority(uri.Authority);
            request.SetPathWithQuery(uri.PathAndQuery);
            using var headers = request.Headers();
            headers.Set("X-Subscription-Token", new List<byte[]> { Encoding.UTF8.GetBytes(key) });
            headers.Set("Accept", new List<byte[]> { Encoding.UTF8.GetBytes("application/json") });

            var response = WaitForResponse(HttpImports.IOutgoingHandlerImports.Handle(request, null));
            var body = Encoding.UTF8.GetString(ReadBody(response).ToArray());
            if (response.Status() is < 200 or >= 300)
            {
                throw new InvalidOperationException($"HTTP request failed: {response.Status()}.");
            }

            using var document = JsonDocument.Parse(body);
            var markdown = new StringBuilder($"# Search Results for: {query}\n\n");
            AppendResults(document.RootElement, "web", "## Web Results", markdown);
            AppendResults(document.RootElement, "news", "## News Results", markdown);
            return markdown.ToString();
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

    private static void AppendResults(JsonElement root, string property, string heading, StringBuilder markdown)
    {
        if (!root.TryGetProperty(property, out var group) ||
            !group.TryGetProperty("results", out var results) ||
            results.GetArrayLength() == 0)
        {
            return;
        }

        markdown.AppendLine(heading);
        markdown.AppendLine();
        var index = 1;
        foreach (var result in results.EnumerateArray())
        {
            var title = result.GetProperty("title").GetString() ?? string.Empty;
            var url = result.GetProperty("url").GetString() ?? string.Empty;
            markdown.AppendLine($"{index++}. **[{title}]({url})**");
            if (result.TryGetProperty("description", out var description))
            {
                markdown.AppendLine($"   {description.GetString()}");
            }

            markdown.AppendLine();
        }
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

    private static List<byte> ReadBody(HttpImports.ITypesImports.IncomingResponse response)
    {
        using var body = response.Consume();
        using var stream = body.Stream();
        var bytes = new List<byte>();
        while (true)
        {
            var chunk = stream.BlockingRead(8192);
            if (chunk.Length == 0)
            {
                return bytes;
            }

            bytes.AddRange(chunk);
        }
    }
}
