// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using GithubDotnetWorld;
using EnvironmentImports = GithubDotnetWorld.wit.Imports.wasi.cli.v0_2_1.IEnvironmentImports;
using HttpImports = GithubDotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace GithubDotnetWorld;

public sealed class GithubDotnetWorldExportsImpl : IGithubDotnetWorldExports
{
    public static string GetRepository(string owner, string repo) =>
        Request($"/repos/{Escape(owner)}/{Escape(repo)}");

    public static string GetFileContents(string owner, string repo, string path, string? reference)
    {
        var endpoint = $"/repos/{Escape(owner)}/{Escape(repo)}/contents/{EscapePath(path)}";
        return Request(reference is null ? endpoint : $"{endpoint}?ref={Uri.EscapeDataString(reference)}");
    }

    public static string ListBranches(string owner, string repo, uint? page, uint? perPage) =>
        Request(AddPaging($"/repos/{Escape(owner)}/{Escape(repo)}/branches", page, perPage));

    public static string ListIssues(string owner, string repo, string? state, uint? page, uint? perPage)
    {
        var endpoint = $"/repos/{Escape(owner)}/{Escape(repo)}/issues";
        if (!string.IsNullOrWhiteSpace(state))
        {
            endpoint += $"?state={Uri.EscapeDataString(state)}";
        }

        return Request(AddPaging(endpoint, page, perPage));
    }

    public static string GetMe() => Request("/user");

    private static string Request(string endpoint)
    {
        try
        {
            var token = FindEnvironmentValue("GITHUB_TOKEN");
            if (string.IsNullOrWhiteSpace(token))
            {
                throw new InvalidOperationException("GITHUB_TOKEN is not set.");
            }

            var uri = new Uri($"https://api.github.com{endpoint}");
            var request = new HttpImports.ITypesImports.OutgoingRequest(
                new HttpImports.ITypesImports.Fields());
            request.SetMethod(HttpImports.ITypesImports.Method.Get());
            request.SetScheme(HttpImports.ITypesImports.Scheme.Https());
            request.SetAuthority(uri.Authority);
            request.SetPathWithQuery(uri.PathAndQuery);
            using var headers = request.Headers();
            SetHeader(headers, "Authorization", $"Bearer {token}");
            SetHeader(headers, "Accept", "application/vnd.github+json");
            SetHeader(headers, "X-GitHub-Api-Version", "2022-11-28");
            SetHeader(headers, "User-Agent", "wassette-github-dotnet");

            var response = WaitForResponse(HttpImports.IOutgoingHandlerImports.Handle(request, null));
            var body = ReadBody(response);
            if (response.Status() is < 200 or >= 300)
            {
                throw new InvalidOperationException($"GitHub API returned {response.Status()}: {body}");
            }

            return body;
        }
        catch (Exception exception)
        {
            throw new WitException<string>(exception.Message, 0);
        }
    }

    private static void SetHeader(HttpImports.ITypesImports.Fields fields, string name, string value) =>
        fields.Set(name, new List<byte[]> { Encoding.UTF8.GetBytes(value) });

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

    private static string AddPaging(string endpoint, uint? page, uint? perPage)
    {
        var separator = endpoint.Contains('?', StringComparison.Ordinal) ? '&' : '?';
        if (page is null && perPage is null)
        {
            return endpoint;
        }

        var paging = page is null ? string.Empty : $"page={page}";
        if (perPage is not null)
        {
            paging += $"{(paging.Length == 0 ? string.Empty : "&")}per_page={perPage}";
        }

        return endpoint + separator + paging;
    }

    private static string Escape(string value) => Uri.EscapeDataString(value);

    private static string EscapePath(string value) =>
        string.Join("/", value.Split('/', StringSplitOptions.None).Select(Uri.EscapeDataString));

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
