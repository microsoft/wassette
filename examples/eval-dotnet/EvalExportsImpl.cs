// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.

using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace EvalDotnetWorld;

[JsonSerializable(typeof(string))]
internal partial class EvalJsonContext : JsonSerializerContext
{
}

public sealed class EvalDotnetWorldExportsImpl : IEvalDotnetWorldExports
{
    public static string Eval(string expression)
    {
        try
        {
            return Evaluate(expression, new Dictionary<string, double>(StringComparer.Ordinal));
        }
        catch (Exception exception)
        {
            throw new WitException<string>($"{exception.GetType().Name}: {exception.Message}", 0);
        }
    }

    public static string Exec(string statements)
    {
        try
        {
            var variables = new Dictionary<string, double>(StringComparer.Ordinal);
            var output = new List<string>();

            foreach (var statement in statements.Split('\n', StringSplitOptions.RemoveEmptyEntries))
            {
                var trimmed = statement.Trim();
                if (trimmed.StartsWith("print(", StringComparison.Ordinal) && trimmed.EndsWith(')'))
                {
                    output.Add(Evaluate(trimmed[6..^1], variables).Trim('"'));
                    continue;
                }

                var assignment = trimmed.IndexOf('=');
                if (assignment > 0)
                {
                    var name = trimmed[..assignment].Trim();
                    variables[name] = double.Parse(
                        Evaluate(trimmed[(assignment + 1)..], variables),
                        CultureInfo.InvariantCulture);
                    continue;
                }

                throw new FormatException($"Unsupported statement: {trimmed}");
            }

            return string.Join(Environment.NewLine, output);
        }
        catch (Exception exception)
        {
            throw new WitException<string>($"{exception.GetType().Name}: {exception.Message}", 0);
        }
    }

    private static string Evaluate(string expression, Dictionary<string, double> variables)
    {
        var trimmed = expression.Trim();
        if (trimmed.Length >= 2 &&
            ((trimmed[0] == '\'' && trimmed[^1] == '\'') ||
             (trimmed[0] == '"' && trimmed[^1] == '"')))
        {
            return JsonSerializer.Serialize(trimmed[1..^1], EvalJsonContext.Default.String);
        }

        if (variables.TryGetValue(trimmed, out var variable))
        {
            return variable.ToString(CultureInfo.InvariantCulture);
        }

        var parser = new ArithmeticParser(trimmed);
        return parser.Parse().ToString(CultureInfo.InvariantCulture);
    }

    private sealed class ArithmeticParser
    {
        private readonly string _text;
        private int _position;

        public ArithmeticParser(string text) => _text = text;

        public double Parse()
        {
            var value = ParseExpression();
            SkipWhitespace();
            if (_position != _text.Length)
            {
                throw new FormatException($"Unexpected token at position {_position}.");
            }

            return value;
        }

        private double ParseExpression()
        {
            var value = ParseTerm();
            while (true)
            {
                SkipWhitespace();
                if (TryConsume('+'))
                {
                    value += ParseTerm();
                }
                else if (TryConsume('-'))
                {
                    value -= ParseTerm();
                }
                else
                {
                    return value;
                }
            }
        }

        private double ParseTerm()
        {
            var value = ParseFactor();
            while (true)
            {
                SkipWhitespace();
                if (TryConsume('*'))
                {
                    value *= ParseFactor();
                }
                else if (TryConsume('/'))
                {
                    value /= ParseFactor();
                }
                else
                {
                    return value;
                }
            }
        }

        private double ParseFactor()
        {
            SkipWhitespace();
            if (TryConsume('('))
            {
                var nestedValue = ParseExpression();
                if (!TryConsume(')'))
                {
                    throw new FormatException("Missing closing parenthesis.");
                }

                return nestedValue;
            }

            var start = _position;
            while (_position < _text.Length &&
                   (char.IsDigit(_text[_position]) || _text[_position] == '.'))
            {
                _position++;
            }

            if (start == _position ||
                !double.TryParse(
                    _text[start.._position],
                    NumberStyles.Float,
                    CultureInfo.InvariantCulture,
                    out var value))
            {
                throw new FormatException("Expected a number.");
            }

            return value;
        }

        private bool TryConsume(char character)
        {
            if (_position < _text.Length && _text[_position] == character)
            {
                _position++;
                return true;
            }

            return false;
        }

        private void SkipWhitespace()
        {
            while (_position < _text.Length && char.IsWhiteSpace(_text[_position]))
            {
                _position++;
            }
        }
    }
}
