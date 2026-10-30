using System;
using System.Globalization;
using System.Text;

namespace Vunly;

/// <summary>
/// Makes text that came from the network safe to put in an exception message, a log line or a terminal: control characters
/// (LF, CR, ESC, DEL, the C1 range), line and paragraph separators and format characters (bidirectional controls,
/// zero-width characters, the byte order mark) become spaces, and the length is bounded.
/// </summary>
internal static class Sanitize
{
    internal const int MaxMessage = 500;
    internal const int MaxRequestId = 128;
    internal const int MaxCode = 64;

    internal static string? Text(string? value, int max)
    {
        if (value is null)
        {
            return null;
        }
        var sb = new StringBuilder(Math.Min(value.Length, max));
        for (var i = 0; i < value.Length && sb.Length < max; i++)
        {
            var c = value[i];
            if (char.IsHighSurrogate(c) && i + 1 < value.Length && char.IsLowSurrogate(value[i + 1]))
            {
                if (sb.Length + 2 > max)
                {
                    break; // never split a pair
                }
                if (CharUnicodeInfo.GetUnicodeCategory(value, i) == UnicodeCategory.Format)
                {
                    sb.Append(' ').Append(' '); // a tag character: two code units, kept the same width
                }
                else
                {
                    sb.Append(c).Append(value[i + 1]);
                }
                i++;
                continue;
            }
            sb.Append(Unsafe(c) ? ' ' : c);
        }
        return sb.ToString();
    }

    private static bool Unsafe(char c) =>
        char.IsSurrogate(c) // a lone half
        || char.IsControl(c) // C0, DEL and C1
        || CharUnicodeInfo.GetUnicodeCategory(c) is UnicodeCategory.Format or UnicodeCategory.LineSeparator or UnicodeCategory.ParagraphSeparator;
}
