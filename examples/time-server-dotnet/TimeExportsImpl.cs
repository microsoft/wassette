// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Globalization;
using TimeServerDotnetWorld.wit.Exports.microsoft.timeServerDotnet;
using WasiWallClock = TimeServerDotnetWorld.wit.Imports.wasi.clocks.v0_2_1.IWallClockImports;

namespace TimeServerDotnetWorld.wit.Exports.microsoft.timeServerDotnet;

public sealed class TimeExportsImpl : ITimeExports
{
    public static string GetCurrentTime()
    {
        var timestamp = WasiWallClock.Now();
        var seconds = checked((long)timestamp.seconds);
        var nanoseconds = timestamp.nanoseconds;
        var milliseconds = checked((long)nanoseconds / 1_000_000);

        return DateTimeOffset
            .FromUnixTimeSeconds(seconds)
            .AddMilliseconds(milliseconds)
            .ToString("O", CultureInfo.InvariantCulture);
    }
}
