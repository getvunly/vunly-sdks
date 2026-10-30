using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.Json;
using Xunit;

namespace Vunly.Tests;

/// <summary>
/// The shared webhook vectors (webhook-vectors.json, an identical copy in every SDK). The signatures were computed outside any
/// SDK. "expect" is "ok", a WebhookException reason, or "config" (an ArgumentException raised before the signature is looked at).
/// </summary>
public class WebhookVectorsTests
{
    private static JsonDocument Load() =>
        JsonDocument.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "webhook-vectors.json")));

    public static IEnumerable<object[]> Names()
    {
        using var doc = Load();
        var i = 0;
        foreach (var v in doc.RootElement.GetProperty("vectors").EnumerateArray())
        {
            yield return new object[] { i++, v.GetProperty("name").GetString()! };
        }
    }

    [Fact]
    public void ThereAreEnoughVectors()
    {
        using var doc = Load();
        Assert.True(doc.RootElement.GetProperty("vectors").GetArrayLength() >= 20);
    }

    [Theory]
    [MemberData(nameof(Names))]
    public void EveryVectorGetsItsAnswer(int index, string name)
    {
        using var doc = Load();
        var now = DateTimeOffset.FromUnixTimeSeconds(doc.RootElement.GetProperty("now").GetInt64());
        var v = doc.RootElement.GetProperty("vectors")[index];
        Assert.Equal(name, v.GetProperty("name").GetString());
        var header = v.GetProperty("header").ValueKind == JsonValueKind.Null ? null : v.GetProperty("header").GetString();
        var secrets = v.GetProperty("secrets").EnumerateArray().Select(s => s.GetString()!).ToList();
        var payload = Encoding.UTF8.GetBytes(v.GetProperty("payload").GetString()!);
        var got = "ok";
        try
        {
            Webhooks.Verify(payload, header, secrets, now);
        }
        catch (WebhookException e)
        {
            got = e.Reason;
        }
        catch (ArgumentException)
        {
            got = "config";
        }
        Assert.Equal(v.GetProperty("expect").GetString(), got);
    }
}
