// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using FilesystemDotnetWorld;

namespace FilesystemDotnetWorld;

public sealed class FilesystemDotnetWorldExportsImpl : IFilesystemDotnetWorldExports
{
    public static List<string> ListDirectory(string path)
    {
        try
        {
            return Directory.EnumerateFileSystemEntries(path)
                .Select(entry => Directory.Exists(entry)
                    ? $"[DIR] {Path.GetFileName(entry)}\n"
                    : $"[FILE] {Path.GetFileName(entry)}\n")
                .ToList();
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string ReadFile(string path)
    {
        try
        {
            return File.ReadAllText(path);
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string WriteFile(string path, string content)
    {
        try
        {
            var parent = Path.GetDirectoryName(path);
            if (!string.IsNullOrEmpty(parent))
            {
                Directory.CreateDirectory(parent);
            }

            File.WriteAllText(path, content);
            return $"Successfully wrote to file '{path}'";
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string CreateDirectory(string path)
    {
        try
        {
            Directory.CreateDirectory(path);
            return $"Successfully created directory '{path}'";
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string MovePath(string source, string destination)
    {
        try
        {
            var parent = Path.GetDirectoryName(destination);
            if (!string.IsNullOrEmpty(parent))
            {
                Directory.CreateDirectory(parent);
            }

            File.Move(source, destination, true);
            return $"Successfully moved '{source}' to '{destination}'";
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string DeleteFile(string path)
    {
        try
        {
            if (Directory.Exists(path))
            {
                throw new IOException($"'{path}' is a directory, use delete-directory instead");
            }

            File.Delete(path);
            return $"Successfully deleted file '{path}'";
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string DeleteDirectory(string path)
    {
        try
        {
            Directory.Delete(path, false);
            return $"Successfully deleted directory '{path}'";
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static bool FileExists(string path) => File.Exists(path) || Directory.Exists(path);

    public static string GetDirectoryTree(string path, uint maxDepth)
    {
        try
        {
            var output = new List<string>();
            AddTreeEntries(path, output, 0, maxDepth, string.Empty);
            return string.Join(Environment.NewLine, output);
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string SearchFile(string path, string pattern)
    {
        try
        {
            var matches = Directory.EnumerateFileSystemEntries(path, "*", SearchOption.AllDirectories)
                .Where(entry => Path.GetFileName(entry).Contains(pattern, StringComparison.OrdinalIgnoreCase))
                .ToList();
            return matches.Count == 0
                ? $"No files matching pattern '{pattern}' found in '{path}'"
                : string.Join(Environment.NewLine, matches);
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    public static string GetFileInfo(string path)
    {
        try
        {
            var attributes = File.GetAttributes(path);
            var isDirectory = attributes.HasFlag(FileAttributes.Directory);
            var size = isDirectory ? 0 : new FileInfo(path).Length;
            var modified = File.GetLastWriteTimeUtc(path).ToString("O");
            return $"Path: {path}\nType: {(isDirectory ? "Directory" : "File")}\nSize: {size} bytes\nModified: {modified}";
        }
        catch (Exception exception)
        {
            throw Error(exception);
        }
    }

    private static void AddTreeEntries(
        string path,
        List<string> output,
        uint depth,
        uint maxDepth,
        string indent)
    {
        if (depth > maxDepth)
        {
            return;
        }

        foreach (var entry in Directory.EnumerateFileSystemEntries(path).OrderBy(Path.GetFileName))
        {
            var directory = Directory.Exists(entry);
            output.Add($"{indent}{(directory ? "[DIR] " : "[FILE] ")}{Path.GetFileName(entry)}");
            if (directory)
            {
                AddTreeEntries(entry, output, depth + 1, maxDepth, indent + "  ");
            }
        }
    }

    private static WitException<string> Error(Exception exception) =>
        new(exception.Message, 0);
}
