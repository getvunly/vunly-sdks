using System;
using System.Globalization;

namespace Vunly;

/// <summary>
/// Reads a <c>Retry-After</c> header by the one rule every Vunly SDK shares (conformance/README.md,
/// "Reading Retry-After"; the vectors are conformance/retry-after-vectors.json). The RAW header string is read: the framework's
/// own parser rejects values this API's contract treats as very long waits (integers above Int32, <c>1e12</c>, <c>Infinity</c>).
/// </summary>
internal static class RetryAfterHeader
{
    /// <summary>The longest wait the SDK sits through; a longer one is surfaced at once.</summary>
    internal static readonly TimeSpan Cap = TimeSpan.FromSeconds(30);

    /// <summary>The largest value reported in <see cref="VunlyException.RetryAfter"/>: one day.</summary>
    internal const double ReportCapSeconds = 86_400;

    /// <summary>How a header was read.</summary>
    internal enum Kind
    {
        /// <summary>No header, or not a plain number of seconds: the normal backoff applies.</summary>
        None,

        /// <summary>A wait of at most 30 seconds (0 means "no directive": the normal backoff applies).</summary>
        Seconds,

        /// <summary>Above 30 seconds, or too large to represent: raise at once, never retry early.</summary>
        Surface,
    }

    internal static (Kind Kind, TimeSpan? Value) Parse(string? raw)
    {
        if (raw is null)
        {
            return (Kind.None, null);
        }
        var s = raw.Trim(' ', '\t');
        var i = 0;
        if (i < s.Length && s[i] == '+')
        {
            i++;
        }
        double seconds;
        var rest = s.AsSpan(i);
        if (rest.Equals("inf", StringComparison.OrdinalIgnoreCase) || rest.Equals("infinity", StringComparison.OrdinalIgnoreCase))
        {
            seconds = double.PositiveInfinity;
        }
        else
        {
            // digits [ "." digits ] [ ("e" | "E") [ "+" | "-" ] digits ], ASCII digits only ([0-9]: char.IsDigit is Unicode)
            var intStart = i;
            while (i < s.Length && s[i] is >= '0' and <= '9')
            {
                i++;
            }
            var intDigits = i - intStart;
            var fracDigits = 0;
            if (i < s.Length && s[i] == '.')
            {
                i++;
                var fracStart = i;
                while (i < s.Length && s[i] is >= '0' and <= '9')
                {
                    i++;
                }
                fracDigits = i - fracStart;
            }
            if (intDigits + fracDigits == 0)
            {
                return (Kind.None, null);
            }
            if (i < s.Length && (s[i] == 'e' || s[i] == 'E'))
            {
                i++;
                if (i < s.Length && (s[i] == '+' || s[i] == '-'))
                {
                    i++;
                }
                var expStart = i;
                while (i < s.Length && s[i] is >= '0' and <= '9')
                {
                    i++;
                }
                if (i == expStart)
                {
                    return (Kind.None, null);
                }
            }
            if (i != s.Length)
            {
                return (Kind.None, null);
            }
            // double.Parse answers Infinity for an overflow (.NET Core 3.0 and later): an unrepresentable number is a long wait
            if (!double.TryParse(s, NumberStyles.Float, CultureInfo.InvariantCulture, out seconds))
            {
                // only an exponent too large for the parser can get here; a huge negative one is a tiny number, not a long wait
                seconds = s.Contains("e-", StringComparison.OrdinalIgnoreCase) ? 0 : double.PositiveInfinity;
            }
        }
        if (double.IsNaN(seconds) || seconds < 0)
        {
            return (Kind.None, null);
        }
        if (seconds <= Cap.TotalSeconds)
        {
            return (Kind.Seconds, TimeSpan.FromSeconds(seconds));
        }
        return (Kind.Surface, TimeSpan.FromSeconds(Math.Min(seconds, ReportCapSeconds)));
    }
}
