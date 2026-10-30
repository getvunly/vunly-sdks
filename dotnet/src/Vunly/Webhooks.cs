using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace Vunly;

/// <summary>A verified webhook delivery. <see cref="Data"/> is a copy of the event's data and stays valid after the call.</summary>
public sealed record WebhookEvent(string Id, string Type, string? CreatedAt, JsonElement Data);

/// <summary>
/// Thrown by <see cref="Webhooks.Verify"/>. Answer a failed verification with a 400 and do not process the payload.
/// </summary>
public sealed class WebhookException : Exception
{
    internal WebhookException(string reason)
        : base($"vunly: webhook verification failed ({reason})") => Reason = reason;

    /// <summary>"missing_header", "malformed_header", "stale_timestamp", "no_matching_signature" or "invalid_payload".</summary>
    public string Reason { get; }
}

/// <summary>Verifies webhook deliveries.</summary>
public static class Webhooks
{
    /// <summary>How far a signed timestamp may be from now (either way) before it counts as stale.</summary>
    public static readonly TimeSpan Tolerance = TimeSpan.FromSeconds(300);

    /// <summary>The largest payload verified (1 MiB, as for an API answer); a bigger one is <c>invalid_payload</c>.</summary>
    private const int MaxPayload = Transport.MaxBody;

    /// <summary>
    /// Checks a delivery. <paramref name="payload"/> is the RAW request body (the exact bytes, not re-encoded JSON),
    /// <paramref name="header"/> the <c>Vunly-Signature</c> header and <paramref name="secrets"/> the endpoint's signing secrets:
    /// pass two while rotating, a match with any one is enough. The signature is HMAC-SHA256 over <c>"&lt;t&gt;." + payload</c>,
    /// compared in constant time.
    /// </summary>
    /// <param name="now">The clock, for tests; null means the current time.</param>
    public static WebhookEvent Verify(ReadOnlySpan<byte> payload, string? header, IReadOnlyList<string> secrets, DateTimeOffset? now = null)
    {
        // A configuration error, checked before anything about the delivery: an empty or blank secret would make a forged
        // signature verify (HMAC with an empty key is computable by anyone), and an empty list could never verify anything.
        ArgumentNullException.ThrowIfNull(secrets);
        if (secrets.Count == 0)
        {
            throw new ArgumentException("vunly: no webhook signing secret was given (pass the endpoint's secret, or two while rotating)", nameof(secrets));
        }
        foreach (var secret in secrets)
        {
            // a usable secret has at least one printable ASCII character that is not a space (HMAC-SHA256, 300 s tolerance)
            if (secret is null || !secret.Any(c => c >= '!' && c <= '~'))
            {
                throw new ArgumentException("vunly: a webhook signing secret is empty or blank (read it from your secret store)", nameof(secrets));
            }
        }
        if (payload.Length > MaxPayload)
        {
            throw new WebhookException("invalid_payload");
        }
        if (string.IsNullOrWhiteSpace(header))
        {
            throw new WebhookException("missing_header");
        }
        string? ts = null;
        var signatures = new List<string>();
        foreach (var part in header.Split(','))
        {
            var eq = part.IndexOf('=', StringComparison.Ordinal);
            if (eq < 0)
            {
                continue;
            }
            var key = part[..eq].Trim();
            var value = part[(eq + 1)..].Trim();
            if (key == "t")
            {
                ts = value;
            }
            else if (key == "v1" && value.Length == 64 && value.All(c => c is (>= '0' and <= '9') or (>= 'a' and <= 'f')))
            {
                signatures.Add(value); // a v1 that is not 64 hex characters is not a signature: the header is malformed
            }
        }
        if (!long.TryParse(ts, NumberStyles.None, CultureInfo.InvariantCulture, out var t) || signatures.Count == 0)
        {
            throw new WebhookException("malformed_header");
        }
        if (t > 253_402_300_799L)
        {
            throw new WebhookException("stale_timestamp"); // beyond year 9999: not a time (and FromUnixTimeSeconds would throw)
        }
        var skew = (now ?? DateTimeOffset.UtcNow) - DateTimeOffset.FromUnixTimeSeconds(t);
        if (skew.Duration() > Tolerance)
        {
            throw new WebhookException("stale_timestamp");
        }

        var prefix = Encoding.UTF8.GetBytes(ts + ".");
        var matched = false;
        foreach (var secret in secrets)
        {
            // fed in two parts: no second copy of the body is built to prepend the timestamp
            using var hmac = IncrementalHash.CreateHMAC(HashAlgorithmName.SHA256, Encoding.UTF8.GetBytes(secret));
            hmac.AppendData(prefix);
            hmac.AppendData(payload);
            var want = hmac.GetHashAndReset();
            foreach (var s in signatures)
            {
                byte[] got;
                try
                {
                    got = Convert.FromHexString(s);
                }
                catch (FormatException)
                {
                    continue;
                }
                if (CryptographicOperations.FixedTimeEquals(got, want))
                {
                    matched = true;
                }
            }
        }
        if (!matched)
        {
            throw new WebhookException("no_matching_signature");
        }
        try
        {
            // the nesting is bounded (a body of a million "[" must end in an SDK error, not in the stack or the heap)
            using var doc = JsonDocument.Parse(payload.ToArray(), Transport.JsonLimits);
            var f = Fields.Of(doc.RootElement);
            var data = doc.RootElement.TryGetProperty("data", out var d) ? d.Clone() : default;
            return new WebhookEvent(f.Str("id"), f.Str("type"), f.StrOrNull("createdAt"), data);
        }
        catch (Exception e) when (e is JsonException or FormatException or InvalidOperationException)
        {
            throw new WebhookException("invalid_payload");
        }
    }
}
