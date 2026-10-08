// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Text;
using FetchWorld = FetchDotnetWorld;
using HttpImports = FetchDotnetWorld.wit.Imports.wasi.http.v0_2_1;

namespace FetchDotnetWorld;

public sealed class FetchDotnetWorldExportsImpl : IFetchDotnetWorldExports
{
    public static string Fetch(string url)
    {
        try
        {
            var uri = new Uri(url);
            var scheme = uri.Scheme.ToLowerInvariant() switch
            {
                "http" => HttpImports.ITypesImports.Scheme.Http(),
                "https" => HttpImports.ITypesImports.Scheme.Https(),
                _ => throw new InvalidOperationException(
                    $"Unsupported URL scheme: {uri.Scheme}")
            };
            var request = new HttpImports.ITypesImports.OutgoingRequest(
                new HttpImports.ITypesImports.Fields());
            request.SetMethod(HttpImports.ITypesImports.Method.Get());
            request.SetScheme(scheme);
            request.SetAuthority(uri.Authority);
            request.SetPathWithQuery(uri.PathAndQuery);

            var responseFuture = HttpImports.IOutgoingHandlerImports.Handle(request, null);
            var response = WaitForResponse(responseFuture);
            var status = response.Status();
            var body = ReadBody(response);

            return status is >= 200 and < 300
                ? body
                : throw new InvalidOperationException(
                    $"Request failed with status code: {status}. Body: {body}");
        }
        catch (Exception exception)
        {
            throw new FetchWorld.WitException<string>(exception.Message, 0);
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

            if (result.Value.IsErr)
            {
                throw new InvalidOperationException("HTTP response is unavailable.");
            }

            var responseResult = result.Value.AsOk;
            if (responseResult.IsErr)
            {
                throw new InvalidOperationException("HTTP response failed.");
            }

            return responseResult.AsOk;
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
                break;
            }

            bytes.AddRange(chunk);
        }

        return Encoding.UTF8.GetString(bytes.ToArray());
    }
}
