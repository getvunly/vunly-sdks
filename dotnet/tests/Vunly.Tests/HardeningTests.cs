using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Net;
using System.Net.Http;
using System.Net.Sockets;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Options;
using Xunit;

namespace Vunly.Tests;

/// <summary>Regression tests for hardening cases: each test pins one of them.</summary>
public class HardeningTests
{
    private const string Balance = "{\"credits\":5,\"consumed\":1,\"granted\":6}";
    /// <summary>C1 next-line, bidirectional override, zero-width space, line separator, paragraph separator, byte order mark.</summary>
    private static readonly char[] Bad = { (char)0x85, (char)0x202E, (char)0x200B, (char)0x2028, (char)0x2029, (char)0xFEFF };

    private const string Err429 = "{\"error\":{\"code\":\"rate_limited\",\"message\":\"slow\"}}";

    // ---- M1, L1: the idempotency id ---------------------------------------------------------------------------------------

    [Fact]
    public async Task ANullIdOnAKeyedCallIsRefusedBeforeAnyRequest()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{}"));
        using var v = s.Client();
        var record = await Assert.ThrowsAsync<InvalidRequestException>(() => v.Usage.RecordAsync(null!, "acme", "tokens", 1));
        var grant = await Assert.ThrowsAsync<InvalidRequestException>(() => v.Credits.GrantAsync(null!, "acme", 5));
        foreach (var e in new[] { record, grant })
        {
            Assert.Equal(400, e.Status);
            Assert.Equal("validation_error", e.Code);
            var fields = Assert.IsAssignableFrom<IReadOnlyDictionary<string, object?>>(e.Details["fields"]);
            Assert.True(fields.ContainsKey("id"));
        }
        Assert.Equal(0, s.Hits);
    }

    [Theory]
    [InlineData("job-12345678\n")]
    [InlineData("job-12345678\r\n")]
    [InlineData("\njob-12345678")]
    [InlineData("job 12345678")]
    public async Task AnIdWithATrailingNewlineOrAnyOtherStrayCharacterIsRefused(string id)
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{}"));
        await Assert.ThrowsAsync<InvalidRequestException>(() => s.Client().Usage.RecordAsync(id, "acme", "tokens", 1));
        Assert.Equal(0, s.Hits);
    }

    [Theory]
    [InlineData(503)]
    [InlineData(429)]
    public async Task EveryRetryOfAKeyedCallSendsTheSameIdempotencyKeyAndBody(int failure)
    {
        var keys = new List<string?>();
        var bodies = new List<string>();
        var n = 0;
        using var s = new TestServer(async ctx =>
        {
            var body = await TestServer.Body(ctx);
            lock (keys)
            {
                keys.Add(ctx.Request.Headers["Idempotency-Key"]);
                bodies.Add(body);
            }
            if (Interlocked.Increment(ref n) < 3)
            {
                await TestServer.Json(ctx, failure, "{\"error\":{\"code\":\"x\",\"message\":\"later\"}}");
            }
            else
            {
                await TestServer.Json(ctx, 200, "{\"event\":" + TestServer.Entry + ",\"balance\":97}");
            }
        });
        var r = await s.Client().Usage.RecordAsync("job-12345678", "acme", "tokens", 3000);
        Assert.Equal(97, r.Remaining);
        Assert.Equal(3, keys.Count);
        Assert.All(keys, k => Assert.Equal("job-12345678", k));
        Assert.Single(bodies.Distinct());
    }

    // ---- M2: Pages.All ----------------------------------------------------------------------------------------------------

    [Fact]
    public async Task APagerWhoseCursorDoesNotAdvanceFailsInsteadOfLooping()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"data\":[" + TestServer.Entry + "],\"nextCursor\":\"same\"}"));
        using var v = s.Client();
        var items = 0;
        using var stop = new CancellationTokenSource(TimeSpan.FromSeconds(10)); // a missing guard must fail the test, not hang it
        var e = await Assert.ThrowsAsync<UnavailableException>(async () =>
        {
            await foreach (var _ in v.Credits.GetLedgerAllAsync("acme", 1, stop.Token))
            {
                items++;
            }
        });
        Assert.Equal("invalid_response", e.Code);
        Assert.True(s.Hits <= 3, $"{s.Hits} requests");
        Assert.True(items <= 3);
    }

    [Fact]
    public async Task ThreeEmptyPagesInARowThatEachPromiseAnotherFailInsteadOfLooping()
    {
        var n = 0;
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"data\":[],\"nextCursor\":\"c" + Interlocked.Increment(ref n) + "\"}"));
        using var v = s.Client();
        using var stop = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        var e = await Assert.ThrowsAsync<UnavailableException>(async () =>
        {
            await foreach (var _ in v.Credits.GetLedgerAllAsync("acme", 1, stop.Token))
            {
            }
        });
        Assert.Equal("invalid_response", e.Code);
        Assert.Equal(3, s.Hits);
    }

    [Fact]
    public async Task TwoEmptyPagesThenDataIsStillFine()
    {
        var n = 0;
        using var s = new TestServer(ctx =>
        {
            var i = Interlocked.Increment(ref n);
            return TestServer.Json(ctx, 200, i < 3 ? "{\"data\":[],\"nextCursor\":\"c" + i + "\"}" : "{\"data\":[" + TestServer.Entry + "],\"nextCursor\":null}");
        });
        using var v = s.Client();
        var count = 0;
        await foreach (var _ in v.Credits.GetLedgerAllAsync("acme", 1))
        {
            count++;
        }
        Assert.Equal(1, count);
    }

    // ---- M3: Retry-After --------------------------------------------------------------------------------------------------

    public static IEnumerable<object[]> Vectors()
    {
        using var doc = JsonDocument.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "retry-after-vectors.json")));
        foreach (var v in doc.RootElement.GetProperty("vectors").EnumerateArray())
        {
            yield return new object[]
            {
                v.GetProperty("header").GetString()!,
                v.GetProperty("kind").GetString()!,
                v.TryGetProperty("seconds", out var sec) ? sec.GetDouble() : -1d,
            };
        }
    }

    [Fact]
    public void TheVectorFileIsTheSharedOne()
    {
        using var doc = JsonDocument.Parse(File.ReadAllText(Path.Combine(AppContext.BaseDirectory, "retry-after-vectors.json")));
        Assert.Equal(30, doc.RootElement.GetProperty("capSeconds").GetInt32());
        Assert.Equal(86400, doc.RootElement.GetProperty("reportCapSeconds").GetInt32());
        Assert.True(doc.RootElement.GetProperty("vectors").GetArrayLength() >= 40);
    }

    [Theory]
    [MemberData(nameof(Vectors))]
    public void EveryRetryAfterVectorIsReadAsTheContractSays(string header, string kind, double seconds)
    {
        var (got, value) = RetryAfterHeader.Parse(header);
        switch (kind)
        {
            case "seconds":
                Assert.Equal(RetryAfterHeader.Kind.Seconds, got);
                Assert.Equal(seconds, value!.Value.TotalSeconds, 6);
                break;
            case "surface":
                Assert.Equal(RetryAfterHeader.Kind.Surface, got);
                Assert.Equal(seconds, value!.Value.TotalSeconds, 6);
                break;
            default:
                Assert.Equal(RetryAfterHeader.Kind.None, got);
                Assert.Null(value);
                break;
        }
    }

    [Theory]
    [InlineData("99999999999")] // above Int32: the framework's own parser rejects it
    [InlineData("1e12")]
    [InlineData("Infinity")]
    [InlineData("1e999")]
    [InlineData("+120")]
    public async Task ALongOrUnrepresentableRetryAfterIsSurfacedAtOnceAndNeverRetriedEarly(string header)
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 429, Err429, ("Retry-After", header)));
        var waits = new List<TimeSpan>();
        using var v = Recording(s, waits);
        var e = await Assert.ThrowsAsync<UnavailableException>(() => v.Credits.GetBalanceAsync("acme"));
        Assert.Equal(1, s.Hits);
        Assert.Empty(waits);
        Assert.True(e.RetryAfter >= TimeSpan.FromSeconds(120) && e.RetryAfter <= TimeSpan.FromDays(1), $"{e.RetryAfter}");
    }

    [Theory]
    [InlineData("0x10")]
    [InlineData("5abc")]
    [InlineData("-1")]
    [InlineData("NaN")]
    [InlineData("")]
    [InlineData("0")]
    public async Task ARetryAfterThatIsNotADirectiveMeansTheNormalBackoff(string header)
    {
        var n = 0;
        using var s = new TestServer(ctx => Interlocked.Increment(ref n) == 1
            ? TestServer.Json(ctx, 429, Err429, ("Retry-After", header))
            : TestServer.Json(ctx, 200, Balance));
        var waits = new List<TimeSpan>();
        using var v = Recording(s, waits);
        Assert.Equal(5, (await v.Credits.GetBalanceAsync("acme")).Remaining);
        Assert.Equal(2, s.Hits);
        var wait = Assert.Single(waits);
        Assert.InRange(wait.TotalMilliseconds, 125, 250); // retry 1 of the jittered backoff, not a number from the header
    }

    [Fact]
    public async Task ARetryAfterWithinTheCapIsTheWait()
    {
        var n = 0;
        using var s = new TestServer(ctx => Interlocked.Increment(ref n) == 1
            ? TestServer.Json(ctx, 429, Err429, ("Retry-After", "1.5"))
            : TestServer.Json(ctx, 200, Balance));
        var waits = new List<TimeSpan>();
        using var v = Recording(s, waits);
        await v.Credits.GetBalanceAsync("acme");
        Assert.Equal(TimeSpan.FromSeconds(1.5), Assert.Single(waits));
    }

    private static VunlyClient Recording(TestServer s, List<TimeSpan> waits) =>
        new(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = s.Url }, null, (d, _) =>
        {
            waits.Add(d);
            return Task.CompletedTask;
        });

    // ---- M5: redirects ----------------------------------------------------------------------------------------------------

    [Theory]
    [InlineData(301)]
    [InlineData(302)]
    [InlineData(307)]
    [InlineData(308)]
    public async Task ARedirectIsUnavailableUnexpectedRedirectAndNotRetried(int status)
    {
        using var s = new TestServer(ctx =>
        {
            ctx.Response.StatusCode = status;
            ctx.Response.Headers["Location"] = "https://elsewhere.example/x";
            return Task.CompletedTask;
        });
        var e = await Assert.ThrowsAsync<UnavailableException>(() => s.Client().Credits.GetBalanceAsync("acme"));
        Assert.Equal("unexpected_redirect", e.Code);
        Assert.Equal(status, e.Status);
        Assert.Equal(1, s.Hits);
        Assert.DoesNotContain("elsewhere", e.Message);
    }

    // ---- M6: what the server sends is sanitised and bounded ---------------------------------------------------------------

    [Fact]
    public async Task ServerTextIsSanitisedAndBoundedInTheException()
    {
        var hostile = "line1\nline2\rEND\u001b[31mred\u007f\u0085‮RTL" + string.Concat(Bad) + "tail " + new string('x', 20_000);
        var body = JsonSerializer.Serialize(new { error = new { code = "bad\ncode" + new string('c', 500), message = hostile, requestId = "req\n" + new string('r', 1000) } });
        using var s = new TestServer(ctx => TestServer.Json(ctx, 400, body));
        var e = await Assert.ThrowsAsync<InvalidRequestException>(() => s.Client().Customers.GetAsync("acme"));
        foreach (var text in new[] { e.Message, e.Code, e.RequestId! })
        {
            Assert.DoesNotContain('\n', text);
            Assert.DoesNotContain('\r', text);
            Assert.DoesNotContain('\u001b', text);
            Assert.DoesNotContain('\u007f', text);
            Assert.DoesNotContain('\u0085', text);
            Assert.DoesNotContain('‮', text);
            Assert.DoesNotContain('​', text);
            foreach (var c in Bad)
            {
                Assert.DoesNotContain(c, text);
            }
        }
        Assert.True(e.Code.Length <= 64, $"{e.Code.Length}");
        Assert.True(e.RequestId!.Length <= 128, $"{e.RequestId.Length}");
        Assert.True(e.Message.Length <= 500 + 64 + 128 + 40, $"{e.Message.Length}");
        Assert.Contains("line1 line2 END", e.Message);
    }

    [Fact]
    public async Task AFiveMegabyteErrorMessageNeitherReachesTheExceptionNorTheMemory()
    {
        var body = "{\"error\":{\"code\":\"x\",\"message\":\"" + new string('m', 5 << 20) + "\"}}";
        using var s = new TestServer(ctx => TestServer.Json(ctx, 500, body));
        var e = await Assert.ThrowsAsync<UnavailableException>(() => s.Client(o => o.MaxRetries = 0).Customers.GetAsync("acme"));
        Assert.True(e.Message.Length < 700, $"{e.Message.Length}");
        Assert.Equal(500, e.Status);
    }

    [Fact]
    public void TheExceptionConstructorSanitisesWhateverItIsGiven()
    {
        var e = new UnavailableException("c\u001bode", 503, "a\nb" + (char)0x202E + "c", "r\nid", null, null);
        Assert.Equal("c ode", e.Code);
        Assert.Equal("r id", e.RequestId);
        Assert.DoesNotContain('\n', e.Message);
        Assert.DoesNotContain((char)0x202E, e.Message);
    }

    // ---- L2: a caller-supplied HttpClient that follows redirects -----------------------------------------------------------

    [Fact]
    public async Task AnAnswerFromAnotherHostAfterAFollowedRedirectIsRefused()
    {
        using var target = new TestServer(ctx => TestServer.Json(ctx, 200, Balance));
        using var s = new TestServer(ctx =>
        {
            ctx.Response.StatusCode = 302;
            ctx.Response.Headers["Location"] = target.Url + "/v1/customers/acme/balance";
            return Task.CompletedTask;
        });
        // 127.0.0.1 on two ports are two origins
        using var http = new HttpClient(new SocketsHttpHandler { AllowAutoRedirect = true });
        using var v = new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = s.Url, MaxRetries = 0 }, http);
        var e = await Assert.ThrowsAsync<UnavailableException>(() => v.Credits.GetBalanceAsync("acme"));
        Assert.Equal("unexpected_redirect", e.Code);
    }

    [Fact]
    public async Task AFollowedRedirectIsNeverRetriedWhateverTheOtherHostAnswered()
    {
        using var target = new TestServer(ctx => TestServer.Json(ctx, 503, "{\"error\":{\"code\":\"unavailable\",\"message\":\"later\"}}"));
        using var s = new TestServer(ctx =>
        {
            ctx.Response.StatusCode = 307;
            ctx.Response.Headers["Location"] = target.Url + "/v1/customers/acme/balance";
            return Task.CompletedTask;
        });
        using var http = new HttpClient(new SocketsHttpHandler { AllowAutoRedirect = true });
        using var v = new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = s.Url }, http, (_, _) => Task.CompletedTask); // retries ON
        var e = await Assert.ThrowsAsync<UnavailableException>(() => v.Credits.GetBalanceAsync("acme"));
        Assert.Equal("unexpected_redirect", e.Code);
        Assert.Equal(1, s.Hits);
        Assert.Equal(1, target.Hits);
    }

    // ---- JSON amplification: a valid body of many tiny nested values ---------------------------------------------------------

    /// <summary>A body of about <paramref name="size"/> bytes made of <paramref name="unit"/> repeated inside <paramref name="head"/> and <paramref name="tail"/>.</summary>
    private static string Amplifier(string head, string unit, string tail, int size)
    {
        var sb = new StringBuilder(head, size + 64);
        while (sb.Length < size)
        {
            sb.Append(unit).Append(',');
        }
        sb.Length--;
        return sb.Append(tail).ToString();
    }

    public static IEnumerable<object[]> AmplifyingBodies()
    {
        // (the bodies are built inside the test, one at a time: 8 of them held as test data would be 200 MB of strings)
        foreach (var size in new[] { 10 << 20, 16 << 20 })
        {
            foreach (var what in new[] { "tiny arrays", "tiny objects", "empty list elements", "nested objects" })
            {
                yield return new object[] { what, size };
            }
        }
    }

    /// <summary>The body as bytes, filled in place (no string of 16 MB, which is 32 MB of UTF-16 plus copies, inside the test).</summary>
    private static byte[] Repeated(string head, string unit, string tail, int size)
    {
        var h = Encoding.ASCII.GetBytes(head);
        var u = Encoding.ASCII.GetBytes(unit + ",");
        var t = Encoding.ASCII.GetBytes(tail);
        var n = (size - h.Length - t.Length) / u.Length;
        var all = new byte[h.Length + (n * u.Length) - 1 + t.Length];
        h.CopyTo(all, 0);
        for (var i = 0; i < n; i++)
        {
            u.CopyTo(all, h.Length + (i * u.Length));
        }
        t.CopyTo(all, h.Length + (n * u.Length) - 1); // overwrites the last comma
        return all;
    }

    private static byte[] AmplifyingBody(string what, int size)
    {
        switch (what)
        {
            case "tiny arrays":
                return Repeated("{\"credits\":5,\"consumed\":1,\"granted\":6,\"x\":[", "[1]", "]}", size);
            case "tiny objects":
                return Repeated("{\"credits\":5,\"consumed\":1,\"granted\":6,\"x\":[", "{\"a\":1}", "]}", size);
            case "empty list elements":
                return Repeated("{\"data\":[", "{}", "],\"nextCursor\":null}", size);
            default:
                // 16 MB of {"a":{"a":{"a": ... never closed: nested objects (the parser stops at its depth limit anyway)
                var unit = Encoding.ASCII.GetBytes("{\"a\":");
                var open = new byte[size];
                for (var i = 0; i + unit.Length <= open.Length; i += unit.Length)
                {
                    unit.CopyTo(open, i);
                }
                return open;
        }
    }

    private static async Task RawJson(HttpListenerContext ctx, int status, byte[] bytes)
    {
        ctx.Response.StatusCode = status;
        ctx.Response.ContentType = "application/json";
        ctx.Response.ContentLength64 = bytes.Length;
        try
        {
            await ctx.Response.OutputStream.WriteAsync(bytes).ConfigureAwait(false);
        }
        catch (Exception)
        {
            // the client stopped reading at its cap: that is the point
        }
    }

    /// <summary>
    /// 10 and 16 MB bodies of values that amplify in a JSON parser. They exceeded a 256 MB container before the 2xx cap was
    /// 1 MiB (OutOfMemoryException). The whole test project also runs under a 256 MB heap limit (runtimeconfig.template.json).
    /// </summary>
    [Theory]
    [MemberData(nameof(AmplifyingBodies))]
    public async Task AnAmplifyingBodyOverTheCapIsInvalidResponseWithoutHoldingItInMemory(string what, int size)
    {
        var body = AmplifyingBody(what, size);
        using var s = new TestServer(ctx => RawJson(ctx, 200, body));
        using var v = s.Client();
        var length = body.Length;
        var before = GC.GetTotalAllocatedBytes(true);
        var e = await Assert.ThrowsAsync<UnavailableException>(() => v.Credits.GetBalanceAsync("acme"));
        var allocated = GC.GetTotalAllocatedBytes(true) - before;
        Assert.Equal("invalid_response", e.Code);
        Assert.Equal(1, s.Hits);
        Assert.True(allocated < 40L * 1024 * 1024, $"{what}: {allocated / (1024 * 1024)} MB allocated for a {length >> 20} MB body");
    }

    [Theory]
    [InlineData("[1]")]
    [InlineData("{\"a\":1}")]
    [InlineData("[[],[]]")]
    [InlineData("{}")]
    public async Task AnAmplifyingBodyJustUnderTheCapIsBoundedToo(string unit)
    {
        var body = Amplifier("{\"credits\":5,\"consumed\":1,\"granted\":6,\"data\":[", unit, "]}", (1 << 20) - 1000);
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, body));
        using var v = s.Client();
        var before = GC.GetTotalAllocatedBytes(true);
        Assert.Equal(5, (await v.Credits.GetBalanceAsync("acme")).Remaining); // the unknown field is never read
        // the list shape through a call that reads "data": every element is an object without the promised fields
        using var l = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"data\":[" + string.Join(",", Enumerable.Repeat("{}", (1 << 20) / 3 - 10)) + "],\"nextCursor\":null}"));
        await Assert.ThrowsAsync<UnavailableException>(() => l.Client().Credits.GetLedgerAsync("acme"));
        Assert.True(GC.GetTotalAllocatedBytes(true) - before < 100L * 1024 * 1024);
    }

    [Fact]
    public async Task AnErrorBodyIsNotBufferedBeyondItsCap()
    {
        var body = Repeated("{\"error\":{\"code\":\"x\",\"message\":\"m\",\"details\":[", "[1]", "]}}", 16 << 20);
        using var s = new TestServer(ctx => RawJson(ctx, 409, body));
        var before = GC.GetTotalAllocatedBytes(true);
        var ex = await Assert.ThrowsAsync<InvalidRequestException>(() => s.Client().Customers.GetAsync("acme"));
        Assert.Equal(409, ex.Status);
        Assert.True(GC.GetTotalAllocatedBytes(true) - before < 40L * 1024 * 1024);
    }

    // ---- hostile response heads -------------------------------------------------------------------------------------------

    [Theory]
    [InlineData(1, 1 << 20)] // one 1 MiB header
    [InlineData(300, 1024)] // 300 headers of 1 KB
    public async Task AnOversizedResponseHeadEndsUnavailableAndIsNotRetriedForever(int count, int size)
    {
        // a valid answer whose head is far over 64 KB: written raw, so the listener's own limits do not get in the way
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        var requests = 0;
        var accept = Task.Run(async () =>
        {
            try
            {
                while (true)
                {
                    var c = await listener.AcceptTcpClientAsync();
                    _ = Task.Run(async () =>
                    {
                        using (c)
                        {
                            try
                            {
                                var stream = c.GetStream();
                                var buf = new byte[4096];
                                var read = 0;
                                var all = new StringBuilder();
                                while (!all.ToString().Contains("\r\n\r\n", StringComparison.Ordinal))
                                {
                                    read = await stream.ReadAsync(buf);
                                    if (read == 0)
                                    {
                                        return;
                                    }
                                    all.Append(Encoding.ASCII.GetString(buf, 0, read));
                                }
                                Interlocked.Increment(ref requests);
                                var head = new StringBuilder("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 38\r\n");
                                for (var i = 0; i < count; i++)
                                {
                                    head.Append("X-Pad-").Append(i).Append(": ").Append('a', size).Append("\r\n");
                                }
                                head.Append("\r\n{\"credits\":5,\"consumed\":1,\"granted\":6}");
                                await stream.WriteAsync(Encoding.ASCII.GetBytes(head.ToString()));
                                await stream.FlushAsync();
                                await Task.Delay(200);
                            }
                            catch (Exception)
                            {
                                // the client hung up on the oversized head: that is the point
                            }
                        }
                    });
                }
            }
            catch (Exception)
            {
                // listener stopped by the test
            }
        });
        try
        {
            using var v = new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = $"http://127.0.0.1:{port}", MaxRetries = 0, Timeout = TimeSpan.FromSeconds(5) });
            var e = await Assert.ThrowsAsync<UnavailableException>(() => v.Credits.GetBalanceAsync("acme"));
            Assert.Equal("unavailable", e.Code);
            Assert.Equal(1, Volatile.Read(ref requests));
        }
        finally
        {
            listener.Stop();
            await accept;
        }
    }

    // ---- a string that is not text ----------------------------------------------------------------------------------------

    [Fact]
    public async Task ALoneSurrogateInARequiredStringIsInvalidResponseNotAnInvalidOperationException()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{\"id\":\"\\ud800\",\"externalId\":\"x\",\"createdAt\":\"2026-01-01T00:00:00Z\"}"));
        var e = await Assert.ThrowsAsync<UnavailableException>(() => s.Client(o => o.MaxRetries = 0).Customers.GetAsync("acme"));
        Assert.Equal("invalid_response", e.Code);
    }

    [Fact]
    public async Task ALoneSurrogateInAnErrorBodyStillClassifiesByStatus()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 409, "{\"error\":{\"code\":\"\\ud800\",\"message\":\"m\"}}"));
        var e = await Assert.ThrowsAsync<InvalidRequestException>(() => s.Client().Customers.GetAsync("acme"));
        Assert.Equal(409, e.Status);
    }

    [Fact]
    public void ALoneSurrogateInAWebhookIsInvalidPayload()
    {
        const long now = 1_800_000_000L;
        var payload = Encoding.UTF8.GetBytes("{\"id\":\"\\ud800\",\"type\":\"t\"}");
        var e = Assert.Throws<WebhookException>(() => Webhooks.Verify(payload, Sign("s", now, payload), new[] { "s" }, DateTimeOffset.FromUnixTimeSeconds(now)));
        Assert.Equal("invalid_payload", e.Reason);
    }

    // ---- path segments that change the route ------------------------------------------------------------------------------

    [Theory]
    [InlineData("")]
    [InlineData(".")]
    [InlineData("..")]
    public async Task AnEmptyDotOrDotDotIdIsRefusedBeforeAnyRequest(string id)
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, "{}"));
        using var v = s.Client();
        var calls = new Func<Task>[]
        {
            () => v.Customers.GetAsync(id),
            () => v.Credits.GetBalanceAsync(id),
            () => v.Credits.GetLedgerAsync(id),
            () => v.Credits.GrantAsync("grant-12345678", id, 5),
            () => v.Plans.SubscribeAsync(id, "starter"),
            () => v.Plans.CancelAsync(id),
            () => v.CustomerTokens.RevokeAsync(id),
        };
        foreach (var call in calls)
        {
            var e = await Assert.ThrowsAsync<InvalidRequestException>(call);
            Assert.Equal(400, e.Status);
            Assert.True(e.Details.ContainsKey("fields"));
        }
        Assert.Equal(0, s.Hits);
    }

    [Fact]
    public async Task APercentEncodedDotsAreEscapedNotDecidedOnTheWire()
    {
        var raw = new List<string?>();
        using var s = new TestServer(ctx =>
        {
            lock (raw)
            {
                raw.Add(ctx.Request.RawUrl);
            }
            return TestServer.Json(ctx, 200, Balance);
        });
        await s.Client().Credits.GetBalanceAsync("%2e%2e");
        await s.Client().Credits.GetBalanceAsync("a.b");
        Assert.Equal(new[] { "/v1/customers/%252e%252e/balance", "/v1/customers/a.b/balance" }, raw);
    }

    // ---- L3, L4, L6: settings ---------------------------------------------------------------------------------------------

    [Fact]
    public async Task AFinalLineBreakOfTheKeyIsDroppedAndNothingElseIsTrimmed()
    {
        foreach (var suffix in new[] { "\n", "\r\n", "" })
        {
            string? auth = null;
            using var s = new TestServer(ctx =>
            {
                auth = ctx.Request.Headers["Authorization"];
                return TestServer.Json(ctx, 200, Balance);
            });
            await s.Client(o => o.ApiKey = TestServer.Key + suffix).Credits.GetBalanceAsync("acme");
            Assert.Equal("Bearer " + TestServer.Key, auth);
        }
    }

    [Theory]
    [InlineData(" KEY")]
    [InlineData("KEY ")]
    [InlineData("\tKEY")]
    [InlineData("KEY\t")]
    [InlineData("KEY\r")]
    [InlineData("\rKEY")]
    [InlineData("\nKEY")]
    [InlineData("KEY\0")]
    [InlineData("\0KEY")]
    [InlineData("KEY\n\n")]
    [InlineData("KEY \n")]
    public void AKeyWithSpacesTabsNulOrALoneCrAtEitherEndIsRefused(string template)
    {
        var key = template.Replace("KEY", "vk_test_synthetic_000000000000", StringComparison.Ordinal);
        var e = Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = key, BaseUrl = "http://127.0.0.1:1" }));
        Assert.Contains("ApiKey", e.Message);
        Assert.DoesNotContain("synthetic", e.Message);
    }

    [Theory]
    [InlineData("vk_test_syn\nthetic_0000000000")]
    [InlineData("vk_test_syn thetic_0000000000")]
    [InlineData("vk_test_syn\u0000thetic_000000000")]
    [InlineData("vk_test_synthetic_é00000000000")]
    public void AKeyWithControlOrNonAsciiCharactersIsRefusedNamingTheSettingNotTheValue(string key)
    {
        var e = Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = key, BaseUrl = "http://127.0.0.1:1" }));
        Assert.Contains("ApiKey", e.Message);
        Assert.DoesNotContain("vk_test", e.Message);
        Assert.DoesNotContain("thetic", e.Message);
    }

    [Theory]
    [InlineData("https://user:secretpw@api.example.test")]
    [InlineData("https://:secretpw@api.example.test/v1")]
    [InlineData("https://api.example.test?token=secretpw")]
    [InlineData("https://api.example.test/#secretpw")]
    [InlineData("https://api.example.test/v1?x=1")]
    public void ABaseUrlWithCredentialsQueryOrFragmentIsRefusedAndNeverPrinted(string url)
    {
        var e = Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = url }));
        Assert.Contains("BaseUrl", e.Message);
        Assert.DoesNotContain("secretpw", e.Message);
        Assert.DoesNotContain("secretpw", e.ToString());
    }

    [Fact]
    public void ToStringShowsTheAddressOnly()
    {
        using var v = new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = "https://api.example.test/v1/" });
        Assert.Equal("VunlyClient { BaseUrl = https://api.example.test, ApiKey = <hidden> }", v.ToString());
    }

    [Theory]
    [InlineData(0)]
    [InlineData(-1)]
    [InlineData(-1000)] // Timeout.InfiniteTimeSpan is -1 ms
    [InlineData(86_400_000)]
    [InlineData(601_000)]
    public void AbsurdTimeoutsAreRefused(int ms)
    {
        var e = Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions
        {
            ApiKey = TestServer.Key,
            BaseUrl = "http://127.0.0.1:1",
            Timeout = TimeSpan.FromMilliseconds(ms),
        }));
        Assert.Contains("Timeout", e.Message);
        Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = "http://127.0.0.1:1", Timeout = System.Threading.Timeout.InfiniteTimeSpan }));
        Assert.Throws<ArgumentException>(() => new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = "http://127.0.0.1:1", MaxRetries = 11 }));
    }

    [Theory]
    [InlineData("ftp://host")]
    [InlineData("https://user:pw@host")]
    [InlineData("not a url")]
    [InlineData("https://host/?q=1")]
    public void StartupValidationRefusesABadBaseUrlWithoutPrintingIt(string url)
    {
        var services = new ServiceCollection();
        services.AddVunly(o =>
        {
            o.ApiKey = TestServer.Key;
            o.BaseUrl = url;
        });
        using var provider = services.BuildServiceProvider();
        var e = Assert.Throws<OptionsValidationException>(() => provider.GetRequiredService<IStartupValidator>().Validate());
        var text = string.Join(" ", e.Failures);
        Assert.Contains("BaseUrl", text);
        Assert.DoesNotContain("pw", text);
        Assert.DoesNotContain(TestServer.Key, text);
    }

    [Fact]
    public void StartupValidationRefusesAKeyWithAControlCharacter()
    {
        var services = new ServiceCollection();
        services.AddVunly(o =>
        {
            o.ApiKey = "vk_test_synthetic\n0000000000";
            o.BaseUrl = "http://127.0.0.1:1";
        });
        using var provider = services.BuildServiceProvider();
        var e = Assert.Throws<OptionsValidationException>(() => provider.GetRequiredService<IStartupValidator>().Validate());
        Assert.Contains("ApiKey", string.Join(" ", e.Failures));
    }

    // ---- L5: MaxRetries = 0 means one request on the wire ------------------------------------------------------------------

    /// <summary>
    /// A raw server answers the first request with keep-alive and then drops every request it reads. The handler repeats an
    /// idempotent GET by itself when a pooled connection turns out to be dead; with MaxRetries = 0 that must not happen.
    /// </summary>
    [Fact]
    public async Task MaxRetriesZeroSendsExactlyOneGetWhenTheServerClosesBeforeAnswering()
    {
        var requests = 0;
        var listener = new TcpListener(IPAddress.Loopback, 0);
        listener.Start();
        var port = ((IPEndPoint)listener.LocalEndpoint).Port;
        var accept = Task.Run(async () =>
        {
            try
            {
                while (true)
                {
                    var c = await listener.AcceptTcpClientAsync();
                    _ = Task.Run(() => ServeFirstThenDrop(c, () => Interlocked.Increment(ref requests)));
                }
            }
            catch (Exception)
            {
                // listener stopped by the test
            }
        });
        try
        {
            using var v = new VunlyClient(new VunlyClientOptions { ApiKey = TestServer.Key, BaseUrl = $"http://127.0.0.1:{port}", MaxRetries = 0 });
            Assert.Equal(1, (await v.Credits.GetBalanceAsync("acme")).Remaining);
            await Assert.ThrowsAsync<UnavailableException>(() => v.Credits.GetBalanceAsync("acme"));
            Assert.Equal(2, Volatile.Read(ref requests)); // the warm-up and exactly one more: no hidden replay
        }
        finally
        {
            listener.Stop();
            await accept;
        }
    }

    private static async Task ServeFirstThenDrop(TcpClient c, Func<int> count)
    {
        using (c)
        {
            var stream = c.GetStream();
            using var reader = new StreamReader(stream, Encoding.ASCII, false, 1024, leaveOpen: true);
            while (true)
            {
                var line = await reader.ReadLineAsync();
                if (line is null)
                {
                    return;
                }
                while (!string.IsNullOrEmpty(await reader.ReadLineAsync()))
                {
                }
                if (count() > 1)
                {
                    return; // read, never answered
                }
                var body = Encoding.UTF8.GetBytes("{\"credits\":1,\"consumed\":0,\"granted\":1}");
                var head = Encoding.ASCII.GetBytes($"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {body.Length}\r\nConnection: keep-alive\r\n\r\n");
                await stream.WriteAsync(head);
                await stream.WriteAsync(body);
                await stream.FlushAsync();
            }
        }
    }

    // ---- bodies: depth and size -------------------------------------------------------------------------------------------

    [Fact]
    public async Task AVeryDeepResponseBodyEndsInAnSdkErrorNotInTheStackOrTheHeap()
    {
        var deep = new string('[', 8 << 20);
        using var s = new TestServer(ctx => TestServer.Json(ctx, 200, deep));
        var e = await Assert.ThrowsAsync<UnavailableException>(() => s.Client().Credits.GetBalanceAsync("acme"));
        Assert.Equal("invalid_response", e.Code);
        Assert.Equal(1, s.Hits);
    }

    [Fact]
    public async Task AVeryDeepErrorBodyStillClassifiesByStatus()
    {
        using var s = new TestServer(ctx => TestServer.Json(ctx, 409, "{\"error\":{\"details\":" + new string('[', 60) + "}}"));
        var e = await Assert.ThrowsAsync<InvalidRequestException>(() => s.Client().Customers.GetAsync("acme"));
        Assert.Equal(409, e.Status);
    }

    private static string Sign(string secret, long t, byte[] payload)
    {
        var signed = Encoding.UTF8.GetBytes($"{t}.").Concat(payload).ToArray();
        return $"t={t},v1={Convert.ToHexString(HMACSHA256.HashData(Encoding.UTF8.GetBytes(secret), signed)).ToLowerInvariant()}";
    }

    [Fact]
    public void AWebhookBodyJustUnderTheCapOfNestingEndsInInvalidPayload()
    {
        const long now = 1_800_000_000L;
        var payload = Encoding.UTF8.GetBytes(new string('[', (1 << 20) - 100));
        var e = Assert.Throws<WebhookException>(() => Webhooks.Verify(payload, Sign("s", now, payload), new[] { "s" }, DateTimeOffset.FromUnixTimeSeconds(now)));
        Assert.Equal("invalid_payload", e.Reason);
    }

    [Fact]
    public void AWebhookBodyOverTheCapIsRefused()
    {
        const long now = 1_800_000_000L;
        var payload = new byte[(1 << 20) + 1];
        Array.Fill(payload, (byte)' ');
        var e = Assert.Throws<WebhookException>(() => Webhooks.Verify(payload, Sign("s", now, payload), new[] { "s" }, DateTimeOffset.FromUnixTimeSeconds(now)));
        Assert.Equal("invalid_payload", e.Reason);
    }

    // ---- webhooks: configuration errors come first ------------------------------------------------------------------------

    [Fact]
    public void WebhookVerificationRefusesAnEmptyOrBlankSecretBeforeLookingAtTheSignature()
    {
        const long now = 1_800_000_000L;
        var at = DateTimeOffset.FromUnixTimeSeconds(now);
        var payload = Encoding.UTF8.GetBytes("{\"id\":\"e\",\"type\":\"t\"}");
        // a signature made WITH an empty key: anyone can compute it, so it must never verify
        var forged = Sign("", now, payload);
        foreach (var secrets in new[] { new string[0], new[] { "" }, new[] { "   " }, new[] { "real", "" }, new[] { "\t\n" }, new string[] { null! } })
        {
            Assert.Throws<ArgumentException>(() => Webhooks.Verify(payload, forged, secrets, at));
            Assert.Throws<ArgumentException>(() => Webhooks.Verify(payload, null, secrets, at)); // before even the header
        }
        Assert.Throws<ArgumentNullException>(() => Webhooks.Verify(payload, forged, null!, at));
    }

    [Fact]
    public void AnAbsurdSignedTimestampIsStaleNotACrash()
    {
        var payload = Encoding.UTF8.GetBytes("{}");
        var e = Assert.Throws<WebhookException>(() => Webhooks.Verify(payload, "t=999999999999999,v1=" + new string('0', 64), new[] { "s" }, DateTimeOffset.UtcNow));
        Assert.Equal("stale_timestamp", e.Reason);
    }
}
