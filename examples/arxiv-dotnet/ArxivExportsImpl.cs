// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Text;
using ArxivDotnetWorld;
using HttpImports = ArxivDotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace ArxivDotnetWorld;

public sealed class ArxivDotnetWorldExportsImpl : IArxivDotnetWorldExports
{
    public static string SearchPapers(string query, uint maxResults, string dateFrom, string categories)
    {
        try
        {
            var search = query;
            if (!string.IsNullOrWhiteSpace(categories))
            {
                search += $" AND ({categories.Replace(",", " OR ", StringComparison.Ordinal)})";
            }

            if (!string.IsNullOrWhiteSpace(dateFrom))
            {
                search += $" AND submittedDate:[{dateFrom} TO *]";
            }

            var url = $"http://export.arxiv.org/api/query?search_query={Uri.EscapeDataString(search)}&start=0&max_results={maxResults}&sortBy=submittedDate&sortOrder=descending";
            return $"# arXiv Search Results for: {query}\n\n{RequestText(url)}";
        }
        catch (Exception exception)
        {
            throw new WitException<string>(exception.Message, 0);
        }
    }

    public static byte[] DownloadPaper(string id)
    {
        try
        {
            return RequestBytes($"http://arxiv.org/pdf/{NormalizeId(id)}.pdf");
        }
        catch (Exception exception)
        {
            throw new WitException<string>(exception.Message, 0);
        }
    }

    public static string ReadPaper(string id)
    {
        try
        {
            return RequestText($"http://export.arxiv.org/api/query?id_list={NormalizeId(id)}");
        }
        catch (Exception exception)
        {
            throw new WitException<string>(exception.Message, 0);
        }
    }

    private static string NormalizeId(string id) =>
        id.Trim()
            .Replace("http://arxiv.org/abs/", string.Empty, StringComparison.OrdinalIgnoreCase)
            .Replace("https://arxiv.org/abs/", string.Empty, StringComparison.OrdinalIgnoreCase)
            .Replace("arxiv:", string.Empty, StringComparison.OrdinalIgnoreCase);

    private static string RequestText(string url) => Encoding.UTF8.GetString(RequestBytes(url));

    private static byte[] RequestBytes(string url)
    {
        var uri = new Uri(url);
        var request = new HttpImports.ITypesImports.OutgoingRequest(
            new HttpImports.ITypesImports.Fields());
        request.SetMethod(HttpImports.ITypesImports.Method.Get());
        request.SetScheme(HttpImports.ITypesImports.Scheme.Http());
        request.SetAuthority(uri.Authority);
        request.SetPathWithQuery(uri.PathAndQuery);
        var response = WaitForResponse(HttpImports.IOutgoingHandlerImports.Handle(request, null));
        var bytes = ReadBody(response);
        if (response.Status() is < 200 or >= 300)
        {
            throw new InvalidOperationException($"HTTP request failed: {response.Status()}.");
        }

        return bytes.ToArray();
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
