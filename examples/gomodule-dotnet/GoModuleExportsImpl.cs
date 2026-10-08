// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Text;
using System.Text.Json;
using GomoduleDotnetWorld;
using HttpImports = GomoduleDotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace GomoduleDotnetWorld;

public sealed class GomoduleDotnetWorldExportsImpl : IGomoduleDotnetWorldExports
{
    public static string GetLatestVersions(string moduleNames)
    {
        try
        {
            var versions = new List<string>();
            foreach (var module in Modules(moduleNames))
            {
                var body = Request($"https://proxy.golang.org/{module}/@latest");
                using var document = JsonDocument.Parse(body);
                if (document.RootElement.TryGetProperty("Version", out var version))
                {
                    versions.Add($"\"{JsonEncodedText.Encode(module)}\":\"{JsonEncodedText.Encode(version.GetString() ?? string.Empty)}\"");
                }
            }

            return versions.Count == 0 ? throw new InvalidOperationException("No modules found.") : $"{{{string.Join(",", versions)}}}";
        }
        catch (Exception exception)
        {
            throw new WitException<string>(exception.Message, 0);
        }
    }

    public static string GetModuleInfo(string moduleNames)
    {
        try
        {
            var results = new List<string>();
            foreach (var module in Modules(moduleNames))
            {
                results.Add(Request($"https://proxy.golang.org/{module}/@latest"));
            }

            return results.Count == 0 ? throw new InvalidOperationException("No modules found.") : $"[{string.Join(",", results)}]";
        }
        catch (Exception exception)
        {
            throw new WitException<string>(exception.Message, 0);
        }
    }

    private static IEnumerable<string> Modules(string moduleNames)
    {
        foreach (var module in moduleNames.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            yield return module.Contains('/', StringComparison.Ordinal) ? module : $"github.com/{module}";
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
        var response = WaitForResponse(HttpImports.IOutgoingHandlerImports.Handle(request, null));
        using var body = response.Consume();
        using var stream = body.Stream();
        var bytes = new List<byte>();
        while (true)
        {
            var chunk = stream.BlockingRead(8192);
            if (chunk.Length == 0)
            {
                break;
            }

            bytes.AddRange(chunk);
        }

        if (response.Status() is < 200 or >= 300)
        {
            throw new InvalidOperationException($"HTTP request failed: {response.Status()}.");
        }

        return Encoding.UTF8.GetString(bytes.ToArray());
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
}
